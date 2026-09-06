//! One idempotent working message AFTER the replacement binding is durable.
use super::{Problem, chatgpt_support::wrapped_message, db_problem, now_ms, storage_problem};
use crate::websocket::{AppEvent, AppState};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use chatcmd_storage::compact::CompactPhase;
use serde_json::{Value, json};
use sqlx::Row;
use std::sync::Arc;

pub(super) async fn resume_compact(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Problem> {
    let job = state
        .repository
        .compact_job(id.trim())
        .await
        .map_err(storage_problem)?;
    if job.phase != CompactPhase::Completed {
        return Err(Problem::new(
            StatusCode::CONFLICT,
            "Compact is not attached",
            "The new ChatGPT binding must be committed before continuing work.",
        ));
    }
    // Server enforcement also protects against an older/reloaded extension.
    if !job.continue_after_compact {
        return Ok(Json(json!({"requestId":null,"continueAfterCompact":false})));
    }
    let request_id = format!("compact-continue-{}", job.id);
    let mut tx = state
        .repository
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(db_problem)?;
    let same: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM chatgpt_conversations WHERE task_id=? AND conversation_id=?) AND NOT EXISTS(SELECT 1 FROM chatgpt_compact_jobs WHERE task_id=? AND phase NOT IN ('completed','cancelled'))")
        .bind(&job.task_id).bind(&job.new_conversation_id).bind(&job.task_id)
        .fetch_one(&mut *tx).await.map_err(db_problem)?;
    if !same {
        return Ok(Json(json!({"requestId":null,"alreadyContinued":true})));
    }
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM chatgpt_bridge_requests WHERE id=?)")
            .bind(&request_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(db_problem)?;
    if exists {
        return Ok(Json(json!({"requestId":request_id})));
    }
    // A real user message wins a race with automatic resume; never duplicate or reorder it.
    let active: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM chatgpt_bridge_requests WHERE task_id=? AND (status IN ('queued','running','stop_requested') OR created_at_ms>=?))")
        .bind(&job.task_id).bind(job.completed_at_ms.unwrap_or(job.updated_at_ms))
        .fetch_one(&mut *tx).await.map_err(db_problem)?;
    if active {
        return Ok(Json(json!({"requestId":null,"alreadyContinued":true})));
    }
    let row = sqlx::query("SELECT agent_id FROM tasks WHERE id=?")
        .bind(&job.task_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_problem)?;
    let content = format!(
        "[[CHATCMD-CONTINUE:{}]]\n\nTiếp tục công việc còn dang dở theo bản handoff vừa nhận. ChatCMD đã gắn cuộc trò chuyện ChatGPT mới vào task hiện tại {}. Không tạo task mới, không làm lại phần đã hoàn tất, giữ nguyên yêu cầu và quyền thực thi hiện có. Nếu mọi yêu cầu đã xong, chỉ báo trạng thái hoàn tất; không tự phát sinh công việc mới.",
        job.id, job.task_id
    );
    let submitted = wrapped_message(
        job.agent_name.as_deref().unwrap_or(""),
        job.project_folder.as_deref(),
        &content,
    );
    let now = now_ms();
    sqlx::query("INSERT INTO chatgpt_bridge_requests(id,task_id,turn_id,agent_id,model,user_content,submitted_content,status,conversation_id,conversation_url,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,?,'queued',?,?,?,?)")
        .bind(&request_id).bind(&job.task_id).bind(format!("compact-turn-{}", job.id))
        .bind(row.get::<String,_>("agent_id")).bind(&job.old_model).bind(&content).bind(&submitted)
        .bind(&job.new_conversation_id).bind(&job.new_conversation_url).bind(now).bind(now)
        .execute(&mut *tx).await.map_err(db_problem)?;
    sqlx::query(
        "UPDATE chatgpt_conversations SET active_request_id=?,updated_at_ms=? WHERE task_id=?",
    )
    .bind(&request_id)
    .bind(now)
    .bind(&job.task_id)
    .execute(&mut *tx)
    .await
    .map_err(db_problem)?;
    sqlx::query("UPDATE tasks SET status='running',updated_at_ms=? WHERE id=?")
        .bind(now)
        .bind(&job.task_id)
        .execute(&mut *tx)
        .await
        .map_err(db_problem)?;
    tx.commit().await.map_err(db_problem)?;
    let mut event = AppEvent::new(
        "chatgpt_compact_updated",
        json!({"taskId":job.task_id,"jobId":job.id,"phase":"completed","resumeRequestId":request_id}),
    );
    event.task_id = Some(job.task_id);
    state.publish(event);
    Ok(Json(json!({"requestId":request_id})))
}
