use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path, Query, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
};
use chatcmd_core::{
    AgentId, McpAgent, McpAgentStore, NewMcpAgent, Setting, SettingsStore, TaskId, TaskStore,
    ToolCapability, ToolCatalogStore,
};
use serde::Deserialize;
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
        "api": "/api", "mcp": "/mcp", "websocket": "/ws",
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
        "mcp": { "state": "listening", "endpoint": format!("http://{}:{}/mcp", state.bind_address, state.port), "connectedClients": state.connected_clients() },
        "database": { "state": "ready", "path": state.database_path, "schemaVersion": chatcmd_storage::CURRENT_SCHEMA_VERSION.to_string() },
        "terminal": { "defaultShell": default_shell(), "activeSessions": count_active(&terminal), "totalSessions": total(&terminal), "failedSessions": *terminal.get("failed").unwrap_or(&0) },
        "tasks": { "running": *tasks.get("running").unwrap_or(&0), "completed": *tasks.get("completed").unwrap_or(&0), "failed": *tasks.get("failed").unwrap_or(&0), "approvals": approval_count },
        "sessions": { "active": count_active(&terminal), "total": total(&terminal) }, "recentEvents": []
    })))
}

async fn mcp_status(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(
        json!({ "state": "listening", "endpoint": format!("http://{}:{}/mcp", state.bind_address, state.port), "connectedClients": state.connected_clients() }),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentInput {
    name: String,
    enabled: bool,
    project_folder: String,
    #[serde(default)]
    preset_id: Option<String>,
    tool_ids: Vec<String>,
}

async fn list_agents(State(state): State<Arc<AppState>>) -> Result<Json<Value>, Problem> {
    let agents = state
        .repository
        .list_agents()
        .await
        .map_err(storage_problem)?;
    Ok(Json(Value::Array(agent_values(&state, agents).await?)))
}

async fn get_agent(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Problem> {
    let id = agent_id(id)?;
    let agent = state
        .repository
        .agent(&id)
        .await
        .map_err(storage_problem)?
        .ok_or_else(not_found)?;
    Ok(Json(agent_value(&state, agent).await?))
}

async fn create_agent(
    State(state): State<Arc<AppState>>,
    Json(input): Json<AgentInput>,
) -> Result<(StatusCode, Json<Value>), Problem> {
    validate_agent_input(&input)?;
    let tool_ids = resolve_tools(&state, &input).await?;
    let result = state
        .repository
        .create_agent(NewMcpAgent {
            id: None,
            name: input.name,
            enabled: input.enabled,
            project_folder: optional(input.project_folder),
        })
        .await
        .map_err(storage_problem)?;
    state
        .repository
        .set_agent_allowed_tools(&result.agent.id, &tool_ids)
        .await
        .map_err(storage_problem)?;
    let agent = agent_value(&state, result.agent).await?;
    let secret = result.secret.expose_once();
    state.publish(AppEvent::new(
        "agent.created",
        json!({ "agentId": agent["id"] }),
    ));
    Ok((
        StatusCode::CREATED,
        Json(json!({ "agent": agent, "secret": secret })),
    ))
}

async fn update_agent(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<AgentInput>,
) -> Result<Json<Value>, Problem> {
    validate_agent_input(&input)?;
    let id = agent_id(id)?;
    let tools = resolve_tools(&state, &input).await?;
    let agent = state
        .repository
        .update_agent(
            &id,
            NewMcpAgent {
                id: None,
                name: input.name,
                enabled: input.enabled,
                project_folder: optional(input.project_folder),
            },
        )
        .await
        .map_err(storage_problem)?;
    state
        .repository
        .set_agent_allowed_tools(&id, &tools)
        .await
        .map_err(storage_problem)?;
    Ok(Json(agent_value(&state, agent).await?))
}

async fn delete_agent(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, Problem> {
    state
        .repository
        .delete_agent(&agent_id(id)?)
        .await
        .map_err(storage_problem)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn rotate_secret(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Problem> {
    let result = state
        .repository
        .rotate_agent_secret(&agent_id(id)?)
        .await
        .map_err(storage_problem)?;
    let agent = agent_value(&state, result.agent).await?;
    Ok(Json(
        json!({ "agent": agent, "secret": result.secret.expose_once() }),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnabledInput {
    enabled: bool,
}
async fn set_enabled(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<EnabledInput>,
) -> Result<Json<Value>, Problem> {
    let id = agent_id(id)?;
    state
        .repository
        .set_agent_enabled(&id, input.enabled)
        .await
        .map_err(storage_problem)?;
    let agent = state
        .repository
        .agent(&id)
        .await
        .map_err(storage_problem)?
        .ok_or_else(not_found)?;
    Ok(Json(agent_value(&state, agent).await?))
}

async fn tools(State(state): State<Arc<AppState>>) -> Result<Json<Value>, Problem> {
    let tools = state
        .repository
        .list_tools()
        .await
        .map_err(storage_problem)?;
    Ok(Json(Value::Array(tools.into_iter().map(|tool| json!({
        "id": tool.id, "name": tool.title, "description": tool.description, "group": tool.group_id,
        "dangerous": tool.capabilities.contains(&ToolCapability::Destructive)
    })).collect())))
}

async fn presets(State(state): State<Arc<AppState>>) -> Result<Json<Value>, Problem> {
    let presets = state
        .repository
        .list_presets()
        .await
        .map_err(storage_problem)?;
    Ok(Json(Value::Array(presets.into_iter().map(|preset| json!({ "id": preset.id, "name": preset.name, "description": preset.description, "toolIds": preset.tool_ids })).collect())))
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

async fn sessions(State(state): State<Arc<AppState>>) -> Result<Json<Value>, Problem> {
    let rows = sqlx::query("SELECT id,task_id,executable,process_id,status,working_directory,created_at_ms,updated_at_ms,closed_at_ms FROM terminal_sessions ORDER BY updated_at_ms DESC LIMIT 500")
        .fetch_all(state.repository.pool()).await.map_err(db_problem)?;
    Ok(Json(Value::Array(rows.iter().map(session_row).collect())))
}

#[derive(Default, Deserialize)]
struct Cursor {
    cursor: Option<String>,
}
async fn session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(cursor): Query<Cursor>,
) -> Result<Json<Value>, Problem> {
    session_detail(&state, &id, cursor.cursor.as_deref()).await
}

async fn session_action(
    State(state): State<Arc<AppState>>,
    Path((id, action)): Path<(String, String)>,
) -> Result<Json<Value>, Problem> {
    let status = match action.as_str() {
        "close" => "closed",
        "stop" | "kill" => "interrupted",
        _ => {
            return Err(Problem::new(
                StatusCode::BAD_REQUEST,
                "Invalid action",
                "unsupported session action",
            ));
        }
    };
    let context = chatcmd_runtime::OperationContext::new(
        uuid::Uuid::new_v4().to_string(),
        "local-ui",
        "shell_close",
    );
    if let Err(error) = state
        .shell
        .close(&context, &id, matches!(action.as_str(), "stop" | "kill"))
        .await
        && error.code != "session_not_found"
    {
        return Err(runtime_problem(error));
    }
    let affected = sqlx::query(
        "UPDATE terminal_sessions SET status=?,closed_at_ms=?,updated_at_ms=? WHERE id=?",
    )
    .bind(status)
    .bind(now_ms())
    .bind(now_ms())
    .bind(&id)
    .execute(state.repository.pool())
    .await
    .map_err(db_problem)?
    .rows_affected();
    if affected == 0 {
        return Err(not_found());
    }
    session_detail(&state, &id, None).await
}

async fn session_detail(
    state: &Arc<AppState>,
    id: &str,
    cursor: Option<&str>,
) -> Result<Json<Value>, Problem> {
    let row = sqlx::query("SELECT id,task_id,executable,process_id,status,working_directory,created_at_ms,updated_at_ms,closed_at_ms FROM terminal_sessions WHERE id=?")
        .bind(id).fetch_optional(state.repository.pool()).await.map_err(db_problem)?.ok_or_else(not_found)?;
    let after = cursor
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(-1);
    let rows = sqlx::query("SELECT sequence,event_id,task_id,turn_id,kind,payload,created_at_ms FROM terminal_event_chunks WHERE session_id=? AND sequence>? ORDER BY sequence LIMIT 1001")
        .bind(id).bind(after).fetch_all(state.repository.pool()).await.map_err(db_problem)?;
    let truncated = rows.len() > 1000;
    let events = rows.iter().take(1000).map(|row| json!({ "id": row.get::<String,_>("event_id"), "type": row.get::<String,_>("kind"), "occurredAt": iso_ms(row.get("created_at_ms")), "taskId": row.get::<Option<String>,_>("task_id"), "sessionId": id, "turnId": row.get::<Option<String>,_>("turn_id"), "payload": { "data": String::from_utf8_lossy(&row.get::<Vec<u8>,_>("payload")) } })).collect::<Vec<_>>();
    let next = rows
        .iter()
        .take(1000)
        .next_back()
        .map(|row| row.get::<i64, _>("sequence").to_string());
    Ok(Json(
        json!({ "session": session_row(&row), "events": events, "nextCursor": next, "truncated": truncated }),
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

async fn settings(State(state): State<Arc<AppState>>) -> Result<Json<Value>, Problem> {
    Ok(Json(settings_value(&state).await?))
}
async fn save_settings(
    State(state): State<Arc<AppState>>,
    Json(value): Json<Value>,
) -> Result<Json<Value>, Problem> {
    let object = value.as_object().ok_or_else(|| {
        Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid settings",
            "JSON object required",
        )
    })?;
    let port = object.get("port").and_then(Value::as_u64).ok_or_else(|| {
        Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid settings",
            "port is required",
        )
    })?;
    if !(1..=65_535).contains(&port) {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid settings",
            "port must be 1..65535",
        ));
    }
    for (key, value) in object {
        state
            .repository
            .set_setting(&Setting {
                key: format!("ui_{key}"),
                value_json: value.to_string(),
                updated_at_ms: now_ms(),
            })
            .await
            .map_err(storage_problem)?;
    }
    Ok(Json(settings_value(&state).await?))
}

async fn settings_value(state: &Arc<AppState>) -> Result<Value, Problem> {
    let defaults = json!({ "bindAddress": state.bind_address, "port": state.port, "mcpEndpoint": format!("http://{}:{}/mcp",state.bind_address,state.port), "databasePath": state.database_path, "databaseState": "ready", "executionMode": "approval", "workspaceRoots": [std::env::current_dir().unwrap_or_default()], "terminalExecutable": default_shell(), "taskConcurrency": 4, "sessionConcurrency": 8, "theme": "system", "language": "en", "sound": false });
    let mut object = defaults.as_object().cloned().unwrap_or_default();
    for key in [
        "executionMode",
        "workspaceRoots",
        "terminalExecutable",
        "taskConcurrency",
        "sessionConcurrency",
        "theme",
        "language",
        "sound",
    ] {
        if let Some(setting) = state
            .repository
            .setting(&format!("ui_{key}"))
            .await
            .map_err(storage_problem)?
            && let Ok(value) = serde_json::from_str(&setting.value_json)
        {
            object.insert(key.to_owned(), value);
        }
    }
    Ok(Value::Object(object))
}

async fn resolve_tools(state: &Arc<AppState>, input: &AgentInput) -> Result<Vec<String>, Problem> {
    if let Some(id) = &input.preset_id {
        let preset = state
            .repository
            .list_presets()
            .await
            .map_err(storage_problem)?
            .into_iter()
            .find(|item| &item.id == id)
            .ok_or_else(|| {
                Problem::new(
                    StatusCode::BAD_REQUEST,
                    "Invalid preset",
                    "presetId does not exist",
                )
            })?;
        if input.tool_ids.is_empty() {
            return Ok(preset.tool_ids);
        }
    }
    let known = state
        .repository
        .list_tools()
        .await
        .map_err(storage_problem)?;
    if input
        .tool_ids
        .iter()
        .any(|id| !known.iter().any(|tool| &tool.id == id))
    {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid tools",
            "one or more toolIds do not exist",
        ));
    }
    let mut values = input.tool_ids.clone();
    values.sort();
    values.dedup();
    Ok(values)
}
async fn agent_values(state: &Arc<AppState>, agents: Vec<McpAgent>) -> Result<Vec<Value>, Problem> {
    let mut values = Vec::with_capacity(agents.len());
    for agent in agents {
        values.push(agent_value(state, agent).await?);
    }
    Ok(values)
}
async fn agent_value(state: &Arc<AppState>, agent: McpAgent) -> Result<Value, Problem> {
    let ids = state
        .repository
        .agent_allowed_tool_ids(&agent.id)
        .await
        .map_err(storage_problem)?;
    let preset = state
        .repository
        .list_presets()
        .await
        .map_err(storage_problem)?
        .into_iter()
        .find(|preset| {
            let mut tools = preset.tool_ids.clone();
            tools.sort();
            tools == ids
        })
        .map(|preset| preset.id);
    Ok(
        json!({ "id":agent.id.as_str(),"name":agent.name,"enabled":agent.enabled,"projectFolder":agent.project_folder.unwrap_or_default(),"presetId":preset,"toolIds":ids,"secretLast4":agent.secret_last4,"updatedAtUtc":iso_ms(agent.updated_at_ms) }),
    )
}
fn validate_agent_input(input: &AgentInput) -> Result<(), Problem> {
    if input.name.trim().is_empty() || input.name.chars().count() > 100 {
        Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid agent",
            "name must contain 1..100 characters",
        ))
    } else {
        Ok(())
    }
}
fn optional(value: String) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.trim().to_owned())
}
fn agent_id(value: String) -> Result<AgentId, Problem> {
    AgentId::new(value).map_err(|_| bad_id())
}
fn task_value(task: chatcmd_core::Task) -> Value {
    json!({"id":task.id.as_str(),"title":task.title,"status":task.status.as_str(),"updatedAtUtc":iso_ms(task.updated_at_ms),"createdAtUtc":iso_ms(task.created_at_ms),"generation":task.generation,"activeSessionId":task.active_session_id.map(|id|id.into_string())})
}
fn session_row(row: &sqlx::sqlite::SqliteRow) -> Value {
    json!({"id":row.get::<String,_>("id"),"taskId":row.get::<Option<String>,_>("task_id"),"shell":row.get::<String,_>("executable"),"processId":row.get::<Option<i64>,_>("process_id"),"status":row.get::<String,_>("status"),"workingDirectory":row.get::<String,_>("working_directory"),"createdAtUtc":iso_ms(row.get("created_at_ms")),"updatedAtUtc":iso_ms(row.get("updated_at_ms")),"closedAtUtc":row.get::<Option<i64>,_>("closed_at_ms").map(iso_ms)})
}
fn timeline_row(row: &sqlx::sqlite::SqliteRow) -> Value {
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
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}
fn iso_ms(ms: i64) -> String {
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
struct Problem {
    status: StatusCode,
    title: &'static str,
    detail: &'static str,
}
impl Problem {
    const fn new(status: StatusCode, title: &'static str, detail: &'static str) -> Self {
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
fn not_found() -> Problem {
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
fn db_problem(_: sqlx::Error) -> Problem {
    Problem::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Storage error",
        "local storage operation failed",
    )
}
fn runtime_problem(_: chatcmd_runtime::RuntimeError) -> Problem {
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
