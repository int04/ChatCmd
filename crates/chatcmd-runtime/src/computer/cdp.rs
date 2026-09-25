use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use super::{ComputerAction, MouseButton};
use crate::{RuntimeError, RuntimeResult};

const CDP_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_SCREENSHOT_BASE64_BYTES: usize = 12 * 1024 * 1024;

pub(super) async fn prepare_session(web_socket_url: &str) -> RuntimeResult<()> {
    let mut client = CdpClient::connect(web_socket_url).await?;
    client.command("Page.enable", json!({})).await?;
    client
        .command(
            "Browser.setDownloadBehavior",
            json!({"behavior": "deny", "eventsEnabled": false}),
        )
        .await?;
    Ok(())
}

pub(super) async fn execute_actions(
    web_socket_url: &str,
    actions: &[ComputerAction],
) -> RuntimeResult<()> {
    let mut client = CdpClient::connect(web_socket_url).await?;
    client.command("Page.enable", json!({})).await?;
    for action in actions {
        execute_action(&mut client, action).await?;
    }
    Ok(())
}

pub(super) async fn capture(web_socket_url: &str) -> RuntimeResult<(String, String, String)> {
    let mut client = CdpClient::connect(web_socket_url).await?;
    client.command("Page.enable", json!({})).await?;
    let page = client
        .command(
            "Runtime.evaluate",
            json!({
                "expression": "({url: location.href, title: document.title})",
                "returnByValue": true
            }),
        )
        .await?;
    let value = page
        .pointer("/result/result/value")
        .and_then(Value::as_object);
    let url = value
        .and_then(|object| object.get("url"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let title = value
        .and_then(|object| object.get("title"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let image = client
        .command(
            "Page.captureScreenshot",
            json!({"format": "png", "fromSurface": true, "captureBeyondViewport": false}),
        )
        .await?;
    let screenshot = image
        .pointer("/result/data")
        .and_then(Value::as_str)
        .ok_or_else(|| cdp_error("Chrome returned no screenshot data"))?
        .to_owned();
    if screenshot.len() > MAX_SCREENSHOT_BASE64_BYTES {
        return Err(RuntimeError::new(
            "computer_screenshot_too_large",
            "captured screenshot exceeds the 12 MiB encoded response limit",
        ));
    }
    Ok((url, title, screenshot))
}

async fn execute_action(client: &mut CdpClient, action: &ComputerAction) -> RuntimeResult<()> {
    match action {
        ComputerAction::Click { x, y, button } => click(client, *x, *y, *button, 1).await,
        ComputerAction::DoubleClick { x, y, button } => {
            click(client, *x, *y, *button, 1).await?;
            click(client, *x, *y, *button, 2).await
        }
        ComputerAction::Move { x, y } => {
            client
                .command(
                    "Input.dispatchMouseEvent",
                    mouse_event("mouseMoved", *x, *y),
                )
                .await?;
            Ok(())
        }
        ComputerAction::Drag {
            start_x,
            start_y,
            end_x,
            end_y,
            duration_ms,
        } => drag(client, *start_x, *start_y, *end_x, *end_y, *duration_ms).await,
        ComputerAction::Scroll {
            x,
            y,
            delta_x,
            delta_y,
        } => {
            client
                .command(
                    "Input.dispatchMouseEvent",
                    json!({
                        "type": "mouseWheel", "x": x, "y": y,
                        "deltaX": delta_x, "deltaY": delta_y
                    }),
                )
                .await?;
            Ok(())
        }
        ComputerAction::Keypress { keys } => keypress(client, keys).await,
        ComputerAction::Type { text } => {
            if text.len() > 32 * 1024 {
                return Err(RuntimeError::new(
                    "computer_input_too_large",
                    "typed text exceeds the 32 KiB action limit",
                ));
            }
            client
                .command("Input.insertText", json!({"text": text}))
                .await?;
            Ok(())
        }
        ComputerAction::Navigate { url } => {
            validate_navigation_url(url)?;
            client.command("Page.navigate", json!({"url": url})).await?;
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok(())
        }
        ComputerAction::Wait { duration_ms } => {
            tokio::time::sleep(Duration::from_millis((*duration_ms).clamp(1, 10_000))).await;
            Ok(())
        }
        ComputerAction::Screenshot => Ok(()),
    }
}

async fn click(
    client: &mut CdpClient,
    x: f64,
    y: f64,
    button: MouseButton,
    click_count: u8,
) -> RuntimeResult<()> {
    for event_type in ["mousePressed", "mouseReleased"] {
        client
            .command(
                "Input.dispatchMouseEvent",
                json!({
                    "type": event_type, "x": x, "y": y,
                    "button": button.as_cdp(), "clickCount": click_count
                }),
            )
            .await?;
    }
    Ok(())
}

fn mouse_event(event_type: &str, x: f64, y: f64) -> Value {
    json!({"type": event_type, "x": x, "y": y, "button": "none"})
}

async fn drag(
    client: &mut CdpClient,
    start_x: f64,
    start_y: f64,
    end_x: f64,
    end_y: f64,
    duration_ms: u64,
) -> RuntimeResult<()> {
    client
        .command(
            "Input.dispatchMouseEvent",
            mouse_event("mouseMoved", start_x, start_y),
        )
        .await?;
    client
        .command(
            "Input.dispatchMouseEvent",
            json!({"type":"mousePressed","x":start_x,"y":start_y,"button":"left","clickCount":1}),
        )
        .await?;
    let steps = 10_u32;
    let delay = Duration::from_millis(duration_ms.clamp(50, 5_000) / u64::from(steps));
    for step in 1..=steps {
        let progress = f64::from(step) / f64::from(steps);
        let x = start_x + ((end_x - start_x) * progress);
        let y = start_y + ((end_y - start_y) * progress);
        client
            .command(
                "Input.dispatchMouseEvent",
                json!({"type":"mouseMoved","x":x,"y":y,"button":"left","buttons":1}),
            )
            .await?;
        tokio::time::sleep(delay).await;
    }
    client
        .command(
            "Input.dispatchMouseEvent",
            json!({"type":"mouseReleased","x":end_x,"y":end_y,"button":"left","clickCount":1}),
        )
        .await?;
    Ok(())
}

async fn keypress(client: &mut CdpClient, keys: &[String]) -> RuntimeResult<()> {
    if keys.is_empty() || keys.len() > 8 {
        return Err(RuntimeError::new(
            "invalid_arguments",
            "keypress requires between 1 and 8 keys",
        ));
    }
    let mut modifiers = 0_u8;
    for key in keys {
        if let Some(bit) = modifier_bit(key) {
            modifiers |= bit;
            key_event(client, "keyDown", key, modifiers).await?;
        } else {
            key_event(client, "keyDown", key, modifiers).await?;
            key_event(client, "keyUp", key, modifiers).await?;
        }
    }
    for key in keys.iter().rev() {
        if modifier_bit(key).is_some() {
            key_event(client, "keyUp", key, modifiers).await?;
            modifiers &= !modifier_bit(key).unwrap_or_default();
        }
    }
    Ok(())
}

fn modifier_bit(key: &str) -> Option<u8> {
    match key.to_ascii_uppercase().as_str() {
        "ALT" => Some(1),
        "CTRL" | "CONTROL" => Some(2),
        "META" | "CMD" | "COMMAND" => Some(4),
        "SHIFT" => Some(8),
        _ => None,
    }
}

async fn key_event(
    client: &mut CdpClient,
    event_type: &str,
    requested_key: &str,
    modifiers: u8,
) -> RuntimeResult<()> {
    let (key, code, virtual_key_code) = normalize_key(requested_key);
    client
        .command(
            "Input.dispatchKeyEvent",
            json!({
                "type": event_type, "key": key, "code": code,
                "windowsVirtualKeyCode": virtual_key_code, "modifiers": modifiers
            }),
        )
        .await?;
    Ok(())
}

fn normalize_key(key: &str) -> (String, String, u32) {
    match key.to_ascii_uppercase().as_str() {
        "ENTER" | "RETURN" => ("Enter".into(), "Enter".into(), 13),
        "TAB" => ("Tab".into(), "Tab".into(), 9),
        "ESC" | "ESCAPE" => ("Escape".into(), "Escape".into(), 27),
        "BACKSPACE" => ("Backspace".into(), "Backspace".into(), 8),
        "DELETE" => ("Delete".into(), "Delete".into(), 46),
        "ARROWUP" | "UP" => ("ArrowUp".into(), "ArrowUp".into(), 38),
        "ARROWDOWN" | "DOWN" => ("ArrowDown".into(), "ArrowDown".into(), 40),
        "ARROWLEFT" | "LEFT" => ("ArrowLeft".into(), "ArrowLeft".into(), 37),
        "ARROWRIGHT" | "RIGHT" => ("ArrowRight".into(), "ArrowRight".into(), 39),
        "CTRL" | "CONTROL" => ("Control".into(), "ControlLeft".into(), 17),
        "ALT" => ("Alt".into(), "AltLeft".into(), 18),
        "SHIFT" => ("Shift".into(), "ShiftLeft".into(), 16),
        "META" | "CMD" | "COMMAND" => ("Meta".into(), "MetaLeft".into(), 91),
        other if other.chars().count() == 1 => {
            let character = other.chars().next().unwrap_or_default();
            (
                character.to_string(),
                format!("Key{character}"),
                u32::from(character),
            )
        }
        _ => (key.to_owned(), key.to_owned(), 0),
    }
}

pub(super) fn validate_navigation_url(url: &str) -> RuntimeResult<()> {
    if url == "about:blank" {
        return Ok(());
    }
    let parsed = reqwest::Url::parse(url).map_err(|_| {
        RuntimeError::new("invalid_arguments", "URL must be an absolute http(s) URL")
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(RuntimeError::new(
            "computer_url_denied",
            "only http(s) URLs and about:blank are allowed",
        ));
    }
    Ok(())
}

struct CdpClient {
    stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    next_id: u64,
}

impl CdpClient {
    async fn connect(web_socket_url: &str) -> RuntimeResult<Self> {
        let (stream, _) = tokio::time::timeout(CDP_TIMEOUT, connect_async(web_socket_url))
            .await
            .map_err(|_| cdp_error("timed out connecting to Chrome DevTools"))?
            .map_err(|error| {
                cdp_error(&format!("failed to connect to Chrome DevTools: {error}"))
            })?;
        Ok(Self { stream, next_id: 1 })
    }

    async fn command(&mut self, method: &str, params: Value) -> RuntimeResult<Value> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let payload = json!({"id": id, "method": method, "params": params}).to_string();
        self.stream
            .send(Message::Text(payload.into()))
            .await
            .map_err(|error| cdp_error(&format!("failed to send Chrome command: {error}")))?;
        loop {
            let message = tokio::time::timeout(CDP_TIMEOUT, self.stream.next())
                .await
                .map_err(|_| cdp_error("Chrome command timed out"))?
                .ok_or_else(|| cdp_error("Chrome DevTools connection closed"))?
                .map_err(|error| cdp_error(&format!("Chrome DevTools read failed: {error}")))?;
            let Message::Text(text) = message else {
                continue;
            };
            let response: Value = serde_json::from_str(&text)
                .map_err(|_| cdp_error("Chrome returned an invalid DevTools response"))?;
            if response.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = response.get("error") {
                return Err(cdp_error(&format!(
                    "Chrome command {method} failed: {error}"
                )));
            }
            return Ok(response);
        }
    }
}

fn cdp_error(message: &str) -> RuntimeError {
    let mut error = RuntimeError::new("computer_backend_error", message);
    error.retryable = true;
    error
}
