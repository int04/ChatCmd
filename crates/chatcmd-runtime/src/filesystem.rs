use crate::{
    FsEntry, OperationContext, PolicyContext, PolicyEngine, RuntimeError, RuntimeResult,
    TextReadResult,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[path = "filesystem_mutations.rs"]
mod mutations;

#[derive(Clone)]
pub struct WorkspaceService {
    roots: Vec<PathBuf>,
    allowed_scopes: Vec<PathBuf>,
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
            roots: canonical.clone(),
            allowed_scopes: canonical,
            policy,
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
        if start_line == 0 {
            return Err(RuntimeError::new(
                "invalid_line_range",
                "startLine must be at least 1",
            ));
        }
        if line_count == Some(0) {
            return Err(RuntimeError::new(
                "invalid_line_range",
                "lineCount must be at least 1 when provided",
            ));
        }
        let resolved = self.existing(path)?;
        let bytes = tokio::fs::read(&resolved).await.map_err(io_error)?;
        let content = String::from_utf8(bytes)
            .map_err(|_| RuntimeError::new("invalid_utf8", "file is not valid UTF-8"))?;
        let total_lines = content.lines().count();
        let limit = max_characters.clamp(1, 1_000_000);
        if start_line == 1 && line_count.is_none() {
            let truncated = content.chars().count() > limit;
            return Ok(TextReadResult {
                path: resolved,
                content: content.chars().take(limit).collect(),
                truncated,
                start_line: 1,
                end_line: total_lines,
                total_lines,
            });
        }
        let lines: Vec<&str> = content.lines().collect();
        let selected = lines
            .iter()
            .skip(start_line.saturating_sub(1))
            .take(line_count.unwrap_or(usize::MAX))
            .copied()
            .collect::<Vec<_>>();
        let selected_content = selected.join("\n");
        let end_line = if selected.is_empty() {
            start_line.saturating_sub(1)
        } else {
            start_line + selected.len() - 1
        };
        let character_truncated = selected_content.chars().count() > limit;
        let line_truncated = end_line < total_lines;
        Ok(TextReadResult {
            path: resolved,
            content: selected_content.chars().take(limit).collect(),
            truncated: character_truncated || line_truncated,
            start_line,
            end_line,
            total_lines,
        })
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
        let occurrences = content.matches(old_text).count();
        if occurrences != expected_occurrences {
            return Err(RuntimeError::new(
                "text_match_count_mismatch",
                format!(
                    "expected {expected_occurrences} occurrence(s) of oldText but found {occurrences}"
                ),
            ));
        }
        let updated = content.replace(old_text, new_text);
        self.write_text(context, &resolved, &updated, true).await
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
