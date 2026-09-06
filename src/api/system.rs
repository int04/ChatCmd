use std::{path::Path, process::Command, time::Duration};

#[cfg(target_os = "windows")]
use std::path::PathBuf;

#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::process::Stdio;

#[cfg(target_os = "macos")]
use std::{
    io::{ErrorKind, Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use axum::Json;
use serde::{Deserialize, Serialize};

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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ExitApplicationResponse {
    closing: bool,
}

pub(super) async fn exit_application() -> Json<ExitApplicationResponse> {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(300));
        std::process::exit(0);
    });

    Json(ExitApplicationResponse { closing: true })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OpenSystemTargetResponse {
    opened: bool,
    target: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OpenBrowserExtensionsRequest {
    browser: String,
}

pub(super) async fn open_chatgpt_extension_folder()
-> Result<Json<OpenSystemTargetResponse>, Problem> {
    let executable = std::env::current_exe()
        .map_err(|error| system_open_problem("Extension folder unavailable", error.to_string()))?;
    let install_root = crate::version::install_root(&executable).ok_or_else(|| {
        system_open_problem(
            "Extension folder unavailable",
            "Could not resolve the ChatCMD install folder.",
        )
    })?;
    let extension_dir = install_root.join("chatgpt-extension");
    if !extension_dir.is_dir() {
        return Err(Problem::new(
            axum::http::StatusCode::NOT_FOUND,
            "Extension folder not found",
            format!(
                "The packaged ChatGPT extension folder was not found at {}.",
                extension_dir.display()
            ),
        ));
    }
    open_path(&extension_dir)
        .map_err(|error| system_open_problem("Could not open extension folder", error))?;
    Ok(Json(OpenSystemTargetResponse {
        opened: true,
        target: extension_dir.to_string_lossy().into_owned(),
    }))
}

pub(super) async fn open_browser_extensions(
    Json(request): Json<OpenBrowserExtensionsRequest>,
) -> Result<Json<OpenSystemTargetResponse>, Problem> {
    let (browser, target) = match request.browser.trim().to_ascii_lowercase().as_str() {
        "chrome" => ("chrome", "chrome://extensions/"),
        "edge" => ("edge", "edge://extensions/"),
        "brave" => ("brave", "brave://extensions/"),
        _ => {
            return Err(Problem::new(
                axum::http::StatusCode::BAD_REQUEST,
                "Unsupported browser",
                "Browser must be chrome, edge, or brave.",
            ));
        }
    };
    open_browser_target(browser, target)
        .map_err(|error| system_open_problem("Could not open browser extensions", error))?;
    Ok(Json(OpenSystemTargetResponse {
        opened: true,
        target: target.to_owned(),
    }))
}

fn system_open_problem(title: &'static str, detail: impl Into<String>) -> Problem {
    Problem::new(
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        title,
        detail.into(),
    )
}

fn open_path(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        Command::new("explorer.exe")
            .arg(path)
            .spawn()
            .map_err(|error| format!("failed to open Explorer: {error}"))?;
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("/usr/bin/open")
            .arg(path)
            .spawn()
            .map_err(|error| format!("failed to open Finder: {error}"))?;
        return Ok(());
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map_err(|error| format!("failed to open file manager: {error}"))?;
        Ok(())
    }
}

fn open_browser_target(browser: &str, target: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        for executable in windows_browser_candidates(browser) {
            let explicit_path = executable.components().count() > 1;
            if explicit_path && !executable.is_file() {
                continue;
            }
            if Command::new(&executable).arg(target).spawn().is_ok() {
                return Ok(());
            }
        }
        return Err(format!(
            "Could not find a supported {browser} executable on this computer."
        ));
    }
    #[cfg(target_os = "macos")]
    {
        let application = match browser {
            "edge" => "Microsoft Edge",
            "brave" => "Brave Browser",
            _ => "Google Chrome",
        };
        let status = Command::new("/usr/bin/open")
            .args(["-a", application, target])
            .status()
            .map_err(|error| format!("failed to launch {application}: {error}"))?;
        return status
            .success()
            .then_some(())
            .ok_or_else(|| format!("{application} could not open {target}."));
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let candidates: &[&str] = match browser {
            "edge" => &["microsoft-edge", "microsoft-edge-stable"],
            "brave" => &["brave-browser", "brave-browser-stable"],
            _ => &[
                "google-chrome",
                "google-chrome-stable",
                "chromium",
                "chromium-browser",
            ],
        };
        for executable in candidates {
            if Command::new(executable).arg(target).spawn().is_ok() {
                return Ok(());
            }
        }
        Err(format!(
            "Could not find a supported {browser} executable on this computer."
        ))
    }
}

#[cfg(target_os = "windows")]
fn windows_browser_candidates(browser: &str) -> Vec<PathBuf> {
    let (executable, vendor_path) = match browser {
        "edge" => ("msedge.exe", ["Microsoft", "Edge", "Application"]),
        "brave" => (
            "brave.exe",
            ["BraveSoftware", "Brave-Browser", "Application"],
        ),
        _ => ("chrome.exe", ["Google", "Chrome", "Application"]),
    };
    let mut candidates = Vec::new();
    for variable in ["LOCALAPPDATA", "PROGRAMFILES", "PROGRAMFILES(X86)"] {
        if let Some(root) = std::env::var_os(variable) {
            let mut path = PathBuf::from(root);
            for segment in vendor_path {
                path.push(segment);
            }
            path.push(executable);
            candidates.push(path);
        }
    }
    candidates.push(PathBuf::from(executable));
    candidates
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

        Ok(Json(ElevationStatus {
            supported: true,
            elevated: false,
        }))
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
        .is_some_and(|output| String::from_utf8_lossy(&output.stdout).trim() == "0")
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
    let (ready_listener, ready_port, ready_token) = elevated_ready_listener()?;

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

    // Keep the elevated executable in the foreground of the privileged shell.
    // macOS can reap a background process created by `do shell script` as soon
    // as that shell exits, even when nohup is used. The replacement process
    // sends the handshake before its startup delay; only then may this process
    // return success and schedule its own exit.
    let shell_command = format!(
        "{}cd {}; exec {} --elevated-restart-ready-port {} --elevated-restart-ready-token {} --elevated-restart-delay-ms 1500 </dev/null >/dev/null 2>&1",
        environment_exports,
        sh_quote_path(&working_directory),
        sh_quote_path(&executable),
        ready_port,
        sh_quote_text(&ready_token),
    );
    let apple_script = format!(
        "do shell script \"{}\" with administrator privileges",
        apple_script_string(&shell_command),
    );

    let mut child = Command::new("/usr/bin/osascript")
        .args(["-e", &apple_script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to start macOS administrator prompt: {error}"))?;

    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("failed to monitor macOS administrator prompt: {error}"))?
        {
            let mut stderr = String::new();
            if let Some(mut stream) = child.stderr.take() {
                let _ = stream.read_to_string(&mut stderr);
            }
            let detail = stderr.trim();
            return if detail.is_empty() {
                Err(format!(
                    "macOS did not start the elevated ChatCMD process (status {status})."
                ))
            } else {
                Err(detail.to_owned())
            };
        }

        match ready_listener.accept() {
            Ok((mut stream, _)) => {
                let mut received = String::new();
                stream.read_to_string(&mut received).map_err(|error| {
                    format!("failed to read macOS elevation handshake: {error}")
                })?;
                if received == ready_token {
                    return Ok(());
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => {
                return Err(format!(
                    "failed to accept macOS elevation handshake: {error}"
                ));
            }
        }

        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(target_os = "macos")]
fn elevated_ready_listener() -> Result<(TcpListener, u16, String), String> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(|error| format!("failed to bind macOS elevation handshake: {error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("failed to configure macOS elevation handshake: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("failed to inspect macOS elevation handshake: {error}"))?
        .port();
    Ok((listener, port, uuid::Uuid::new_v4().to_string()))
}

#[cfg(target_os = "macos")]
pub(crate) fn signal_elevated_restart_ready(port: u16, token: &str) -> Result<(), String> {
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
        .map_err(|error| format!("failed to connect macOS elevation handshake: {error}"))?;
    stream
        .write_all(token.as_bytes())
        .map_err(|error| format!("failed to signal macOS elevation readiness: {error}"))
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
