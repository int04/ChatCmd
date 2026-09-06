use std::sync::Arc;

use axum::{Json, extract::State};

use crate::{updater::UpdateStatus, websocket::AppState};

pub(super) async fn update_status(State(state): State<Arc<AppState>>) -> Json<UpdateStatus> {
    Json(state.updater.status())
}

pub(super) async fn check_update(State(state): State<Arc<AppState>>) -> Json<UpdateStatus> {
    Json(state.updater.check_latest().await)
}

pub(super) async fn start_update(State(state): State<Arc<AppState>>) -> Json<UpdateStatus> {
    Json(state.updater.clone().start_update())
}

pub(super) async fn restart_update(State(state): State<Arc<AppState>>) -> Json<UpdateStatus> {
    Json(state.updater.restart_to_install())
}
