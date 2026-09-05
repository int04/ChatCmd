use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use chatcmd_storage::SqliteRepository;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row as _;

use crate::websocket::AppState;

use super::chatgpt_support::{not_found_chat, publish, request_json, validate_conversation};
use super::{Problem, db_problem, now_ms};

const MAX_ASSISTANT_CHARS: usize = 100_000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BrowserCompletionInput {
    conversation_id: Option<String>,
    conversation_url: Option<String>,
    assistant_content: String,
}

pub(super) struct BrowserCompletion<'a> {
    pub(super) request_id: &'a str,
    pub(super) conversation_id: Option<&'a str>,
    pub(super) conversation_url: Option<&'a str>,
    pub(super) assistant_content: &'a str,
    pub(super) now: i64,
}

pub(super) struct BrowserCompletionEvents {
    pub(super) task_id: String,
    pub(super) turn_id: String,
    pub(super) user: Option<(String, Value)>,
    pub(super) status: Option<(String, Value)>,
}

pub(super) async fn bridge_browser_completed(
    State(state): State<Arc<AppState>>,
    Path(request_id): Path<String>,
    Json(input): Json<BrowserCompletionInput>,
) -> Result<Json<Value>, Problem> {
    let request_id = request_id.trim();
    let assistant_content = input.assistant_content.trim();
    if assistant_content.is_empty() || assistant_content.chars().count() > MAX_ASSISTANT_CHARS {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid browser completion",
            "A non-empty ChatGPT assistant bubble of at most 100,000 characters is required.",
        ));
    }
    let identity = match (
        input.conversation_id.as_deref().map(str::trim),
        input.conversation_url.as_deref().map(str::trim),
    ) {
        (Some(id), Some(url)) => {
            validate_conversation(id, url)?;
            (Some(id), Some(url))
        }
        (None, None) => (None, None),
        _ => {
            return Err(Problem::new(
                StatusCode::BAD_REQUEST,
                "Incomplete ChatGPT identity",
                "conversationId and conversationUrl must be supplied together.",
            ));
        }
    };
    let events = persist_browser_completion(
        &state.repository,
        &BrowserCompletion {
            request_id,
            conversation_id: identity.0,
            conversation_url: identity.1,
            assistant_content,
            now: now_ms(),
        },
    )
    .await?;
    super::chatgpt_observation::retain_result(&state, request_id, assistant_content, true).await?;
    let demoted =
        crate::chatgpt_queue::demote_all_immediate(&state.repository, &events.task_id, now_ms())
            .await
            .map_err(db_problem)?;
    if demoted > 0 {
        super::chatgpt_queue::publish_queue_event(
            &state,
            &events.task_id,
            "demoted_after_browser_completion",
            None,
        );
    }
    if let Some((id, payload)) = events.user {
        publish(
            &state,
            &id,
            "message",
            &events.task_id,
            &events.turn_id,
            payload,
        );
    }
    if let Some((id, payload)) = events.status {
        publish(
            &state,
            &id,
            "status",
            &events.task_id,
            &events.turn_id,
            payload,
        );
    }
    request_json(&state, request_id).await
}

pub(super) async fn persist_browser_completion(
    repository: &SqliteRepository,
    completion: &BrowserCompletion<'_>,
) -> Result<BrowserCompletionEvents, Problem> {
    // This short transaction reads before writing. Reserve the SQLite writer first
    // so concurrent chat captures wait under busy_timeout instead of failing an upgrade.
    let mut transaction = repository
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(db_problem)?;
    let row = sqlx::query("SELECT task_id,turn_id,user_content,submitted_content,status,created_at_ms FROM chatgpt_bridge_requests WHERE id=?")
        .bind(completion.request_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(db_problem)?
        .ok_or_else(not_found_chat)?;
    let status = row.get::<String, _>("status");
    if matches!(status.as_str(), "stop_requested" | "stopped" | "failed") {
        return Err(Problem::new(
            StatusCode::CONFLICT,
            "ChatGPT request is not completable",
            "A stopped or failed ChatGPT request cannot be completed from a browser bubble.",
        ));
    }
    let task_id = row.get::<Option<String>, _>("task_id").ok_or_else(|| {
        Problem::new(
            StatusCode::CONFLICT,
            "ChatGPT conversation not bound",
            "The browser request must be bound to a task before completion.",
        )
    })?;
    let turn_id = row.get::<String, _>("turn_id");
    let user_content = row.get::<String, _>("user_content");
    let submitted = row.get::<String, _>("submitted_content");
    let created_at_ms = row.get::<i64, _>("created_at_ms");

    let mcp_turn_id = crate::chatgpt_transcript::mcp_turn(
        &mut transaction,
        &task_id,
        completion.request_id,
        created_at_ms,
        &submitted,
    )
    .await
    .map_err(db_problem)?;
    let final_turn_id = mcp_turn_id.as_deref().unwrap_or(&turn_id);
    crate::chatgpt_transcript::rehome_events(
        &mut transaction,
        &task_id,
        completion.request_id,
        final_turn_id,
    )
    .await
    .map_err(db_problem)?;
    let already_final: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM timeline_events WHERE task_id=? AND turn_id=? AND actor='assistant' AND kind='status' AND json_extract(payload_json,'$.status')='completed')")
        .bind(&task_id).bind(final_turn_id).fetch_one(&mut *transaction).await.map_err(db_problem)?;

    sqlx::query("UPDATE chatgpt_bridge_requests SET status='completed',conversation_id=COALESCE(?,conversation_id),conversation_url=COALESCE(?,conversation_url),assistant_content=?,error_message=NULL,updated_at_ms=?,completed_at_ms=COALESCE(completed_at_ms,?) WHERE id=?")
        .bind(completion.conversation_id)
        .bind(completion.conversation_url)
        .bind(completion.assistant_content)
        .bind(completion.now)
        .bind(completion.now)
        .bind(completion.request_id)
        .execute(&mut *transaction)
        .await
        .map_err(db_problem)?;
    sqlx::query("UPDATE tasks SET status=CASE WHEN status='stopped' THEN status ELSE 'completed' END,updated_at_ms=? WHERE id=? AND NOT EXISTS(SELECT 1 FROM chatgpt_bridge_requests WHERE task_id=? AND id<>? AND status IN ('queued','running','stop_requested'))")
        .bind(completion.now)
        .bind(&task_id)
        .bind(&task_id)
        .bind(completion.request_id)
        .execute(&mut *transaction)
        .await
        .map_err(db_problem)?;
    sqlx::query("UPDATE chatgpt_conversations SET active_request_id=NULL,conversation_id=COALESCE(?,conversation_id),conversation_url=COALESCE(?,conversation_url),updated_at_ms=? WHERE task_id=? AND active_request_id=?")
        .bind(completion.conversation_id)
        .bind(completion.conversation_url)
        .bind(completion.now)
        .bind(&task_id)
        .bind(completion.request_id)
        .execute(&mut *transaction)
        .await
        .map_err(db_problem)?;

    let user = if already_final || mcp_turn_id.is_some() {
        None
    } else {
        insert_browser_user_event(
            &mut transaction,
            &task_id,
            final_turn_id,
            completion.request_id,
            &user_content,
            &submitted,
            completion.now,
        )
        .await?
    };
    let status = if already_final {
        None
    } else {
        insert_browser_status_event(
            &mut transaction,
            &task_id,
            final_turn_id,
            completion.request_id,
            completion.assistant_content,
            completion.now,
        )
        .await?
    };
    transaction.commit().await.map_err(db_problem)?;
    Ok(BrowserCompletionEvents {
        task_id,
        turn_id: final_turn_id.to_owned(),
        user,
        status,
    })
}

async fn insert_browser_user_event(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    task_id: &str,
    turn_id: &str,
    request_id: &str,
    content: &str,
    submitted: &str,
    now: i64,
) -> Result<Option<(String, Value)>, Problem> {
    let event_id = format!("chatgpt-user-{request_id}");
    let payload = json!({"role":"user","content":content,"submittedContent":submitted,"provider":"chatgpt_web"});
    let changed = sqlx::query("INSERT OR IGNORE INTO timeline_events(event_id,task_id,turn_id,session_id,actor,kind,idempotency_key,payload_json,metadata_json,created_at_ms) VALUES(?,?,?,NULL,'user','message',?,?,NULL,?)")
        .bind(&event_id).bind(task_id).bind(turn_id).bind(&event_id).bind(payload.to_string()).bind(now)
        .execute(&mut **transaction).await.map_err(db_problem)?.rows_affected() == 1;
    Ok(changed.then_some((event_id, payload)))
}

async fn insert_browser_status_event(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    task_id: &str,
    turn_id: &str,
    request_id: &str,
    content: &str,
    now: i64,
) -> Result<Option<(String, Value)>, Problem> {
    let event_id = format!("chatgpt-result-{request_id}");
    let payload = json!({"status":"completed","content":content,"provider":"chatgpt_web","completionSource":"browser_bubble"});
    let changed = sqlx::query("INSERT OR IGNORE INTO timeline_events(event_id,task_id,turn_id,session_id,actor,kind,idempotency_key,payload_json,metadata_json,created_at_ms) VALUES(?,?,?,NULL,'assistant','status',?,?,NULL,?)")
        .bind(&event_id).bind(task_id).bind(turn_id).bind(&event_id).bind(payload.to_string()).bind(now)
        .execute(&mut **transaction).await.map_err(db_problem)?.rows_affected() == 1;
    Ok(changed.then_some((event_id, payload)))
}
