use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row as _;

use crate::websocket::{AppEvent, AppState};

use super::{Problem, db_problem, not_found, now_ms};

const MAX_EXTENSION_FALLBACK_ATTEMPTS: i64 = 3;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SubagentFallbackStarted {
    attempt: i64,
    conversation_id: Option<String>,
    conversation_url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SubagentFallbackResult {
    attempt: i64,
    status: String,
    assistant_content: Option<String>,
    error_message: Option<String>,
    conversation_id: Option<String>,
    conversation_url: Option<String>,
}

pub(super) async fn pending_subagent_fallbacks(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<Value>>, Problem> {
    let rows = sqlx::query(
        "SELECT r.id,r.parent_task_id,r.parent_turn_id,r.child_task_id,r.name,r.request,r.fallback_attempts,r.fallback_conversation_id,r.fallback_conversation_url,a.name AS agent_name FROM subagent_runs r LEFT JOIN tasks t ON t.id=r.child_task_id LEFT JOIN mcp_agents a ON a.id=t.agent_id WHERE r.status='pending' AND r.fallback_state IN ('requested','started') AND r.fallback_attempts BETWEEN 1 AND ? ORDER BY r.updated_at_ms,r.id",
    )
    .bind(MAX_EXTENSION_FALLBACK_ATTEMPTS)
    .fetch_all(state.repository.pool())
    .await
    .map_err(db_problem)?;
    Ok(Json(
        rows.iter()
            .map(|row| fallback_request_value(row, row.get::<i64, _>("fallback_attempts")))
            .collect(),
    ))
}

pub(super) async fn subagent_fallback_started(
    State(state): State<Arc<AppState>>,
    Path(subagent_id): Path<String>,
    Json(input): Json<SubagentFallbackStarted>,
) -> Result<Json<Value>, Problem> {
    validate_attempt(input.attempt)?;
    let subagent_id = subagent_id.trim();
    let row = fallback_row(&state, subagent_id).await?;
    if let Some(response) = reject_if_not_current(&row, input.attempt) {
        return Ok(Json(response));
    }

    let conversation_id = clean_optional(input.conversation_id.as_deref());
    let conversation_url = clean_optional(input.conversation_url.as_deref());
    let now = now_ms();
    let updated = sqlx::query("UPDATE subagent_runs SET fallback_state='started',fallback_error=NULL,fallback_conversation_id=COALESCE(?,fallback_conversation_id),fallback_conversation_url=COALESCE(?,fallback_conversation_url),updated_at_ms=? WHERE id=? AND status='pending' AND fallback_state IN ('requested','started') AND fallback_attempts=?")
        .bind(conversation_id)
        .bind(conversation_url)
        .bind(now)
        .bind(subagent_id)
        .bind(input.attempt)
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    if updated.rows_affected() != 1 {
        return Ok(Json(state_changed_response(&state, subagent_id).await?));
    }

    let child_task_id = row.get::<Option<String>, _>("child_task_id");
    persist_conversation_identity(
        &state,
        child_task_id.as_deref(),
        conversation_id,
        conversation_url,
        now,
    )
    .await?;

    let mut event = AppEvent::new(
        "subagent.fallback_started",
        json!({
            "subagentId": subagent_id,
            "childTaskId": child_task_id,
            "attempt": input.attempt,
            "conversationId": conversation_id,
            "conversationUrl": conversation_url
        }),
    );
    event.task_id = Some(row.get::<String, _>("parent_task_id"));
    event.turn_id = Some(row.get::<String, _>("parent_turn_id"));
    state.publish(event);
    Ok(Json(json!({"accepted": true, "attempt": input.attempt})))
}

pub(super) async fn subagent_fallback_result(
    State(state): State<Arc<AppState>>,
    Path(subagent_id): Path<String>,
    Json(input): Json<SubagentFallbackResult>,
) -> Result<Json<Value>, Problem> {
    if !matches!(input.status.as_str(), "completed" | "failed" | "stopped") {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid fallback result",
            "Fallback status must be completed, failed, or stopped.",
        ));
    }
    validate_attempt(input.attempt)?;
    let subagent_id = subagent_id.trim();
    let row = fallback_row(&state, subagent_id).await?;
    if let Some(response) = reject_if_not_current(&row, input.attempt) {
        return Ok(Json(response));
    }

    let child_task_id = row
        .get::<Option<String>, _>("child_task_id")
        .ok_or_else(not_found)?;
    let assistant = input.assistant_content.as_deref().unwrap_or("").trim();
    let completed_without_mcp = input.status == "completed" && !assistant.is_empty();
    let now = now_ms();
    let conversation_id = clean_optional(input.conversation_id.as_deref());
    let conversation_url = clean_optional(input.conversation_url.as_deref());

    if completed_without_mcp {
        let updated = sqlx::query("UPDATE subagent_runs SET status='completed',fallback_state='exhausted',fallback_error=NULL,fallback_conversation_id=COALESCE(?,fallback_conversation_id),fallback_conversation_url=COALESCE(?,fallback_conversation_url),updated_at_ms=?,completed_at_ms=? WHERE id=? AND status='pending' AND fallback_state IN ('requested','started') AND fallback_attempts=?")
            .bind(conversation_id)
            .bind(conversation_url)
            .bind(now)
            .bind(now)
            .bind(subagent_id)
            .bind(input.attempt)
            .execute(state.repository.pool())
            .await
            .map_err(db_problem)?;
        if updated.rows_affected() != 1 {
            return Ok(Json(state_changed_response(&state, subagent_id).await?));
        }
        persist_conversation_identity(
            &state,
            Some(&child_task_id),
            conversation_id,
            conversation_url,
            now,
        )
        .await?;
        sqlx::query("UPDATE tasks SET status='completed',active_session_id=NULL,updated_at_ms=? WHERE id=? AND status NOT IN ('stopped','completed','failed')")
            .bind(now)
            .bind(&child_task_id)
            .execute(state.repository.pool())
            .await
            .map_err(db_problem)?;
        append_browser_only_completion(
            &state,
            &row,
            subagent_id,
            &child_task_id,
            input.attempt,
            assistant,
        )
        .await?;
        publish_subagent_fallback_terminal(&state, &row, &child_task_id, "completed", None);
        return Ok(Json(json!({
            "accepted": true,
            "completed": true,
            "retryScheduled": false
        })));
    }

    let error = input
        .error_message
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(if input.status == "completed" {
            "ChatGPT finished without a usable final response and did not claim MCP."
        } else {
            "ChatGPT extension fallback did not claim MCP."
        });
    let current_attempt = row.get::<i64, _>("fallback_attempts");
    if current_attempt < MAX_EXTENSION_FALLBACK_ATTEMPTS {
        let next_attempt = current_attempt + 1;
        let updated = sqlx::query("UPDATE subagent_runs SET fallback_state='requested',fallback_attempts=?,fallback_error=?,fallback_conversation_id=COALESCE(?,fallback_conversation_id),fallback_conversation_url=COALESCE(?,fallback_conversation_url),updated_at_ms=? WHERE id=? AND status='pending' AND fallback_state IN ('requested','started') AND fallback_attempts=?")
            .bind(next_attempt)
            .bind(error)
            .bind(conversation_id)
            .bind(conversation_url)
            .bind(now)
            .bind(subagent_id)
            .bind(input.attempt)
            .execute(state.repository.pool())
            .await
            .map_err(db_problem)?;
        if updated.rows_affected() != 1 {
            return Ok(Json(state_changed_response(&state, subagent_id).await?));
        }
        persist_conversation_identity(
            &state,
            Some(&child_task_id),
            conversation_id,
            conversation_url,
            now,
        )
        .await?;
        let retry_row = fallback_row(&state, subagent_id).await?;
        publish_fallback_requested(&state, &retry_row, next_attempt);
        return Ok(Json(json!({
            "accepted": true,
            "completed": false,
            "retryScheduled": true,
            "attempt": next_attempt,
            "maxAttempts": MAX_EXTENSION_FALLBACK_ATTEMPTS
        })));
    }

    let updated = sqlx::query("UPDATE subagent_runs SET status='failed',fallback_state='exhausted',fallback_error=?,fallback_conversation_id=COALESCE(?,fallback_conversation_id),fallback_conversation_url=COALESCE(?,fallback_conversation_url),updated_at_ms=?,completed_at_ms=? WHERE id=? AND status='pending' AND fallback_state IN ('requested','started') AND fallback_attempts=?")
        .bind(error)
        .bind(conversation_id)
        .bind(conversation_url)
        .bind(now)
        .bind(now)
        .bind(subagent_id)
        .bind(input.attempt)
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    if updated.rows_affected() != 1 {
        return Ok(Json(state_changed_response(&state, subagent_id).await?));
    }
    persist_conversation_identity(
        &state,
        Some(&child_task_id),
        conversation_id,
        conversation_url,
        now,
    )
    .await?;
    sqlx::query("UPDATE tasks SET status='failed',active_session_id=NULL,updated_at_ms=? WHERE id=? AND status NOT IN ('stopped','completed','failed')")
        .bind(now)
        .bind(&child_task_id)
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    publish_subagent_fallback_terminal(&state, &row, &child_task_id, "failed", Some(error));
    Ok(Json(json!({
        "accepted": true,
        "completed": false,
        "retryScheduled": false,
        "exhausted": true
    })))
}

async fn append_browser_only_completion(
    state: &Arc<AppState>,
    row: &sqlx::sqlite::SqliteRow,
    subagent_id: &str,
    child_task_id: &str,
    attempt: i64,
    assistant: &str,
) -> Result<(), Problem> {
    let turn_id = format!("turn-{subagent_id}");
    let request_id = format!("subagent-fallback-{subagent_id}-{attempt}");
    let request = row.get::<String, _>("request");
    let submitted = fallback_submitted_content(
        row.get::<Option<String>, _>("agent_name").as_deref(),
        &request,
        subagent_id,
    );
    super::chatgpt_support::append_user_message(
        state,
        child_task_id,
        &turn_id,
        &request_id,
        &request,
        &submitted,
    )
    .await?;
    super::chatgpt_support::append_status(
        state,
        child_task_id,
        &turn_id,
        &request_id,
        "completed",
        assistant,
    )
    .await?;
    Ok(())
}

async fn persist_conversation_identity(
    state: &Arc<AppState>,
    child_task_id: Option<&str>,
    conversation_id: Option<&str>,
    conversation_url: Option<&str>,
    now: i64,
) -> Result<(), Problem> {
    let (Some(child_task_id), Some(conversation_id), Some(conversation_url)) =
        (child_task_id, conversation_id, conversation_url)
    else {
        return Ok(());
    };
    sqlx::query("INSERT INTO chatgpt_conversations(task_id,conversation_id,conversation_url,model,active_request_id,created_at_ms,updated_at_ms) VALUES(?,?,?,'Auto',NULL,?,?) ON CONFLICT(task_id) DO UPDATE SET conversation_id=excluded.conversation_id,conversation_url=excluded.conversation_url,model=excluded.model,updated_at_ms=excluded.updated_at_ms")
        .bind(child_task_id)
        .bind(conversation_id)
        .bind(conversation_url)
        .bind(now)
        .bind(now)
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    Ok(())
}

async fn fallback_row(
    state: &Arc<AppState>,
    subagent_id: &str,
) -> Result<sqlx::sqlite::SqliteRow, Problem> {
    sqlx::query("SELECT r.id,r.parent_task_id,r.parent_turn_id,r.child_task_id,r.name,r.request,r.status,r.fallback_state,r.fallback_attempts,r.fallback_conversation_id,r.fallback_conversation_url,a.name AS agent_name FROM subagent_runs r LEFT JOIN tasks t ON t.id=r.child_task_id LEFT JOIN mcp_agents a ON a.id=t.agent_id WHERE r.id=? LIMIT 1")
        .bind(subagent_id)
        .fetch_optional(state.repository.pool())
        .await
        .map_err(db_problem)?
        .ok_or_else(not_found)
}

fn validate_attempt(attempt: i64) -> Result<(), Problem> {
    if (1..=MAX_EXTENSION_FALLBACK_ATTEMPTS).contains(&attempt) {
        return Ok(());
    }
    Err(Problem::new(
        StatusCode::BAD_REQUEST,
        "Invalid fallback attempt",
        "Fallback attempt is outside the supported retry range.",
    ))
}

fn reject_if_not_current(row: &sqlx::sqlite::SqliteRow, attempt: i64) -> Option<Value> {
    let current_attempt = row.get::<i64, _>("fallback_attempts");
    let status = row.get::<String, _>("status");
    let fallback_state = row.get::<String, _>("fallback_state");
    if status != "pending" || fallback_state == "claimed" {
        return Some(json!({
            "accepted": false,
            "reason": "already_claimed_or_finished",
            "status": status,
            "fallbackState": fallback_state,
            "attempt": current_attempt
        }));
    }
    if attempt != current_attempt {
        return Some(json!({
            "accepted": false,
            "reason": "stale_attempt",
            "attempt": current_attempt
        }));
    }
    None
}

async fn state_changed_response(
    state: &Arc<AppState>,
    subagent_id: &str,
) -> Result<Value, Problem> {
    let row = fallback_row(state, subagent_id).await?;
    Ok(json!({
        "accepted": false,
        "reason": "already_claimed_or_finished",
        "status": row.get::<String, _>("status"),
        "fallbackState": row.get::<String, _>("fallback_state"),
        "attempt": row.get::<i64, _>("fallback_attempts")
    }))
}

fn fallback_request_value(row: &sqlx::sqlite::SqliteRow, attempt: i64) -> Value {
    let subagent_id = row.get::<String, _>("id");
    let request = row.get::<String, _>("request");
    let submitted_content = fallback_submitted_content(
        row.get::<Option<String>, _>("agent_name").as_deref(),
        &request,
        &subagent_id,
    );
    json!({
        "subagentId": subagent_id,
        "parentTaskId": row.get::<String, _>("parent_task_id"),
        "parentTurnId": row.get::<String, _>("parent_turn_id"),
        "childTaskId": row.get::<Option<String>, _>("child_task_id"),
        "name": row.get::<String, _>("name"),
        "submittedContent": submitted_content,
        "attempt": attempt,
        "maxAttempts": MAX_EXTENSION_FALLBACK_ATTEMPTS,
        "conversationId": row.get::<Option<String>, _>("fallback_conversation_id"),
        "conversationUrl": row.get::<Option<String>, _>("fallback_conversation_url")
    })
}

fn fallback_submitted_content(
    agent_name: Option<&str>,
    request: &str,
    subagent_id: &str,
) -> String {
    let delegated_prompt = format!("{request}\n\nCMDGPT_SUBAGENT_ID={subagent_id}");
    match agent_name.map(str::trim).filter(|value| !value.is_empty()) {
        Some(agent_name) => {
            format!("Sử dụng plugin @{agent_name} để thực hiện yêu cầu sau:\n\n{delegated_prompt}")
        }
        None => delegated_prompt,
    }
}

fn publish_fallback_requested(state: &Arc<AppState>, row: &sqlx::sqlite::SqliteRow, attempt: i64) {
    let value = fallback_request_value(row, attempt);
    let mut event = AppEvent::new("subagent.fallback_requested", value);
    event.id = format!(
        "subagent-fallback-retry-{}-{attempt}",
        row.get::<String, _>("id")
    );
    event.task_id = Some(row.get::<String, _>("parent_task_id"));
    event.turn_id = Some(row.get::<String, _>("parent_turn_id"));
    state.publish(event);
}

fn publish_subagent_fallback_terminal(
    state: &Arc<AppState>,
    row: &sqlx::sqlite::SqliteRow,
    child_task_id: &str,
    status: &str,
    error: Option<&str>,
) {
    let mut event = AppEvent::new(
        "subagent.status",
        json!({
            "subagentId": row.get::<String, _>("id"),
            "childTaskId": child_task_id,
            "name": row.get::<String, _>("name"),
            "status": status,
            "error": error
        }),
    );
    event.task_id = Some(row.get::<String, _>("parent_task_id"));
    event.session_id = Some(child_task_id.to_owned());
    event.turn_id = Some(row.get::<String, _>("parent_turn_id"));
    state.publish(event);
}

fn clean_optional(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[cfg(test)]
#[path = "subagent_fallback_tests.rs"]
mod tests;
