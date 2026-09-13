use chatcmd_core::{
    AgentId, SessionId, SettingsStore as _, Task, TaskId, TaskSession, TaskStatus, TaskStore as _,
    TerminalSessionStatus, TurnBinding, TurnId,
};
use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use uuid::Uuid;

use super::{RuntimeHost, invalid, now_ms, storage_error};

#[path = "identity_bridge.rs"]
mod bridge;
#[path = "identity_routing.rs"]
mod routing;
#[path = "identity_scope.rs"]
mod scope;
#[cfg(test)]
use bridge::unique_bridge_task_for_message;

impl RuntimeHost {
    pub(super) async fn ensure_call_identity(
        &self,
        context: &mut OperationContext,
        first_user_message: Option<&str>,
    ) -> RuntimeResult<()> {
        let conversation_scope = context.conversation_scope_id.clone();
        let explicit = context
            .task_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let bound = if explicit.is_none() {
            self.bound_task_for_turn(context).await?
        } else {
            None
        };
        let delegated = self
            .delegated_subagent_task_id(context, first_user_message)
            .await?;
        let alias = self.mapped_mcp_scope_task(context).await?;
        if alias.as_deref().is_some_and(|owner| {
            explicit
                .as_deref()
                .or(bound.as_deref())
                .or(delegated.as_deref())
                .is_some_and(|expected| expected != owner)
        }) {
            return Err(RuntimeError::new(
                "conversation_identity_conflict",
                "the supplied task conflicts with the durable MCP conversation binding",
            ));
        }
        let mapped =
            if explicit.is_none() && bound.is_none() && delegated.is_none() && alias.is_none() {
                if let Some(scope) = conversation_scope.as_deref() {
                    self.bound_task_for_conversation_scope(&context.agent_id, scope)
                        .await?
                } else {
                    None
                }
            } else {
                None
            };
        let known = explicit
            .clone()
            .or(bound)
            .or(delegated)
            .or(alias)
            .or(mapped);
        let routed = if first_user_message.is_some() || known.is_none() {
            self.bridge_task_from_route(context, first_user_message, known.as_deref())
                .await?
        } else {
            None
        };
        let mut selected = known.or(routed);
        if selected.is_none() {
            selected = if let Some(message) = first_user_message {
                self.chatgpt_bridge_task_for_message(&context.agent_id, None, message)
                    .await?
            } else {
                self.unique_recent_chatgpt_bridge_task(&context.agent_id)
                    .await?
            };
        }
        if selected.is_none() {
            selected = self
                .claim_unbound_chatgpt_bridge_task(
                    &context.agent_id,
                    conversation_scope.as_deref(),
                    first_user_message,
                )
                .await?;
        }
        if selected.is_none() {
            if let Some(message) = first_user_message {
                self.guard_rewritten_chatui_message(context, message)
                    .await?;
            } else if conversation_scope
                .as_deref()
                .is_some_and(|scope| scope.starts_with("openai:"))
            {
                return Err(RuntimeError::new(
                    "conversation_identity_unbound",
                    "this ChatGPT tool call is not bound to an existing local conversation; call agent_user_message with the ChatUI routing turnId and reuse its taskId",
                ));
            }
        }
        let task = selected.unwrap_or_else(|| {
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
        if explicit.is_some()
            && conversation_scope
                .as_deref()
                .is_some_and(|scope| scope.starts_with("openai:"))
        {
            let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tasks WHERE id=?)")
                .bind(&task)
                .fetch_one(self.repository.pool())
                .await
                .map_err(|_| {
                    RuntimeError::new("storage_error", "explicit conversation lookup failed")
                })?;
            if !exists {
                return Err(RuntimeError::new(
                    "conversation_identity_unbound",
                    "the supplied taskId does not exist; use the ChatUI routing turnId instead of inventing a new conversation",
                ));
            }
        }
        self.validate_resolved_task(context, &task).await?;
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
            .map_err(storage_error)?;
        // A provisional tool-only match must not become a durable conversation alias.
        if first_user_message.is_some() {
            self.persist_mcp_scope_binding(context).await?;
        }
        Ok(())
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

#[cfg(test)]
#[path = "identity_routing_tests.rs"]
mod routing_tests;

#[cfg(test)]
#[path = "identity_delegation_regression_tests.rs"]
mod delegation_regression_tests;

#[cfg(test)]
#[path = "identity_routing_guard_tests.rs"]
mod routing_guard_tests;
