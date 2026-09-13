use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use serde_json::{Value, json};
use sqlx::Row as _;

use super::{RuntimeHost, now_ms};

pub(super) const MAX_EXTENSION_FALLBACK_ATTEMPTS: i64 = 3;

/// Browser children own their MCP lifecycle; sampling children use a different protocol.
/// Shared by first dispatch and API retries so neither drops the finalization contract.
pub(crate) fn browser_subagent_prompt(
    agent_name: Option<&str>,
    request: &str,
    subagent_id: &str,
    child_task_id: &str,
) -> String {
    let marker = format!("CMDGPT_SUBAGENT_ID={subagent_id}");
    let request = request
        .trim()
        .strip_suffix(&marker)
        .unwrap_or(request.trim())
        .trim_end();
    let delegated = format!(
        "{request}\n\n{marker}\n\n\
         BROWSER CHILD MCP LIFECYCLE (not the runtime-owned sampling protocol):\n\
         Work only on the delegated objective above; this protocol grants no additional permissions.\n\
         Call agent_user_message first with this exact full message, taskId={child_task_id}, turnId=turn-{subagent_id}. Reuse the returned taskId and the same turnId for every tool in this delegated turn.\n\
         INITIALIZATION CHECK: continue with MCP tools only after agent_user_message returns accepted=true AND userMessageSynced=true.\n\
         If initialization is rejected, blocked, or unavailable, do not call agent_turn_complete or any file/command tool: the turn is not synchronized. Do not switch connectors, disguise the request, or bypass a host safety or permission denial. Return a plain blocked report with files inspected/changed, the exact observable error code/message if available, and MCP finish not received. If the cause is not exposed, report it as unknown rather than attributing it to OpenAI.\n\
         AFTER successful initialization and BEFORE posting your final answer, you MUST call agent_turn_complete with the exact final report in content, including files inspected/changed, actual evidence, blockers and honest workOutcome. This applies to read-only, partial, failed or blocked work only after initialization succeeded.\n\
         Wait for all tools and descendants before finishing. If completion is rejected as active_tools_running or subagents_still_running, wait for that work and retry completion in this same turn. Do not repeat completed work or open another conversation.\n\
         If the finalizer schema is not visible, discover agent_turn_complete on the same connector. Only accepted=true from that MCP call acknowledges finalization; plain text such as done/finished is NOT an MCP finish. After acceptance, post the same report and make no further tools calls.\n\
         If the MCP transport remains unavailable, report that limitation truthfully; never claim that MCP finish was received."
    );
    match agent_name.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => format!("Sử dụng plugin @{name} để thực hiện yêu cầu sau:\n\n{delegated}"),
        None => delegated,
    }
}

#[cfg(test)]
mod prompt_tests {
    use super::browser_subagent_prompt;

    #[test]
    fn browser_subagent_prompt_preserves_single_marker_and_requires_acknowledged_finish() {
        let initial = browser_subagent_prompt(
            Some("reader"),
            "Inspect files\n\nCMDGPT_SUBAGENT_ID=child-1",
            "child-1",
            "task-child",
        );
        let retry =
            browser_subagent_prompt(Some("reader"), "Inspect files", "child-1", "task-child");
        assert_eq!(initial, retry);
        assert_eq!(initial.matches("CMDGPT_SUBAGENT_ID=").count(), 1);
        assert!(initial.contains("taskId=task-child, turnId=turn-child-1"));
        assert!(initial.contains("MUST call agent_turn_complete"));
        assert!(initial.contains("accepted=true"));
        assert!(initial.contains("read-only, partial, failed or blocked"));
        assert!(initial.contains("accepted=true AND userMessageSynced=true"));
        assert!(initial.contains("do not call agent_turn_complete or any file/command tool"));
        assert!(initial.contains("AFTER successful initialization"));
        assert!(
            initial.contains("do not switch connectors")
                || initial.contains("Do not switch connectors")
        );
        assert!(initial.contains("report it as unknown rather than attributing it to OpenAI"));
    }
}

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

        if self.subagent_concurrency_limit().await? == 0 {
            return Err(RuntimeError::new(
                "subagents_disabled",
                "Browser child dispatch is disabled by the user.",
            ));
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
        let project_folder = sqlx::query_scalar::<_, String>(
            "SELECT project_folder FROM tasks WHERE id=? AND project_folder IS NOT NULL LIMIT 1",
        )
        .bind(&parent_task_id)
        .fetch_optional(self.repository.pool())
        .await
        .ok()
        .flatten();
        let agent_name =
            sqlx::query_scalar::<_, String>("SELECT name FROM mcp_agents WHERE id=? LIMIT 1")
                .bind(&parent_context.agent_id)
                .fetch_optional(self.repository.pool())
                .await
                .ok()
                .flatten();
        let submitted_content = browser_subagent_prompt(
            agent_name.as_deref(),
            delegated_prompt,
            subagent_id,
            child_task_id,
        );
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
                "projectFolder": project_folder,
                "submittedContent": submitted_content,
                "attempt": attempt,
                "maxAttempts": MAX_EXTENSION_FALLBACK_ATTEMPTS,
                "parentRequestId": parent_context.request_id
            }),
        );
        Ok(json!({ "attempt": attempt, "maxAttempts": MAX_EXTENSION_FALLBACK_ATTEMPTS }))
    }
}
