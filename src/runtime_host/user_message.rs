use chatcmd_core::{
    ActorKind, EventId, EventKind, SessionId, TaskId, TerminalEventStore as _, TimelineEvent,
    TurnId,
};
use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{RuntimeHost, invalid, now_ms, storage_error};

impl RuntimeHost {
    pub(super) async fn ensure_user_message_synced(
        &self,
        context: &OperationContext,
    ) -> RuntimeResult<()> {
        let task_id = required_task_id(context)?;
        let turn_id = required_turn_id(context)?;
        let found = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM timeline_events WHERE task_id=? AND turn_id=? AND actor='user' AND kind='message' LIMIT 1)",
        )
        .bind(task_id.as_str())
        .bind(turn_id.as_str())
        .fetch_one(self.repository.pool())
        .await
        .map_err(|_| RuntimeError::new("storage_error", "user message sync state unavailable"))?;
        if found == 1 {
            Ok(())
        } else {
            Err(RuntimeError::new(
                "user_message_sync_required",
                "call agent_user_message first with the exact current user message and the same turnId before using any other ChatCMD tool",
            ))
        }
    }

    pub(super) async fn save_user_message(
        &self,
        context: &OperationContext,
        content: &str,
    ) -> RuntimeResult<Value> {
        if content.trim().is_empty() {
            return Err(RuntimeError::new(
                "user_message_required",
                "agent_user_message content must contain the exact current user message",
            ));
        }
        let task_id = required_task_id(context)?;
        let turn_id = required_turn_id(context)?;
        let session_id = required_session_id(context)?;
        let key = safe_id(
            "user-message",
            &context.agent_id,
            &format!("{}\0{}", task_id.as_str(), turn_id.as_str()),
        );
        let payload = json!({
            "tool": context.tool_name,
            "role": "user",
            "content": content
        });
        let inserted = self
            .repository
            .append_timeline_events(&[TimelineEvent {
                id: EventId::new(key.clone()).map_err(|error| invalid("eventId", error))?,
                task_id: task_id.clone(),
                turn_id: Some(turn_id.clone()),
                session_id: Some(session_id.clone()),
                actor: ActorKind::User,
                kind: EventKind::Message,
                idempotency_key: key.clone(),
                payload_json: payload.to_string(),
                metadata_json: None,
                created_at_ms: now_ms(),
            }])
            .await
            .map_err(storage_error)?;
        if inserted == 0 {
            let existing = sqlx::query_scalar::<_, String>(
                "SELECT payload_json FROM timeline_events WHERE event_id=? LIMIT 1",
            )
            .bind(&key)
            .fetch_optional(self.repository.pool())
            .await
            .map_err(|_| RuntimeError::new("storage_error", "user message payload unavailable"))?;
            if existing
                .as_deref()
                .is_none_or(|value| !same_user_message(value, content))
            {
                return Err(RuntimeError::new(
                    "turn_user_message_conflict",
                    "the current turnId is already bound to a different user message; use a new turnId for each user message",
                ));
            }
        }
        if inserted > 0 {
            self.publish_event(
                key,
                EventKind::Message.as_str(),
                Some(task_id.as_str().to_owned()),
                Some(session_id.as_str().to_owned()),
                Some(turn_id.as_str().to_owned()),
                payload,
            );
        }
        Ok(json!({
            "accepted": true,
            "duplicate": inserted == 0,
            "userMessageSynced": true,
            "taskId": task_id.as_str(),
            "turnId": turn_id.as_str()
        }))
    }
}

fn required_task_id(context: &OperationContext) -> RuntimeResult<TaskId> {
    TaskId::new(context.task_id.as_deref().unwrap_or_default())
        .map_err(|error| invalid("taskId", error))
}

fn required_turn_id(context: &OperationContext) -> RuntimeResult<TurnId> {
    TurnId::new(context.turn_id.as_deref().unwrap_or_default())
        .map_err(|error| invalid("turnId", error))
}

fn required_session_id(context: &OperationContext) -> RuntimeResult<SessionId> {
    SessionId::new(context.mcp_session_id.as_deref().unwrap_or_default())
        .map_err(|error| invalid("sessionId", error))
}

fn safe_id(prefix: &str, agent_id: &str, scope: &str) -> String {
    let material = format!("{prefix}\0agent:{agent_id}\0scope:{scope}");
    format!(
        "{prefix}-{}",
        Uuid::new_v5(&Uuid::NAMESPACE_OID, material.as_bytes())
    )
}

fn same_user_message(payload_json: &str, content: &str) -> bool {
    serde_json::from_str::<Value>(payload_json)
        .ok()
        .and_then(|payload| {
            payload
                .get("content")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .is_some_and(|existing| existing == content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_user_message_must_match_exact_content() {
        let payload = json!({"role":"user","content":"xin chào"}).to_string();
        assert!(same_user_message(&payload, "xin chào"));
        assert!(!same_user_message(&payload, "xin chào!"));
    }

    #[test]
    fn user_message_key_is_stable_per_turn_and_changes_between_turns() {
        let first = safe_id("user-message", "agent", "task-a\0turn-a");
        assert_eq!(first, safe_id("user-message", "agent", "task-a\0turn-a"));
        assert_ne!(first, safe_id("user-message", "agent", "task-a\0turn-b"));
    }
}
