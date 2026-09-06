//! Compact's task identity fence precedes normal MCP identity creation/adoption.
use super::{RuntimeHost, storage_error};
use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use serde_json::Value;

impl RuntimeHost {
    pub(super) async fn call_compact_checked(
        &self,
        tool: &str,
        context: OperationContext,
        arguments: Value,
    ) -> RuntimeResult<Value> {
        let _call = self.activities.track_compact_call(&context)?;
        let scope = context.conversation_scope_id.as_deref();
        let task = context.task_id.as_deref();
        // An authenticated conversation scope wins over a transport session that
        // the provider may reuse for another chat. Session is only a fallback.
        let session = scope
            .is_none()
            .then_some(context.mcp_session_id.as_deref())
            .flatten();
        let blocked: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM chatgpt_compact_archives WHERE scope_hash=? OR previous_scope_hash=? OR old_session_id=?) OR EXISTS(SELECT 1 FROM chatgpt_compact_jobs WHERE phase NOT IN ('completed','cancelled') AND (task_id=? OR old_scope_hash=? OR new_scope_hash=?))"
        ).bind(scope).bind(scope).bind(session).bind(task).bind(scope).bind(scope)
            .fetch_one(self.repository.pool()).await
            .map_err(|error| storage_error(chatcmd_core::StorageError::Backend(error.to_string())))?;
        let bootstrap = tool == "agent_user_message"
            && arguments
                .get("content")
                .and_then(Value::as_str)
                .is_some_and(|text| text.trim_start().starts_with("[[CHATCMD-RESUME:"));
        if blocked || bootstrap {
            return Err(RuntimeError::new(
                "conversation_compacting_or_archived",
                "This conversation is being compacted or is archived. No local tool was run. Write the handoff without tools; resume only in the confirmed replacement ChatGPT conversation.",
            ));
        }
        self.call_persisted(tool, context, arguments).await
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime_host::ActivityRegistry;
    use chatcmd_runtime::OperationContext;

    #[test]
    fn compact_barrier_tracks_lifecycle_calls_and_duplicate_request_ids_until_both_finish() {
        let activities = ActivityRegistry::default();
        let mut context = OperationContext::new("same-request", "agent", "agent_user_message");
        context.conversation_scope_id = Some("openai:old".to_owned());
        let first = activities.track_compact_call(&context).unwrap();
        let second = activities.track_compact_call(&context).unwrap();
        assert!(!activities.compact_settled("task", Some("openai:old")));
        assert!(activities.compact_settled("other-task", Some("openai:other")));
        drop(first);
        assert!(!activities.compact_settled("task", Some("openai:old")));
        drop(second);
        assert!(activities.compact_settled("task", Some("openai:old")));
    }
}
