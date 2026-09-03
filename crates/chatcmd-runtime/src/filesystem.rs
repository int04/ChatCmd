use crate::{
    FsEntry, OperationContext, PolicyContext, PolicyEngine, RuntimeError, RuntimeResult,
    TextReadBudget, TextReadRange, TextReadRequestV2, TextReadResult, TextReadResultV2,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

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
#[path = "filesystem_walk.rs"]
mod walk;
pub use search::SearchProgress;

#[derive(Clone)]
pub struct WorkspaceService {
    roots: Vec<PathBuf>,
    allowed_scopes: Vec<PathBuf>,
    policy: PolicyEngine,
    list_states: Arc<list::DirectoryListStore>,
    find_states: Arc<find::FindStateStore>,
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
        Ok(Self {
            roots: canonical.clone(),
            allowed_scopes: canonical,
            policy,
            list_states: Arc::new(list::DirectoryListStore::default()),
            find_states: Arc::new(find::FindStateStore::default()),
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
        let metadata = tokio::fs::symlink_metadata(&resolved)
            .await
            .map_err(io_error)?;
        Ok(FsEntry {
            name: resolved.file_name().map_or_else(
                || resolved.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            ),
            path: resolved,
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
        read::read_text_v2(resolved, context, request).await
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

    fn existing(&self, path: &Path) -> RuntimeResult<PathBuf> {
        let requested_absolute = path.is_absolute();
        let resolved = path.canonicalize().map_err(io_error)?;
        if !requested_absolute {
            self.ensure_allowed(&resolved)?;
        }
        Ok(resolved)
    }
    fn creation(&self, path: &Path) -> RuntimeResult<PathBuf> {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            return Err(RuntimeError::new(
                "invalid_path",
                "filesystem paths must be absolute",
            ));
        };
        let parent = absolute
            .parent()
            .ok_or_else(|| RuntimeError::new("invalid_path", "path has no parent"))?
            .canonicalize()
            .map_err(io_error)?;
        Ok(parent.join(
            absolute
                .file_name()
                .ok_or_else(|| RuntimeError::new("invalid_path", "path has no file name"))?,
        ))
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
