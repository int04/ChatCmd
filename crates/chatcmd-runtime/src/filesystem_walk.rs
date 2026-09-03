use crate::{RuntimeError, RuntimeResult};
use ignore::{
    WalkBuilder,
    gitignore::{Gitignore, GitignoreBuilder},
};
use std::path::Path;

pub(super) fn configured_walker(
    root: &Path,
    max_depth: usize,
    include_hidden: bool,
    include_ignored: bool,
    exclude: &[String],
) -> RuntimeResult<WalkBuilder> {
    let explicit_excludes = build_excludes(root, exclude)?;
    let mut walker = WalkBuilder::new(root);
    walker
        .follow_links(false)
        .max_depth(Some(max_depth.clamp(1, 128)))
        .hidden(!include_hidden)
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
        include_ignored || !is_default_ignored_directory(entry.path())
    });
    Ok(walker)
}

fn build_excludes(root: &Path, patterns: &[String]) -> RuntimeResult<Gitignore> {
    let mut builder = GitignoreBuilder::new(root);
    for pattern in patterns {
        let normalized = pattern.trim().replace('\\', "/");
        if normalized.is_empty() {
            continue;
        }
        builder
            .add_line(None, &normalized)
            .map_err(|error| RuntimeError::new("invalid_exclude_pattern", error.to_string()))?;
    }
    builder
        .build()
        .map_err(|error| RuntimeError::new("invalid_exclude_pattern", error.to_string()))
}

fn is_default_ignored_directory(path: &Path) -> bool {
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
