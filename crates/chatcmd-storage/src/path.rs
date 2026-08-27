use std::path::{Path, PathBuf};

/// Data-path resolution failures.
#[derive(Debug, thiserror::Error)]
pub enum DataPathError {
    #[error("cannot resolve local data directory: missing {0}")]
    MissingEnvironment(&'static str),
}

/// Resolves the cross-platform database path, honoring an explicit override.
pub fn resolve_database_path(override_path: Option<&Path>) -> Result<PathBuf, DataPathError> {
    if let Some(path) = override_path {
        return Ok(path.to_path_buf());
    }

    if cfg!(target_os = "windows") {
        let root = std::env::var_os("LOCALAPPDATA")
            .ok_or(DataPathError::MissingEnvironment("LOCALAPPDATA"))?;
        return Ok(PathBuf::from(root)
            .join("ChatCmdClient")
            .join("data")
            .join("chatcmd.db"));
    }

    let home = || {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(DataPathError::MissingEnvironment("HOME"))
    };
    if cfg!(target_os = "macos") {
        return Ok(home()?
            .join("Library")
            .join("Application Support")
            .join("ChatCmdClient")
            .join("chatcmd.db"));
    }

    if let Some(root) = std::env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(root)
            .join("chatcmd-client")
            .join("chatcmd.db"));
    }
    Ok(home()?
        .join(".local")
        .join("share")
        .join("chatcmd-client")
        .join("chatcmd.db"))
}
