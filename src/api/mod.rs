mod agents;
mod chatgpt;
mod chatgpt_support;
mod sessions;
mod settings;
mod subagents;
mod task_controls;

use agents::*;
use chatgpt::*;
use sessions::*;
use settings::*;
use subagents::*;
use task_controls::*;

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
        .route("/chatgpt/requests", post(create_request))
        .route("/chatgpt/requests/{id}", get(request))
        .route("/chatgpt/tasks/{task_id}", get(task_bridge))
        .route("/chatgpt/tasks/{task_id}/messages", post(continue_message))
        .route("/chatgpt/tasks/{task_id}/stop", post(stop_message))
        .route("/chatgpt/bridge/{request_id}/started", post(bridge_started))
        .route("/chatgpt/bridge/{request_id}/result", post(bridge_result))
        .route("/tasks", get(tasks))
        .route("/tasks/{id}", get(task))
        .route(
            "/tasks/{id}/command-execution-mode",
            get(task_execution_mode).put(set_task_execution_mode),
        )
        .route(
            "/tasks/{id}/activities/{activity_id}/approval",
            post(resolve_task_approval),
        )
        .route(
            "/tasks/{task_id}/activities/{activity_id}/stop",
            post(stop_task_activity),
        )
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
    let caller = request.headers().get("x-chatcmdclient");
    if caller == Some(&HeaderValue::from_static("local-ui"))
        || caller == Some(&HeaderValue::from_static("chatgpt-extension"))
    {
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
    let summary_rows = sqlx::query("SELECT timeline_events.task_id,COUNT(DISTINCT timeline_events.turn_id) AS turn_count,(SELECT COALESCE(json_extract(latest.payload_json,'$.content'),json_extract(latest.payload_json,'$.message'),json_extract(latest.payload_json,'$.text'),json_extract(latest.payload_json,'$.response')) FROM timeline_events latest WHERE latest.task_id=timeline_events.task_id AND COALESCE(json_extract(latest.payload_json,'$.content'),json_extract(latest.payload_json,'$.message'),json_extract(latest.payload_json,'$.text'),json_extract(latest.payload_json,'$.response')) IS NOT NULL ORDER BY latest.created_at_ms DESC,latest.event_id DESC LIMIT 1) AS output_preview FROM timeline_events GROUP BY timeline_events.task_id")
        .fetch_all(state.repository.pool()).await.map_err(db_problem)?;
    let summaries = summary_rows
        .into_iter()
        .map(|row| {
            (
                row.get::<String, _>("task_id"),
                (
                    row.get::<i64, _>("turn_count"),
                    row.get::<Option<String>, _>("output_preview"),
                ),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let final_rows = sqlx::query("SELECT task_id,COUNT(*) AS final_response_count FROM timeline_events WHERE actor='assistant' AND kind='status' AND json_extract(payload_json,'$.status')='completed' GROUP BY task_id")
        .fetch_all(state.repository.pool()).await.map_err(db_problem)?;
    let final_counts = final_rows
        .into_iter()
        .map(|row| {
            (
                row.get::<String, _>("task_id"),
                row.get::<i64, _>("final_response_count"),
            )
        })
        .collect::<BTreeMap<_, _>>();
    Ok(Json(Value::Array(
        tasks
            .into_iter()
            .map(|task| {
                let id = task.id.as_str().to_owned();
                let mut value = task_value(task);
                if let Some(object) = value.as_object_mut() {
                    let (turn_count, preview) = summaries.get(&id).cloned().unwrap_or((0, None));
                    object.insert("turnCount".to_owned(), json!(turn_count));
                    object.insert(
                        "finalResponseCount".to_owned(),
                        json!(final_counts.get(&id).copied().unwrap_or(0)),
                    );
                    if let Some(preview) = preview.filter(|value| !value.trim().is_empty()) {
                        object.insert(
                            "outputPreview".to_owned(),
                            Value::String(compact_preview(&preview)),
                        );
                    }
                }
                value
            })
            .collect(),
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
    if action == "stop" {
        return stop_conversation(&state, &id).await;
    }
    let status = match action.as_str() {
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
    let (subagent_parent, subagents) = task_subagent_data(state, id).await?;
    let mut task_value = task_value(task);
    if let (Some(parent), Some(object)) = (subagent_parent, task_value.as_object_mut()) {
        object.insert("isSubagent".to_owned(), Value::Bool(true));
        object.insert("parentTaskId".to_owned(), Value::String(parent.task_id));
        object.insert("parentTurnId".to_owned(), Value::String(parent.turn_id));
        object.insert("agentName".to_owned(), Value::String(parent.name));
    }
    let rows = sqlx::query("SELECT event_id,turn_id,session_id,kind,payload_json,created_at_ms FROM timeline_events WHERE task_id=? ORDER BY created_at_ms DESC,event_id DESC LIMIT 1000")
        .bind(id).fetch_all(state.repository.pool()).await.map_err(db_problem)?;
    let terminal_rows = sqlx::query("SELECT event_id,turn_id,session_id,kind,stream,payload,payload_encoding,created_at_ms FROM terminal_event_chunks WHERE task_id=? ORDER BY created_at_ms DESC,event_id DESC LIMIT 5000")
        .bind(id).fetch_all(state.repository.pool()).await.map_err(db_problem)?;
    let mut ordered_events = rows
        .iter()
        .map(|row| {
            let timestamp = row.get::<i64, _>("created_at_ms");
            let event_id = row.get::<String, _>("event_id");
            (timestamp, event_id, timeline_row(row))
        })
        .collect::<Vec<_>>();
    ordered_events.extend(terminal_rows.iter().map(|row| {
        let timestamp = row.get::<i64, _>("created_at_ms");
        let event_id = row.get::<String, _>("event_id");
        let payload = row.get::<Vec<u8>, _>("payload");
        let text = String::from_utf8_lossy(&payload).into_owned();
        let value = json!({
            "id": event_id,
            "type": row.get::<String, _>("kind"),
            "occurredAt": iso_ms(timestamp),
            "turnId": row.get::<Option<String>, _>("turn_id"),
            "sessionId": row.get::<Option<String>, _>("session_id"),
            "payload": {
                "text": text,
                "stream": row.get::<Option<String>, _>("stream"),
                "encoding": row.get::<String, _>("payload_encoding")
            }
        });
        (timestamp, event_id, value)
    }));
    ordered_events.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    let events = ordered_events
        .into_iter()
        .map(|(_, _, value)| value)
        .collect::<Vec<_>>();
    Ok(Json(
        json!({ "task": task_value, "turns": [], "events": events, "subagents": subagents, "executionMode": execution_mode_name(state.repository.execution_mode(Some(&TaskId::new(id).map_err(|_| bad_id())?)).await.map_err(storage_problem)?) }),
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

fn compact_preview(value: &str) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = normalized.chars();
    let preview = chars.by_ref().take(180).collect::<String>();
    if chars.next().is_some() {
        format!("{preview}…")
    } else {
        preview
    }
}

fn task_value(task: chatcmd_core::Task) -> Value {
    json!({"id":task.id.as_str(),"title":task.title,"source":task.source,"status":task.status.as_str(),"updatedAtUtc":iso_ms(task.updated_at_ms),"createdAtUtc":iso_ms(task.created_at_ms),"generation":task.generation,"activeSessionId":task.active_session_id.map(|id|id.into_string())})
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
