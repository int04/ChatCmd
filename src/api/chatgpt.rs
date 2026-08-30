use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::websocket::AppState;

use super::chatgpt_support::*;
use super::{Problem, db_problem, now_ms};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CreateRequest {
    agent_id: String,
    model: Option<String>,
    content: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ContinueRequest {
    model: Option<String>,
    content: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BridgeStarted {
    conversation_id: String,
    conversation_url: String,
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BridgeResult {
    status: String,
    conversation_id: Option<String>,
    conversation_url: Option<String>,
    assistant_content: Option<String>,
    error_message: Option<String>,
}

pub(super) async fn create_request(
    State(state): State<Arc<AppState>>,
    Json(input): Json<CreateRequest>,
) -> Result<Json<Value>, Problem> {
    validate_message(&input.content)?;
    let agent_id = input.agent_id.trim();
    let agent_name = enabled_agent_name(&state, agent_id).await?;
    let model = normalize_model(input.model.as_deref());
    let submitted = wrapped_message(&agent_name, input.content.trim());
    let now = now_ms();
    let request_id = Uuid::new_v4().to_string();
    let turn_id = format!("chatgpt-turn-{}", Uuid::new_v4());
    sqlx::query("INSERT INTO chatgpt_bridge_requests(id,task_id,turn_id,agent_id,model,user_content,submitted_content,status,conversation_id,conversation_url,assistant_content,error_message,created_at_ms,updated_at_ms,completed_at_ms) VALUES(?,NULL,?,?,?,?,?,'queued',NULL,NULL,NULL,NULL,?,?,NULL)")
        .bind(&request_id)
        .bind(&turn_id)
        .bind(agent_id)
        .bind(&model)
        .bind(input.content.trim())
        .bind(&submitted)
        .bind(now)
        .bind(now)
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    request_json(&state, &request_id).await
}

pub(super) async fn request(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Problem> {
    request_json(&state, id.trim()).await
}

pub(super) async fn task_bridge(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> Result<Json<Value>, Problem> {
    let row = sqlx::query("SELECT c.task_id,c.conversation_id,c.conversation_url,c.model,c.active_request_id,r.status AS active_status,r.submitted_content AS active_submitted_content FROM chatgpt_conversations c LEFT JOIN chatgpt_bridge_requests r ON r.id=c.active_request_id WHERE c.task_id=?")
        .bind(task_id.trim())
        .fetch_optional(state.repository.pool())
        .await
        .map_err(db_problem)?
        .ok_or_else(not_found_chat)?;
    Ok(Json(json!({
        "taskId": row.get::<String, _>("task_id"),
        "conversationId": row.get::<String, _>("conversation_id"),
        "conversationUrl": row.get::<String, _>("conversation_url"),
        "model": row.get::<String, _>("model"),
        "activeRequestId": row.get::<Option<String>, _>("active_request_id"),
        "activeStatus": row.get::<Option<String>, _>("active_status"),
        "activeSubmittedContent": row.get::<Option<String>, _>("active_submitted_content"),
    })))
}

pub(super) async fn continue_message(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Json(input): Json<ContinueRequest>,
) -> Result<Json<Value>, Problem> {
    validate_message(&input.content)?;
    let task_id = task_id.trim();
    let row = sqlx::query("SELECT c.conversation_id,c.conversation_url,c.model,t.agent_id,a.name FROM chatgpt_conversations c JOIN tasks t ON t.id=c.task_id JOIN mcp_agents a ON a.id=t.agent_id WHERE c.task_id=?")
        .bind(task_id)
        .fetch_optional(state.repository.pool())
        .await
        .map_err(db_problem)?
        .ok_or_else(not_found_chat)?;
    ensure_no_active_request(&state, task_id).await?;
    let agent_id = row.get::<String, _>("agent_id");
    let agent_name = row.get::<String, _>("name");
    let model = input
        .model
        .as_deref()
        .map(|value| normalize_model(Some(value)))
        .unwrap_or_else(|| row.get::<String, _>("model"));
    let submitted = wrapped_message(&agent_name, input.content.trim());
    let request_id = Uuid::new_v4().to_string();
    let turn_id = format!("chatgpt-turn-{}", Uuid::new_v4());
    let now = now_ms();
    sqlx::query("INSERT INTO chatgpt_bridge_requests(id,task_id,turn_id,agent_id,model,user_content,submitted_content,status,conversation_id,conversation_url,assistant_content,error_message,created_at_ms,updated_at_ms,completed_at_ms) VALUES(?,?,?,?,?,?,?,'queued',?,?,NULL,NULL,?,?,NULL)")
        .bind(&request_id)
        .bind(task_id)
        .bind(&turn_id)
        .bind(&agent_id)
        .bind(&model)
        .bind(input.content.trim())
        .bind(&submitted)
        .bind(row.get::<String, _>("conversation_id"))
        .bind(row.get::<String, _>("conversation_url"))
        .bind(now)
        .bind(now)
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    sqlx::query("UPDATE chatgpt_conversations SET model=?,active_request_id=?,updated_at_ms=? WHERE task_id=?")
        .bind(&model)
        .bind(&request_id)
        .bind(now)
        .bind(task_id)
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    sqlx::query("UPDATE tasks SET status='running',stopped_at_ms=NULL,updated_at_ms=? WHERE id=?")
        .bind(now)
        .bind(task_id)
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    append_user_message(
        &state,
        task_id,
        &turn_id,
        &request_id,
        input.content.trim(),
        &submitted,
    )
    .await?;
    request_json(&state, &request_id).await
}

pub(super) async fn stop_message(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> Result<Json<Value>, Problem> {
    let request_id = sqlx::query_scalar::<_, String>("SELECT r.id FROM chatgpt_conversations c JOIN chatgpt_bridge_requests r ON r.id=c.active_request_id WHERE c.task_id=? AND r.status IN ('queued','running','stop_requested') LIMIT 1")
        .bind(task_id.trim())
        .fetch_optional(state.repository.pool())
        .await
        .map_err(db_problem)?
        .ok_or_else(|| Problem::new(StatusCode::CONFLICT, "No active ChatGPT message", "This ChatGPT conversation does not have a message that can be stopped."))?;
    sqlx::query("UPDATE chatgpt_bridge_requests SET status='stop_requested',updated_at_ms=? WHERE id=? AND status IN ('queued','running')")
        .bind(now_ms())
        .bind(&request_id)
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    request_json(&state, &request_id).await
}

pub(super) async fn bridge_started(
    State(state): State<Arc<AppState>>,
    Path(request_id): Path<String>,
    Json(input): Json<BridgeStarted>,
) -> Result<Json<Value>, Problem> {
    validate_conversation(&input.conversation_id, &input.conversation_url)?;
    let row = bridge_request_row(&state, request_id.trim()).await?;
    let agent_id = row.get::<String, _>("agent_id");
    let turn_id = row.get::<String, _>("turn_id");
    let submitted = row.get::<String, _>("submitted_content");
    let user_content = row.get::<String, _>("user_content");
    let existing_task = row.get::<Option<String>, _>("task_id");
    let scope = openai_scope(&input.conversation_id);
    let task_id = if let Some(id) = existing_task {
        id
    } else if let Some(id) = task_for_scope(&state, &agent_id, &scope).await? {
        id
    } else {
        bridge_task_id(&agent_id, &scope, &submitted)
    };
    let now = now_ms();
    let title = compact_title(&user_content);
    sqlx::query("INSERT INTO tasks(id,agent_id,device_id,conversation_scope_hash,title,source,allow_execute,status,active_session_id,generation,stopped_at_ms,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,'chatgpt_web',1,'running',NULL,1,NULL,?,?) ON CONFLICT(id) DO UPDATE SET conversation_scope_hash=excluded.conversation_scope_hash,title=COALESCE(tasks.title,excluded.title),source='chatgpt_web',allow_execute=1,status='running',stopped_at_ms=NULL,updated_at_ms=excluded.updated_at_ms")
        .bind(&task_id)
        .bind(&agent_id)
        .bind(state.device.id.as_str())
        .bind(&scope)
        .bind(title)
        .bind(now)
        .bind(now)
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    let model = input
        .model
        .as_deref()
        .map(|value| normalize_model(Some(value)))
        .unwrap_or_else(|| row.get::<String, _>("model"));
    sqlx::query("INSERT INTO chatgpt_conversations(task_id,conversation_id,conversation_url,model,active_request_id,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,?) ON CONFLICT(task_id) DO UPDATE SET conversation_id=excluded.conversation_id,conversation_url=excluded.conversation_url,model=excluded.model,active_request_id=excluded.active_request_id,updated_at_ms=excluded.updated_at_ms")
        .bind(&task_id)
        .bind(input.conversation_id.trim())
        .bind(input.conversation_url.trim())
        .bind(&model)
        .bind(request_id.trim())
        .bind(now)
        .bind(now)
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    sqlx::query("UPDATE chatgpt_bridge_requests SET task_id=?,status=CASE WHEN status='stop_requested' THEN status ELSE 'running' END,conversation_id=?,conversation_url=?,model=?,updated_at_ms=? WHERE id=?")
        .bind(&task_id)
        .bind(input.conversation_id.trim())
        .bind(input.conversation_url.trim())
        .bind(&model)
        .bind(now)
        .bind(request_id.trim())
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    append_user_message(
        &state,
        &task_id,
        &turn_id,
        request_id.trim(),
        &user_content,
        &submitted,
    )
    .await?;
    request_json(&state, request_id.trim()).await
}

pub(super) async fn bridge_result(
    State(state): State<Arc<AppState>>,
    Path(request_id): Path<String>,
    Json(input): Json<BridgeResult>,
) -> Result<Json<Value>, Problem> {
    if !matches!(input.status.as_str(), "completed" | "stopped" | "failed") {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid bridge result",
            "ChatGPT bridge status must be completed, stopped, or failed.",
        ));
    }
    let row = bridge_request_row(&state, request_id.trim()).await?;
    let now = now_ms();
    let assistant = input.assistant_content.as_deref().unwrap_or("").trim();
    let error = input.error_message.as_deref().unwrap_or("").trim();
    let task_id = match row.get::<Option<String>, _>("task_id") {
        Some(task_id) => task_id,
        None if input.status == "failed" => {
            sqlx::query("UPDATE chatgpt_bridge_requests SET status='failed',assistant_content=?,error_message=?,updated_at_ms=?,completed_at_ms=? WHERE id=?")
                .bind(if assistant.is_empty() { None::<&str> } else { Some(assistant) })
                .bind(if error.is_empty() { Some("ChatGPT bridge failed before the conversation ID was available.") } else { Some(error) })
                .bind(now)
                .bind(now)
                .bind(request_id.trim())
                .execute(state.repository.pool())
                .await
                .map_err(db_problem)?;
            return request_json(&state, request_id.trim()).await;
        }
        None => {
            return Err(Problem::new(
                StatusCode::CONFLICT,
                "ChatGPT conversation not bound",
                "The browser extension must report the ChatGPT conversation ID before completing the request.",
            ));
        }
    };
    let turn_id = row.get::<String, _>("turn_id");
    sqlx::query("UPDATE chatgpt_bridge_requests SET status=?,conversation_id=COALESCE(?,conversation_id),conversation_url=COALESCE(?,conversation_url),assistant_content=?,error_message=?,updated_at_ms=?,completed_at_ms=? WHERE id=?")
        .bind(&input.status)
        .bind(input.conversation_id.as_deref())
        .bind(input.conversation_url.as_deref())
        .bind(if assistant.is_empty() { None::<&str> } else { Some(assistant) })
        .bind(if error.is_empty() { None::<&str> } else { Some(error) })
        .bind(now)
        .bind(now)
        .bind(request_id.trim())
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    let task_status = match input.status.as_str() {
        "completed" => "completed",
        "stopped" => "interrupted",
        _ => "failed",
    };
    sqlx::query("UPDATE tasks SET status=CASE WHEN status='stopped' THEN status ELSE ? END,updated_at_ms=? WHERE id=?")
        .bind(task_status)
        .bind(now)
        .bind(&task_id)
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    sqlx::query("UPDATE chatgpt_conversations SET active_request_id=NULL,conversation_id=COALESCE(?,conversation_id),conversation_url=COALESCE(?,conversation_url),updated_at_ms=? WHERE task_id=? AND active_request_id=?")
        .bind(input.conversation_id.as_deref())
        .bind(input.conversation_url.as_deref())
        .bind(now)
        .bind(&task_id)
        .bind(request_id.trim())
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    let content = if !assistant.is_empty() {
        assistant
    } else if !error.is_empty() {
        error
    } else if input.status == "stopped" {
        "Đã dừng phản hồi ChatGPT."
    } else {
        "ChatGPT bridge không trả về nội dung."
    };
    append_status(
        &state,
        &task_id,
        &turn_id,
        request_id.trim(),
        &input.status,
        content,
    )
    .await?;
    request_json(&state, request_id.trim()).await
}
