//! Shared workspace traversal and ignore policy.

use std::path::Path;

/// Options shared by filesystem traversal consumers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraversalOptions {
    pub respect_gitignore: bool,
    pub include_hidden: bool,
    pub include_ignored: bool,
    pub exclude: Vec<String>,
    pub follow_symlinks: bool,
    pub max_depth: usize,
}

impl Default for TraversalOptions {
    fn default() -> Self {
        Self {
            respect_gitignore: true,
            include_hidden: false,
            include_ignored: false,
            exclude: Vec::new(),
            follow_symlinks: false,
            max_depth: 64,
        }
    }
}

/// Central policy for generated directories ignored by workspace consumers.
#[derive(Clone, Copy, Debug, Default)]
pub struct WorkspaceIgnorePolicy;

impl WorkspaceIgnorePolicy {
    #[must_use]
    pub fn should_ignore_default(self, path: &Path) -> bool {
        path.file_name()
            .and_then(|value| value.to_str())
            .is_some_and(is_default_ignored_component)
    }
}

/// Returns whether one path component is a generated/default-excluded directory.
#[must_use]
pub fn is_default_ignored_component(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_directory_matching_is_case_insensitive() {
        assert!(is_default_ignored_component("Node_Modules"));
        assert!(is_default_ignored_component("TARGET"));
        assert!(!is_default_ignored_component("src"));
    }
}
