use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{RuntimeHost, now_ms};

impl RuntimeHost {
    pub(super) async fn attach_immediate_messages(
        &self,
        context: &OperationContext,
        tool: &str,
        value: Value,
    ) -> RuntimeResult<Value> {
        if tool == "agent_turn_complete" {
            return Ok(value);
        }
        let Some(task_id) = context.task_id.as_deref().filter(|value| !value.is_empty()) else {
            return Ok(value);
        };
        let messages = crate::chatgpt_queue::claim_immediate(&self.repository, task_id)
            .await
            .map_err(queue_storage_error)?;
        if messages.is_empty() {
            return Ok(value);
        }

        let message_ids = messages
            .iter()
            .map(|message| message.id.clone())
            .collect::<Vec<_>>();
        let immediate_messages = messages
            .iter()
            .map(|message| json!({ "id": message.id, "content": message.content }))
            .collect::<Vec<_>>();
        self.publish_event(
            format!("chatgpt-queue-consumed-{}", Uuid::new_v4()),
            "chatgpt.queue.consumed",
            Some(task_id.to_owned()),
            context.mcp_session_id.clone(),
            context.turn_id.clone(),
            json!({ "messageIds": message_ids }),
        );

        let mut object = match value {
            Value::Object(object) => object,
            other => {
                let mut object = serde_json::Map::new();
                object.insert("result".to_owned(), other);
                object
            }
        };
        object.insert(
            "immediateMessages".to_owned(),
            Value::Array(immediate_messages),
        );
        object.insert(
            "immediateMessagesRequirePriority".to_owned(),
            Value::Bool(true),
        );
        object.insert(
            "immediateMessageInstruction".to_owned(),
            Value::String(
                "URGENT USER UPDATE: These messages were sent while this exact task was already running. Before continuing the previous work, immediately call agent_progress once to acknowledge the new request in Vietnamese, for example: Đã nhận được yêu cầu mới là: \"...\" và sẽ xử lý nó trước. Then handle every immediateMessages item in the listed priority order before resuming prior work. Do not defer or ignore them, and do not apply them to another task."
                    .to_owned(),
            ),
        );
        Ok(Value::Object(object))
    }

    pub(super) async fn demote_immediate_messages(
        &self,
        context: &OperationContext,
    ) -> RuntimeResult<()> {
        let Some(task_id) = context.task_id.as_deref().filter(|value| !value.is_empty()) else {
            return Ok(());
        };
        let demoted =
            crate::chatgpt_queue::demote_all_immediate(&self.repository, task_id, now_ms())
                .await
                .map_err(queue_storage_error)?;
        if demoted > 0 {
            self.publish_event(
                format!("chatgpt-queue-demoted-{}", Uuid::new_v4()),
                "chatgpt.queue.changed",
                Some(task_id.to_owned()),
                context.mcp_session_id.clone(),
                context.turn_id.clone(),
                json!({ "action": "demoted_after_final" }),
            );
        }
        Ok(())
    }
}

fn queue_storage_error(_: sqlx::Error) -> RuntimeError {
    RuntimeError::new(
        "storage_error",
        "queued ChatGPT message storage is unavailable",
    )
}
