use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::websocket::{AppEvent, AppState};

pub fn router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/health", get(health))
        .route("/info", get(info))
        .route("/events", post(publish_event))
        .with_state(state)
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    service: &'static str,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        service: "ChatCmdClient",
    })
}

async fn info(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({
        "name": "ChatCmdClient",
        "version": env!("CARGO_PKG_VERSION"),
        "api": "/api",
        "websocket": "/ws",
        "connectedClients": state.connected_clients(),
    }))
}

#[derive(Debug, Deserialize)]
struct PublishEventRequest {
    #[serde(default = "default_event_type")]
    event_type: String,
    payload: Value,
}

fn default_event_type() -> String {
    "message".to_owned()
}

async fn publish_event(
    State(state): State<Arc<AppState>>,
    Json(request): Json<PublishEventRequest>,
) -> Json<Value> {
    let event = AppEvent::new(request.event_type, request.payload);
    let delivered = state.publish(event.clone());

    Json(json!({
        "accepted": true,
        "delivered": delivered,
        "event": event,
    }))
}
