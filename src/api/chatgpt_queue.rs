use std::{collections::HashSet, sync::Arc};

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row as _;
use uuid::Uuid;

use crate::{
    chatgpt_queue::{
        QueuedChatGptMessage, demote_immediate_if_inactive, immediate_allowed, list_messages,
    },
    websocket::{AppEvent, AppState},
};

use super::chatgpt_support::validate_message;
use super::{Problem, db_problem, now_ms};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CreateQueueMessage {
    content: String,
    mode: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpdateQueueMessage {
    content: Option<String>,
    mode: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReorderQueueMessages {
    message_ids: Vec<String>,
}

pub(super) async fn queue_messages(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> Result<Json<Value>, Problem> {
    let task_id = task_id.trim();
    ensure_chatgpt_task(&state, task_id).await?;
    let demoted = demote_immediate_if_inactive(&state.repository, task_id, now_ms())
        .await
        .map_err(db_problem)?;
    if demoted > 0 {
        publish_queue_event(&state, task_id, "demoted", None);
    }
    queue_json(&state, task_id).await
}

pub(super) async fn create_queue_message(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Json(input): Json<CreateQueueMessage>,
) -> Result<Json<Value>, Problem> {
    validate_message(&input.content)?;
    let task_id = task_id.trim();
    ensure_chatgpt_task(&state, task_id).await?;
    let requested_mode = normalize_mode(&input.mode)?;
    let mode = if requested_mode == "immediate"
        && immediate_allowed(&state.repository, task_id)
            .await
            .map_err(db_problem)?
    {
        "immediate"
    } else {
        "queued"
    };
    let sort_order = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(sort_order),0)+100 FROM chatgpt_message_queue WHERE task_id=?",
    )
    .bind(task_id)
    .fetch_one(state.repository.pool())
    .await
    .map_err(db_problem)?;
    let id = Uuid::new_v4().to_string();
    let now = now_ms();
    sqlx::query("INSERT INTO chatgpt_message_queue(id,task_id,content,mode,sort_order,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,?)")
        .bind(&id)
        .bind(task_id)
        .bind(input.content.trim())
        .bind(mode)
        .bind(sort_order)
        .bind(now)
        .bind(now)
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    publish_queue_event(&state, task_id, "created", Some(&id));
    message_json(&state, task_id, &id).await
}

pub(super) async fn update_queue_message(
    State(state): State<Arc<AppState>>,
    Path((task_id, message_id)): Path<(String, String)>,
    Json(input): Json<UpdateQueueMessage>,
) -> Result<Json<Value>, Problem> {
    let task_id = task_id.trim();
    let message_id = message_id.trim();
    ensure_queue_message(&state, task_id, message_id).await?;
    let content = input.content.as_deref().map(str::trim);
    if let Some(content) = content {
        validate_message(content)?;
    }
    if content.is_none() && input.mode.is_none() {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "No queue changes",
            "Provide content or mode to update the queued message.",
        ));
    }
    let mode = if let Some(mode) = input.mode.as_deref() {
        let requested = normalize_mode(mode)?;
        Some(
            if requested == "immediate"
                && immediate_allowed(&state.repository, task_id)
                    .await
                    .map_err(db_problem)?
            {
                "immediate"
            } else {
                "queued"
            },
        )
    } else {
        None
    };
    let now = now_ms();
    sqlx::query("UPDATE chatgpt_message_queue SET content=COALESCE(?,content),mode=COALESCE(?,mode),updated_at_ms=? WHERE id=? AND task_id=?")
        .bind(content)
        .bind(mode)
        .bind(now)
        .bind(message_id)
        .bind(task_id)
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    publish_queue_event(&state, task_id, "updated", Some(message_id));
    message_json(&state, task_id, message_id).await
}

pub(super) async fn delete_queue_message(
    State(state): State<Arc<AppState>>,
    Path((task_id, message_id)): Path<(String, String)>,
) -> Result<StatusCode, Problem> {
    let task_id = task_id.trim();
    let message_id = message_id.trim();
    let deleted = sqlx::query("DELETE FROM chatgpt_message_queue WHERE id=? AND task_id=?")
        .bind(message_id)
        .bind(task_id)
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?
        .rows_affected();
    if deleted == 0 {
        return Err(queue_not_found());
    }
    publish_queue_event(&state, task_id, "deleted", Some(message_id));
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn reorder_queue_messages(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Json(input): Json<ReorderQueueMessages>,
) -> Result<StatusCode, Problem> {
    let task_id = task_id.trim();
    ensure_chatgpt_task(&state, task_id).await?;
    let unique = input.message_ids.iter().collect::<HashSet<_>>();
    if unique.len() != input.message_ids.len() {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid queue order",
            "Queued message IDs must be unique.",
        ));
    }
    let total =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM chatgpt_message_queue WHERE task_id=?")
            .bind(task_id)
            .fetch_one(state.repository.pool())
            .await
            .map_err(db_problem)?;
    if usize::try_from(total).ok() != Some(input.message_ids.len()) {
        return Err(Problem::new(
            StatusCode::CONFLICT,
            "Queue changed",
            "The queued messages changed while reordering. Reload the queue and try again.",
        ));
    }
    let mut transaction = state.repository.pool().begin().await.map_err(db_problem)?;
    for (index, id) in input.message_ids.iter().enumerate() {
        let sort_order = i64::try_from(index + 1)
            .unwrap_or(i64::MAX)
            .saturating_mul(100);
        let updated = sqlx::query(
            "UPDATE chatgpt_message_queue SET sort_order=?,updated_at_ms=? WHERE id=? AND task_id=?",
        )
        .bind(sort_order)
        .bind(now_ms())
        .bind(id)
        .bind(task_id)
        .execute(&mut *transaction)
        .await
        .map_err(db_problem)?
        .rows_affected();
        if updated != 1 {
            return Err(queue_not_found());
        }
    }
    transaction.commit().await.map_err(db_problem)?;
    publish_queue_event(&state, task_id, "reordered", None);
    Ok(StatusCode::NO_CONTENT)
}

async fn ensure_chatgpt_task(state: &AppState, task_id: &str) -> Result<(), Problem> {
    let exists = sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM chatgpt_conversations WHERE task_id=?)",
    )
    .bind(task_id)
    .fetch_one(state.repository.pool())
    .await
    .map_err(db_problem)?;
    if exists == 0 {
        Err(queue_not_found())
    } else {
        Ok(())
    }
}

async fn ensure_queue_message(
    state: &AppState,
    task_id: &str,
    message_id: &str,
) -> Result<(), Problem> {
    let exists = sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM chatgpt_message_queue WHERE id=? AND task_id=?)",
    )
    .bind(message_id)
    .bind(task_id)
    .fetch_one(state.repository.pool())
    .await
    .map_err(db_problem)?;
    if exists == 0 {
        Err(queue_not_found())
    } else {
        Ok(())
    }
}

async fn queue_json(state: &AppState, task_id: &str) -> Result<Json<Value>, Problem> {
    let messages = list_messages(&state.repository, task_id)
        .await
        .map_err(db_problem)?;
    Ok(Json(Value::Array(
        messages.iter().map(queue_message_value).collect(),
    )))
}

async fn message_json(
    state: &AppState,
    task_id: &str,
    message_id: &str,
) -> Result<Json<Value>, Problem> {
    let row = sqlx::query("SELECT id,task_id,content,mode,sort_order,created_at_ms,updated_at_ms FROM chatgpt_message_queue WHERE id=? AND task_id=?")
        .bind(message_id)
        .bind(task_id)
        .fetch_optional(state.repository.pool())
        .await
        .map_err(db_problem)?
        .ok_or_else(queue_not_found)?;
    let message = QueuedChatGptMessage {
        id: row.get("id"),
        task_id: row.get("task_id"),
        content: row.get("content"),
        mode: row.get("mode"),
        sort_order: row.get("sort_order"),
        created_at_ms: row.get("created_at_ms"),
        updated_at_ms: row.get("updated_at_ms"),
    };
    Ok(Json(queue_message_value(&message)))
}

fn queue_message_value(message: &QueuedChatGptMessage) -> Value {
    json!({
        "id": message.id,
        "taskId": message.task_id,
        "content": message.content,
        "mode": message.mode,
        "sortOrder": message.sort_order,
        "createdAtMs": message.created_at_ms,
        "updatedAtMs": message.updated_at_ms,
    })
}

fn normalize_mode(mode: &str) -> Result<&'static str, Problem> {
    match mode.trim() {
        "queued" => Ok("queued"),
        "immediate" => Ok("immediate"),
        _ => Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid queue mode",
            "Queue mode must be queued or immediate.",
        )),
    }
}

fn queue_not_found() -> Problem {
    Problem::new(
        StatusCode::NOT_FOUND,
        "Queued message not found",
        "The queued ChatGPT message does not exist for this conversation.",
    )
}

pub(super) fn publish_queue_event(
    state: &AppState,
    task_id: &str,
    action: &str,
    message_id: Option<&str>,
) {
    let mut event = AppEvent::new(
        "chatgpt.queue.changed",
        json!({ "action": action, "messageId": message_id }),
    );
    event.task_id = Some(task_id.to_owned());
    state.publish(event);
}
