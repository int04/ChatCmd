use crate::{
    FsEntry, OperationContext, PolicyContext, PolicyEngine, RuntimeError, RuntimeResult,
    TextReadResult,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub struct WorkspaceService {
    roots: Vec<PathBuf>,
    policy: PolicyEngine,
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
            roots: canonical,
            policy,
        })
    }

    #[must_use]
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
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
        let resolved = self.existing(path)?;
        let bytes = tokio::fs::read(&resolved).await.map_err(io_error)?;
        let content = String::from_utf8(bytes)
            .map_err(|_| RuntimeError::new("invalid_utf8", "file is not valid UTF-8"))?;
        let limit = max_characters.clamp(1, 1_000_000);
        let truncated = content.chars().count() > limit;
        Ok(TextReadResult {
            path: resolved,
            content: content.chars().take(limit).collect(),
            truncated,
        })
    }

    pub async fn write_text(
        &self,
        context: &OperationContext,
        path: &Path,
        content: &str,
        overwrite: bool,
    ) -> RuntimeResult<FsEntry> {
        self.write_bytes(context, path, content.as_bytes(), overwrite)
            .await
    }

    pub async fn write_raw(
        &self,
        context: &OperationContext,
        path: &Path,
        base64: &str,
        overwrite: bool,
    ) -> RuntimeResult<FsEntry> {
        let bytes = STANDARD.decode(base64).map_err(|_| {
            RuntimeError::new("invalid_base64", "raw file content is not valid Base64")
        })?;
        self.write_bytes(context, path, &bytes, overwrite).await
    }

    async fn write_bytes(
        &self,
        context: &OperationContext,
        path: &Path,
        bytes: &[u8],
        overwrite: bool,
    ) -> RuntimeResult<FsEntry> {
        let target = self.creation(path)?;
        self.policy
            .authorize(&PolicyContext {
                agent_id: context.agent_id.clone(),
                tool_name: context.tool_name.clone(),
                root: self.containing_root(&target),
                destructive: target.exists() && overwrite,
            })
            .await?;
        if target.exists() && !overwrite {
            return Err(RuntimeError::new(
                "already_exists",
                "destination exists and overwrite is false",
            ));
        }
        let parent = target
            .parent()
            .ok_or_else(|| RuntimeError::new("invalid_path", "destination has no parent"))?
            .to_path_buf();
        let target_clone = target.clone();
        let bytes = bytes.to_vec();
        tokio::task::spawn_blocking(move || -> RuntimeResult<()> {
            let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(io_error)?;
            temporary.write_all(&bytes).map_err(io_error)?;
            temporary.flush().map_err(io_error)?;
            if overwrite && target_clone.exists() {
                fs::remove_file(&target_clone).map_err(io_error)?;
            }
            temporary
                .persist(&target_clone)
                .map_err(|error| io_error(error.error))?;
            Ok(())
        })
        .await
        .map_err(join_error)??;
        self.stat(&target).await
    }

    pub async fn create_directory(&self, path: &Path) -> RuntimeResult<FsEntry> {
        let target = self.creation(path)?;
        tokio::fs::create_dir_all(&target).await.map_err(io_error)?;
        self.stat(&target).await
    }

    pub async fn copy(
        &self,
        context: &OperationContext,
        source: &Path,
        destination: &Path,
        overwrite: bool,
    ) -> RuntimeResult<FsEntry> {
        let source = self.existing(source)?;
        let destination = self.creation(destination)?;
        self.policy
            .authorize(&PolicyContext {
                agent_id: context.agent_id.clone(),
                tool_name: "fs_copy".into(),
                root: self.containing_root(&destination),
                destructive: overwrite,
            })
            .await?;
        if destination.exists() && !overwrite {
            return Err(RuntimeError::new(
                "already_exists",
                "destination exists and overwrite is false",
            ));
        }
        let source_clone = source.clone();
        let destination_clone = destination.clone();
        tokio::task::spawn_blocking(move || {
            copy_recursive(&source_clone, &destination_clone, overwrite)
        })
        .await
        .map_err(join_error)??;
        self.stat(&destination).await
    }

    pub async fn move_path(
        &self,
        context: &OperationContext,
        source: &Path,
        destination: &Path,
        overwrite: bool,
    ) -> RuntimeResult<FsEntry> {
        let source = self.existing(source)?;
        let destination = self.creation(destination)?;
        self.policy
            .authorize(&PolicyContext {
                agent_id: context.agent_id.clone(),
                tool_name: "fs_move".into(),
                root: self.containing_root(&source),
                destructive: true,
            })
            .await?;
        if destination.exists() {
            if !overwrite {
                return Err(RuntimeError::new(
                    "already_exists",
                    "destination exists and overwrite is false",
                ));
            }
            remove_recursive(&destination)?;
        }
        match tokio::fs::rename(&source, &destination).await {
            Ok(()) => {}
            Err(_) => {
                let source_clone = source.clone();
                let destination_clone = destination.clone();
                tokio::task::spawn_blocking(move || -> RuntimeResult<()> {
                    copy_recursive(&source_clone, &destination_clone, true)?;
                    remove_recursive(&source_clone)
                })
                .await
                .map_err(join_error)??;
            }
        }
        self.stat(&destination).await
    }

    pub async fn delete(
        &self,
        context: &OperationContext,
        path: &Path,
        recursive: bool,
    ) -> RuntimeResult<bool> {
        let resolved = self.existing(path)?;
        if self.roots.contains(&resolved) {
            return Err(RuntimeError::new(
                "root_path_rejected",
                "configured workspace roots cannot be deleted",
            ));
        }
        self.policy
            .authorize(&PolicyContext {
                agent_id: context.agent_id.clone(),
                tool_name: "fs_delete".into(),
                root: self.containing_root(&resolved),
                destructive: true,
            })
            .await?;
        tokio::task::spawn_blocking(move || -> RuntimeResult<bool> {
            if resolved.is_dir() {
                if recursive {
                    fs::remove_dir_all(resolved).map_err(io_error)?;
                } else {
                    fs::remove_dir(resolved).map_err(io_error)?;
                }
            } else {
                fs::remove_file(resolved).map_err(io_error)?;
            }
            Ok(true)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn find(
        &self,
        path: &Path,
        pattern: &str,
        max_results: usize,
        max_depth: usize,
    ) -> RuntimeResult<Vec<PathBuf>> {
        let root = self.existing(path)?;
        let needle = pattern.trim_matches('*').to_lowercase();
        tokio::task::spawn_blocking(move || -> RuntimeResult<Vec<PathBuf>> {
            let mut found = Vec::new();
            visit(&root, 0, max_depth.clamp(1, 128), &mut |path, _| {
                let name = path
                    .file_name()
                    .map_or_else(String::new, |value| value.to_string_lossy().to_lowercase());
                if (needle.is_empty() || name.contains(&needle))
                    && found.len() < max_results.clamp(1, 5000)
                {
                    found.push(path.to_path_buf());
                }
                Ok(())
            })?;
            Ok(found)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn search(
        &self,
        path: &Path,
        query: &str,
        case_sensitive: bool,
        max_results: usize,
        max_file_bytes: u64,
    ) -> RuntimeResult<Vec<serde_json::Value>> {
        let root = self.existing(path)?;
        let query = query.to_owned();
        tokio::task::spawn_blocking(move || -> RuntimeResult<Vec<serde_json::Value>> {
            let mut found = Vec::new();
            let limit = max_results.clamp(1, 2_000);
            let query_cmp = if case_sensitive {
                query
            } else {
                query.to_lowercase()
            };
            visit(&root, 0, 64, &mut |path, metadata| {
                if metadata.is_file()
                    && metadata.len() <= max_file_bytes
                    && found.len() < limit
                    && let Ok(content) = fs::read_to_string(path)
                {
                    for (index, line) in content.lines().enumerate() {
                        let matches = if case_sensitive {
                            line.contains(&query_cmp)
                        } else {
                            line.to_lowercase().contains(&query_cmp)
                        };
                        if matches {
                            found.push(serde_json::json!({
                                "path": path,
                                "line": index + 1,
                                "text": line
                            }));
                            if found.len() >= limit {
                                break;
                            }
                        }
                    }
                }
                Ok(())
            })?;
            Ok(found)
        })
        .await
        .map_err(join_error)?
    }

    fn existing(&self, path: &Path) -> RuntimeResult<PathBuf> {
        let resolved = path.canonicalize().map_err(io_error)?;
        self.ensure_allowed(&resolved)?;
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
        self.ensure_allowed(&parent)?;
        Ok(parent.join(
            absolute
                .file_name()
                .ok_or_else(|| RuntimeError::new("invalid_path", "path has no file name"))?,
        ))
    }
    fn ensure_allowed(&self, path: &Path) -> RuntimeResult<()> {
        if self.roots.iter().any(|root| path.starts_with(root)) {
            Ok(())
        } else {
            Err(RuntimeError::new(
                "path_outside_allowed_scope",
                "path escapes configured workspace roots",
            ))
        }
    }
    fn containing_root(&self, path: &Path) -> Option<PathBuf> {
        self.roots
            .iter()
            .find(|root| path.starts_with(root))
            .cloned()
    }
}

fn visit(
    path: &Path,
    depth: usize,
    max_depth: usize,
    callback: &mut impl FnMut(&Path, &fs::Metadata) -> RuntimeResult<()>,
) -> RuntimeResult<()> {
    if depth > max_depth {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    callback(path, &metadata)?;
    if metadata.is_dir() {
        for child in fs::read_dir(path).map_err(io_error)? {
            visit(
                &child.map_err(io_error)?.path(),
                depth + 1,
                max_depth,
                callback,
            )?;
        }
    }
    Ok(())
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
