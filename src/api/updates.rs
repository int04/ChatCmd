use std::sync::Arc;

use axum::{Json, extract::State, http::StatusCode};

use crate::{updater::UpdateStatus, websocket::AppState};

use super::Problem;

pub(super) async fn update_status(State(state): State<Arc<AppState>>) -> Json<UpdateStatus> {
    Json(state.updater.status())
}

pub(super) async fn check_update(
    State(state): State<Arc<AppState>>,
) -> Result<Json<UpdateStatus>, Problem> {
    state
        .updater
        .check_latest(state.backend_api.base_url())
        .await
        .map(Json)
        .map_err(|error| update_problem(StatusCode::BAD_GATEWAY, error))
}

pub(super) async fn start_update(State(state): State<Arc<AppState>>) -> Json<UpdateStatus> {
    Json(
        state
            .updater
            .start_update(state.backend_api.base_url().to_owned()),
    )
}

pub(super) async fn restart_update(
    State(state): State<Arc<AppState>>,
) -> Result<Json<UpdateStatus>, Problem> {
    state
        .updater
        .restart_to_install()
        .map(Json)
        .map_err(|error| update_problem(StatusCode::CONFLICT, error))
}

fn update_problem(status: StatusCode, error: anyhow::Error) -> Problem {
    Problem::new(status, "Update error", format!("{error:#}"))
}
