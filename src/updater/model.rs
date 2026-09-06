use std::cmp::Ordering;

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum UpdatePhase {
    Idle,
    Checking,
    Available,
    UpToDate,
    Downloading,
    Verifying,
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
    pub release_tag: Option<String>,
    pub release_name: Option<String>,
    pub release_url: Option<String>,
    pub note: Option<String>,
    pub platform: String,
    pub architecture: String,
    pub debug_build: bool,
    pub phase: UpdatePhase,
    pub update_available: bool,
    pub download_available: bool,
    pub asset_name: Option<String>,
    pub checksum_verified: bool,
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
            release_tag: None,
            release_name: None,
            release_url: None,
            note: None,
            platform: target.platform.to_owned(),
            architecture: target.architecture.to_owned(),
            debug_build: cfg!(debug_assertions),
            phase: if target.supported {
                UpdatePhase::Idle
            } else {
                UpdatePhase::Unsupported
            },
            update_available: false,
            download_available: false,
            asset_name: None,
            checksum_verified: false,
            progress_percent: None,
            downloaded_bytes: 0,
            total_bytes: None,
            message: (!target.supported)
                .then(|| "Automatic updates are supported only on Windows and macOS.".to_owned()),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ReleaseSelection {
    pub version: String,
    pub tag: String,
    pub name: String,
    pub release_url: String,
    pub note: String,
    pub asset: Option<UpdateAsset>,
}

#[derive(Clone, Debug)]
pub(crate) struct UpdateAsset {
    pub name: String,
    pub download_url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct UpdateTarget {
    pub platform: &'static str,
    pub architecture: &'static str,
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
                supported: matches!(architecture, "x86_64" | "x86"),
            };
        }
        #[cfg(target_os = "macos")]
        {
            return Self {
                platform: "macos",
                architecture,
                supported: matches!(architecture, "aarch64" | "x86_64"),
            };
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        Self {
            platform: std::env::consts::OS,
            architecture,
            supported: false,
        }
    }

    pub(crate) fn asset_name(self) -> Option<&'static str> {
        match (self.platform, self.architecture) {
            ("windows", "x86_64") => Some("ChatCMD-windows-x64.zip"),
            ("windows", "x86") => Some("ChatCMD-windows-x86.zip"),
            ("macos", "aarch64") => Some("ChatCMD-macos-apple-silicon.zip"),
            ("macos", "x86_64") => Some("ChatCMD-macos-intel.zip"),
            _ => None,
        }
    }
}

pub(crate) fn is_remote_newer(remote: &str, current: &str) -> bool {
    let remote = normalize_version(remote);
    let current = normalize_version(current);
    if remote.eq_ignore_ascii_case(&current) {
        return false;
    }
    match (numeric_version(&remote), numeric_version(&current)) {
        (Some(remote), Some(current)) => compare_numeric_versions(&remote, &current).is_gt(),
        _ => remote != current,
    }
}

fn normalize_version(value: &str) -> String {
    value
        .trim()
        .trim_start_matches(['v', 'V'])
        .trim_start_matches('.')
        .to_owned()
}

fn numeric_version(value: &str) -> Option<Vec<u64>> {
    let base = value.split(['-', '+', '_']).next()?;
    if base.is_empty() {
        return None;
    }
    base.split('.')
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
    fn compares_timestamp_and_semver_versions() {
        assert!(is_remote_newer("26.09.04.0001", "26.09.03.2207"));
        assert!(is_remote_newer("26.09.03.2208", "26.09.03.2207"));
        assert!(!is_remote_newer("26.09.03.2207", "26.09.03.2207"));
        assert!(!is_remote_newer("26.09.03.2206", "26.09.03.2207"));
        assert!(is_remote_newer("26.09.03.2207", "0.1.0"));
        assert!(!is_remote_newer("v.26.09.03", "26.09.03.2207"));
    }

    #[test]
    fn chooses_stable_release_asset_names() {
        let windows = UpdateTarget {
            platform: "windows",
            architecture: "x86_64",
            supported: true,
        };
        assert_eq!(windows.asset_name(), Some("ChatCMD-windows-x64.zip"));
        let mac = UpdateTarget {
            platform: "macos",
            architecture: "aarch64",
            supported: true,
        };
        assert_eq!(mac.asset_name(), Some("ChatCMD-macos-apple-silicon.zip"));
    }
}
