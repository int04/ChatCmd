use std::cmp::Ordering;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum UpdatePhase {
    Idle,
    Checking,
    Available,
    UpToDate,
    Downloading,
    Extracting,
    Preparing,
    ReadyToRestart,
    Restarting,
    Failed,
    Unsupported,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateStatus {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub note: Option<String>,
    pub platform: String,
    pub architecture: String,
    pub phase: UpdatePhase,
    pub update_available: bool,
    pub download_available: bool,
    pub progress_percent: Option<u8>,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub message: Option<String>,
}

impl UpdateStatus {
    pub(crate) fn initial() -> Self {
        let target = UpdateTarget::current();
        Self {
            current_version: crate::version::app_version(),
            latest_version: None,
            note: None,
            platform: target.platform.to_owned(),
            architecture: target.architecture.to_owned(),
            phase: if target.supported {
                UpdatePhase::Idle
            } else {
                UpdatePhase::Unsupported
            },
            update_available: false,
            download_available: false,
            progress_percent: None,
            downloaded_bytes: 0,
            total_bytes: None,
            message: (!target.supported)
                .then(|| "Updates are supported only on Windows and macOS.".to_owned()),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct RemoteVersion {
    pub note: String,
    pub version: String,
    #[serde(default)]
    pub window_32: Option<String>,
    #[serde(default)]
    pub window_64: Option<String>,
    #[serde(default)]
    pub mac_intel: Option<String>,
    #[serde(default)]
    pub mac_silicon: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct UpdateAsset {
    pub version: String,
    pub note: String,
    pub object_key: String,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct UpdateTarget {
    pub platform: &'static str,
    pub architecture: &'static str,
    pub api_target: Option<&'static str>,
    pub supported: bool,
}

impl UpdateTarget {
    pub(crate) fn current() -> Self {
        let architecture = std::env::consts::ARCH;
        #[cfg(target_os = "windows")]
        {
            return Self {
                platform: "windows",
                architecture,
                api_target: Some("windows"),
                supported: matches!(architecture, "x86_64" | "x86"),
            };
        }
        #[cfg(target_os = "macos")]
        {
            return Self {
                platform: "macos",
                architecture,
                api_target: Some("macos"),
                supported: matches!(architecture, "aarch64" | "x86_64"),
            };
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        Self {
            platform: std::env::consts::OS,
            architecture,
            api_target: None,
            supported: false,
        }
    }

    pub(crate) fn asset(self, remote: &RemoteVersion) -> Option<UpdateAsset> {
        let key = match (self.platform, self.architecture) {
            ("windows", "x86_64") => remote.window_64.as_deref(),
            ("windows", "x86") => remote.window_32.as_deref(),
            ("macos", "aarch64") => remote.mac_silicon.as_deref(),
            ("macos", "x86_64") => remote.mac_intel.as_deref(),
            _ => None,
        }?;
        let object_key = validate_object_key(key).ok()?.to_owned();
        Some(UpdateAsset {
            version: remote.version.trim().to_owned(),
            note: remote.note.trim().to_owned(),
            object_key,
        })
    }
}

pub(crate) fn validate_object_key(value: &str) -> Result<&str> {
    let key = value.trim().trim_start_matches('/');
    if key.is_empty() || key.contains("://") || key.contains('\\') {
        bail!("invalid update object key");
    }
    if key
        .split('/')
        .any(|segment| segment == "." || segment == "..")
    {
        bail!("invalid update object key");
    }
    Ok(key)
}

pub(crate) fn is_remote_newer(remote: &str, current: &str) -> bool {
    let remote = normalize_version(remote);
    let current = normalize_version(current);
    if remote == current {
        return false;
    }
    match (numeric_version(&remote), numeric_version(&current)) {
        (Some(remote), Some(current)) if remote.len() == current.len() => {
            compare_numeric_versions(&remote, &current).is_gt()
        }
        // Legacy ChatCMD builds used a 4-part timestamp while the version API uses
        // semantic versions. They are different schemes, so allow a one-time migration.
        _ => true,
    }
}

fn normalize_version(value: &str) -> String {
    value.trim().trim_start_matches(['v', 'V']).to_owned()
}

fn numeric_version(value: &str) -> Option<Vec<u64>> {
    if value.is_empty() {
        return None;
    }
    value
        .split('.')
        .map(|part| part.parse::<u64>().ok())
        .collect()
}

fn compare_numeric_versions(left: &[u64], right: &[u64]) -> Ordering {
    let length = left.len().max(right.len());
    for index in 0..length {
        let left = left.get(index).copied().unwrap_or(0);
        let right = right.get(index).copied().unwrap_or(0);
        match left.cmp(&right) {
            Ordering::Equal => {}
            ordering => return ordering,
        }
    }
    Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_supported_version_formats() {
        assert!(is_remote_newer("1.4.0", "1.3.9"));
        assert!(!is_remote_newer("1.4.0", "1.4.0"));
        assert!(!is_remote_newer("1.3.9", "1.4.0"));
        assert!(is_remote_newer("1.4.0", "26.08.31.1055"));
        assert!(is_remote_newer("26.08.31.1100", "26.08.31.1055"));
        assert!(!is_remote_newer("26.08.30.2359", "26.08.31.0001"));
    }
}
