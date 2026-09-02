use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use serde_json::{Value, json};
use sqlx::Row as _;

use super::{RuntimeHost, now_ms};

pub(super) const MAX_EXTENSION_FALLBACK_ATTEMPTS: i64 = 3;

impl RuntimeHost {
    pub(super) async fn request_subagent_extension_fallback(
        &self,
        parent_context: &OperationContext,
        registration: &Value,
        delegated_prompt: &str,
    ) -> RuntimeResult<Value> {
        let subagent_id = registration
            .get("subagentId")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                RuntimeError::new("subagent_registration_invalid", "missing subagentId")
            })?;
        let child_task_id = registration
            .get("childTaskId")
            .or_else(|| registration.get("taskId"))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                RuntimeError::new("subagent_registration_invalid", "missing childTaskId")
            })?;

        let row = sqlx::query(
            "SELECT parent_task_id,parent_turn_id,name,status,fallback_state,fallback_attempts FROM subagent_runs WHERE id=? AND child_task_id=? LIMIT 1",
        )
        .bind(subagent_id)
        .bind(child_task_id)
        .fetch_optional(self.repository.pool())
        .await
        .map_err(|_| RuntimeError::new("storage_error", "sub-agent fallback state lookup failed"))?
        .ok_or_else(|| RuntimeError::new("not_found", "registered sub-agent was not found"))?;

        let status = row.get::<String, _>("status");
        let fallback_state = row.get::<String, _>("fallback_state");
        let current_attempt = row.get::<i64, _>("fallback_attempts");
        if status != "pending" {
            return Ok(json!({ "attempt": current_attempt, "status": status }));
        }

        let attempt =
            if matches!(fallback_state.as_str(), "requested" | "started") && current_attempt > 0 {
                current_attempt
            } else {
                current_attempt.saturating_add(1)
            };
        if attempt > MAX_EXTENSION_FALLBACK_ATTEMPTS {
            return Err(RuntimeError::new(
                "subagent_fallback_exhausted",
                "ChatGPT extension fallback exhausted its retry limit",
            ));
        }

        let now = now_ms();
        sqlx::query(
            "UPDATE subagent_runs SET fallback_state='requested',fallback_attempts=?,fallback_error=NULL,updated_at_ms=? WHERE id=? AND status='pending'",
        )
        .bind(attempt)
        .bind(now)
        .bind(subagent_id)
        .execute(self.repository.pool())
        .await
        .map_err(|_| RuntimeError::new("storage_error", "sub-agent fallback request could not be persisted"))?;

        let parent_task_id = row.get::<String, _>("parent_task_id");
        let parent_turn_id = row.get::<String, _>("parent_turn_id");
        let name = row.get::<String, _>("name");
        let agent_name = sqlx::query_scalar::<_, String>(
            "SELECT name FROM mcp_agents WHERE id=? LIMIT 1",
        )
        .bind(&parent_context.agent_id)
        .fetch_optional(self.repository.pool())
        .await
        .ok()
        .flatten();
        let submitted_content = match agent_name.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
            Some(agent_name) => format!(
                "Sử dụng plugin @{agent_name} để thực hiện yêu cầu sau:\n\n{delegated_prompt}"
            ),
            None => delegated_prompt.to_owned(),
        };
        self.publish_event(
            format!("subagent-fallback-requested-{subagent_id}-{attempt}"),
            "subagent.fallback_requested",
            Some(parent_task_id.clone()),
            None,
            Some(parent_turn_id.clone()),
            json!({
                "subagentId": subagent_id,
                "parentTaskId": parent_task_id,
                "parentTurnId": parent_turn_id,
                "childTaskId": child_task_id,
                "name": name,
                "submittedContent": submitted_content,
                "attempt": attempt,
                "maxAttempts": MAX_EXTENSION_FALLBACK_ATTEMPTS,
                "parentRequestId": parent_context.request_id
            }),
        );
        Ok(json!({ "attempt": attempt, "maxAttempts": MAX_EXTENSION_FALLBACK_ATTEMPTS }))
    }
}
