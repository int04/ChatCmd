use chatcmd_core::{
    AgentId, SessionId, SettingsStore as _, Task, TaskId, TaskSession, TaskStatus, TaskStore as _,
    TerminalSessionStatus, TurnBinding, TurnId,
};
use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use uuid::Uuid;

use super::{RuntimeHost, invalid, now_ms, storage_error};

#[path = "identity_bridge.rs"]
mod bridge;
#[cfg(test)]
use bridge::unique_bridge_task_for_message;

impl RuntimeHost {
    pub(super) async fn ensure_call_identity(
        &self,
        context: &mut OperationContext,
        first_user_message: Option<&str>,
    ) -> RuntimeResult<()> {
        let conversation_scope = context.conversation_scope_id.clone();
        let explicit_task = context
            .task_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let bound_task = if explicit_task.is_none() {
            self.bound_task_for_turn(context).await?
        } else {
            None
        };
        let delegated_task = self
            .delegated_subagent_task_id(context, first_user_message)
            .await?;
        // Persisted identity outranks prompt similarity: two chats may both say "commit".
        let mapped_scope_task =
            if explicit_task.is_none() && bound_task.is_none() && delegated_task.is_none() {
                if let Some(scope) = conversation_scope.as_deref() {
                    self.bound_task_for_conversation_scope(&context.agent_id, scope)
                        .await?
                } else {
                    None
                }
            } else {
                None
            };
        let chatgpt_bridge_task = if explicit_task.is_none()
            && bound_task.is_none()
            && delegated_task.is_none()
            && mapped_scope_task.is_none()
        {
            if let Some(message) = first_user_message {
                self.chatgpt_bridge_task_for_message(&context.agent_id, None, message)
                    .await?
            } else {
                None
            }
        } else {
            None
        };
        let pending_chatgpt_bridge_task = if explicit_task.is_none()
            && bound_task.is_none()
            && delegated_task.is_none()
            && chatgpt_bridge_task.is_none()
            && mapped_scope_task.is_none()
        {
            let bound = if first_user_message.is_none() {
                self.unique_recent_chatgpt_bridge_task(&context.agent_id)
                    .await?
            } else {
                None
            };
            match bound {
                Some(task_id) => Some(task_id),
                None => {
                    self.claim_unbound_chatgpt_bridge_task(
                        &context.agent_id,
                        conversation_scope.as_deref(),
                        first_user_message,
                    )
                    .await?
                }
            }
        } else {
            None
        };
        let unresolved_openai_tool_call = first_user_message.is_none()
            && explicit_task.is_none()
            && bound_task.is_none()
            && delegated_task.is_none()
            && chatgpt_bridge_task.is_none()
            && mapped_scope_task.is_none()
            && pending_chatgpt_bridge_task.is_none()
            && conversation_scope
                .as_deref()
                .is_some_and(|scope| scope.starts_with("openai:"));
        if unresolved_openai_tool_call {
            return Err(RuntimeError::new(
                "conversation_identity_unbound",
                "this ChatGPT tool call is not bound to an existing local conversation. Call agent_user_message first and reuse its taskId/turnId instead of creating a new conversation",
            ));
        }
        let task = explicit_task
            .or(bound_task)
            .or(delegated_task)
            .or(mapped_scope_task)
            .or(chatgpt_bridge_task)
            .or(pending_chatgpt_bridge_task)
            .unwrap_or_else(|| {
                if let (Some(scope), Some(message)) =
                    (conversation_scope.as_deref(), first_user_message)
                {
                    task_identity_from_first_message(&context.agent_id, scope, message)
                } else {
                    select_task_identity(
                        &context.agent_id,
                        conversation_scope.as_deref(),
                        None,
                        None,
                        &context.request_id,
                    )
                }
            });
        let turn = context.turn_id.clone().unwrap_or_else(|| {
            safe_id(
                "turn",
                &context.agent_id,
                &format!("{task}\0{}", context.request_id),
            )
        });
        context.task_id = Some(task.clone());
        context.turn_id = Some(turn);

        let task_id = TaskId::new(&task).map_err(|error| invalid("taskId", error))?;
        let current = self
            .repository
            .task(&task_id)
            .await
            .map_err(storage_error)?;
        let reopening_stopped = current
            .as_ref()
            .is_some_and(|task| task.status == TaskStatus::Stopped);
        if reopening_stopped && first_user_message.is_none() {
            return Err(RuntimeError::new(
                "conversation_stopped",
                "this conversation was stopped by the user; only a new user message can reopen it",
            ));
        }
        let generation = current.as_ref().map_or(1, |task| {
            if reopening_stopped {
                task.generation.saturating_add(1)
            } else {
                task.generation
            }
        });
        let logical_session_scope = if generation > 1 {
            format!("{task}\0generation:{generation}")
        } else {
            task
        };
        let logical_session = safe_id("mcp-session", &context.agent_id, &logical_session_scope);
        context.mcp_session_id = Some(logical_session.clone());
        let session_id =
            SessionId::new(logical_session).map_err(|error| invalid("sessionId", error))?;
        if current
            .as_ref()
            .is_some_and(|task| task.allow_execute == Some(false))
        {
            return Err(conversation_approval_denied());
        }
        let now = now_ms();
        let allow_execute = if let Some(task) = current.as_ref() {
            task.allow_execute
        } else if self.approve_new_conversations_enabled().await? {
            None
        } else {
            Some(true)
        };
        self.repository
            .upsert_task(&Task {
                id: task_id.clone(),
                agent_id: AgentId::new(&context.agent_id).ok(),
                device_id: self.device.id.clone(),
                conversation_scope_hash: current
                    .as_ref()
                    .and_then(|task| task.conversation_scope_hash.clone())
                    .or(conversation_scope),
                title: current.as_ref().and_then(|task| task.title.clone()),
                source: current
                    .as_ref()
                    .and_then(|task| task.source.clone())
                    .or_else(|| Some("mcp".to_owned())),
                project_folder: current
                    .as_ref()
                    .and_then(|task| task.project_folder.clone()),
                allow_execute,
                status: if allow_execute.is_none() {
                    TaskStatus::Pending
                } else {
                    TaskStatus::Running
                },
                active_session_id: allow_execute.map(|_| session_id.clone()),
                generation,
                stopped_at_ms: None,
                created_at_ms: current.as_ref().map_or(now, |task| task.created_at_ms),
                updated_at_ms: now,
            })
            .await
            .map_err(storage_error)?;
        if allow_execute.is_none() {
            self.publish_event(
                format!("conversation-approval-pending-{}", task_id.as_str()),
                "conversation.approval_pending",
                Some(task_id.as_str().to_owned()),
                None,
                context.turn_id.clone(),
                serde_json::json!({
                    "allowExecute": serde_json::Value::Null,
                    "approvalDeadlineUtc": time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(now.saturating_add(60_000)) * 1_000_000)
                        .ok()
                        .and_then(|value| value.format(&time::format_description::well_known::Rfc3339).ok())
                }),
            );
            self.wait_for_conversation_approval(&task_id).await?;
            let Some(mut approved_task) = self
                .repository
                .task(&task_id)
                .await
                .map_err(storage_error)?
            else {
                return Err(RuntimeError::new(
                    "not_found",
                    "task missing after approval",
                ));
            };
            approved_task.status = TaskStatus::Running;
            approved_task.active_session_id = Some(session_id.clone());
            approved_task.updated_at_ms = now_ms();
            self.repository
                .upsert_task(&approved_task)
                .await
                .map_err(storage_error)?;
        }
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

    async fn approve_new_conversations_enabled(&self) -> RuntimeResult<bool> {
        let setting = self
            .repository
            .setting("ui_approveNewConversations")
            .await
            .map_err(storage_error)?;
        Ok(setting
            .and_then(|value| serde_json::from_str::<bool>(&value.value_json).ok())
            .unwrap_or(true))
    }

    async fn wait_for_conversation_approval(&self, task_id: &TaskId) -> RuntimeResult<()> {
        const APPROVAL_WINDOW_MS: i64 = 60_000;
        loop {
            let Some(task) = self.repository.task(task_id).await.map_err(storage_error)? else {
                return Err(RuntimeError::new(
                    "not_found",
                    "task missing while waiting for approval",
                ));
            };
            match task.allow_execute {
                Some(true) => return Ok(()),
                Some(false) => return Err(conversation_approval_denied()),
                None if now_ms().saturating_sub(task.created_at_ms) >= APPROVAL_WINDOW_MS => {
                    sqlx::query("UPDATE tasks SET allow_execute=0,status='failed',updated_at_ms=? WHERE id=? AND allow_execute IS NULL")
                        .bind(now_ms())
                        .bind(task_id.as_str())
                        .execute(self.repository.pool())
                        .await
                        .map_err(|_| RuntimeError::new("storage_error", "conversation approval timeout could not be persisted"))?;
                    return Err(conversation_approval_denied());
                }
                None => tokio::time::sleep(std::time::Duration::from_millis(250)).await,
            }
        }
    }
}

fn conversation_approval_denied() -> RuntimeError {
    RuntimeError::new(
        "conversation_approval_denied",
        "Anti-hack verification mode is enabled. This conversation was not approved or the 60-second approval window expired, so it cannot execute.",
    )
}

fn select_task_identity(
    agent_id: &str,
    conversation_scope: Option<&str>,
    explicit_task_id: Option<&str>,
    bound_task_id: Option<&str>,
    request_id: &str,
) -> String {
    if let Some(task_id) = explicit_task_id.filter(|value| !value.trim().is_empty()) {
        return task_id.trim().to_owned();
    }
    if let Some(task_id) = bound_task_id.filter(|value| !value.trim().is_empty()) {
        return task_id.trim().to_owned();
    }
    if let Some(scope) = conversation_scope.filter(|value| !value.trim().is_empty()) {
        return safe_id("task-chat", agent_id, scope);
    }
    safe_id("task", agent_id, request_id)
}

fn task_identity_from_first_message(
    agent_id: &str,
    conversation_scope: &str,
    _message: &str,
) -> String {
    safe_id("task-chat", agent_id, conversation_scope)
}

fn safe_id(prefix: &str, agent_id: &str, scope: &str) -> String {
    let material = format!("{prefix}\0agent:{agent_id}\0scope:{scope}");
    format!(
        "{prefix}-{}",
        Uuid::new_v5(&Uuid::NAMESPACE_OID, material.as_bytes())
    )
}

#[cfg(test)]
#[path = "identity_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "identity_regression_tests.rs"]
mod regression_tests;
