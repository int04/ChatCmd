use super::chatgpt_support::*;
use super::{Problem, db_problem, now_ms};
use crate::websocket::AppState;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Deserialize;
use serde_json::Value;
use sqlx::Row;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BridgeResult {
    status: String,
    conversation_id: Option<String>,
    conversation_url: Option<String>,
    assistant_content: Option<String>,
    error_message: Option<String>,
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
    super::chatgpt_observation::retain_result(
        &state,
        request_id.trim(),
        input.assistant_content.as_deref().unwrap_or(""),
        input.status == "completed",
    )
    .await?;
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
    let submitted = row.get::<String, _>("submitted_content");
    let user_content = row.get::<String, _>("user_content");
    let created_at_ms = row.get::<i64, _>("created_at_ms");
    let (result_conversation_id, result_conversation_url) = match (
        input.conversation_id.as_deref(),
        input.conversation_url.as_deref(),
    ) {
        (Some(id), Some(url)) if !is_provisional_conversation_id(id) => (Some(id), Some(url)),
        _ => (None, None),
    };
    let mut transaction = state.repository.pool().begin().await.map_err(db_problem)?;
    let mcp_authoritative = crate::chatgpt_transcript::mcp_turn(
        &mut transaction,
        &task_id,
        request_id.trim(),
        created_at_ms,
        &submitted,
    )
    .await
    .map_err(db_problem)?
    .is_some();
    transaction.commit().await.map_err(db_problem)?;
    sqlx::query("UPDATE chatgpt_bridge_requests SET status=?,conversation_id=COALESCE(?,conversation_id),conversation_url=COALESCE(?,conversation_url),assistant_content=?,error_message=?,updated_at_ms=?,completed_at_ms=? WHERE id=?")
        .bind(&input.status)
        .bind(result_conversation_id)
        .bind(result_conversation_url)
        .bind(if assistant.is_empty() { None::<&str> } else { Some(assistant) })
        .bind(if error.is_empty() { None::<&str> } else { Some(error) })
        .bind(now)
        .bind(now)
        .bind(request_id.trim())
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    if !mcp_authoritative {
        let task_status = match input.status.as_str() {
            "completed" => "completed",
            "stopped" => "interrupted",
            _ => "failed",
        };
        sqlx::query("UPDATE tasks SET status=CASE WHEN status='stopped' THEN status ELSE ? END,updated_at_ms=? WHERE id=? AND NOT EXISTS(SELECT 1 FROM chatgpt_bridge_requests WHERE task_id=? AND id<>? AND status IN ('queued','running','stop_requested'))")
            .bind(task_status)
            .bind(now)
            .bind(&task_id)
            .bind(&task_id)
            .bind(request_id.trim())
            .execute(state.repository.pool())
            .await
            .map_err(db_problem)?;
    }
    sqlx::query("UPDATE chatgpt_conversations SET active_request_id=NULL,conversation_id=COALESCE(?,conversation_id),conversation_url=COALESCE(?,conversation_url),updated_at_ms=? WHERE task_id=? AND active_request_id=?")
        .bind(result_conversation_id)
        .bind(result_conversation_url)
        .bind(now)
        .bind(&task_id)
        .bind(request_id.trim())
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    let demoted = crate::chatgpt_queue::demote_all_immediate(&state.repository, &task_id, now)
        .await
        .map_err(db_problem)?;
    if demoted > 0 {
        super::chatgpt_queue::publish_queue_event(
            &state,
            &task_id,
            "demoted_after_bridge_result",
            None,
        );
    }
    if !mcp_authoritative {
        let content = if !assistant.is_empty() {
            assistant
        } else if !error.is_empty() {
            error
        } else if input.status == "stopped" {
            "Đã dừng phản hồi ChatGPT."
        } else {
            "ChatGPT bridge không trả về nội dung."
        };
        append_user_message(
            &state,
            &task_id,
            &turn_id,
            request_id.trim(),
            &user_content,
            &submitted,
        )
        .await?;
        append_status(
            &state,
            &task_id,
            &turn_id,
            request_id.trim(),
            &input.status,
            content,
        )
        .await?;
    }
    request_json(&state, request_id.trim()).await
}
