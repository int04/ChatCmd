mod agents;
mod sessions;
mod settings;

use agents::*;
use sessions::*;
use settings::*;

use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
};
use chatcmd_core::{
    AgentId, McpAgent, McpAgentStore, NewMcpAgent, Setting, SettingsStore, TaskId, TaskStore,
    ToolCapability, ToolCatalogStore,
};
use serde_json::{Value, json};
use sqlx::Row;

use crate::websocket::{AppEvent, AppState};

pub(crate) fn router() -> Router<Arc<AppState>> {
    let local = Router::new()
        .route("/overview", get(overview))
        .route("/mcp/status", get(mcp_status))
        .route("/mcp/agents", get(list_agents).post(create_agent))
        .route(
            "/mcp/agents/{id}",
            get(get_agent).put(update_agent).delete(delete_agent),
        )
        .route("/mcp/agents/{id}/rotate-secret", post(rotate_secret))
        .route("/mcp/agents/{id}/enabled", patch(set_enabled))
        .route("/mcp/tools", get(tools))
        .route("/mcp/tool-presets", get(presets))
        .route("/tasks", get(tasks))
        .route("/tasks/{id}", get(task))
        .route("/tasks/{id}/{action}", post(task_action))
        .route("/sessions", get(sessions))
        .route("/sessions/{id}", get(session))
        .route("/sessions/{id}/{action}", post(session_action))
        .route("/skills", get(skills))
        .route("/skills/{id}", get(skill))
        .route("/settings", get(settings).put(save_settings))
        .layer(middleware::from_fn(management_header));
    Router::new()
        .route("/health", get(health))
        .route("/info", get(info))
        .nest("/local", local)
}

async fn management_header(request: Request, next: Next) -> Result<Response, Problem> {
    if request.headers().get("x-chatcmdclient") == Some(&HeaderValue::from_static("local-ui")) {
        Ok(next.run(request).await)
    } else {
        Err(Problem::new(
            StatusCode::FORBIDDEN,
            "Forbidden",
            "local UI header is required",
        ))
    }
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok", "service": "ChatCmdClient" }))
}

async fn info(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({
        "name": "ChatCmdClient", "version": env!("CARGO_PKG_VERSION"),
        "api": "/api", "mcp": "/mcp/{token}", "websocket": "/ws",
        "connectedClients": state.connected_clients()
    }))
}

async fn overview(State(state): State<Arc<AppState>>) -> Result<Json<Value>, Problem> {
    let task_counts = sqlx::query("SELECT status,COUNT(*) AS count FROM tasks GROUP BY status")
        .fetch_all(state.repository.pool())
        .await
        .map_err(db_problem)?;
    let session_counts =
        sqlx::query("SELECT status,COUNT(*) AS count FROM terminal_sessions GROUP BY status")
            .fetch_all(state.repository.pool())
            .await
            .map_err(db_problem)?;
    let approval_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM approvals WHERE state='pending'")
            .fetch_one(state.repository.pool())
            .await
            .map_err(db_problem)?;
    let tasks = counts(&task_counts);
    let terminal = counts(&session_counts);
    Ok(Json(json!({
        "app": { "version": env!("CARGO_PKG_VERSION"), "startedAtUtc": state.started_at, "state": "ready" },
        "device": { "id": state.device.id.as_str(), "name": state.device.name, "platform": state.device.platform, "osVersion": state.device.os_version, "architecture": state.device.architecture },
        "mcp": { "state": "listening", "endpoint": mcp_endpoint_template(&state), "connectedClients": state.connected_clients() },
        "database": { "state": "ready", "path": state.database_path, "schemaVersion": chatcmd_storage::CURRENT_SCHEMA_VERSION.to_string() },
        "terminal": { "defaultShell": default_shell(), "activeSessions": count_active(&terminal), "totalSessions": total(&terminal), "failedSessions": *terminal.get("failed").unwrap_or(&0) },
        "tasks": { "running": *tasks.get("running").unwrap_or(&0), "completed": *tasks.get("completed").unwrap_or(&0), "failed": *tasks.get("failed").unwrap_or(&0), "approvals": approval_count },
        "sessions": { "active": count_active(&terminal), "total": total(&terminal) }, "recentEvents": []
    })))
}

async fn mcp_status(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(
        json!({ "state": "listening", "endpoint": mcp_endpoint_template(&state), "connectedClients": state.connected_clients() }),
    )
}

async fn tasks(State(state): State<Arc<AppState>>) -> Result<Json<Value>, Problem> {
    let tasks = state
        .repository
        .list_tasks(500)
        .await
        .map_err(storage_problem)?;
    Ok(Json(Value::Array(
        tasks.into_iter().map(task_value).collect(),
    )))
}

async fn task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Problem> {
    task_detail(&state, &id).await
}

async fn task_action(
    State(state): State<Arc<AppState>>,
    Path((id, action)): Path<(String, String)>,
) -> Result<Json<Value>, Problem> {
    let status = match action.as_str() {
        "stop" => "stopped",
        "retry" | "resume" => "pending",
        _ => {
            return Err(Problem::new(
                StatusCode::BAD_REQUEST,
                "Invalid action",
                "unsupported task action",
            ));
        }
    };
    let affected = sqlx::query("UPDATE tasks SET status=?,stopped_at_ms=CASE WHEN ?='stopped' THEN ? ELSE NULL END,updated_at_ms=? WHERE id=?")
        .bind(status).bind(status).bind(now_ms()).bind(now_ms()).bind(&id).execute(state.repository.pool()).await.map_err(db_problem)?.rows_affected();
    if affected == 0 {
        return Err(not_found());
    }
    task_detail(&state, &id).await
}

async fn task_detail(state: &Arc<AppState>, id: &str) -> Result<Json<Value>, Problem> {
    let task = state
        .repository
        .task(&TaskId::new(id).map_err(|_| bad_id())?)
        .await
        .map_err(storage_problem)?
        .ok_or_else(not_found)?;
    let rows = sqlx::query("SELECT event_id,turn_id,session_id,kind,payload_json,created_at_ms FROM timeline_events WHERE task_id=? ORDER BY created_at_ms,event_id LIMIT 1000")
        .bind(id).fetch_all(state.repository.pool()).await.map_err(db_problem)?;
    let events = rows.iter().map(timeline_row).collect::<Vec<_>>();
    Ok(Json(
        json!({ "task": task_value(task), "turns": [], "events": events, "executionMode": state.repository.execution_mode(Some(&TaskId::new(id).map_err(|_| bad_id())?)).await.map_err(storage_problem)?.as_str() }),
    ))
}

async fn skills(State(state): State<Arc<AppState>>) -> Result<Json<Value>, Problem> {
    let values = state.skills.list().await.map_err(runtime_problem)?;
    Ok(Json(Value::Array(values.into_iter().enumerate().map(|(index, skill)| json!({ "id": skill.id, "name": skill.title, "source": skill.source, "precedence": index, "enabled": true, "shadowed": false, "description": skill.description })).collect())))
}
async fn skill(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Problem> {
    let skill = state.skills.read(&id).await.map_err(runtime_problem)?;
    Ok(Json(
        json!({ "id": skill.id, "name": skill.name, "source": skill.source, "enabled": true, "shadowed": false, "content": skill.instructions }),
    ))
}

fn task_value(task: chatcmd_core::Task) -> Value {
    json!({"id":task.id.as_str(),"title":task.title,"status":task.status.as_str(),"updatedAtUtc":iso_ms(task.updated_at_ms),"createdAtUtc":iso_ms(task.created_at_ms),"generation":task.generation,"activeSessionId":task.active_session_id.map(|id|id.into_string())})
}
pub(super) fn timeline_row(row: &sqlx::sqlite::SqliteRow) -> Value {
    json!({"id":row.get::<String,_>("event_id"),"type":row.get::<String,_>("kind"),"occurredAt":iso_ms(row.get("created_at_ms")),"turnId":row.get::<Option<String>,_>("turn_id"),"sessionId":row.get::<Option<String>,_>("session_id"),"payload":serde_json::from_str::<Value>(&row.get::<String,_>("payload_json")).unwrap_or(Value::Null)})
}
fn counts(rows: &[sqlx::sqlite::SqliteRow]) -> BTreeMap<String, i64> {
    rows.iter()
        .map(|row| (row.get("status"), row.get("count")))
        .collect()
}
fn total(values: &BTreeMap<String, i64>) -> i64 {
    values.values().sum()
}
fn count_active(values: &BTreeMap<String, i64>) -> i64 {
    values.get("running").unwrap_or(&0) + values.get("starting").unwrap_or(&0)
}
fn default_shell() -> String {
    if cfg!(windows) {
        "powershell.exe".to_owned()
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned())
    }
}
pub(crate) fn iso_now() -> String {
    iso_ms(now_ms())
}
pub(super) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}
pub(super) fn iso_ms(ms: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .ok()
        .and_then(|value| {
            value
                .format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_owned())
}

#[derive(Debug)]
pub(super) struct Problem {
    status: StatusCode,
    title: &'static str,
    detail: &'static str,
}
impl Problem {
    pub(super) const fn new(status: StatusCode, title: &'static str, detail: &'static str) -> Self {
        Self {
            status,
            title,
            detail,
        }
    }
}
impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let mut response=(self.status,Json(json!({"type":"about:blank","title":self.title,"status":self.status.as_u16(),"detail":self.detail}))).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        response
    }
}
pub(super) fn not_found() -> Problem {
    Problem::new(
        StatusCode::NOT_FOUND,
        "Not found",
        "requested local record was not found",
    )
}
fn bad_id() -> Problem {
    Problem::new(
        StatusCode::BAD_REQUEST,
        "Invalid identifier",
        "identifier must be a non-empty string",
    )
}
pub(super) fn db_problem(_: sqlx::Error) -> Problem {
    Problem::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Storage error",
        "local storage operation failed",
    )
}
pub(super) fn runtime_problem(_: chatcmd_runtime::RuntimeError) -> Problem {
    Problem::new(
        StatusCode::BAD_REQUEST,
        "Runtime error",
        "local runtime operation failed",
    )
}
fn storage_problem(error: chatcmd_core::StorageError) -> Problem {
    match error {
        chatcmd_core::StorageError::NotFound(_) => not_found(),
        chatcmd_core::StorageError::Conflict(_) => Problem::new(
            StatusCode::CONFLICT,
            "Conflict",
            "record conflicts with existing local data",
        ),
        chatcmd_core::StorageError::InvalidData(_) => Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid data",
            "stored or supplied data is invalid",
        ),
        _ => Problem::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Storage error",
            "local storage operation failed",
        ),
    }
}
