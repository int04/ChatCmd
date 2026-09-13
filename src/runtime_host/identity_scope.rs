//! Durable MCP-scope aliases preserve task identity after bridge completion or restart.
use super::{RuntimeHost, now_ms};
use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use std::{collections::BTreeSet, time::Duration};

impl RuntimeHost {
    pub(super) async fn mapped_mcp_scope_task(
        &self,
        context: &OperationContext,
    ) -> RuntimeResult<Option<String>> {
        let Some(scope) = context.conversation_scope_id.as_deref() else {
            return Ok(None);
        };
        let row = sqlx::query_as::<_, (String, i64, i64)>(
            "SELECT b.task_id,b.generation,t.generation FROM chatgpt_mcp_scope_bindings b JOIN tasks t ON t.id=b.task_id WHERE b.agent_id=? AND b.device_id=? AND b.scope_hash=? AND t.agent_id=b.agent_id AND t.device_id=b.device_id",
        ).bind(&context.agent_id).bind(self.device.id.as_str()).bind(scope)
            .fetch_optional(self.repository.pool()).await.map_err(scope_storage_error)?;
        let Some((task, bound_generation, current_generation)) = row else {
            return Ok(None);
        };
        if bound_generation != current_generation {
            return Err(RuntimeError::new(
                "conversation_compacting_or_archived",
                "this MCP scope belongs to an earlier conversation generation; use the confirmed replacement conversation, not a new task",
            ));
        }
        Ok(Some(task))
    }

    pub(super) async fn recover_manual_compact_task(
        &self,
        context: &OperationContext,
        message: &str,
    ) -> RuntimeResult<Option<String>> {
        let Some(scope) = context
            .conversation_scope_id
            .as_deref()
            .filter(|scope| scope.starts_with("openai:"))
        else {
            return Ok(None);
        };
        if message.trim().is_empty() {
            return Ok(None);
        }

        const RECENT_COMPACT_MS: i64 = 30_000;
        const BROWSER_BIND_WAIT_MS: u64 = 1_200;
        let cutoff = now_ms().saturating_sub(RECENT_COMPACT_MS);
        let eligible: i64 = sqlx::query_scalar(
            r#"SELECT COUNT(*)
               FROM chatgpt_compact_jobs j
               JOIN tasks t ON t.id=j.task_id
               JOIN chatgpt_conversations c
                 ON c.task_id=j.task_id AND c.conversation_id=j.new_conversation_id
               WHERE j.phase='completed'
                 AND j.continue_after_compact=0
                 AND j.completed_at_ms IS NOT NULL AND j.completed_at_ms>=?
                 AND t.agent_id=? AND t.device_id=?
                 AND t.source='chatgpt_web' AND t.allow_execute=1
                 AND NOT EXISTS(
                   SELECT 1 FROM chatgpt_mcp_scope_bindings b
                   WHERE b.task_id=t.id AND b.generation=t.generation
                 )"#,
        )
        .bind(cutoff)
        .bind(&context.agent_id)
        .bind(self.device.id.as_str())
        .fetch_one(self.repository.pool())
        .await
        .map_err(scope_storage_error)?;
        if eligible == 0 {
            return Ok(None);
        }

        let deadline = tokio::time::Instant::now() + Duration::from_millis(BROWSER_BIND_WAIT_MS);
        loop {
            let rows = sqlx::query_as::<_, (String, String)>(
                r#"SELECT j.task_id,r.submitted_content
                   FROM chatgpt_compact_jobs j
                   JOIN tasks t ON t.id=j.task_id
                   JOIN chatgpt_conversations c
                     ON c.task_id=j.task_id AND c.conversation_id=j.new_conversation_id
                   JOIN chatgpt_bridge_requests r
                     ON r.task_id=j.task_id AND r.conversation_id=c.conversation_id
                   WHERE j.phase='completed'
                     AND j.continue_after_compact=0
                     AND j.completed_at_ms IS NOT NULL AND j.completed_at_ms>=?
                     AND t.agent_id=? AND t.device_id=?
                     AND t.source='chatgpt_web' AND t.allow_execute=1
                     AND r.agent_id=t.agent_id
                     AND r.created_at_ms>=j.completed_at_ms
                     AND r.status IN ('queued','running','stop_requested','completed')
                     AND NOT EXISTS(
                       SELECT 1 FROM chatgpt_compact_obsolete_requests o WHERE o.request_id=r.id
                     )
                     AND NOT EXISTS(
                       SELECT 1 FROM chatgpt_mcp_scope_bindings b
                       WHERE b.task_id=t.id AND b.generation=t.generation
                     )
                   ORDER BY r.updated_at_ms DESC,r.id DESC
                   LIMIT 32"#,
            )
            .bind(cutoff)
            .bind(&context.agent_id)
            .bind(self.device.id.as_str())
            .fetch_all(self.repository.pool())
            .await
            .map_err(scope_storage_error)?;
            let matches = rows
                .into_iter()
                .filter(|(_, submitted)| {
                    submitted == message || crate::chatgpt_message::equivalent(submitted, message)
                })
                .map(|(task, _)| task)
                .collect::<BTreeSet<_>>();
            if matches.len() == 1 {
                return Ok(matches.into_iter().next());
            }
            if matches.len() > 1 {
                return Err(RuntimeError::new(
                    "conversation_identity_ambiguous",
                    "multiple replacement conversations recorded the same user turn; retry from the intended ChatGPT tab instead of creating another conversation",
                ));
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        tracing::debug!(
            agent_id = %context.agent_id,
            provider_scope = %scope,
            "manual post-compact MCP turn arrived before browser identity binding"
        );
        Err(RuntimeError::new(
            "conversation_identity_pending",
            "the replacement ChatGPT conversation is still binding this user turn to the existing task; retry the same message instead of approving a new conversation",
        ))
    }

    pub(super) async fn validate_resolved_task(
        &self,
        context: &OperationContext,
        task: &str,
    ) -> RuntimeResult<()> {
        let row = sqlx::query_as::<_, (Option<String>, String)>(
            "SELECT agent_id,device_id FROM tasks WHERE id=?",
        )
        .bind(task)
        .fetch_optional(self.repository.pool())
        .await
        .map_err(scope_storage_error)?;
        if row.is_some_and(|(agent, device)| {
            agent.as_deref() != Some(&context.agent_id) || device != self.device.id.as_str()
        }) {
            return Err(RuntimeError::new(
                "conversation_identity_conflict",
                "the task does not belong to the authenticated agent and local device",
            ));
        }
        let compacting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM chatgpt_compact_jobs WHERE task_id=? AND phase NOT IN ('completed','cancelled'))")
            .bind(task).fetch_one(self.repository.pool()).await.map_err(scope_storage_error)?;
        if compacting {
            return Err(RuntimeError::new(
                "conversation_compacting_or_archived",
                "the resolved task is compacting; wait for its confirmed replacement binding",
            ));
        }
        Ok(())
    }

    pub(super) async fn persist_mcp_scope_binding(
        &self,
        context: &OperationContext,
    ) -> RuntimeResult<()> {
        let (Some(scope), Some(task)) = (
            context.conversation_scope_id.as_deref(),
            context.task_id.as_deref(),
        ) else {
            return Ok(());
        };
        // Do not turn the shared HTTP transport session into a conversation identity.
        if !scope.starts_with("openai:") {
            return Ok(());
        }
        let generation: Option<i64> = sqlx::query_scalar("SELECT generation FROM tasks WHERE id=? AND agent_id=? AND device_id=? AND source='chatgpt_web' AND allow_execute=1")
            .bind(task).bind(&context.agent_id).bind(self.device.id.as_str())
            .fetch_optional(self.repository.pool()).await.map_err(scope_storage_error)?;
        let Some(generation) = generation else {
            return Ok(());
        };
        let now = now_ms();
        let changed = sqlx::query("INSERT INTO chatgpt_mcp_scope_bindings(agent_id,device_id,scope_hash,task_id,generation,created_at_ms,updated_at_ms) VALUES(?,?,?,?,?,?,?) ON CONFLICT(agent_id,device_id,scope_hash) DO UPDATE SET generation=excluded.generation,updated_at_ms=excluded.updated_at_ms WHERE chatgpt_mcp_scope_bindings.task_id=excluded.task_id")
            .bind(&context.agent_id).bind(self.device.id.as_str()).bind(scope).bind(task).bind(generation).bind(now).bind(now)
            .execute(self.repository.pool()).await.map_err(scope_storage_error)?;
        if changed.rows_affected() == 0 {
            return Err(RuntimeError::new(
                "conversation_identity_conflict",
                "the MCP scope is already attached to another task; no rebinding is allowed",
            ));
        }
        Ok(())
    }
}

fn scope_storage_error(_: sqlx::Error) -> RuntimeError {
    RuntimeError::new(
        "storage_error",
        "durable conversation identity lookup or persistence failed",
    )
}
