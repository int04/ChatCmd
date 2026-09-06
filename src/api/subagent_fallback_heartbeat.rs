use super::*;
use chatcmd_storage::subagent_tree::BROWSER_HEARTBEAT_LEASE_MS;

/// A browser heartbeat extends liveness only. It does not grant tool permissions,
/// claim a worker, change attempts, or revive an already terminal child.
pub(in crate::api) async fn subagent_fallback_heartbeat(
    State(state): State<Arc<AppState>>,
    Path(subagent_id): Path<String>,
    Json(input): Json<SubagentFallbackStarted>,
) -> Result<Json<Value>, Problem> {
    validate_attempt(input.attempt)?;
    let row = fallback_row(&state, subagent_id.trim()).await?;
    if row.get::<String, _>("status") == "pending" && !creation_enabled(&state).await? {
        return Ok(Json(
            json!({"accepted": false, "active": false, "status": "pending", "reason": "subagents_disabled", "attempt": input.attempt}),
        ));
    }
    let now = now_ms();
    let mut transaction = state.repository.pool().begin().await.map_err(db_problem)?;
    let renewed = sqlx::query("UPDATE subagent_runs SET last_heartbeat_at_ms=?,updated_at_ms=?,lease_expires_at_ms=CASE WHEN status='running' THEN MIN(?,started_at_ms+max_runtime_ms) ELSE lease_expires_at_ms END WHERE id=? AND fallback_attempts=? AND ((status='pending' AND fallback_state IN ('requested','started') AND ?<created_at_ms+max_runtime_ms) OR (status='running' AND fallback_state='claimed' AND ?<started_at_ms+max_runtime_ms))")
        .bind(now).bind(now).bind(now.saturating_add(BROWSER_HEARTBEAT_LEASE_MS))
        .bind(subagent_id.trim()).bind(input.attempt).bind(now).bind(now)
        .execute(&mut *transaction).await.map_err(db_problem)?;
    if renewed.rows_affected() > 0 {
        // Match the MCP heartbeat's inherited-grant lifecycle without widening its parent bounds.
        sqlx::query("UPDATE approval_grants SET expires_at_ms=MIN(COALESCE((SELECT parent.expires_at_ms FROM approval_grants parent WHERE parent.id=approval_grants.inherited_from),expires_at_ms),(SELECT lease_expires_at_ms FROM subagent_runs WHERE id=?)),updated_at_ms=? WHERE task_id=? AND state='active' AND inherited_from IS NOT NULL AND (SELECT status FROM subagent_runs WHERE id=?)='running'")
            .bind(subagent_id.trim()).bind(now).bind(row.get::<Option<String>, _>("child_task_id"))
            .bind(subagent_id.trim()).execute(&mut *transaction).await.map_err(db_problem)?;
    }
    let current = sqlx::query("SELECT status,fallback_state,fallback_attempts,terminal_reason,COALESCE(started_at_ms,created_at_ms)+max_runtime_ms AS deadline FROM subagent_runs WHERE id=?")
        .bind(subagent_id.trim()).fetch_one(&mut *transaction).await.map_err(db_problem)?;
    transaction.commit().await.map_err(db_problem)?;
    let status = current.get::<String, _>("status");
    let current_attempt = current.get::<i64, _>("fallback_attempts");
    let active = renewed.rows_affected() == 1;
    Ok(Json(json!({
        "accepted": active,
        "active": active,
        "status": status,
        "attempt": current_attempt,
        "fallbackState": current.get::<String, _>("fallback_state"),
        "reason": if current_attempt != input.attempt { Some("stale_attempt".to_owned()) }
            else { current.get::<Option<String>, _>("terminal_reason") },
        "completed": status == "completed",
        "deadlineAtMs": current.get::<i64, _>("deadline"),
        "retryAfterMs": 15_000
    })))
}

pub(super) async fn creation_enabled(state: &AppState) -> Result<bool, Problem> {
    use chatcmd_core::SettingsStore as _;
    let value = state
        .repository
        .setting("ui_subagentConcurrency")
        .await
        .map_err(super::super::storage_problem)?;
    Ok(value
        .and_then(|setting| serde_json::from_str::<i64>(&setting.value_json).ok())
        .unwrap_or(0)
        > 0)
}
