use crate::{
    FsEntry, OperationContext, PolicyContext, PolicyEngine, RuntimeError, RuntimeResult,
    TextReadBudget, TextReadRange, TextReadRequestV2, TextReadResult, TextReadResultV2,
};
use std::{
    ffi::OsString,
    fs,
    ops::Deref,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[path = "filesystem_apply_edits.rs"]
mod apply_edits;
mod file_version;
#[path = "filesystem_find.rs"]
mod find;
#[path = "filesystem_list.rs"]
mod list;
#[path = "filesystem_mutations.rs"]
mod mutations;
#[path = "filesystem_read.rs"]
mod read;
#[path = "filesystem_search.rs"]
mod search;
#[path = "filesystem_search_helpers.rs"]
mod search_helpers;
#[path = "filesystem_search_state.rs"]
mod search_state;
#[path = "filesystem_walk.rs"]
mod walk;
pub use file_version::FileVersion;
pub use search::SearchProgress;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PathAccess {
    Read,
    Create,
    Replace,
    Delete,
    MoveSource,
    MoveDestination,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EntryKind {
    File,
    Directory,
    Other,
}

impl EntryKind {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        if metadata.is_file() {
            Self::File
        } else if metadata.is_dir() {
            Self::Directory
        } else {
            Self::Other
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    creation_time: u64,
    len: u64,
    modified_ns: u128,
}

impl FileIdentity {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt as _;
        #[cfg(windows)]
        use std::os::windows::fs::MetadataExt as _;

        let is_directory = metadata.is_dir();
        Self {
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(windows)]
            creation_time: metadata.creation_time(),
            len: if is_directory { 0 } else { metadata.len() },
            modified_ns: if is_directory {
                0
            } else {
                metadata
                    .modified()
                    .unwrap_or(SystemTime::UNIX_EPOCH)
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or(Duration::ZERO)
                    .as_nanos()
            },
        }
    }
}

#[derive(Clone, Debug)]
struct ExistingWorkspacePath {
    canonical_path: PathBuf,
    root: PathBuf,
    identity: FileIdentity,
    kind: EntryKind,
    access: PathAccess,
}

impl ExistingWorkspacePath {
    fn revalidate(&self) -> RuntimeResult<()> {
        let _authorized_access = self.access;
        let metadata = fs::symlink_metadata(&self.canonical_path).map_err(io_error)?;
        reject_reparse_metadata(&metadata)?;
        if FileIdentity::from_metadata(&metadata) != self.identity
            || EntryKind::from_metadata(&metadata) != self.kind
        {
            return Err(RuntimeError::new(
                "path_changed_after_authorization",
                "filesystem entry changed after path authorization",
            ));
        }
        Ok(())
    }

    fn into_path(self) -> PathBuf {
        self.canonical_path
    }
}

impl Deref for ExistingWorkspacePath {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.canonical_path
    }
}

impl AsRef<Path> for ExistingWorkspacePath {
    fn as_ref(&self) -> &Path {
        &self.canonical_path
    }
}

#[derive(Clone, Debug)]
struct CreationWorkspacePath {
    canonical_parent: PathBuf,
    final_name: OsString,
    root: PathBuf,
    parent_identity: FileIdentity,
    access: PathAccess,
}

impl CreationWorkspacePath {
    fn revalidate_parent(&self) -> RuntimeResult<()> {
        let _authorized_access = self.access;
        let metadata = fs::symlink_metadata(&self.canonical_parent).map_err(io_error)?;
        reject_reparse_metadata(&metadata)?;
        if FileIdentity::from_metadata(&metadata) != self.parent_identity {
            return Err(RuntimeError::new(
                "path_changed_after_authorization",
                "destination parent changed after path authorization",
            ));
        }
        Ok(())
    }

    fn path(&self) -> PathBuf {
        self.canonical_parent.join(&self.final_name)
    }
}

#[derive(Clone)]
pub struct WorkspaceService {
    roots: Vec<PathBuf>,
    allowed_scopes: Vec<PathBuf>,
    policy: PolicyEngine,
    list_states: Arc<list::DirectoryListStore>,
    find_states: Arc<find::FindStateStore>,
    search_states: Arc<search::SearchStateStore>,
    version_key: Arc<[u8; 32]>,
}

impl WorkspaceService {
    pub fn new(roots: &[PathBuf], policy: PolicyEngine) -> RuntimeResult<Self> {
        let mut canonical = Vec::with_capacity(roots.len());
        for root in roots {
            let resolved = root.canonicalize().map_err(io_error)?;
            if !resolved.is_dir() {
                return Err(RuntimeError::new(
                    "invalid_workspace_root",
                    "configured workspace root is not a directory",
                ));
            }
            canonical.push(resolved);
        }
        canonical.sort();
        canonical.dedup();
        let mut key_hasher = sha2::Sha256::new();
        use sha2::Digest as _;
        key_hasher.update(uuid::Uuid::new_v4().as_bytes());
        key_hasher.update(uuid::Uuid::new_v4().as_bytes());
        let version_key: [u8; 32] = key_hasher.finalize().into();
        Ok(Self {
            roots: canonical.clone(),
            allowed_scopes: canonical,
            policy,
            list_states: Arc::new(list::DirectoryListStore::default()),
            find_states: Arc::new(find::FindStateStore::default()),
            search_states: Arc::new(search::SearchStateStore::default()),
            version_key: Arc::new(version_key),
        })
    }

    #[must_use]
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    pub fn with_additional_scopes(&self, scopes: &[PathBuf]) -> RuntimeResult<Self> {
        let mut allowed_scopes = self.allowed_scopes.clone();
        for scope in scopes {
            if !scope.is_absolute() {
                return Err(RuntimeError::new(
                    "invalid_path_scope",
                    "temporary filesystem scopes must be absolute",
                ));
            }
            let resolved = scope.canonicalize().map_err(io_error)?;
            if resolved.parent().is_none() {
                return Err(RuntimeError::new(
                    "path_scope_too_broad",
                    "temporary filesystem scope cannot be a filesystem root",
                ));
            }
            allowed_scopes.push(resolved);
        }
        allowed_scopes.sort();
        allowed_scopes.dedup();
        Ok(Self {
            roots: self.roots.clone(),
            allowed_scopes,
            policy: self.policy.clone(),
            list_states: self.list_states.clone(),
            find_states: self.find_states.clone(),
            search_states: self.search_states.clone(),
            version_key: self.version_key.clone(),
        })
    }

    pub async fn list(
        &self,
        path: &Path,
        offset: usize,
        limit: usize,
    ) -> RuntimeResult<Vec<FsEntry>> {
        let resolved = self.existing(path)?;
        let mut entries = tokio::task::spawn_blocking(move || -> RuntimeResult<Vec<FsEntry>> {
            resolved.revalidate()?;
            let mut values = Vec::new();
            for item in fs::read_dir(resolved).map_err(io_error)? {
                let item = item.map_err(io_error)?;
                let metadata = fs::symlink_metadata(item.path()).map_err(io_error)?;
                values.push(FsEntry {
                    path: item.path(),
                    name: item.file_name().to_string_lossy().into_owned(),
                    entry_type: if metadata.file_type().is_symlink() {
                        "symlink"
                    } else if metadata.is_dir() {
                        "directory"
                    } else {
                        "file"
                    }
                    .into(),
                    size: metadata.len(),
                    readonly: metadata.permissions().readonly(),
                });
            }
            values.sort_by(|a, b| {
                a.name
                    .to_lowercase()
                    .cmp(&b.name.to_lowercase())
                    .then_with(|| a.name.cmp(&b.name))
            });
            Ok(values)
        })
        .await
        .map_err(join_error)??;
        Ok(entries
            .drain(offset.min(entries.len())..)
            .take(limit.clamp(1, 2000))
            .collect())
    }

    pub async fn stat(&self, path: &Path) -> RuntimeResult<FsEntry> {
        let resolved = self.existing(path)?;
        resolved.revalidate()?;
        let metadata = tokio::fs::symlink_metadata(&resolved)
            .await
            .map_err(io_error)?;
        Ok(FsEntry {
            name: resolved.file_name().map_or_else(
                || resolved.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            ),
            path: resolved.into_path(),
            entry_type: if metadata.file_type().is_symlink() {
                "symlink"
            } else if metadata.is_dir() {
                "directory"
            } else {
                "file"
            }
            .into(),
            size: metadata.len(),
            readonly: metadata.permissions().readonly(),
        })
    }

    pub async fn search(
        &self,
        path: &Path,
        query: &str,
        case_sensitive: bool,
        max_results: usize,
        max_file_bytes: u64,
        include_ignored: bool,
        exclude: Vec<String>,
        progress: impl Fn(SearchProgress) + Send + Sync + 'static,
    ) -> RuntimeResult<Vec<serde_json::Value>> {
        let request = crate::FsSearchRequest {
            path: path.to_path_buf(),
            query: query.to_owned(),
            mode: crate::SearchMode::Literal,
            case_sensitive,
            word_boundary: false,
            include: Vec::new(),
            exclude,
            include_ignored,
            context_before: 0,
            context_after: 0,
            max_matches_per_file: max_results.max(1),
            limit: max_results.max(1),
            max_snippet_bytes: 8 * 1024,
            budget: crate::FsSearchBudget {
                max_file_bytes,
                ..crate::FsSearchBudget::default()
            },
        };
        let context = OperationContext::new(
            uuid::Uuid::new_v4().to_string(),
            "legacy-workspace-search",
            "fs_search",
        );
        let (page, _) = self
            .search_v2(&context, &request, None, None, progress)
            .await?;
        page.data
            .matches
            .into_iter()
            .map(|value| {
                serde_json::to_value(value).map_err(|error| {
                    RuntimeError::new("result_serialization_failed", error.to_string())
                })
            })
            .collect()
    }

    pub async fn read_text(
        &self,
        path: &Path,
        max_characters: usize,
    ) -> RuntimeResult<TextReadResult> {
        self.read_text_range(path, max_characters, 1, None).await
    }

    pub async fn read_text_range(
        &self,
        path: &Path,
        max_characters: usize,
        start_line: usize,
        line_count: Option<usize>,
    ) -> RuntimeResult<TextReadResult> {
        if start_line == 0 || line_count == Some(0) {
            return Err(RuntimeError::new(
                "invalid_line_range",
                "startLine and lineCount must be at least 1",
            ));
        }
        let character_limit = max_characters.clamp(1, 1_000_000);
        let request = TextReadRequestV2 {
            path: path.to_path_buf(),
            range: TextReadRange::Line {
                start: start_line,
                limit: line_count.unwrap_or(usize::MAX),
            },
            max_bytes: character_limit.saturating_mul(4),
            include_line_endings: start_line == 1 && line_count.is_none(),
            expected_version: None,
            budget: TextReadBudget {
                timeout_ms: 60_000,
                max_bytes_read: u64::MAX,
            },
        };
        let result = self.read_text_v2(None, &request).await?;
        let character_truncated = result.content.chars().count() > character_limit;
        let content = result.content.chars().take(character_limit).collect();
        let mut end_line = result
            .range
            .end_line
            .unwrap_or(start_line.saturating_sub(1));
        if !result.content.is_empty() && end_line < start_line {
            end_line = start_line;
        }
        let total_lines = match result.total_lines {
            Some(total) => total,
            None => read::legacy_text_total_lines(&result.path).await?,
        };
        Ok(TextReadResult {
            path: result.path,
            content,
            truncated: result.truncated || character_truncated,
            start_line,
            end_line,
            total_lines,
        })
    }

    pub async fn read_text_v2(
        &self,
        context: Option<&OperationContext>,
        request: &TextReadRequestV2,
    ) -> RuntimeResult<TextReadResultV2> {
        let resolved = self.existing(&request.path)?;
        resolved.revalidate()?;
        read::read_text_v2(resolved.into_path(), context, request).await
    }

    pub async fn replace_text(
        &self,
        context: &OperationContext,
        path: &Path,
        old_text: &str,
        new_text: &str,
        expected_occurrences: usize,
    ) -> RuntimeResult<FsEntry> {
        if old_text.is_empty() {
            return Err(RuntimeError::new(
                "invalid_text_replacement",
                "oldText cannot be empty",
            ));
        }
        if expected_occurrences == 0 {
            return Err(RuntimeError::new(
                "invalid_text_replacement",
                "expectedOccurrences must be at least 1",
            ));
        }
        let resolved = self.existing(path)?;
        resolved.revalidate()?;
        const LEGACY_REPLACE_MAX_BYTES: u64 = 8 * 1024 * 1024;
        if resolved.identity.len > LEGACY_REPLACE_MAX_BYTES {
            return Err(RuntimeError::new(
                "legacyReplaceFileTooLarge",
                "fs_replace_text is limited to 8 MiB; use fs_apply_edits with expectedVersion",
            ));
        }
        let content = tokio::fs::read_to_string(&resolved)
            .await
            .map_err(io_error)?;
        let exact_occurrences = content.matches(old_text).count();
        let (matched_old_text, replacement_text, occurrences) =
            if exact_occurrences == expected_occurrences {
                (old_text.to_owned(), new_text.to_owned(), exact_occurrences)
            } else {
                let line_ending = if content.contains("\r\n") {
                    "\r\n"
                } else {
                    "\n"
                };
                let adapted_old = adapt_line_endings(old_text, line_ending);
                let adapted_occurrences = content.matches(&adapted_old).count();
                (
                    adapted_old,
                    adapt_line_endings(new_text, line_ending),
                    adapted_occurrences,
                )
            };
        if occurrences != expected_occurrences {
            return Err(RuntimeError::new(
                "text_match_count_mismatch",
                format!(
                    "expected {expected_occurrences} occurrence(s) of oldText but found {occurrences}"
                ),
            ));
        }
        let updated = content.replace(&matched_old_text, &replacement_text);
        self.write_text(context, &resolved, &updated, true).await
    }

    fn existing(&self, path: &Path) -> RuntimeResult<ExistingWorkspacePath> {
        self.existing_for(path, PathAccess::Read)
    }

    fn existing_for(
        &self,
        path: &Path,
        access: PathAccess,
    ) -> RuntimeResult<ExistingWorkspacePath> {
        reject_reparse_components(path)?;
        let resolved = path.canonicalize().map_err(io_error)?;
        self.ensure_allowed(&resolved)?;
        let root = self.containing_root(&resolved).ok_or_else(scope_error)?;
        let metadata = fs::symlink_metadata(&resolved).map_err(io_error)?;
        reject_reparse_metadata(&metadata)?;
        let kind = EntryKind::from_metadata(&metadata);
        Ok(ExistingWorkspacePath {
            canonical_path: resolved,
            root,
            identity: FileIdentity::from_metadata(&metadata),
            kind,
            access,
        })
    }
    fn creation(&self, path: &Path) -> RuntimeResult<CreationWorkspacePath> {
        self.creation_for(path, PathAccess::Create)
    }

    fn creation_for(
        &self,
        path: &Path,
        access: PathAccess,
    ) -> RuntimeResult<CreationWorkspacePath> {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            return Err(RuntimeError::new(
                "invalid_path",
                "filesystem paths must be absolute",
            ));
        };
        let final_name = absolute
            .file_name()
            .ok_or_else(|| RuntimeError::new("invalid_path", "path has no file name"))?;
        validate_final_name(final_name)?;
        let requested_parent = absolute
            .parent()
            .ok_or_else(|| RuntimeError::new("invalid_path", "path has no parent"))?;
        reject_reparse_components(requested_parent)?;
        let canonical_parent = requested_parent.canonicalize().map_err(io_error)?;
        self.ensure_allowed(&canonical_parent)?;
        let root = self
            .containing_root(&canonical_parent)
            .ok_or_else(scope_error)?;
        let metadata = fs::symlink_metadata(&canonical_parent).map_err(io_error)?;
        reject_reparse_metadata(&metadata)?;
        Ok(CreationWorkspacePath {
            canonical_parent,
            final_name: final_name.to_os_string(),
            root,
            parent_identity: FileIdentity::from_metadata(&metadata),
            access,
        })
    }
    fn ensure_allowed(&self, path: &Path) -> RuntimeResult<()> {
        if self
            .allowed_scopes
            .iter()
            .any(|scope| path.starts_with(scope))
        {
            Ok(())
        } else {
            Err(RuntimeError::new(
                "path_outside_allowed_scope",
                "path escapes configured workspace roots and user-provided task path grants",
            ))
        }
    }
    fn containing_root(&self, path: &Path) -> Option<PathBuf> {
        self.allowed_scopes
            .iter()
            .filter(|scope| path.starts_with(scope))
            .max_by_key(|scope| scope.components().count())
            .cloned()
    }
}

fn scope_error() -> RuntimeError {
    RuntimeError::new(
        "path_outside_allowed_scope",
        "path escapes configured workspace roots and user-provided task path grants",
    )
}

fn validate_final_name(name: &std::ffi::OsStr) -> RuntimeResult<()> {
    if name == "." || name == ".." || name.is_empty() {
        return Err(RuntimeError::new(
            "invalid_path",
            "invalid final path component",
        ));
    }
    #[cfg(windows)]
    if name.to_string_lossy().contains(':') {
        return Err(RuntimeError::new(
            "invalid_path",
            "alternate data streams are not allowed",
        ));
    }
    Ok(())
}

fn reject_reparse_components(path: &Path) -> RuntimeResult<()> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_err(io_error)?.join(path)
    };
    for ancestor in absolute.ancestors().collect::<Vec<_>>().into_iter().rev() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => reject_reparse_metadata(&metadata)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(io_error(error)),
        }
    }
    Ok(())
}

fn reject_reparse_metadata(metadata: &fs::Metadata) -> RuntimeResult<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(RuntimeError::new(
                "symlink_traversal_rejected",
                "symbolic links and reparse points are not followed",
            ));
        }
    }
    if metadata.file_type().is_symlink() {
        return Err(RuntimeError::new(
            "symlink_traversal_rejected",
            "symbolic links and reparse points are not followed",
        ));
    }
    Ok(())
}

fn adapt_line_endings(value: &str, line_ending: &str) -> String {
    value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', line_ending)
}

fn copy_recursive(source: &Path, destination: &Path, overwrite: bool) -> RuntimeResult<()> {
    let metadata = fs::symlink_metadata(source).map_err(io_error)?;
    if metadata.file_type().is_symlink() {
        return Err(RuntimeError::new(
            "symlink_traversal_rejected",
            "copy through symbolic links is denied",
        ));
    }
    if metadata.is_dir() {
        fs::create_dir_all(destination).map_err(io_error)?;
        for item in fs::read_dir(source).map_err(io_error)? {
            let item = item.map_err(io_error)?;
            copy_recursive(&item.path(), &destination.join(item.file_name()), overwrite)?;
        }
    } else {
        if destination.exists() && !overwrite {
            return Err(RuntimeError::new(
                "already_exists",
                "destination exists and overwrite is false",
            ));
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        fs::copy(source, destination).map_err(io_error)?;
    }
    Ok(())
}
fn remove_recursive(path: &Path) -> RuntimeResult<()> {
    if path.is_dir() {
        fs::remove_dir_all(path).map_err(io_error)
    } else {
        fs::remove_file(path).map_err(io_error)
    }
}
fn io_error(error: std::io::Error) -> RuntimeError {
    RuntimeError::new(
        if error.kind() == std::io::ErrorKind::NotFound {
            "not_found"
        } else {
            "io_error"
        },
        error.to_string(),
    )
}
fn join_error(error: tokio::task::JoinError) -> RuntimeError {
    RuntimeError::new("worker_failed", error.to_string())
}

#[cfg(test)]
mod path_safety_tests {
    use super::*;
    use crate::{ApprovalDecision, BoxFuture};
    use tempfile::TempDir;

    struct Reject;

    impl ApprovalDecision for Reject {
        fn request<'a>(
            &'a self,
            _context: &'a PolicyContext,
        ) -> BoxFuture<'a, RuntimeResult<bool>> {
            Box::pin(async { Ok(false) })
        }
    }

    fn service(root: &Path) -> WorkspaceService {
        WorkspaceService::new(
            &[root.to_path_buf()],
            PolicyEngine::new(None, Arc::new(Reject)),
        )
        .expect("workspace service")
    }

    #[test]
    fn absolute_path_inside_root_is_authorized() {
        let workspace = TempDir::new().expect("workspace");
        let file = workspace.path().join("allowed.txt");
        fs::write(&file, "allowed").expect("file");

        assert!(service(workspace.path()).existing(&file).is_ok());
    }

    #[test]
    fn absolute_path_outside_root_is_rejected() {
        let workspace = TempDir::new().expect("workspace");
        let external = TempDir::new().expect("external");
        let file = external.path().join("denied.txt");
        fs::write(&file, "denied").expect("file");

        let error = service(workspace.path())
            .existing(&file)
            .expect_err("external absolute path must be rejected");
        assert_eq!(error.code, "path_outside_allowed_scope");
    }

    #[test]
    fn exact_file_grant_does_not_authorize_sibling() {
        let workspace = TempDir::new().expect("workspace");
        let external = TempDir::new().expect("external");
        let granted = external.path().join("granted.txt");
        let sibling = external.path().join("sibling.txt");
        fs::write(&granted, "granted").expect("granted file");
        fs::write(&sibling, "sibling").expect("sibling file");
        let scoped = service(workspace.path())
            .with_additional_scopes(std::slice::from_ref(&granted))
            .expect("file grant");

        assert!(scoped.existing(&granted).is_ok());
        assert_eq!(
            scoped
                .existing(&sibling)
                .expect_err("sibling must not inherit file grant")
                .code,
            "path_outside_allowed_scope"
        );
    }

    #[test]
    fn directory_grant_authorizes_only_its_subtree() {
        let workspace = TempDir::new().expect("workspace");
        let external = TempDir::new().expect("external");
        let granted = external.path().join("granted");
        fs::create_dir(&granted).expect("granted directory");
        let child = granted.join("child.txt");
        let sibling = external.path().join("sibling.txt");
        fs::write(&child, "child").expect("child file");
        fs::write(&sibling, "sibling").expect("sibling file");
        let scoped = service(workspace.path())
            .with_additional_scopes(std::slice::from_ref(&granted))
            .expect("directory grant");

        assert!(scoped.existing(&child).is_ok());
        assert!(scoped.existing(&sibling).is_err());
    }

    #[test]
    fn creation_outside_authorized_scope_is_rejected() {
        let workspace = TempDir::new().expect("workspace");
        let external = TempDir::new().expect("external");
        let error = service(workspace.path())
            .creation(&external.path().join("new.txt"))
            .expect_err("external creation must be rejected");

        assert_eq!(error.code, "path_outside_allowed_scope");
    }

    #[test]
    fn relative_parent_traversal_cannot_escape_workspace() {
        let workspace = TempDir::new().expect("workspace");
        let external_name = workspace
            .path()
            .file_name()
            .expect("workspace name")
            .to_os_string();
        let escaped = workspace.path().join("..").join(external_name);

        assert!(service(workspace.path()).existing(&escaped).is_ok());

        let denied = workspace.path().join("..");
        assert_eq!(
            service(workspace.path())
                .existing(&denied)
                .expect_err("parent must not be granted")
                .code,
            "path_outside_allowed_scope"
        );
    }

    #[test]
    fn unicode_path_is_authorized_without_lossy_normalization() {
        let workspace = TempDir::new().expect("workspace");
        let file = workspace.path().join("dữ-liệu-猫.txt");
        fs::write(&file, "unicode").expect("unicode file");

        assert_eq!(
            service(workspace.path())
                .existing(&file)
                .expect("unicode path")
                .as_ref(),
            file.canonicalize().expect("canonical unicode path")
        );
    }

    #[test]
    fn identity_revalidation_detects_replaced_file() {
        let workspace = TempDir::new().expect("workspace");
        let file = workspace.path().join("replace.txt");
        fs::write(&file, "before").expect("initial file");
        let authorized = service(workspace.path())
            .existing(&file)
            .expect("authorized path");

        fs::write(&file, "replacement with different identity").expect("replace file");

        assert_eq!(
            authorized
                .revalidate()
                .expect_err("replacement must invalidate capability")
                .code,
            "path_changed_after_authorization"
        );
    }

    #[cfg(windows)]
    #[test]
    fn mixed_windows_separators_stay_within_scope() {
        let workspace = TempDir::new().expect("workspace");
        let directory = workspace.path().join("nested");
        fs::create_dir(&directory).expect("nested directory");
        let file = directory.join("file.txt");
        fs::write(&file, "content").expect("file");
        let mixed = PathBuf::from(file.to_string_lossy().replace("\\", "/"));

        assert!(service(workspace.path()).existing(&mixed).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn alternate_data_stream_creation_is_rejected() {
        let workspace = TempDir::new().expect("workspace");
        let error = service(workspace.path())
            .creation(&workspace.path().join("file.txt:stream"))
            .expect_err("alternate data stream must be rejected");

        assert_eq!(error.code, "invalid_path");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_component_is_rejected_even_when_target_is_inside_root() {
        use std::os::unix::fs::symlink;

        let workspace = TempDir::new().expect("workspace");
        let real = workspace.path().join("real");
        fs::create_dir(&real).expect("real directory");
        fs::write(real.join("file.txt"), "content").expect("file");
        let link = workspace.path().join("link");
        symlink(&real, &link).expect("symlink");

        let error = service(workspace.path())
            .existing(&link.join("file.txt"))
            .expect_err("symlink traversal must be rejected");
        assert_eq!(error.code, "symlink_traversal_rejected");
    }

    #[cfg(unix)]
    #[test]
    fn broken_symlink_is_rejected() {
        use std::os::unix::fs::symlink;

        let workspace = TempDir::new().expect("workspace");
        let link = workspace.path().join("broken");
        symlink(workspace.path().join("missing"), &link).expect("broken symlink");

        let error = service(workspace.path())
            .existing(&link)
            .expect_err("broken symlink must be rejected");
        assert_eq!(error.code, "symlink_traversal_rejected");
    }
}
