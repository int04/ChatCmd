use sha2::{Digest, Sha256};
#[cfg(target_os = "linux")]
use std::fs;
#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::process::Command;

const MACHINE_ID_DOMAIN: &str = "ChatCMD:";

pub(crate) fn machine_id() -> Option<String> {
    raw_machine_id().map(|raw| hash_machine_id(&raw))
}

pub(crate) fn os_version() -> Option<String> {
    sysinfo::System::long_os_version()
        .or_else(sysinfo::System::os_version)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn hash_machine_id(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(MACHINE_ID_DOMAIN.as_bytes());
    hasher.update(raw.trim().as_bytes());
    format!("{:x}", hasher.finalize())
}

fn raw_machine_id() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        return windows_machine_guid();
    }

    #[cfg(target_os = "macos")]
    {
        return macos_platform_uuid();
    }

    #[cfg(target_os = "linux")]
    {
        return linux_machine_id();
    }

    #[allow(unreachable_code)]
    None
}

#[cfg(target_os = "windows")]
fn windows_machine_guid() -> Option<String> {
    let output = Command::new("reg.exe")
        .args([
            "query",
            r"HKLM\SOFTWARE\Microsoft\Cryptography",
            "/v",
            "MachineGuid",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().find_map(|line| {
        let trimmed = line.trim();
        if !trimmed.to_ascii_lowercase().starts_with("machineguid") {
            return None;
        }
        trimmed
            .split_whitespace()
            .last()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

#[cfg(target_os = "macos")]
fn macos_platform_uuid() -> Option<String> {
    let output = Command::new("ioreg")
        .args(["-rd1", "-c", "IOPlatformExpertDevice"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().find_map(|line| {
        let (_, value) = line.split_once("IOPlatformUUID")?;
        let value = value.split_once('=')?.1.trim().trim_matches('"').trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

#[cfg(target_os = "linux")]
fn linux_machine_id() -> Option<String> {
    ["/etc/machine-id", "/var/lib/dbus/machine-id"]
        .into_iter()
        .find_map(|path| {
            fs::read_to_string(path)
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_id_hash_is_namespaced_and_deterministic() {
        assert_eq!(hash_machine_id("abc"), hash_machine_id(" abc \n"));
        assert_ne!(hash_machine_id("abc"), hash_machine_id("def"));
        assert_eq!(hash_machine_id("abc").len(), 64);
    }
}
