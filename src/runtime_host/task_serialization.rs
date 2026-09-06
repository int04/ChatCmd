use chatcmd_core::Task;
use serde_json::{Value, json};

pub(crate) fn task_json(task: Task) -> Value {
    json!({
        "id": task.id.as_str(),
        "agentId": task.agent_id.map(|id| id.into_string()),
        "deviceId": task.device_id.as_str(),
        "title": task.title,
        "source": task.source,
        "status": task.status.as_str(),
        "activeSessionId": task.active_session_id.map(|id| id.into_string()),
        "generation": task.generation,
        "createdAtMs": task.created_at_ms,
        "updatedAtMs": task.updated_at_ms
    })
}
