use super::*;

impl RuntimeHost {
    pub(in crate::runtime_host) async fn finish_subagent_for_child(
        &self,
        child_task_id: &str,
        status: &str,
    ) -> RuntimeResult<bool> {
        let row = sqlx::query("SELECT id,parent_task_id,parent_turn_id,name,status FROM subagent_runs WHERE child_task_id=? LIMIT 1")
            .bind(child_task_id)
            .fetch_optional(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "sub-agent completion lookup failed"))?;
        let Some(row) = row else {
            return Ok(true);
        };
        let terminal_status = normalize_terminal_status(status);
        let now = now_ms();
        let affected = sqlx::query("UPDATE subagent_runs SET status=?,terminal_reason=NULL,lease_expires_at_ms=NULL,updated_at_ms=?,completed_at_ms=? WHERE child_task_id=? AND status IN ('pending','running')")
            .bind(terminal_status)
            .bind(now)
            .bind(now)
            .bind(child_task_id)
            .execute(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "sub-agent completion update failed"))?
            .rows_affected();
        if affected == 0 {
            let current = sqlx::query_scalar::<_, String>(
                "SELECT status FROM subagent_runs WHERE child_task_id=? LIMIT 1",
            )
            .bind(child_task_id)
            .fetch_optional(self.repository.pool())
            .await
            .map_err(|_| {
                RuntimeError::new("storage_error", "sub-agent completion refresh failed")
            })?;
            return Ok(current.as_deref() == Some(terminal_status));
        }
        sqlx::query("UPDATE approval_grants SET state='revoked',updated_at_ms=? WHERE task_id=? AND state='active'")
            .bind(now)
            .bind(child_task_id)
            .execute(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "child approval grant revoke failed"))?;
        let subagent_id = row.get::<String, _>("id");
        let parent_task_id = row.get::<String, _>("parent_task_id");
        let parent_turn_id = row.get::<String, _>("parent_turn_id");
        let name = row.get::<String, _>("name");
        self.publish_subagent_status(
            &parent_task_id,
            &parent_turn_id,
            &subagent_id,
            Some(child_task_id),
            &name,
            terminal_status,
        );
        Ok(true)
    }

    pub(in crate::runtime_host) async fn fail_subagent_worker(
        &self,
        child_task_id: &str,
        message: &str,
    ) -> RuntimeResult<()> {
        if !self
            .finish_subagent_for_child(child_task_id, "failed")
            .await?
        {
            return Ok(());
        }
        let registered_status = sqlx::query_scalar::<_, String>(
            "SELECT status FROM subagent_runs WHERE child_task_id=? LIMIT 1",
        )
        .bind(child_task_id)
        .fetch_optional(self.repository.pool())
        .await
        .map_err(|_| RuntimeError::new("storage_error", "child failure state lookup failed"))?;
        if registered_status
            .as_deref()
            .is_some_and(|status| status != "failed")
        {
            return Ok(());
        }
        let now = now_ms();
        let reason: String = message.chars().take(2_000).collect();
        sqlx::query("UPDATE subagent_runs SET terminal_reason=COALESCE(terminal_reason,?),updated_at_ms=? WHERE child_task_id=? AND status='failed'")
            .bind(&reason).bind(now).bind(child_task_id)
            .execute(self.repository.pool()).await
            .map_err(|_| RuntimeError::new("storage_error", "child failure reason could not be persisted"))?;
        let affected = sqlx::query("UPDATE tasks SET status='failed',active_session_id=NULL,updated_at_ms=? WHERE id=? AND status NOT IN ('completed','stopped')")
            .bind(now)
            .bind(child_task_id)
            .execute(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "child task failure state update failed"))?
            .rows_affected();
        if affected == 0 {
            return Ok(());
        }
        self.publish_event(
            format!("subagent-worker-failed:{child_task_id}:{now}"),
            "status",
            Some(child_task_id.to_owned()),
            None,
            None,
            json!({ "status": "failed", "content": message }),
        );
        Ok(())
    }

    pub(in crate::runtime_host) async fn heartbeat_subagent(
        &self,
        child_task_id: &str,
    ) -> RuntimeResult<bool> {
        let now = now_ms();
        let lease_expires_at = now.saturating_add(subagent_lease_ms());
        let affected = sqlx::query("UPDATE subagent_runs SET last_heartbeat_at_ms=?,lease_expires_at_ms=MIN(MAX(?,CASE WHEN fallback_state='claimed' THEN ? ELSE 0 END),started_at_ms+max_runtime_ms),updated_at_ms=? WHERE child_task_id=? AND worker_id=? AND status='running' AND ? < started_at_ms+max_runtime_ms")
            .bind(now)
            .bind(lease_expires_at)
            .bind(now.saturating_add(chatcmd_storage::subagent_tree::BROWSER_HEARTBEAT_LEASE_MS))
            .bind(now)
            .bind(child_task_id)
            .bind(self.subagent_worker_id.as_ref())
            .bind(now)
            .execute(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "sub-agent heartbeat failed"))?
            .rows_affected();
        if affected == 1 {
            sqlx::query("UPDATE approval_grants SET expires_at_ms=MIN(COALESCE((SELECT parent.expires_at_ms FROM approval_grants parent WHERE parent.id=approval_grants.inherited_from),expires_at_ms),(SELECT lease_expires_at_ms FROM subagent_runs WHERE child_task_id=?)),updated_at_ms=? WHERE task_id=? AND state='active' AND inherited_from IS NOT NULL")
                .bind(child_task_id)
                .bind(now)
                .bind(child_task_id)
                .execute(self.repository.pool())
                .await
                .map_err(|_| RuntimeError::new("storage_error", "child approval grant heartbeat failed"))?;
        }
        Ok(affected == 1)
    }
}
