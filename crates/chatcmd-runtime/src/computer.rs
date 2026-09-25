//! Isolated, pointer-free browser automation through the Chrome DevTools Protocol.

mod browser_find;
mod cdp;
mod types;

pub use types::*;

use serde::Deserialize;
use std::{collections::HashMap, path::Path, process::Stdio, sync::Arc, time::Duration};
use tempfile::TempDir;
use tokio::{process::Child, sync::Mutex};

use crate::{OperationContext, RuntimeError, RuntimeResult};
use browser_find::find_browser;

const MAX_SESSIONS: usize = 4;
const MAX_ACTIONS: usize = 32;
const START_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Default)]
pub struct ComputerControlService {
    sessions: Arc<Mutex<HashMap<String, BrowserSession>>>,
}

struct BrowserSession {
    owner: SessionOwner,
    width: u32,
    height: u32,
    web_socket_url: String,
    client: Arc<Mutex<cdp::CdpClient>>,
    child: Child,
    _profile: TempDir,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionOwner {
    agent_id: String,
    task_id: Option<String>,
}

impl ComputerControlService {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn start(
        &self,
        context: &OperationContext,
        request: ComputerSessionStartRequest,
    ) -> RuntimeResult<ComputerSessionInfo> {
        validate_dimensions(request.width, request.height)?;
        cdp::validate_navigation_url(&request.start_url)?;
        let mut sessions = self.sessions.lock().await;
        sessions.retain(|_, session| {
            session
                .child
                .try_wait()
                .map_or(true, |status| status.is_none())
        });
        if sessions.len() >= MAX_SESSIONS {
            return Err(RuntimeError::busy("computer session limit reached"));
        }
        drop(sessions);

        let executable = find_browser(request.browser)?;
        let profile = tempfile::Builder::new()
            .prefix("chatcmd-computer-")
            .tempdir()
            .map_err(|error| backend_error(format!("failed to create browser profile: {error}")))?;
        let mut command = tokio::process::Command::new(&executable);
        command
            .arg("--headless=new")
            .arg("--remote-debugging-address=127.0.0.1")
            .arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", profile.path().display()))
            .arg(format!(
                "--window-size={},{}",
                request.width, request.height
            ))
            .args([
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-background-networking",
                "--disable-component-update",
                "--disable-default-apps",
                "--disable-extensions",
                "--disable-sync",
                "--metrics-recording-only",
                "--mute-audio",
                "about:blank",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|error| backend_error(format!("failed to start browser: {error}")))?;
        let port = wait_for_devtools_port(profile.path(), &mut child, context).await?;
        let web_socket_url = find_page_target(port).await?;
        let mut client = cdp::CdpClient::connect(&web_socket_url).await?;
        cdp::prepare_session(&mut client).await?;
        if request.start_url != "about:blank" {
            cdp::execute_actions(
                &mut client,
                &[ComputerAction::Navigate {
                    url: request.start_url,
                }],
            )
            .await
            .map_err(|failure| failure.error)?;
        }

        let session_id = uuid::Uuid::new_v4().to_string();
        let info = ComputerSessionInfo {
            session_id: session_id.clone(),
            browser: request.browser,
            width: request.width,
            height: request.height,
            headless: true,
            isolated_profile: true,
        };
        let session = BrowserSession {
            owner: owner(context),
            width: request.width,
            height: request.height,
            web_socket_url,
            client: Arc::new(Mutex::new(client)),
            child,
            _profile: profile,
        };
        self.sessions.lock().await.insert(session_id, session);
        Ok(info)
    }

    pub async fn observe(
        &self,
        context: &OperationContext,
        session_id: &str,
    ) -> RuntimeResult<ComputerObservation> {
        let session = self.session_access(context, session_id).await?;
        let mut client = session.client.lock().await;
        observation(&session, &mut client).await
    }

    pub async fn act(
        &self,
        context: &OperationContext,
        request: ComputerActRequest,
    ) -> RuntimeResult<ComputerObservation> {
        if request.actions.is_empty() || request.actions.len() > MAX_ACTIONS {
            return Err(RuntimeError::new(
                "invalid_arguments",
                "actions must contain between 1 and 32 items",
            ));
        }
        let session = self.session_access(context, &request.session_id).await?;
        validate_actions(&request.actions, session.width, session.height)?;
        let mut client = session.client.lock().await;
        let action_result = if client.is_broken() {
            Err(cdp::ActionFailure {
                completed_action_count: 0,
                error: RuntimeError::new(
                    "computer_session_connection_lost",
                    "browser connection was lost; observe the session before sending more actions",
                ),
            })
        } else {
            cdp::execute_actions(&mut client, &request.actions).await
        };
        let capture_after = request.screenshot_after
            || request
                .actions
                .iter()
                .any(|action| matches!(action, ComputerAction::Screenshot))
            || action_result.is_err();
        let mut result = if capture_after {
            match observation(&session, &mut client).await {
                Ok(observation) => observation,
                Err(error) => {
                    let mut observation = empty_observation(&session);
                    observation.verification_warning = Some(format!(
                        "browser action may already have occurred; observe again without replaying it: {error}"
                    ));
                    observation
                }
            }
        } else {
            empty_observation(&session)
        };
        result.completed_action_count = Some(match action_result {
            Ok(()) => request.actions.len(),
            Err(failure) => {
                let message = if failure.error.code == "computer_session_connection_lost" {
                    "no action was sent; the browser connection was lost; observe the session before continuing".to_owned()
                } else {
                    format!(
                        "action {} may have partially executed; inspect the browser state before continuing: {}",
                        failure.completed_action_count + 1,
                        failure.error
                    )
                };
                result.execution_warning = Some(ComputerExecutionWarning {
                    completed_action_count: failure.completed_action_count,
                    retry_action: false,
                    message,
                });
                failure.completed_action_count
            }
        });
        Ok(result)
    }

    pub async fn close(&self, context: &OperationContext, session_id: &str) -> RuntimeResult<()> {
        let mut sessions = self.sessions.lock().await;
        let session = sessions.get(session_id).ok_or_else(|| {
            RuntimeError::new("computer_session_not_found", "session was not found")
        })?;
        ensure_owner(&session.owner, context)?;
        let mut session = sessions.remove(session_id).ok_or_else(|| {
            RuntimeError::new("computer_session_not_found", "session was not found")
        })?;
        drop(sessions);
        if session.child.try_wait().ok().flatten().is_none() {
            session
                .child
                .kill()
                .await
                .map_err(|error| backend_error(format!("failed to stop browser: {error}")))?;
        }
        Ok(())
    }

    async fn session_access(
        &self,
        context: &OperationContext,
        session_id: &str,
    ) -> RuntimeResult<SessionAccess> {
        let mut sessions = self.sessions.lock().await;
        let session = sessions.get_mut(session_id).ok_or_else(|| {
            RuntimeError::new("computer_session_not_found", "session was not found")
        })?;
        ensure_owner(&session.owner, context)?;
        let ended = session
            .child
            .try_wait()
            .map_err(|error| backend_error(format!("failed to inspect browser process: {error}")))?
            .is_some();
        if ended {
            sessions.remove(session_id);
            return Err(RuntimeError::new(
                "computer_session_ended",
                "the isolated browser process has exited",
            ));
        }
        Ok(SessionAccess {
            session_id: session_id.to_owned(),
            width: session.width,
            height: session.height,
            web_socket_url: session.web_socket_url.clone(),
            client: session.client.clone(),
        })
    }
}

struct SessionAccess {
    session_id: String,
    width: u32,
    height: u32,
    web_socket_url: String,
    client: Arc<Mutex<cdp::CdpClient>>,
}

async fn observation(
    session: &SessionAccess,
    client: &mut cdp::CdpClient,
) -> RuntimeResult<ComputerObservation> {
    if client.is_broken() {
        reconnect_browser_session(client, &session.web_socket_url).await?;
    }
    let (url, title, screenshot_base64) = match cdp::capture(client).await {
        Ok(observation) => observation,
        Err(_) if client.is_broken() => {
            reconnect_browser_session(client, &session.web_socket_url).await?;
            cdp::capture(client).await?
        }
        Err(error) => return Err(error),
    };
    Ok(ComputerObservation {
        session_id: session.session_id.clone(),
        url,
        title,
        width: session.width,
        height: session.height,
        screenshot_base64: Some(screenshot_base64),
        verification_warning: None,
        completed_action_count: None,
        execution_warning: None,
    })
}

async fn reconnect_browser_session(client: &mut cdp::CdpClient, url: &str) -> RuntimeResult<()> {
    let mut replacement = cdp::CdpClient::connect(url).await?;
    cdp::prepare_session(&mut replacement).await?;
    *client = replacement;
    Ok(())
}

fn empty_observation(session: &SessionAccess) -> ComputerObservation {
    ComputerObservation {
        session_id: session.session_id.clone(),
        url: String::new(),
        title: String::new(),
        width: session.width,
        height: session.height,
        screenshot_base64: None,
        verification_warning: None,
        completed_action_count: None,
        execution_warning: None,
    }
}

fn owner(context: &OperationContext) -> SessionOwner {
    SessionOwner {
        agent_id: context.agent_id.clone(),
        task_id: context.task_id.clone(),
    }
}

fn ensure_owner(owner: &SessionOwner, context: &OperationContext) -> RuntimeResult<()> {
    if owner == &self::owner(context) {
        Ok(())
    } else {
        Err(RuntimeError::new(
            "computer_session_not_found",
            "session was not found",
        ))
    }
}

fn validate_dimensions(width: u32, height: u32) -> RuntimeResult<()> {
    if !(640..=1920).contains(&width) || !(480..=1080).contains(&height) {
        return Err(RuntimeError::new(
            "invalid_arguments",
            "browser dimensions must be within 640x480 and 1920x1080",
        ));
    }
    Ok(())
}

fn validate_actions(actions: &[ComputerAction], width: u32, height: u32) -> RuntimeResult<()> {
    for action in actions {
        match action {
            ComputerAction::Click { x, y, .. }
            | ComputerAction::DoubleClick { x, y, .. }
            | ComputerAction::Move { x, y } => validate_point(*x, *y, width, height)?,
            ComputerAction::Drag {
                start_x,
                start_y,
                end_x,
                end_y,
                duration_ms,
            } => {
                validate_point(*start_x, *start_y, width, height)?;
                validate_point(*end_x, *end_y, width, height)?;
                if !(1..=5_000).contains(duration_ms) {
                    return Err(invalid_action("drag durationMs must be between 1 and 5000"));
                }
            }
            ComputerAction::Scroll {
                x,
                y,
                delta_x,
                delta_y,
            } => {
                validate_point(*x, *y, width, height)?;
                if !delta_x.is_finite()
                    || !delta_y.is_finite()
                    || delta_x.abs() > 100_000.0
                    || delta_y.abs() > 100_000.0
                {
                    return Err(invalid_action("scroll delta is invalid or too large"));
                }
            }
            ComputerAction::Keypress { keys } if keys.is_empty() || keys.len() > 8 => {
                return Err(invalid_action("keypress requires between 1 and 8 keys"));
            }
            ComputerAction::Keypress { keys } if keys.iter().any(|key| key.len() > 64) => {
                return Err(invalid_action("keypress key names are limited to 64 bytes"));
            }
            ComputerAction::Type { text } if text.len() > 32 * 1024 => {
                return Err(invalid_action("typed text exceeds the 32 KiB action limit"));
            }
            ComputerAction::Navigate { url } => cdp::validate_navigation_url(url)?,
            ComputerAction::Wait { duration_ms } if !(1..=10_000).contains(duration_ms) => {
                return Err(invalid_action(
                    "wait durationMs must be between 1 and 10000",
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_point(x: f64, y: f64, width: u32, height: u32) -> RuntimeResult<()> {
    if x.is_finite()
        && y.is_finite()
        && x >= 0.0
        && y >= 0.0
        && x < f64::from(width)
        && y < f64::from(height)
    {
        Ok(())
    } else {
        Err(invalid_action(
            "action coordinates are outside the viewport",
        ))
    }
}

fn invalid_action(message: &str) -> RuntimeError {
    RuntimeError::new("invalid_arguments", message)
}

async fn wait_for_devtools_port(
    profile: &Path,
    child: &mut Child,
    context: &OperationContext,
) -> RuntimeResult<u16> {
    let active_port = profile.join("DevToolsActivePort");
    let deadline = tokio::time::Instant::now() + START_TIMEOUT;
    loop {
        if context.cancellation.is_cancelled() {
            let _ = child.kill().await;
            return Err(RuntimeError::new(
                "operationCancelled",
                "browser start was cancelled",
            ));
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| backend_error(format!("failed to inspect browser process: {error}")))?
        {
            return Err(backend_error(format!(
                "browser exited before DevTools became ready: {status}"
            )));
        }
        if let Ok(contents) = tokio::fs::read_to_string(&active_port).await
            && let Some(port) = contents.lines().next().and_then(|line| line.parse().ok())
        {
            return Ok(port);
        }
        if tokio::time::Instant::now() >= deadline {
            let _ = child.kill().await;
            return Err(backend_error("timed out waiting for Chrome DevTools"));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DebugTarget {
    #[serde(rename = "type")]
    target_type: String,
    web_socket_debugger_url: Option<String>,
}

async fn find_page_target(port: u16) -> RuntimeResult<String> {
    let url = format!("http://127.0.0.1:{port}/json/list");
    let targets = reqwest::Client::new()
        .get(url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|error| backend_error(format!("failed to query Chrome targets: {error}")))?
        .error_for_status()
        .map_err(|error| backend_error(format!("Chrome target query failed: {error}")))?
        .json::<Vec<DebugTarget>>()
        .await
        .map_err(|error| backend_error(format!("invalid Chrome target response: {error}")))?;
    targets
        .into_iter()
        .find(|target| target.target_type == "page")
        .and_then(|target| target.web_socket_debugger_url)
        .ok_or_else(|| backend_error("Chrome exposed no page target"))
}

fn backend_error(message: impl Into<String>) -> RuntimeError {
    let mut error = RuntimeError::new("computer_backend_error", message);
    error.retryable = true;
    error
}

#[cfg(test)]
mod tests;
