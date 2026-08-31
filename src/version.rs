use std::path::{Path, PathBuf};

const VERSION_MARKER_FILE: &str = "chatcmd-version.txt";

pub(crate) fn compiled_version() -> &'static str {
    option_env!("CHATCMD_BUILD_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

pub(crate) fn app_version() -> String {
    installed_version().unwrap_or_else(|| compiled_version().to_owned())
}

pub(crate) fn version_marker_path() -> Option<PathBuf> {
    let current_exe = std::env::current_exe().ok()?;
    let root = install_root(&current_exe)?;
    Some(root.join(VERSION_MARKER_FILE))
}

fn installed_version() -> Option<String> {
    let marker = version_marker_path()?;
    let value = std::fs::read_to_string(marker).ok()?;
    let value = value.trim().trim_start_matches('\u{feff}');
    is_valid_version(value).then(|| value.to_owned())
}

fn install_root(current_exe: &Path) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    if let Some(app_bundle) = current_exe.ancestors().find(|path| {
        path.extension()
            .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("app"))
    }) {
        return app_bundle.parent().map(Path::to_path_buf);
    }

    current_exe.parent().map(Path::to_path_buf)
}

pub(crate) fn is_valid_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | '+')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_supported_version_strings() {
        assert!(is_valid_version("1.4.0"));
        assert!(is_valid_version("26.08.31.1055"));
        assert!(is_valid_version("1.4.0-beta+3"));
        assert!(!is_valid_version(""));
        assert!(!is_valid_version("1.4.0; rm -rf /"));
    }
}
