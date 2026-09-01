use std::{
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use axum::Json;
use serde::Serialize;

use super::Problem;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ElevationStatus {
    supported: bool,
    elevated: bool,
}

pub(super) async fn elevation_status() -> Json<ElevationStatus> {
    Json(ElevationStatus {
        supported: cfg!(any(target_os = "windows", target_os = "macos")),
        elevated: is_elevated(),
    })
}

pub(super) async fn restart_elevated() -> Result<Json<ElevationStatus>, Problem> {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        if is_elevated() {
            return Ok(Json(ElevationStatus {
                supported: true,
                elevated: true,
            }));
        }

        spawn_elevated_copy().map_err(|error| {
            Problem::new(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "Elevation failed",
                error,
            )
        })?;

        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(450));
            std::process::exit(0);
        });

        return Ok(Json(ElevationStatus {
            supported: true,
            elevated: false,
        }));
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    Err(Problem::new(
        axum::http::StatusCode::NOT_IMPLEMENTED,
        "Elevation is unavailable",
        "Administrator restart is currently supported only on Windows and macOS.",
    ))
}

#[cfg(target_os = "windows")]
fn is_elevated() -> bool {
    hidden_powershell()
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn is_elevated() -> bool {
    Command::new("/usr/bin/id")
        .arg("-u")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim() == "0")
        .unwrap_or(false)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn is_elevated() -> bool {
    false
}

#[cfg(target_os = "windows")]
fn spawn_elevated_copy() -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let working_directory = std::env::current_dir().map_err(|error| error.to_string())?;
    let command = format!(
        "$ErrorActionPreference='Stop'; Start-Process -FilePath {} -WorkingDirectory {} -ArgumentList @('--elevated-restart-delay-ms','900') -Verb RunAs -WindowStyle Hidden -PassThru | Out-Null",
        ps_quote_path(&executable),
        ps_quote_path(&working_directory),
    );

    let output = hidden_powershell()
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &command,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("failed to start Windows elevation prompt: {error}"))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if stderr.is_empty() {
        Err("Windows did not start the elevated ChatCMD process.".to_owned())
    } else {
        Err(stderr)
    }
}

#[cfg(target_os = "macos")]
fn spawn_elevated_copy() -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let working_directory = std::env::current_dir().map_err(|error| error.to_string())?;

    let mut environment_exports = String::new();
    for key in ["HOME", "PATH", "USER", "LOGNAME", "SHELL", "TMPDIR"] {
        if let Some(value) = std::env::var_os(key) {
            environment_exports.push_str("export ");
            environment_exports.push_str(key);
            environment_exports.push('=');
            environment_exports.push_str(&sh_quote_text(&value.to_string_lossy()));
            environment_exports.push_str("; ");
        }
    }

    let shell_command = format!(
        "{}cd {}; nohup {} --elevated-restart-delay-ms 900 >/dev/null 2>&1 &",
        environment_exports,
        sh_quote_path(&working_directory),
        sh_quote_path(&executable),
    );
    let apple_script = format!(
        "do shell script \"{}\" with administrator privileges",
        apple_script_string(&shell_command),
    );

    let output = Command::new("/usr/bin/osascript")
        .args(["-e", &apple_script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("failed to start macOS administrator prompt: {error}"))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if stderr.is_empty() {
        Err("macOS did not start the elevated ChatCMD process.".to_owned())
    } else {
        Err(stderr)
    }
}

#[cfg(target_os = "macos")]
fn apple_script_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(target_os = "macos")]
fn sh_quote_text(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(target_os = "macos")]
fn sh_quote_path(path: &Path) -> String {
    sh_quote_text(&path.to_string_lossy())
}

#[cfg(target_os = "windows")]
fn hidden_powershell() -> Command {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = Command::new("powershell.exe");
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

#[cfg(target_os = "windows")]
fn ps_quote_path(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "''"))
}
