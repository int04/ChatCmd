use super::{WorkspaceService, walk::configured_walker};
use crate::{RuntimeError, RuntimeResult};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct SearchProgress {
    pub path: PathBuf,
    pub files_scanned: usize,
    pub matches_found: usize,
    pub matched: Option<serde_json::Value>,
}

impl WorkspaceService {
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
        let root = self.existing(path)?;
        let query = query.to_owned();
        tokio::task::spawn_blocking(move || -> RuntimeResult<Vec<serde_json::Value>> {
            let walker = configured_walker(&root, 64, true, include_ignored, &exclude)?;

            let mut found = Vec::new();
            let mut files_scanned = 0usize;
            let limit = max_results.clamp(1, 2_000);
            let query_cmp = if case_sensitive {
                query
            } else {
                query.to_lowercase()
            };

            for entry in walker.build() {
                if found.len() >= limit {
                    break;
                }
                let entry = entry.map_err(|error| {
                    RuntimeError::new("filesystem_walk_error", error.to_string())
                })?;
                let Some(file_type) = entry.file_type() else {
                    continue;
                };
                if !file_type.is_file() {
                    continue;
                }
                let metadata = entry.metadata().map_err(|error| {
                    RuntimeError::new("filesystem_metadata_error", error.to_string())
                })?;
                if metadata.len() > max_file_bytes {
                    continue;
                }

                files_scanned = files_scanned.saturating_add(1);
                if files_scanned == 1 || files_scanned % 250 == 0 {
                    progress(SearchProgress {
                        path: entry.path().to_path_buf(),
                        files_scanned,
                        matches_found: found.len(),
                        matched: None,
                    });
                }
                if let Ok(content) = fs::read_to_string(entry.path()) {
                    for (index, line) in content.lines().enumerate() {
                        let matches = if case_sensitive {
                            line.contains(&query_cmp)
                        } else {
                            line.to_lowercase().contains(&query_cmp)
                        };
                        if matches {
                            let matched = serde_json::json!({
                                "path": entry.path(),
                                "line": index + 1,
                                "text": line
                            });
                            found.push(matched.clone());
                            progress(SearchProgress {
                                path: entry.path().to_path_buf(),
                                files_scanned,
                                matches_found: found.len(),
                                matched: Some(matched),
                            });
                            if found.len() >= limit {
                                break;
                            }
                        }
                    }
                }
            }
            Ok(found)
        })
        .await
        .map_err(super::join_error)?
    }
}
