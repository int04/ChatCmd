use crate::{RuntimeError, RuntimeResult, TraversalOptions, WorkspaceIgnorePolicy};
use ignore::{
    WalkBuilder,
    gitignore::{Gitignore, GitignoreBuilder},
};
use std::path::Path;

pub(super) fn configured_walker(
    root: &Path,
    options: &TraversalOptions,
) -> RuntimeResult<WalkBuilder> {
    let explicit_excludes = build_excludes(root, &options.exclude)?;
    let ignore_policy = WorkspaceIgnorePolicy;
    let mut walker = WalkBuilder::new(root);
    walker
        .follow_links(options.follow_symlinks)
        .max_depth(Some(options.max_depth.clamp(1, 128)))
        .hidden(!options.include_hidden)
        .parents(false)
        .git_global(false)
        .git_exclude(false)
        .require_git(false)
        .git_ignore(options.respect_gitignore && !options.include_ignored);
    let include_ignored = options.include_ignored;
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
        include_ignored || !ignore_policy.should_ignore_default(entry.path())
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
