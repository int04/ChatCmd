use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use axum::{
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::Response,
};
use chatcmd_storage::SqliteRepository;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppEvent {
    pub id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub occurred_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub payload: Value,
}

impl AppEvent {
    pub(crate) fn new(event_type: impl Into<String>, payload: Value) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            event_type: event_type.into(),
            occurred_at: crate::api::iso_now(),
            task_id: None,
            session_id: None,
            turn_id: None,
            payload: redact_value(payload, 0),
        }
    }
}

pub(crate) struct AppState {
    pub repository: SqliteRepository,
    pub database_path: String,
    pub bind_address: String,
    pub port: u16,
    pub started_at: String,
    pub device: chatcmd_core::LocalDevice,
    pub shell: chatcmd_runtime::ShellRuntime,
    pub skills: chatcmd_runtime::SkillService,
    events: broadcast::Sender<AppEvent>,
    connected_clients: AtomicUsize,
}

impl AppState {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        repository: SqliteRepository,
        database_path: String,
        bind_address: String,
        port: u16,
        device: chatcmd_core::LocalDevice,
        shell: chatcmd_runtime::ShellRuntime,
        skills: chatcmd_runtime::SkillService,
        events: broadcast::Sender<AppEvent>,
    ) -> Self {
        Self {
            repository,
            database_path,
            bind_address,
            port,
            started_at: crate::api::iso_now(),
            device,
            shell,
            skills,
            events,
            connected_clients: AtomicUsize::new(0),
        }
    }

    pub(crate) fn connected_clients(&self) -> usize {
        self.connected_clients.load(Ordering::Relaxed)
    }

    pub(crate) fn publish(&self, event: AppEvent) {
        let _ = self.events.send(event);
    }
}

pub(crate) async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    state.connected_clients.fetch_add(1, Ordering::Relaxed);
    let mut receiver = state.events.subscribe();
    let (mut sender, mut incoming) = socket.split();
    let connected = AppEvent::new(
        "system.connected",
        json!({ "connectedClients": state.connected_clients() }),
    );
    if let Ok(text) = serde_json::to_string(&connected) {
        let _ = sender.send(Message::Text(text.into())).await;
    }
    loop {
        tokio::select! {
            event = receiver.recv() => match event {
                Ok(event) => {
                    let Ok(text) = serde_json::to_string(&event) else { continue };
                    if sender.send(Message::Text(text.into())).await.is_err() { break; }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            },
            message = incoming.next() => match message {
                Some(Ok(Message::Ping(data))) => {
                    if sender.send(Message::Pong(data)).await.is_err() { break; }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                _ => {}
            }
        }
    }
    state.connected_clients.fetch_sub(1, Ordering::Relaxed);
}

fn redact_value(value: Value, depth: usize) -> Value {
    if depth >= 8 {
        return Value::String("[TRUNCATED]".to_owned());
    }
    match value {
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| {
                    let lower = key.to_ascii_lowercase();
                    let value = if lower.contains("token")
                        || lower.contains("secret")
                        || lower == "authorization"
                    {
                        Value::String("[REDACTED]".to_owned())
                    } else {
                        redact_value(value, depth + 1)
                    };
                    (key, value)
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .take(500)
                .map(|value| redact_value(value, depth + 1))
                .collect(),
        ),
        Value::String(value) if value.len() > 65_536 => {
            Value::String(value.chars().take(65_536).collect())
        }
        other => other,
    }
}
