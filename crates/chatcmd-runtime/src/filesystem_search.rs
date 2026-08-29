use super::WorkspaceService;
use crate::{RuntimeError, RuntimeResult};
use ignore::{
    WalkBuilder,
    gitignore::{Gitignore, GitignoreBuilder},
};
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
            let explicit_excludes = build_search_excludes(&root, &exclude)?;
            let mut walker = WalkBuilder::new(&root);
            walker
                .follow_links(false)
                .max_depth(Some(64))
                .hidden(false)
                .parents(false)
                .git_global(false)
                .git_exclude(false)
                .require_git(false)
                .git_ignore(!include_ignored);
            walker.filter_entry(move |entry| {
                if entry.depth() == 0 {
                    return true;
                }
                let is_dir = entry.file_type().is_some_and(|kind| kind.is_dir());
                if explicit_excludes
                    .matched_path_or_any_parents(entry.path(), is_dir)
                    .is_ignore()
                {
                    return false;
                }
                include_ignored || !is_default_ignored_search_directory(entry.path())
            });

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

fn build_search_excludes(root: &Path, patterns: &[String]) -> RuntimeResult<Gitignore> {
    let mut builder = GitignoreBuilder::new(root);
    for pattern in patterns {
        let normalized = pattern.trim().replace('\\', "/");
        if normalized.is_empty() {
            continue;
        }
        builder
            .add_line(None, &normalized)
            .map_err(|error| RuntimeError::new("invalid_search_exclude", error.to_string()))?;
    }
    builder
        .build()
        .map_err(|error| RuntimeError::new("invalid_search_exclude", error.to_string()))
}

fn is_default_ignored_search_directory(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    matches!(
        name.to_ascii_lowercase().as_str(),
        ".git"
            | ".idea"
            | ".next"
            | ".nuxt"
            | ".turbo"
            | ".vite"
            | ".vs"
            | ".gradle"
            | ".venv"
            | "venv"
            | "__pycache__"
            | ".pytest_cache"
            | ".mypy_cache"
            | ".ruff_cache"
            | ".tox"
            | ".parcel-cache"
            | ".svelte-kit"
            | ".angular"
            | ".expo"
            | ".pnpm-store"
            | ".dart_tool"
            | ".symlinks"
            | ".cxx"
            | ".externalnativebuild"
            | ".nyc_output"
            | "artifacts"
            | "bin"
            | "bower_components"
            | "build"
            | "coverage"
            | "deriveddata"
            | "dist"
            | "htmlcov"
            | "jspm_packages"
            | "node_modules"
            | "obj"
            | "pods"
            | "target"
            | "testresults"
    )
}
