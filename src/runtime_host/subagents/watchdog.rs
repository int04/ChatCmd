use super::*;

impl RuntimeHost {
    pub(in crate::runtime_host) async fn expire_stale_subagents(
        &self,
        parent: Option<(&str, &str)>,
    ) -> RuntimeResult<usize> {
        self.expire_stale_subagents_at(parent, now_ms()).await
    }

    pub(in crate::runtime_host) async fn expire_stale_subagents_at(
        &self,
        parent: Option<(&str, &str)>,
        now: i64,
    ) -> RuntimeResult<usize> {
        self.expire_unclaimed_subagents().await?;
        let rows = if let Some((parent_task_id, parent_turn_id)) = parent {
            sqlx::query("SELECT id,parent_task_id,parent_turn_id,child_task_id,name,worker_id,attempt,started_at_ms,max_runtime_ms,lease_expires_at_ms FROM subagent_runs WHERE parent_task_id=? AND parent_turn_id=? AND status='running' AND (lease_expires_at_ms IS NULL OR lease_expires_at_ms<=? OR started_at_ms+max_runtime_ms<=? OR worker_id<>?) ORDER BY COALESCE(lease_expires_at_ms,0),id LIMIT ?")
                .bind(parent_task_id).bind(parent_turn_id).bind(now).bind(now).bind(self.subagent_worker_id.as_ref()).bind(SUBAGENT_WATCHDOG_BATCH)
                .fetch_all(self.repository.pool()).await
        } else {
            sqlx::query("SELECT id,parent_task_id,parent_turn_id,child_task_id,name,worker_id,attempt,started_at_ms,max_runtime_ms,lease_expires_at_ms FROM subagent_runs WHERE status='running' AND (lease_expires_at_ms IS NULL OR lease_expires_at_ms<=? OR started_at_ms+max_runtime_ms<=? OR worker_id<>?) ORDER BY COALESCE(lease_expires_at_ms,0),id LIMIT ?")
                .bind(now).bind(now).bind(self.subagent_worker_id.as_ref()).bind(SUBAGENT_WATCHDOG_BATCH)
                .fetch_all(self.repository.pool()).await
        }
        .map_err(|_| RuntimeError::new("storage_error", "stale running sub-agent lookup failed"))?;
        let mut expired = 0;
        for row in rows {
            let id = row.get::<String, _>("id");
            let child_task_id = row.get::<Option<String>, _>("child_task_id");
            let worker_id = row.get::<Option<String>, _>("worker_id");
            let attempt = row.get::<i64, _>("attempt");
            let hard_deadline = row
                .get::<Option<i64>, _>("started_at_ms")
                .map(|started| started.saturating_add(row.get::<i64, _>("max_runtime_ms")))
                .is_some_and(|deadline| deadline <= now);
            let reason = if worker_id
                .as_deref()
                .is_some_and(|owner| owner != self.subagent_worker_id.as_ref())
            {
                "worker process restarted before the child completed"
            } else if hard_deadline {
                "child exceeded its maximum runtime"
            } else {
                "child heartbeat lease expired"
            };
            if !self
                .transition_expired_subagent(&id, attempt, worker_id.as_deref(), now, reason)
                .await?
            {
                continue;
            }
            self.telemetry.record_subagent_lease_expired(
                if worker_id
                    .as_deref()
                    .is_some_and(|owner| owner != self.subagent_worker_id.as_ref())
                {
                    chatcmd_runtime::SubagentLeaseExpiryReason::WorkerRestart
                } else if hard_deadline {
                    chatcmd_runtime::SubagentLeaseExpiryReason::HardDeadline
                } else {
                    chatcmd_runtime::SubagentLeaseExpiryReason::Heartbeat
                },
            );
            expired += 1;
            if let Some(child_task_id) = child_task_id.as_deref() {
                self.cleanup_timed_out_subagent(child_task_id, reason, now)
                    .await?;
            }
            self.publish_subagent_status_with_reason(
                &row.get::<String, _>("parent_task_id"),
                &row.get::<String, _>("parent_turn_id"),
                &id,
                child_task_id.as_deref(),
                &row.get::<String, _>("name"),
                "timedOut",
                Some(reason),
                attempt,
            );
        }
        Ok(expired)
    }

    pub(super) async fn cleanup_timed_out_subagent(
        &self,
        child_task_id: &str,
        reason: &str,
        now: i64,
    ) -> RuntimeResult<()> {
        self.activities.cancel_task(child_task_id);
        let agent_id = sqlx::query_scalar::<_, String>("SELECT agent_id FROM tasks WHERE id=?")
            .bind(child_task_id)
            .fetch_optional(self.repository.pool())
            .await
            .map_err(|_| {
                RuntimeError::new("storage_error", "timed-out child agent lookup failed")
            })?;
        let terminal_rows = sqlx::query("SELECT id,turn_id FROM terminal_sessions WHERE task_id=? AND status IN ('starting','running') ORDER BY created_at_ms,id")
            .bind(child_task_id)
            .fetch_all(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "timed-out child terminal lookup failed"))?;
        if let Some(agent_id) = agent_id.as_deref() {
            for row in &terminal_rows {
                let session_id = row.get::<String, _>("id");
                let mut close_context = OperationContext::new(
                    format!("subagent-timeout-close:{child_task_id}:{session_id}"),
                    agent_id,
                    "shell_close",
                );
                close_context.task_id = Some(child_task_id.to_owned());
                close_context.turn_id = row.get::<Option<String>, _>("turn_id");
                let _ = self.shell.close(&close_context, &session_id, true).await;
            }
        }
        sqlx::query("UPDATE tasks SET status=CASE WHEN status IN ('pending','running') THEN 'interrupted' ELSE status END,active_session_id=NULL,updated_at_ms=? WHERE id=?")
            .bind(now).bind(child_task_id).execute(self.repository.pool()).await
            .map_err(|_| RuntimeError::new("storage_error", "timed-out child task cleanup failed"))?;
        sqlx::query("UPDATE terminal_sessions SET status='interrupted',closed_at_ms=COALESCE(closed_at_ms,?),updated_at_ms=? WHERE task_id=? AND status IN ('starting','running')")
            .bind(now).bind(now).bind(child_task_id).execute(self.repository.pool()).await
            .map_err(|_| RuntimeError::new("storage_error", "timed-out child terminal cleanup failed"))?;
        sqlx::query("UPDATE task_sessions SET status='interrupted',updated_at_ms=? WHERE task_id=? AND status IN ('starting','running')")
            .bind(now).bind(child_task_id).execute(self.repository.pool()).await
            .map_err(|_| RuntimeError::new("storage_error", "timed-out child session cleanup failed"))?;
        sqlx::query("UPDATE approvals SET state='cancelled',decision_json=?,resolved_at_ms=? WHERE task_id=? AND state='pending'")
            .bind(json!({"reason": reason}).to_string()).bind(now).bind(child_task_id)
            .execute(self.repository.pool()).await
            .map_err(|_| RuntimeError::new("storage_error", "timed-out child approval cleanup failed"))?;
        sqlx::query("UPDATE approval_grants SET state='revoked',updated_at_ms=? WHERE task_id=? AND state='active'")
            .bind(now)
            .bind(child_task_id)
            .execute(self.repository.pool()).await
            .map_err(|_| RuntimeError::new("storage_error", "timed-out child grant cleanup failed"))?;
        if let Some(turn_id) = sqlx::query_scalar::<_, String>("SELECT turn_id FROM timeline_events WHERE task_id=? AND turn_id IS NOT NULL ORDER BY created_at_ms DESC,event_id DESC LIMIT 1")
            .bind(child_task_id).fetch_optional(self.repository.pool()).await
            .map_err(|_| RuntimeError::new("storage_error", "timed-out child turn lookup failed"))?
        {
            let _ = self.reconcile_orphaned_tool_calls(child_task_id, &turn_id, None, "subagent_timeout", now).await;
        }
        self.publish_event(
            format!("subagent-timeout:{child_task_id}:{now}"),
            "status",
            Some(child_task_id.to_owned()),
            None,
            None,
            json!({"status":"interrupted","subagentStatus":"timedOut","content":reason}),
        );
        Ok(())
    }

    // Recheck the deadline in the write, not just in the watchdog's earlier SELECT.
    // A heartbeat may have committed while the watchdog was examining that snapshot.
    pub(in crate::runtime_host) async fn transition_expired_subagent(
        &self,
        id: &str,
        attempt: i64,
        worker_id: Option<&str>,
        now: i64,
        reason: &str,
    ) -> RuntimeResult<bool> {
        let affected = sqlx::query("UPDATE subagent_runs SET status='timedOut',terminal_reason=?,lease_expires_at_ms=NULL,updated_at_ms=?,completed_at_ms=? WHERE id=? AND status='running' AND attempt=? AND worker_id IS ? AND (lease_expires_at_ms IS NULL OR lease_expires_at_ms<=? OR started_at_ms+max_runtime_ms<=? OR worker_id<>?)")
            .bind(reason).bind(now).bind(now).bind(id).bind(attempt).bind(worker_id)
            .bind(now).bind(now).bind(self.subagent_worker_id.as_ref())
            .execute(self.repository.pool()).await
            .map_err(|_| RuntimeError::new("storage_error", "stale sub-agent transition failed"))?;
        Ok(affected.rows_affected() == 1)
    }

    pub(super) async fn expire_unclaimed_subagents(&self) -> RuntimeResult<()> {
        let now = now_ms();
        let native_cutoff = now.saturating_sub(UNCLAIMED_SUBAGENT_TIMEOUT_MS);
        let extension_cutoff = now.saturating_sub(EXTENSION_UNCLAIMED_SUBAGENT_TIMEOUT_MS);
        let stale = "status='pending' AND child_task_id IS NOT NULL AND ((fallback_state='none' AND created_at_ms<=?) OR (fallback_state IN ('requested','started') AND (updated_at_ms<=? OR created_at_ms+max_runtime_ms<=?)))";
        let sql = format!(
            "SELECT id,child_task_id,parent_task_id,parent_turn_id,name,fallback_state FROM subagent_runs WHERE {stale} ORDER BY created_at_ms,id LIMIT {SUBAGENT_WATCHDOG_BATCH}"
        );
        let rows = sqlx::query(&sql)
            .bind(native_cutoff)
            .bind(extension_cutoff)
            .bind(now)
            .fetch_all(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "stale sub-agent lookup failed"))?;
        for row in rows {
            let child_task_id = row.get::<String, _>("child_task_id");
            let message = if matches!(
                row.get::<String, _>("fallback_state").as_str(),
                "requested" | "started"
            ) {
                "Child browser fallback stopped responding or exceeded its maximum runtime."
            } else {
                "Child agent was registered but no worker claimed it within 60 seconds."
            };
            let sql = format!(
                "UPDATE subagent_runs SET status='failed',terminal_reason=?,updated_at_ms=?,completed_at_ms=? WHERE id=? AND {stale}"
            );
            let affected = sqlx::query(&sql)
                .bind(message)
                .bind(now)
                .bind(now)
                .bind(row.get::<String, _>("id"))
                .bind(native_cutoff)
                .bind(extension_cutoff)
                .bind(now)
                .execute(self.repository.pool())
                .await
                .map_err(|_| RuntimeError::new("storage_error", "unclaimed child expiry failed"))?;
            if affected.rows_affected() == 0 {
                continue;
            }
            self.fail_subagent_worker(&child_task_id, message).await?;
            self.publish_subagent_status_with_reason(
                &row.get::<String, _>("parent_task_id"),
                &row.get::<String, _>("parent_turn_id"),
                &row.get::<String, _>("id"),
                Some(&child_task_id),
                &row.get::<String, _>("name"),
                "failed",
                Some(message),
                0,
            );
        }
        Ok(())
    }
}
