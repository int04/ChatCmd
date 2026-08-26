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
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
pub struct AppEvent {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub event_type: String,
    pub payload: Value,
}

impl AppEvent {
    pub fn new(event_type: String, payload: Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            event_type,
            payload,
        }
    }
}

pub struct AppState {
    events: broadcast::Sender<AppEvent>,
    connected_clients: AtomicUsize,
}

impl AppState {
    pub fn new(events: broadcast::Sender<AppEvent>) -> Self {
        Self {
            events,
            connected_clients: AtomicUsize::new(0),
        }
    }

    pub fn connected_clients(&self) -> usize {
        self.connected_clients.load(Ordering::Relaxed)
    }

    pub fn publish(&self, event: AppEvent) -> usize {
        self.events.send(event).unwrap_or(0)
    }
}

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    state.connected_clients.fetch_add(1, Ordering::Relaxed);
    let mut receiver = state.events.subscribe();
    let (mut sender, mut incoming) = socket.split();

    let welcome = json!({
        "type": "system.connected",
        "payload": {
            "message": "WebSocket connected",
            "connectedClients": state.connected_clients(),
        }
    });
    let _ = sender.send(Message::Text(welcome.to_string().into())).await;

    loop {
        tokio::select! {
            event = receiver.recv() => {
                match event {
                    Ok(event) => {
                        let Ok(text) = serde_json::to_string(&event) else { continue; };
                        if sender.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            message = incoming.next() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        let event = AppEvent::new("client.message".to_owned(), json!({ "message": text.to_string() }));
                        state.publish(event);
                    }
                    Some(Ok(Message::Ping(data))) => {
                        if sender.send(Message::Pong(data)).await.is_err() { break; }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }

    state.connected_clients.fetch_sub(1, Ordering::Relaxed);
}
