//! Local management/extension contract for the durable compact worker.
use super::{Problem, db_problem, now_ms, storage_problem};
use crate::websocket::{AppEvent, AppState};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use chatcmd_storage::compact::{CompactCheckpoint, CompactJob, CompactTaskJobs};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqliteConnection;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct StartCompact {
    #[serde(default)]
    continue_after_compact: bool,
}

pub(super) async fn task_compact(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> Result<Json<CompactTaskJobs>, Problem> {
    state
        .repository
        .compact_task_jobs(task_id.trim())
        .await
        .map(Json)
        .map_err(storage_problem)
}

pub(super) async fn start_compact(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<StartCompact>,
) -> Result<Json<CompactJob>, Problem> {
    let operation = operation_id(&headers, format!("compact-start-{}", Uuid::new_v4()))?;
    let job = state
        .repository
        .compact_start_with_continuation(
            task_id.trim(),
            &operation,
            input.continue_after_compact,
            now_ms(),
        )
        .await
        .map_err(storage_problem)?;
    publish_update(&state, &job);
    Ok(Json(job))
}

pub(super) async fn pending_compact(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, Problem> {
    let jobs = state
        .repository
        .compact_pending()
        .await
        .map_err(storage_problem)?;
    Ok(Json(json!({"jobs": jobs})))
}

pub(super) async fn get_compact(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<CompactJob>, Problem> {
    state
        .repository
        .compact_job(id.trim())
        .await
        .map(Json)
        .map_err(storage_problem)
}

pub(super) async fn checkpoint_compact(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<CompactCheckpoint>,
) -> Result<Json<CompactJob>, Problem> {
    if input.phase.is_some_and(|phase| {
        !matches!(
            phase,
            chatcmd_storage::compact::CompactPhase::Preparing
                | chatcmd_storage::compact::CompactPhase::Cancelled
        )
    }) {
        let current = state
            .repository
            .compact_job(id.trim())
            .await
            .map_err(storage_problem)?;
        let source_scope = chatcmd_storage::compact::openai_scope(&current.old_conversation_id);
        if !state
            .activities
            .compact_settled(&current.task_id, current.old_scope_hash.as_deref())
            || !state
                .activities
                .compact_settled(&current.task_id, Some(&source_scope))
        {
            return Err(Problem::new(
                StatusCode::CONFLICT,
                "Local tools are still running",
                "Đang chờ các lệnh local kết thúc an toàn trước khi viết handoff. Không có tin nhắn nào được gửi thêm.",
            ));
        }
    }
    let operation = operation_id(
        &headers,
        format!(
            "compact-checkpoint-{}-{}",
            id.trim(),
            input.expected_revision
        ),
    )?;
    let job = state
        .repository
        .compact_checkpoint(id.trim(), &input, &operation, now_ms())
        .await
        .map_err(storage_problem)?;
    publish_update(&state, &job);
    Ok(Json(job))
}

fn operation_id(headers: &HeaderMap, fallback: String) -> Result<String, Problem> {
    match headers.get("idempotency-key") {
        None => Ok(fallback),
        Some(value) => value
            .to_str()
            .ok()
            .filter(|id| !id.is_empty() && id.len() <= 160 && !id.chars().any(char::is_control))
            .map(str::to_owned)
            .ok_or_else(|| {
                Problem::new(
                    StatusCode::BAD_REQUEST,
                    "Invalid operation id",
                    "Idempotency-Key must be a bounded string.",
                )
            }),
    }
}

fn publish_update(state: &AppState, job: &CompactJob) {
    // Handoff and private request parameters never enter the event bus.
    let mut event = AppEvent::new(
        "chatgpt_compact_updated",
        json!({
            "jobId": job.id, "taskId": job.task_id, "phase": job.phase,
            "revision": job.revision, "updatedAtMs": job.updated_at_ms,
        }),
    );
    event.id = format!("chatgpt-compact-{}-{}", job.id, job.revision);
    event.task_id = Some(job.task_id.clone());
    state.publish(event);
}

pub(super) async fn guard_send(conn: &mut SqliteConnection, task_id: &str) -> Result<(), Problem> {
    let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM chatgpt_compact_jobs WHERE task_id=? AND phase NOT IN ('completed','cancelled')) OR EXISTS(SELECT 1 FROM chatgpt_bridge_requests WHERE task_id=? AND status IN ('queued','running','stop_requested'))")
        .bind(task_id).bind(task_id).fetch_one(conn).await.map_err(db_problem)?;
    if blocked {
        return Err(Problem::new(
            StatusCode::CONFLICT,
            "ChatGPT conversation is busy",
            "Finish compact or the current response before sending. Queued messages are preserved.",
        ));
    }
    Ok(())
}
