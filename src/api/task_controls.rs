use std::collections::HashSet;

use crate::runtime_host::StopActivityResult;
use chatcmd_core::{
    ActorKind, EventId, EventKind, ExecutionMode, TaskExecutionMode, TerminalEventStore as _,
    TimelineEvent,
};
use chatcmd_runtime::{OperationContext, ShellSignal};
use serde::Deserialize;

use super::task_views::task_detail;
use super::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskExecutionModeRequest {
    mode: String,
}

pub(super) async fn task_execution_mode(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Problem> {
    let task_id = TaskId::new(&id).map_err(|_| bad_id())?;
    if state
        .repository
        .task(&task_id)
        .await
        .map_err(storage_problem)?
        .is_none()
    {
        return Err(not_found());
    }
    let overridden = sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM task_execution_modes WHERE task_id=? LIMIT 1)",
    )
    .bind(task_id.as_str())
    .fetch_one(state.repository.pool())
    .await
    .map_err(db_problem)?
        == 1;
    let mode = state
        .repository
        .execution_mode(Some(&task_id))
        .await
        .map_err(storage_problem)?;
    Ok(Json(
        json!({ "mode": execution_mode_name(mode), "overridden": overridden }),
    ))
}

pub(super) async fn set_task_execution_mode(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(request): Json<TaskExecutionModeRequest>,
) -> Result<Json<Value>, Problem> {
    let task_id = TaskId::new(&id).map_err(|_| bad_id())?;
    if state
        .repository
        .task(&task_id)
        .await
        .map_err(storage_problem)?
        .is_none()
    {
        return Err(not_found());
    }
    let mode = match request.mode.as_str() {
        "approval" => ExecutionMode::Approval,
        "allowAll" | "allow" => ExecutionMode::Allow,
        _ => {
            return Err(Problem::new(
                StatusCode::BAD_REQUEST,
                "Invalid command execution mode",
                "Mode must be 'approval' or 'allowAll'.",
            ));
        }
    };
    state
        .repository
        .set_execution_mode(&TaskExecutionMode {
            task_id,
            mode,
            updated_at_ms: now_ms(),
        })
        .await
        .map_err(storage_problem)?;
    Ok(Json(
        json!({ "mode": execution_mode_name(mode), "overridden": true }),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ApprovalDecisionRequest {
    turn_id: Option<String>,
    decision: String,
    reason: Option<String>,
}

pub(super) async fn resolve_task_approval(
    State(state): State<Arc<AppState>>,
    Path((task_id, activity_id)): Path<(String, String)>,
    Json(request): Json<ApprovalDecisionRequest>,
) -> Result<Json<Value>, Problem> {
    let row =
        sqlx::query("SELECT state,request_json FROM approvals WHERE id=? AND task_id=? LIMIT 1")
            .bind(&activity_id)
            .bind(&task_id)
            .fetch_optional(state.repository.pool())
            .await
            .map_err(db_problem)?
            .ok_or_else(not_found)?;
    if row.get::<String, _>("state") != "pending" {
        return Err(Problem::new(
            StatusCode::CONFLICT,
            "Approval already resolved",
            "This approval request is no longer pending.",
        ));
    }
    let approval_request =
        serde_json::from_str::<Value>(&row.get::<String, _>("request_json")).unwrap_or(Value::Null);
    if let Some(expected_turn) = approval_request.get("turnId").and_then(Value::as_str)
        && request.turn_id.as_deref() != Some(expected_turn)
    {
        return Err(Problem::new(
            StatusCode::CONFLICT,
            "Approval ownership mismatch",
            "The turn ID does not own this approval request.",
        ));
    }
    let state_name = match request.decision.as_str() {
        "allow" | "allowSimilar" => "approved",
        "reject" => "rejected",
        _ => {
            return Err(Problem::new(
                StatusCode::BAD_REQUEST,
                "Invalid approval decision",
                "Decision must be 'allow', 'allowSimilar', or 'reject'.",
            ));
        }
    };
    let decision_json = json!({
        "decision": request.decision,
        "reason": request.reason.as_deref().map(str::trim).filter(|value| !value.is_empty()),
    })
    .to_string();
    let resolved_at = now_ms();
    let mut transaction = state.repository.pool().begin().await.map_err(db_problem)?;
    let affected = sqlx::query("UPDATE approvals SET state=?,decision_json=?,resolved_at_ms=? WHERE id=? AND task_id=? AND state='pending'")
        .bind(state_name)
        .bind(decision_json)
        .bind(resolved_at)
        .bind(&activity_id)
        .bind(&task_id)
        .execute(&mut *transaction)
        .await
        .map_err(db_problem)?
        .rows_affected();
    if affected == 0 {
        return Err(Problem::new(
            StatusCode::CONFLICT,
            "Approval already resolved",
            "This approval request was resolved by another action.",
        ));
    }
    let mut grant_id = None;
    if request.decision == "allowSimilar" {
        let preview = approval_request
            .get("grantPreview")
            .filter(|value| !value.is_null())
            .ok_or_else(|| {
                Problem::new(
                    StatusCode::BAD_REQUEST,
                    "Unsafe reusable approval",
                    "Only bounded safe-read requests can create a reusable grant.",
                )
            })?;
        let request_catalog = approval_request
            .get("catalogHash")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if request_catalog != chatcmd_mcp::catalog_hash() {
            return Err(Problem::new(
                StatusCode::CONFLICT,
                "Tool catalog changed",
                "Refresh the request and approve it again.",
            ));
        }
        let owner_agent_id = approval_request
            .get("agentId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                Problem::new(
                    StatusCode::BAD_REQUEST,
                    "Invalid approval owner",
                    "The approval has no bound agent identity.",
                )
            })?;
        let tools = preview
            .get("allowedTools")
            .cloned()
            .unwrap_or_else(|| json!([]));
        let scopes = preview
            .get("pathScopes")
            .cloned()
            .unwrap_or_else(|| json!([]));
        if tools.as_array().is_none_or(Vec::is_empty) || scopes.as_array().is_none_or(Vec::is_empty)
        {
            return Err(Problem::new(
                StatusCode::BAD_REQUEST,
                "Invalid approval grant",
                "Reusable grants require explicit tool and path scopes.",
            ));
        }
        let new_grant_id = uuid::Uuid::new_v4().to_string();
        let child_attempt = sqlx::query_scalar::<_, i64>(
            "SELECT attempt FROM subagent_runs WHERE child_task_id=? LIMIT 1",
        )
        .bind(&task_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(db_problem)?;
        sqlx::query("INSERT INTO approval_grants(id,owner_agent_id,task_id,turn_id,child_attempt,allowed_tools_json,path_scopes_json,option_constraints_json,max_calls,max_files_scanned,max_bytes_read,max_bytes_written,expires_at_ms,inherited_from,catalog_hash,state,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,?,?,?,?,?,NULL,?,NULL,?,'active',?,?)")
            .bind(&new_grant_id).bind(owner_agent_id).bind(&task_id)
            .bind(approval_request.get("turnId").and_then(Value::as_str))
            .bind(child_attempt)
            .bind(to_json_string(&tools)?).bind(to_json_string(&scopes)?)
            .bind(to_json_string(preview.get("optionConstraints").unwrap_or(&json!({})))?)
            .bind(preview.get("maxCalls").and_then(Value::as_i64).unwrap_or(1).clamp(1, 256))
            .bind(preview.get("maxFilesScanned").and_then(Value::as_i64).map(|v| v.clamp(0, 100_000)))
            .bind(preview.get("maxBytesRead").and_then(Value::as_i64).map(|v| v.clamp(0, 1_073_741_824)))
            .bind(preview.get("expiresAtMs").and_then(Value::as_i64).unwrap_or(resolved_at).min(resolved_at.saturating_add(15 * 60 * 1_000)))
            .bind(request_catalog).bind(resolved_at).bind(resolved_at)
            .execute(&mut *transaction).await.map_err(db_problem)?;
        sqlx::query("INSERT INTO approval_grant_audit(id,grant_id,task_id,event,path_count,created_at_ms) VALUES(?,?,?,'created',?,?)")
            .bind(uuid::Uuid::new_v4().to_string()).bind(&new_grant_id).bind(&task_id)
            .bind(i64::try_from(scopes.as_array().map_or(0, Vec::len)).unwrap_or(i64::MAX)).bind(resolved_at)
            .execute(&mut *transaction).await.map_err(db_problem)?;
        grant_id = Some(new_grant_id);
    }
    transaction.commit().await.map_err(db_problem)?;
    let mut event = AppEvent::new(
        "approval.resolved",
        json!({ "activityId": &activity_id, "decision": &request.decision }),
    );
    event.task_id = Some(task_id.clone());
    event.turn_id = approval_request
        .get("turnId")
        .and_then(Value::as_str)
        .map(str::to_owned);
    state.publish(event);
    publish_subagent_approval_resolved(&state, &task_id, &activity_id, &request.decision).await?;
    Ok(Json(
        json!({ "accepted": true, "decision": request.decision, "grantId": grant_id }),
    ))
}

fn to_json_string(value: &Value) -> Result<String, Problem> {
    serde_json::to_string(value).map_err(|_| {
        Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid approval grant",
            "Grant fields could not be encoded.",
        )
    })
}

pub(super) async fn revoke_task_approval_grant(
    State(state): State<Arc<AppState>>,
    Path((task_id, grant_id)): Path<(String, String)>,
) -> Result<StatusCode, Problem> {
    let now = now_ms();
    let mut transaction = state.repository.pool().begin().await.map_err(db_problem)?;
    let affected = sqlx::query("UPDATE approval_grants SET state='revoked',updated_at_ms=? WHERE id=? AND task_id=? AND state='active'")
        .bind(now).bind(&grant_id).bind(&task_id).execute(&mut *transaction).await.map_err(db_problem)?.rows_affected();
    if affected == 0 {
        return Err(not_found());
    }
    transaction.commit().await.map_err(db_problem)?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StopTaskActivityRequest {
    turn_id: Option<String>,
    reason: Option<String>,
}

pub(super) async fn stop_task_activity(
    State(state): State<Arc<AppState>>,
    Path((task_id, activity_id)): Path<(String, String)>,
    Json(request): Json<StopTaskActivityRequest>,
) -> Result<StatusCode, Problem> {
    if task_id.trim().is_empty()
        || task_id.len() > 200
        || activity_id.trim().is_empty()
        || activity_id.len() > 200
    {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid activity identity",
            "Task ID and activity ID are required and cannot exceed 200 characters.",
        ));
    }
    if request
        .turn_id
        .as_deref()
        .is_some_and(|value| value.len() > 200)
        || request
            .reason
            .as_deref()
            .is_some_and(|value| value.len() > 2_000)
    {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid stop request",
            "The optional reason cannot exceed 2,000 characters and turn ID cannot exceed 200 characters.",
        ));
    }
    let task_id = task_id.trim();
    let activity_id = activity_id.trim();
    let reason = request
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let turn_id = request
        .turn_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let (result, active) = state
        .activities
        .prepare_stop(task_id, activity_id, turn_id, reason);
    match result {
        StopActivityResult::OwnershipMismatch => return Err(not_found()),
        StopActivityResult::NotRunning => {
            return stopped_or_missing_activity(&state, task_id, activity_id).await;
        }
        StopActivityResult::Stopped => {}
    }
    let active = active.ok_or_else(not_found)?;
    let event_id = format!("activity-stop-request-{}", uuid::Uuid::new_v4());
    let payload = json!({
        "activityId": activity_id,
        "tool": active.tool,
        "status": "stop_requested",
        "stopReason": reason,
    });
    state
        .repository
        .append_timeline_events(&[TimelineEvent {
            id: EventId::new(&event_id).map_err(|_| bad_id())?,
            task_id: TaskId::new(task_id).map_err(|_| bad_id())?,
            turn_id: active
                .context
                .turn_id
                .as_deref()
                .and_then(|value| chatcmd_core::TurnId::new(value).ok()),
            session_id: None,
            actor: ActorKind::Tool,
            kind: EventKind::ToolCall,
            idempotency_key: event_id.clone(),
            payload_json: payload.to_string(),
            metadata_json: None,
            created_at_ms: now_ms(),
        }])
        .await
        .map_err(storage_problem)?;
    let mut event = AppEvent::new("tool_call", payload);
    event.id = event_id;
    event.task_id = Some(task_id.to_owned());
    event.turn_id = active.context.turn_id.clone();
    state.publish(event);
    active.context.cancellation.cancel();
    if matches!(active.tool.as_str(), "shell_wait" | "shell_write")
        && let Some(session_id) = active.shell_session_id.as_deref()
    {
        let mut stop_context = OperationContext::new(
            format!("local-stop-activity-{activity_id}"),
            active.context.agent_id.clone(),
            "shell_signal",
        );
        stop_context.task_id = Some(task_id.to_owned());
        stop_context.turn_id = active.context.turn_id.clone();
        let _ = state
            .shell
            .signal(&stop_context, session_id, ShellSignal::CtrlC)
            .await;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn stopped_or_missing_activity(
    state: &Arc<AppState>,
    task_id: &str,
    activity_id: &str,
) -> Result<StatusCode, Problem> {
    if state
        .repository
        .task(&TaskId::new(task_id).map_err(|_| bad_id())?)
        .await
        .map_err(storage_problem)?
        .is_none()
    {
        return Err(not_found());
    }
    let latest = sqlx::query_scalar::<_, String>(
        "SELECT COALESCE(json_extract(payload_json,'$.status'),'') FROM timeline_events WHERE task_id=? AND json_extract(payload_json,'$.activityId')=? ORDER BY created_at_ms DESC,event_id DESC LIMIT 1",
    )
    .bind(task_id)
    .bind(activity_id)
    .fetch_optional(state.repository.pool())
    .await
    .map_err(db_problem)?;
    match latest.as_deref() {
        Some("stopped") => Ok(StatusCode::NO_CONTENT),
        Some(_) => Err(Problem::new(
            StatusCode::CONFLICT,
            "Activity is not running",
            "The activity has already finished or was stopped.",
        )),
        None => Err(not_found()),
    }
}
pub(super) async fn stop_conversation(
    state: &Arc<AppState>,
    id: &str,
) -> Result<Json<Value>, Problem> {
    let task_id = TaskId::new(id).map_err(|_| bad_id())?;
    let task = state
        .repository
        .task(&task_id)
        .await
        .map_err(storage_problem)?
        .ok_or_else(not_found)?;
    let active_rows = sqlx::query(
        "SELECT id FROM terminal_sessions WHERE task_id=? AND status IN ('starting','running')",
    )
    .bind(id)
    .fetch_all(state.repository.pool())
    .await
    .map_err(db_problem)?;
    let active_ids = active_rows
        .iter()
        .map(|row| row.get::<String, _>("id"))
        .collect::<HashSet<_>>();
    let live_sessions = state.shell.list().await.map_err(runtime_problem)?;
    let agent_id = task
        .agent_id
        .as_ref()
        .map_or_else(|| "local-ui".to_owned(), |value| value.as_str().to_owned());
    for session in live_sessions
        .into_iter()
        .filter(|session| active_ids.contains(&session.session_id))
    {
        let mut context = OperationContext::new(
            format!("local-stop-{id}-{}", session.session_id),
            agent_id.clone(),
            "shell_close",
        );
        context.task_id = Some(id.to_owned());
        state
            .shell
            .close(&context, &session.session_id, true)
            .await
            .map_err(runtime_problem)?;
    }

    let now = now_ms();
    sqlx::query("UPDATE chatgpt_bridge_requests SET status='stop_requested',updated_at_ms=? WHERE task_id=? AND status IN ('queued','running')")
        .bind(now)
        .bind(id)
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    sqlx::query("UPDATE terminal_sessions SET status='closed',updated_at_ms=?,closed_at_ms=COALESCE(closed_at_ms,?) WHERE task_id=? AND status IN ('starting','running')")
        .bind(now).bind(now).bind(id).execute(state.repository.pool()).await.map_err(db_problem)?;
    sqlx::query("UPDATE task_sessions SET status='closed',updated_at_ms=? WHERE task_id=? AND status IN ('starting','running')")
        .bind(now).bind(id).execute(state.repository.pool()).await.map_err(db_problem)?;
    sqlx::query("UPDATE tasks SET status='stopped',active_session_id=NULL,stopped_at_ms=?,updated_at_ms=? WHERE id=?")
        .bind(now)
        .bind(now)
        .bind(id)
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    interrupt_active_child_subagents(state, id).await?;
    mark_child_subagent_terminal(state, id, "stopped").await?;
    sqlx::query("UPDATE approvals SET state='cancelled',decision_json='{}',resolved_at_ms=? WHERE task_id=? AND state='pending'")
        .bind(now)
        .bind(id)
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    sqlx::query("UPDATE approval_grants SET state='revoked',updated_at_ms=? WHERE task_id=? AND state='active'")
        .bind(now).bind(id).execute(state.repository.pool()).await.map_err(db_problem)?;
    sqlx::query("DELETE FROM task_execution_modes WHERE task_id=?")
        .bind(id)
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;

    let stop_event_id = format!("conversation-stop-{id}");
    let stop_payload = json!({ "status": "stopped", "content": "Cuộc trò chuyện đã ngừng" });
    state
        .repository
        .append_timeline_events(&[TimelineEvent {
            id: EventId::new(&stop_event_id).map_err(|_| bad_id())?,
            task_id: task_id.clone(),
            turn_id: None,
            session_id: task.active_session_id.clone(),
            actor: ActorKind::System,
            kind: EventKind::Status,
            idempotency_key: stop_event_id.clone(),
            payload_json: stop_payload.to_string(),
            metadata_json: None,
            created_at_ms: now,
        }])
        .await
        .map_err(storage_problem)?;
    let mut event = AppEvent::new("status", stop_payload);
    event.id = stop_event_id;
    event.task_id = Some(id.to_owned());
    state.publish(event);
    task_detail(state, id).await
}

pub(super) fn execution_mode_name(mode: ExecutionMode) -> &'static str {
    match mode {
        ExecutionMode::Allow => "allowAll",
        ExecutionMode::Approval | ExecutionMode::Deny => "approval",
    }
}
