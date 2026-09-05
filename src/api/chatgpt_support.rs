use std::sync::Arc;

use axum::{Json, http::StatusCode};
use serde_json::{Value, json};
use sqlx::{Row, sqlite::SqliteRow};
use uuid::Uuid;

use crate::websocket::{AppEvent, AppState};

use super::{Problem, db_problem, now_ms};

const DEFAULT_MODEL: &str = "Auto";
const MAX_MESSAGE_CHARS: usize = 100_000;

pub(super) async fn request_json(state: &Arc<AppState>, id: &str) -> Result<Json<Value>, Problem> {
    let mut row = bridge_request_row(state, id).await?;
    let task_id = row.get::<Option<String>, _>("task_id");
    let submitted = row.get::<String, _>("submitted_content");
    let created_at_ms = row.get::<i64, _>("created_at_ms");
    let has_final_response = if let Some(task_id) = task_id.as_deref() {
        let mut tx = state.repository.pool().begin().await.map_err(db_problem)?;
        let mcp_turn =
            crate::chatgpt_transcript::mcp_turn(&mut tx, task_id, id, created_at_ms, &submitted)
                .await
                .map_err(db_problem)?;
        let turn_id = mcp_turn.unwrap_or_else(|| row.get::<String, _>("turn_id"));
        let found: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM timeline_events WHERE task_id=? AND turn_id=? AND actor='assistant' AND kind='status' AND json_extract(payload_json,'$.status')='completed')")
            .bind(task_id).bind(turn_id).fetch_one(&mut *tx).await.map_err(db_problem)?;
        tx.commit().await.map_err(db_problem)?;
        found
    } else {
        false
    };
    let status = row.get::<String, _>("status");
    if has_final_response && matches!(status.as_str(), "queued" | "running" | "stop_requested") {
        let now = now_ms();
        sqlx::query("UPDATE chatgpt_bridge_requests SET status='completed',updated_at_ms=?,completed_at_ms=COALESCE(completed_at_ms,?) WHERE id=? AND status IN ('queued','running','stop_requested')")
            .bind(now)
            .bind(now)
            .bind(id)
            .execute(state.repository.pool())
            .await
            .map_err(db_problem)?;
        if let Some(task_id) = task_id.as_deref() {
            sqlx::query("UPDATE chatgpt_conversations SET active_request_id=NULL,updated_at_ms=? WHERE task_id=? AND active_request_id=?")
                .bind(now)
                .bind(task_id)
                .bind(id)
                .execute(state.repository.pool())
                .await
                .map_err(db_problem)?;
            let demoted =
                crate::chatgpt_queue::demote_all_immediate(&state.repository, task_id, now)
                    .await
                    .map_err(db_problem)?;
            if demoted > 0 {
                super::chatgpt_queue::publish_queue_event(
                    state,
                    task_id,
                    "demoted_after_final_detection",
                    None,
                );
            }
        }
        row = bridge_request_row(state, id).await?;
    }
    let mut value = request_value(&row);
    if let Some(object) = value.as_object_mut() {
        object.insert("hasFinalResponse".to_owned(), json!(has_final_response));
    }
    Ok(Json(value))
}

pub(super) async fn bridge_request_row(
    state: &Arc<AppState>,
    id: &str,
) -> Result<SqliteRow, Problem> {
    sqlx::query("SELECT id,task_id,turn_id,agent_id,model,user_content,submitted_content,project_folder,status,conversation_id,conversation_url,assistant_content,error_message,created_at_ms,updated_at_ms,completed_at_ms FROM chatgpt_bridge_requests WHERE id=?")
        .bind(id)
        .fetch_optional(state.repository.pool())
        .await
        .map_err(db_problem)?
        .ok_or_else(not_found_chat)
}

fn request_value(row: &SqliteRow) -> Value {
    json!({
        "id": row.get::<String, _>("id"), "taskId": row.get::<Option<String>, _>("task_id"),
        "turnId": row.get::<String, _>("turn_id"), "agentId": row.get::<String, _>("agent_id"),
        "model": row.get::<String, _>("model"), "userContent": row.get::<String, _>("user_content"),
        "submittedContent": row.get::<String, _>("submitted_content"), "projectFolder": row.get::<Option<String>, _>("project_folder"), "status": row.get::<String, _>("status"),
        "conversationId": row.get::<Option<String>, _>("conversation_id"), "conversationUrl": row.get::<Option<String>, _>("conversation_url"),
        "assistantContent": row.get::<Option<String>, _>("assistant_content"), "errorMessage": row.get::<Option<String>, _>("error_message")
    })
}

pub(super) async fn enabled_agent_name(state: &Arc<AppState>, id: &str) -> Result<String, Problem> {
    let row = sqlx::query("SELECT name,enabled FROM mcp_agents WHERE id=?")
        .bind(id)
        .fetch_optional(state.repository.pool())
        .await
        .map_err(db_problem)?
        .ok_or_else(|| {
            Problem::new(
                StatusCode::BAD_REQUEST,
                "Invalid agent",
                "Select an existing MCP agent before sending the ChatGPT message.",
            )
        })?;
    if row.get::<i64, _>("enabled") == 0 {
        return Err(Problem::new(
            StatusCode::CONFLICT,
            "Agent disabled",
            "The selected MCP agent is disabled.",
        ));
    }
    Ok(row.get("name"))
}

pub(super) async fn ensure_no_active_request(
    state: &Arc<AppState>,
    task_id: &str,
) -> Result<(), Problem> {
    let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chatgpt_bridge_requests WHERE task_id=? AND status IN ('queued','running','stop_requested')")
        .bind(task_id)
        .fetch_one(state.repository.pool())
        .await
        .map_err(db_problem)?;
    if active > 0 {
        return Err(Problem::new(
            StatusCode::CONFLICT,
            "ChatGPT is already responding",
            "Stop or finish the current ChatGPT message before sending another one.",
        ));
    }
    Ok(())
}

pub(super) async fn task_for_scope(
    state: &Arc<AppState>,
    agent_id: &str,
    scope: &str,
) -> Result<Option<String>, Problem> {
    sqlx::query_scalar("SELECT id FROM tasks WHERE agent_id=? AND conversation_scope_hash=? ORDER BY created_at_ms,id LIMIT 1")
        .bind(agent_id)
        .bind(scope)
        .fetch_optional(state.repository.pool())
        .await
        .map_err(db_problem)
}

pub(super) async fn append_user_message(
    state: &Arc<AppState>,
    task_id: &str,
    turn_id: &str,
    request_id: &str,
    content: &str,
    submitted: &str,
) -> Result<(), Problem> {
    let event_id = format!("chatgpt-user-{request_id}");
    let payload = json!({"role":"user","content":content,"submittedContent":submitted,"provider":"chatgpt_web"});
    let changed = sqlx::query("INSERT OR IGNORE INTO timeline_events(event_id,task_id,turn_id,session_id,actor,kind,idempotency_key,payload_json,metadata_json,created_at_ms) VALUES(?,?,?,NULL,'user','message',?,?,NULL,?)")
        .bind(&event_id)
        .bind(task_id)
        .bind(turn_id)
        .bind(&event_id)
        .bind(payload.to_string())
        .bind(now_ms())
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?
        .rows_affected() > 0;
    if changed {
        publish(state, &event_id, "message", task_id, turn_id, payload);
    }
    Ok(())
}

pub(super) async fn append_status(
    state: &Arc<AppState>,
    task_id: &str,
    turn_id: &str,
    request_id: &str,
    status: &str,
    content: &str,
) -> Result<(), Problem> {
    let event_id = format!("chatgpt-result-{request_id}");
    let payload = json!({"status":status,"content":content,"provider":"chatgpt_web"});
    let changed = sqlx::query("INSERT OR IGNORE INTO timeline_events(event_id,task_id,turn_id,session_id,actor,kind,idempotency_key,payload_json,metadata_json,created_at_ms) VALUES(?,?,?,NULL,'assistant','status',?,?,NULL,?)")
        .bind(&event_id)
        .bind(task_id)
        .bind(turn_id)
        .bind(&event_id)
        .bind(payload.to_string())
        .bind(now_ms())
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?
        .rows_affected() > 0;
    if changed {
        publish(state, &event_id, "status", task_id, turn_id, payload);
    }
    Ok(())
}

pub(super) fn publish(
    state: &Arc<AppState>,
    id: &str,
    kind: &str,
    task_id: &str,
    turn_id: &str,
    payload: Value,
) {
    let mut event = AppEvent::new(kind, payload);
    event.id = id.to_owned();
    event.task_id = Some(task_id.to_owned());
    event.turn_id = Some(turn_id.to_owned());
    state.publish(event);
}

pub(super) fn validate_message(content: &str) -> Result<(), Problem> {
    let trimmed = content.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_MESSAGE_CHARS {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid ChatGPT message",
            "Message content is required and cannot exceed 100,000 characters.",
        ));
    }
    Ok(())
}

pub(super) fn validate_conversation(id: &str, url: &str) -> Result<(), Problem> {
    let id = id.trim();
    let url = url.trim();
    if id.is_empty()
        || id.len() > 500
        || url.len() > 2_000
        || !url.starts_with("https://chatgpt.com/")
    {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid ChatGPT conversation",
            "The extension returned an invalid ChatGPT conversation ID or URL.",
        ));
    }
    Ok(())
}

pub(super) fn is_provisional_conversation_id(id: &str) -> bool {
    id.trim().to_ascii_uppercase().starts_with("WEB:")
}

pub(super) fn normalize_model(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_MODEL)
        .chars()
        .take(120)
        .collect()
}

pub(super) fn wrapped_message(
    agent_name: &str,
    project_folder: Option<&str>,
    content: &str,
) -> String {
    if agent_name == super::chatgpt_native::RECORDER_AGENT_NAME {
        return content.to_owned();
    }
    match project_folder
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(project_folder) => format!(
            "Sử dụng plugin @{agent_name}\n\nThư mục dự án: {project_folder}\n\nđể thực hiện yêu cầu sau: {content}"
        ),
        None => format!("Sử dụng plugin @{agent_name} để thực hiện yêu cầu sau:\n\n{content}"),
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::{is_provisional_conversation_id, wrapped_message};

    #[test]
    fn detects_provisional_web_conversation_ids() {
        assert!(is_provisional_conversation_id("WEB:abc"));
        assert!(is_provisional_conversation_id(" web:abc "));
        assert!(!is_provisional_conversation_id(
            "6a961d4f-a928-83ec-a645-298e743e207f"
        ));
    }

    #[test]
    fn wrapped_message_omits_empty_project_folder() {
        let message = wrapped_message("worker", Some("  "), "Kiểm tra dự án");

        assert_eq!(
            message,
            "Sử dụng plugin @worker để thực hiện yêu cầu sau:\n\nKiểm tra dự án"
        );
        assert!(!message.contains("Thư mục dự án:"));
    }

    #[test]
    fn wrapped_message_includes_trimmed_project_folder() {
        let message = wrapped_message("worker", Some(" D:\\DEV\\Dotty "), "Kiểm tra");

        assert!(message.contains("Thư mục dự án: D:\\DEV\\Dotty"));
    }
}

pub(super) fn openai_scope(conversation_id: &str) -> String {
    let material = format!("openai\0{}", conversation_id.trim());
    format!(
        "openai:{}",
        Uuid::new_v5(&Uuid::NAMESPACE_OID, material.as_bytes())
    )
}

pub(super) fn bridge_task_id(agent_id: &str, scope: &str, first_message: &str) -> String {
    let identity_scope = format!("{scope}\0first-user-message:{first_message}");
    let material = format!("task-chat\0agent:{agent_id}\0scope:{identity_scope}");
    format!(
        "task-chat-{}",
        Uuid::new_v5(&Uuid::NAMESPACE_OID, material.as_bytes())
    )
}

pub(super) fn compact_title(content: &str) -> String {
    let normalized = content.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = normalized.chars();
    let title = chars.by_ref().take(72).collect::<String>();
    if chars.next().is_some() {
        format!("{title}…")
    } else {
        title
    }
}

pub(super) fn not_found_chat() -> Problem {
    Problem::new(
        StatusCode::NOT_FOUND,
        "ChatGPT bridge record not found",
        "The requested ChatGPT bridge record was not found.",
    )
}
