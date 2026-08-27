use chatcmd_core::{
    AgentId, SessionId, Task, TaskId, TaskSession, TaskStatus, TaskStore as _,
    TerminalSessionStatus, TurnBinding, TurnId,
};
use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use uuid::Uuid;

use super::{RuntimeHost, invalid, now_ms, storage_error};

impl RuntimeHost {
    pub(super) async fn ensure_call_identity(
        &self,
        context: &mut OperationContext,
        first_user_message: Option<&str>,
    ) -> RuntimeResult<()> {
        let conversation_scope = context.conversation_scope_id.clone();
        let bound_task = if conversation_scope.is_none() && context.task_id.is_none() {
            self.bound_task_for_turn(context).await?
        } else {
            None
        };
        let delegated_task = self
            .delegated_subagent_task_id(context, first_user_message)
            .await?;
        let mapped_scope_task = if delegated_task.is_none() {
            if let Some(scope) = conversation_scope.as_deref() {
                self.bound_task_for_conversation_scope(&context.agent_id, scope)
                    .await?
            } else {
                None
            }
        } else {
            None
        };
        let task = delegated_task.unwrap_or_else(|| {
            mapped_scope_task.unwrap_or_else(|| {
                if let (Some(scope), Some(message)) =
                    (conversation_scope.as_deref(), first_user_message)
                {
                    task_identity_from_first_message(&context.agent_id, scope, message)
                } else {
                    select_task_identity(
                        &context.agent_id,
                        conversation_scope.as_deref(),
                        context.task_id.as_deref(),
                        bound_task.as_deref(),
                        &context.request_id,
                    )
                }
            })
        });
        let logical_session = safe_id("mcp-session", &context.agent_id, &task);
        let turn = context.turn_id.clone().unwrap_or_else(|| {
            safe_id(
                "turn",
                &context.agent_id,
                &format!("{task}\0{}", context.request_id),
            )
        });
        context.task_id = Some(task.clone());
        context.turn_id = Some(turn);
        context.mcp_session_id = Some(logical_session.clone());

        let task_id = TaskId::new(task).map_err(|error| invalid("taskId", error))?;
        let session_id =
            SessionId::new(logical_session).map_err(|error| invalid("sessionId", error))?;
        let current = self
            .repository
            .task(&task_id)
            .await
            .map_err(storage_error)?;
        if current
            .as_ref()
            .is_some_and(|task| task.status == TaskStatus::Stopped)
        {
            return Err(RuntimeError::new(
                "conversation_stopped",
                "this conversation was stopped by the user and cannot continue",
            ));
        }
        let now = now_ms();
        let generation = current.as_ref().map_or(1, |task| task.generation);
        self.repository
            .upsert_task(&Task {
                id: task_id.clone(),
                agent_id: AgentId::new(&context.agent_id).ok(),
                device_id: self.device.id.clone(),
                conversation_scope_hash: conversation_scope.or_else(|| {
                    current
                        .as_ref()
                        .and_then(|task| task.conversation_scope_hash.clone())
                }),
                title: current.as_ref().and_then(|task| task.title.clone()),
                source: Some("mcp".to_owned()),
                status: TaskStatus::Running,
                active_session_id: Some(session_id.clone()),
                generation,
                stopped_at_ms: None,
                created_at_ms: current.as_ref().map_or(now, |task| task.created_at_ms),
                updated_at_ms: now,
            })
            .await
            .map_err(storage_error)?;
        self.claim_subagent_from_message(context, task_id.as_str(), first_user_message)
            .await?;
        self.repository
            .upsert_task_session(&TaskSession {
                task_id: task_id.clone(),
                session_id,
                generation,
                replaced_session_id: None,
                status: TerminalSessionStatus::Running,
                created_at_ms: now,
                updated_at_ms: now,
            })
            .await
            .map_err(storage_error)?;
        let agent_id =
            AgentId::new(&context.agent_id).map_err(|error| invalid("agentId", error))?;
        let turn_id = TurnId::new(context.turn_id.as_deref().unwrap_or_default())
            .map_err(|error| invalid("turnId", error))?;
        self.repository
            .bind_turn(&TurnBinding {
                agent_id,
                device_id: self.device.id.clone(),
                turn_id,
                task_id,
                last_used_at_ms: now,
            })
            .await
            .map_err(storage_error)
    }

    async fn bound_task_for_conversation_scope(
        &self,
        agent_id: &str,
        conversation_scope: &str,
    ) -> RuntimeResult<Option<String>> {
        sqlx::query_scalar::<_, String>(
            "SELECT id FROM tasks WHERE agent_id=? AND conversation_scope_hash=? ORDER BY created_at_ms,id LIMIT 1",
        )
        .bind(agent_id)
        .bind(conversation_scope)
        .fetch_optional(self.repository.pool())
        .await
        .map_err(|_| RuntimeError::new("storage_error", "conversation task binding lookup failed"))
    }

    async fn bound_task_for_turn(
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

fn select_task_identity(
    agent_id: &str,
    conversation_scope: Option<&str>,
    explicit_task_id: Option<&str>,
    bound_task_id: Option<&str>,
    request_id: &str,
) -> String {
    if let Some(scope) = conversation_scope.filter(|value| !value.trim().is_empty()) {
        return safe_id("task-chat", agent_id, scope);
    }
    if let Some(task_id) = explicit_task_id.filter(|value| !value.trim().is_empty()) {
        return task_id.trim().to_owned();
    }
    if let Some(task_id) = bound_task_id.filter(|value| !value.trim().is_empty()) {
        return task_id.trim().to_owned();
    }
    safe_id("task", agent_id, request_id)
}

fn task_identity_from_first_message(
    agent_id: &str,
    conversation_scope: &str,
    message: &str,
) -> String {
    safe_id(
        "task-chat",
        agent_id,
        &format!("{conversation_scope}\0first-user-message:{message}"),
    )
}

fn safe_id(prefix: &str, agent_id: &str, scope: &str) -> String {
    let material = format!("{prefix}\0agent:{agent_id}\0scope:{scope}");
    format!(
        "{prefix}-{}",
        Uuid::new_v5(&Uuid::NAMESPACE_OID, material.as_bytes())
    )
}

#[cfg(test)]
mod tests {
    use super::{select_task_identity, task_identity_from_first_message};

    #[test]
    fn conversation_scope_overrides_stale_explicit_task() {
        let first =
            select_task_identity("agent", Some("openai:chat-a"), Some("old-task"), None, "r1");
        let second =
            select_task_identity("agent", Some("openai:chat-b"), Some("old-task"), None, "r2");
        assert_ne!(first, "old-task");
        assert_ne!(second, "old-task");
        assert_ne!(first, second);
    }

    #[test]
    fn first_user_message_participates_in_new_chat_identity() {
        let first = task_identity_from_first_message("agent", "openai:scope", "xin chào");
        assert_eq!(
            first,
            task_identity_from_first_message("agent", "openai:scope", "xin chào")
        );
        assert_ne!(
            first,
            task_identity_from_first_message("agent", "openai:scope", "một tin nhắn khác")
        );
    }

    #[test]
    fn explicit_task_and_turn_binding_are_safe_fallbacks_without_private_scope() {
        assert_eq!(
            select_task_identity("agent", None, Some("task-known"), Some("task-bound"), "r1"),
            "task-known"
        );
        assert_eq!(
            select_task_identity("agent", None, None, Some("task-bound"), "r1"),
            "task-bound"
        );
        assert_ne!(
            select_task_identity("agent", None, None, None, "r1"),
            select_task_identity("agent", None, None, None, "r2")
        );
    }
}
