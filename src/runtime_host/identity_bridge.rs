//! Task lookups used only after explicit and durable identities are exhausted.
use super::{RuntimeHost, now_ms};
use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use std::collections::BTreeSet;

impl RuntimeHost {
    pub(super) async fn bound_task_for_conversation_scope(
        &self,
        agent_id: &str,
        conversation_scope: &str,
    ) -> RuntimeResult<Option<String>> {
        sqlx::query_scalar::<_, String>(
            r#"SELECT t.id
               FROM tasks t
               WHERE t.agent_id=? AND t.conversation_scope_hash=?
                 AND (
                   t.source='chatgpt_web'
                   OR EXISTS(
                     SELECT 1 FROM timeline_events e
                     WHERE e.task_id=t.id AND e.actor='user' AND e.kind='message'
                   )
                 )
               ORDER BY t.created_at_ms,t.id
               LIMIT 1"#,
        )
        .bind(agent_id)
        .bind(conversation_scope)
        .fetch_optional(self.repository.pool())
        .await
        .map_err(|_| RuntimeError::new("storage_error", "conversation task binding lookup failed"))
    }

    pub(super) async fn chatgpt_bridge_task_for_message(
        &self,
        agent_id: &str,
        preferred_task_id: Option<&str>,
        message: &str,
    ) -> RuntimeResult<Option<String>> {
        let cutoff = now_ms().saturating_sub(5 * 60 * 1000);
        let rows = sqlx::query_as::<_, (String, String)>(
            r#"SELECT r.task_id,r.submitted_content
               FROM chatgpt_bridge_requests r
               JOIN tasks t ON t.id=r.task_id
               LEFT JOIN chatgpt_conversations c ON c.task_id=r.task_id
               WHERE r.task_id IS NOT NULL
                 AND t.agent_id=? AND t.source='chatgpt_web'
                 AND r.status IN ('queued','running','stop_requested')
                 AND (c.active_request_id=r.id OR r.updated_at_ms>=?)
               ORDER BY r.updated_at_ms DESC,r.id DESC
               LIMIT 32"#,
        )
        .bind(agent_id)
        .bind(cutoff)
        .fetch_all(self.repository.pool())
        .await
        .map_err(|_| {
            RuntimeError::new("storage_error", "ChatGPT bridge task binding lookup failed")
        })?;
        let matched = unique_bridge_task_for_message(&rows, preferred_task_id, message);
        if matched.is_none()
            && rows
                .iter()
                .any(|(_, submitted)| crate::chatgpt_message::equivalent(submitted, message))
        {
            return Err(RuntimeError::new(
                "conversation_identity_ambiguous",
                "multiple ChatUI conversations match this message; reuse the existing taskId/turnId instead of creating a new conversation",
            ));
        }
        Ok(matched)
    }

    pub(super) async fn unique_recent_chatgpt_bridge_task(
        &self,
        agent_id: &str,
    ) -> RuntimeResult<Option<String>> {
        let cutoff = now_ms().saturating_sub(90_000);
        let rows = sqlx::query_scalar::<_, String>(
            r#"SELECT r.task_id
               FROM chatgpt_bridge_requests r
               JOIN tasks t ON t.id=r.task_id
               LEFT JOIN chatgpt_conversations c ON c.task_id=r.task_id
               WHERE r.task_id IS NOT NULL
                 AND t.agent_id=? AND t.source='chatgpt_web' AND t.status='running'
                 AND r.status IN ('queued','running','stop_requested')
                 AND (c.active_request_id=r.id OR r.updated_at_ms>=?)
               GROUP BY r.task_id
               ORDER BY MAX(CASE WHEN c.active_request_id=r.id THEN 1 ELSE 0 END) DESC,
                        MAX(r.updated_at_ms) DESC
               LIMIT 2"#,
        )
        .bind(agent_id)
        .bind(cutoff)
        .fetch_all(self.repository.pool())
        .await
        .map_err(|_| {
            RuntimeError::new("storage_error", "active ChatGPT bridge task lookup failed")
        })?;
        Ok((rows.len() == 1).then(|| rows[0].clone()))
    }

    pub(super) async fn bound_task_for_turn(
        &self,
        context: &OperationContext,
    ) -> RuntimeResult<Option<String>> {
        let Some(turn_id) = context
            .turn_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(None);
        };
        let cutoff = now_ms().saturating_sub(2 * 60 * 60 * 1000);
        sqlx::query_scalar::<_, String>(
            "SELECT task_id FROM turn_bindings WHERE agent_id=? AND device_id=? AND turn_id=? AND last_used_at_ms>=? LIMIT 1",
        )
        .bind(&context.agent_id)
        .bind(self.device.id.as_str())
        .bind(turn_id)
        .bind(cutoff)
        .fetch_optional(self.repository.pool())
        .await
        .map_err(|_| RuntimeError::new("storage_error", "turn binding lookup failed"))
    }
}

pub(super) fn unique_bridge_task_for_message(
    rows: &[(String, String)],
    preferred_task_id: Option<&str>,
    message: &str,
) -> Option<String> {
    let exact = rows
        .iter()
        .filter(|(_, submitted)| submitted == message)
        .map(|(task_id, _)| task_id.as_str())
        .collect::<BTreeSet<_>>();
    if !exact.is_empty() {
        return preferred_or_unique_task(exact, preferred_task_id);
    }
    let equivalent = rows
        .iter()
        .filter(|(_, submitted)| crate::chatgpt_message::equivalent(submitted, message))
        .map(|(task_id, _)| task_id.as_str())
        .collect::<BTreeSet<_>>();
    preferred_or_unique_task(equivalent, preferred_task_id)
}

fn preferred_or_unique_task(
    candidates: BTreeSet<&str>,
    preferred_task_id: Option<&str>,
) -> Option<String> {
    if let Some(preferred) = preferred_task_id
        && candidates.contains(preferred)
    {
        return Some(preferred.to_owned());
    }
    (candidates.len() == 1)
        .then(|| candidates.into_iter().next())
        .flatten()
        .map(str::to_owned)
}
