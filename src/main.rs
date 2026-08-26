mod api;
mod websocket;

use std::{net::SocketAddr, sync::Arc};

use axum::{Router, routing::get};
use tokio::sync::broadcast;
use tower_http::{
    cors::CorsLayer,
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use tracing::info;

use websocket::{AppState, ws_handler};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "chat_cmd_client=debug,tower_http=info".into()),
        )
        .init();

    let (event_tx, _) = broadcast::channel(256);
    let state = Arc::new(AppState::new(event_tx));

    let api_router = api::router(state.clone());
    let frontend =
        ServeDir::new("web/dist").not_found_service(ServeFile::new("web/dist/index.html"));

    let app = Router::new()
        .nest("/api", api_router)
        .route("/ws", get(ws_handler))
        .fallback_service(frontend)
        .with_state(state)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind server port");

    info!(%addr, "ChatCmdClient server started");
    axum::serve(listener, app)
        .await
        .expect("server terminated unexpectedly");
}
