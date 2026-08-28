use std::time::{Duration, Instant};

use chatcmd_core::{Approval, ApprovalState, SettingsStore as _, TaskId, TaskStore as _};
use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use serde_json::{Value, json};
use sqlx::Row as _;

use super::{RuntimeHost, invalid, now_ms, storage_error};

const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);
const APPROVAL_POLL_INTERVAL: Duration = Duration::from_millis(150);

impl RuntimeHost {
    pub(super) async fn authorize_execution(
        &self,
        context: &OperationContext,
        tool: &str,
        arguments: &Value,
    ) -> RuntimeResult<()> {
        if !requires_execution_approval(tool) {
            return Ok(());
        }
        let task_id = TaskId::new(context.task_id.as_deref().unwrap_or_default())
            .map_err(|error| invalid("taskId", error))?;
        let mode_task_id = self.execution_mode_task_id(&task_id).await?;
        match self
            .repository
            .execution_mode(Some(&mode_task_id))
            .await
            .map_err(storage_error)?
        {
            chatcmd_core::ExecutionMode::Allow => return Ok(()),
            chatcmd_core::ExecutionMode::Deny => {
                return Err(RuntimeError::new(
                    "policy_denied",
                    "conversation access mode denied this operation",
                ));
            }
            chatcmd_core::ExecutionMode::Approval => {}
        }

        let similarity_key = similarity_key(tool, arguments);
        if self
            .similar_operation_allowed(mode_task_id.as_str(), &similarity_key)
            .await?
        {
            return Ok(());
        }

        let approval_id = context.request_id.clone();
        let turn_id = context.turn_id.as_deref().unwrap_or_default();
        let request_json = json!({
            "activityId": approval_id,
            "tool": tool,
            "turnId": turn_id,
            "input": arguments,
            "similarityKey": similarity_key,
        })
        .to_string();
        self.repository
            .save_approval(&Approval {
                id: approval_id.clone(),
                task_id: task_id.clone(),
                // MCP logical sessions are not terminal_sessions rows. The task/turn/activity
                // identifiers are sufficient to resolve this task-scoped approval.
                session_id: None,
                state: ApprovalState::Pending,
                request_json,
                decision_json: None,
                created_at_ms: now_ms(),
                resolved_at_ms: None,
            })
            .await
            .map_err(storage_error)?;
        self.append_call_event(
            context,
            tool,
            "pending_approval",
            Some(arguments),
            None,
            None,
        )
        .await?;
        self.publish_parent_subagent_approval(
            &task_id,
            &approval_id,
            turn_id,
            tool,
            arguments,
            "subagent.approval_pending",
        )
        .await?;
        self.wait_for_approval(context, &task_id, &approval_id)
            .await
    }

    async fn publish_parent_subagent_approval(
        &self,
        child_task_id: &TaskId,
        approval_id: &str,
        child_turn_id: &str,
        tool: &str,
        arguments: &Value,
        event_type: &str,
    ) -> RuntimeResult<()> {
        let row = sqlx::query(
            "SELECT id,parent_task_id,parent_turn_id,name FROM subagent_runs WHERE child_task_id=? LIMIT 1",
        )
        .bind(child_task_id.as_str())
        .fetch_optional(self.repository.pool())
        .await
        .map_err(|_| RuntimeError::new("storage_error", "sub-agent approval routing lookup failed"))?;
        let Some(row) = row else {
            return Ok(());
        };
        let subagent_id = row.get::<String, _>("id");
        let parent_task_id = row.get::<String, _>("parent_task_id");
        let parent_turn_id = row.get::<String, _>("parent_turn_id");
        let agent_name = row.get::<String, _>("name");
        self.publish_event(
            format!("subagent-approval:{approval_id}:{}", now_ms()),
            event_type,
            Some(parent_task_id),
            Some(child_task_id.as_str().to_owned()),
            Some(parent_turn_id),
            json!({
                "subagentId": subagent_id,
                "childTaskId": child_task_id.as_str(),
                "activityId": approval_id,
                "childTurnId": child_turn_id,
                "agentName": agent_name,
                "tool": tool,
                "input": arguments,
            }),
        );
        Ok(())
    }

    async fn execution_mode_task_id(&self, task_id: &TaskId) -> RuntimeResult<TaskId> {
        let parent_task_id = sqlx::query_scalar::<_, String>(
            "SELECT parent_task_id FROM subagent_runs WHERE child_task_id=? LIMIT 1",
        )
        .bind(task_id.as_str())
        .fetch_optional(self.repository.pool())
        .await
        .map_err(|_| {
            RuntimeError::new("storage_error", "sub-agent approval scope lookup failed")
        })?;
        match parent_task_id {
            Some(parent_task_id) => {
                TaskId::new(parent_task_id).map_err(|error| invalid("taskId", error))
            }
            None => Ok(task_id.clone()),
        }
    }

    async fn similar_operation_allowed(
        &self,
        task_id: &str,
        similarity_key: &str,
    ) -> RuntimeResult<bool> {
        sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM approvals a LEFT JOIN subagent_runs r ON r.child_task_id=a.task_id WHERE (a.task_id=? OR r.parent_task_id=?) AND a.state='approved' AND json_extract(a.request_json,'$.similarityKey')=? AND json_extract(a.decision_json,'$.decision')='allowSimilar' LIMIT 1)",
        )
        .bind(task_id)
        .bind(task_id)
        .bind(similarity_key)
        .fetch_one(self.repository.pool())
        .await
        .map(|value| value == 1)
        .map_err(|_| RuntimeError::new("storage_error", "approval rule lookup failed"))
    }

    async fn wait_for_approval(
        &self,
        context: &OperationContext,
        task_id: &TaskId,
        approval_id: &str,
    ) -> RuntimeResult<()> {
        let deadline = Instant::now() + APPROVAL_TIMEOUT;
        loop {
            let row = sqlx::query(
                "SELECT state,decision_json FROM approvals WHERE id=? AND task_id=? LIMIT 1",
            )
            .bind(approval_id)
            .bind(task_id.as_str())
            .fetch_optional(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "approval state lookup failed"))?
            .ok_or_else(|| RuntimeError::new("approval_missing", "approval request disappeared"))?;
            let state = row.get::<String, _>("state");
            let decision_json = row.get::<Option<String>, _>("decision_json");
            match state.as_str() {
                "approved" => return Ok(()),
                "rejected" => {
                    let reason = rejection_reason(decision_json.as_deref());
                    return Err(RuntimeError::new(
                        "command_rejected_by_user",
                        reason.map_or_else(
                            || "the user rejected this operation".to_owned(),
                            |value| format!("the user rejected this operation: {value}"),
                        ),
                    ));
                }
                "cancelled" => {
                    return Err(RuntimeError::new(
                        "approval_cancelled",
                        "approval was cancelled",
                    ));
                }
                "expired" => {
                    return Err(RuntimeError::new(
                        "approval_timeout",
                        "command approval timed out before the user responded",
                    ));
                }
                "pending" => {}
                _ => {
                    return Err(RuntimeError::new(
                        "approval_state_invalid",
                        "approval has an invalid state",
                    ));
                }
            }
            if Instant::now() >= deadline {
                self.expire_approval(approval_id).await?;
                return Err(RuntimeError::new(
                    "approval_timeout",
                    "command approval timed out before the user responded",
                ));
            }
            tokio::select! {
                () = context.cancellation.cancelled() => {
                    self.cancel_approval(approval_id).await?;
                    return Err(RuntimeError::new("cancelled", "operation was cancelled while waiting for approval"));
                }
                () = tokio::time::sleep(APPROVAL_POLL_INTERVAL) => {}
            }
        }
    }

    async fn expire_approval(&self, approval_id: &str) -> RuntimeResult<()> {
        self.resolve_waiting_approval(approval_id, "expired").await
    }

    async fn cancel_approval(&self, approval_id: &str) -> RuntimeResult<()> {
        self.resolve_waiting_approval(approval_id, "cancelled")
            .await
    }

    async fn resolve_waiting_approval(&self, approval_id: &str, state: &str) -> RuntimeResult<()> {
        sqlx::query("UPDATE approvals SET state=?,resolved_at_ms=? WHERE id=? AND state='pending'")
            .bind(state)
            .bind(now_ms())
            .bind(approval_id)
            .execute(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "approval state update failed"))?;
        Ok(())
    }
}

fn requires_execution_approval(tool: &str) -> bool {
    tool == "shell_write"
        || tool.starts_with("fs_")
        || tool.starts_with("git_")
        || tool == "process_kill"
}

fn similarity_key(tool: &str, arguments: &Value) -> String {
    format!("{tool}\n{}", arguments)
}

fn rejection_reason(decision_json: Option<&str>) -> Option<String> {
    serde_json::from_str::<Value>(decision_json?)
        .ok()?
        .get("reason")?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_execution_and_workspace_tools_require_approval() {
        assert!(requires_execution_approval("shell_write"));
        assert!(requires_execution_approval("fs_read_text"));
        assert!(requires_execution_approval("git_status"));
        assert!(requires_execution_approval("process_kill"));
        assert!(!requires_execution_approval("agent_progress"));
        assert!(!requires_execution_approval("task_get"));
    }
}
