//! Resolve delegation intent from the acknowledged turn's original ChatUI request.
//! An arbitrary browser transcript is never an initialization or authorization receipt.
use chatcmd_runtime::{RuntimeError, RuntimeResult};
use chatcmd_storage::SqliteRepository;
use serde_json::Value;

pub(super) async fn content(
    repository: &SqliteRepository,
    root_task_id: &str,
    payload_json: Option<&str>,
) -> RuntimeResult<String> {
    let payload = payload_json.and_then(|raw| serde_json::from_str::<Value>(raw).ok());
    let echoed = payload
        .as_ref()
        .and_then(|value| value.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(payload) = payload.as_ref() else {
        return Ok(String::new());
    };
    if payload.get("provider").and_then(Value::as_str) == Some("chatgpt_web") {
        return Ok(String::new());
    }
    let link = (
        payload.get("tool").and_then(Value::as_str),
        payload.get("bridgeRequestId").and_then(Value::as_str),
        payload.get("browserTurnId").and_then(Value::as_str),
    );
    let (Some("agent_user_message"), Some(request_id), Some(browser_turn)) = link else {
        return Ok(echoed.to_owned());
    };
    // The link was produced by save_user_message after identity/approval validation.
    // Scope it to this exact task and browser turn, never the latest request or a
    // caller-supplied browser snapshot. Native recording alone has no such receipt.
    let request: Option<(String, String)> = sqlx::query_as(
        "SELECT r.user_content,r.submitted_content FROM chatgpt_bridge_requests r JOIN tasks t ON t.id=r.task_id WHERE r.id=? AND r.task_id=? AND r.turn_id=? AND r.agent_id=t.agent_id AND t.allow_execute=1",
    ).bind(request_id).bind(root_task_id).bind(browser_turn)
        .fetch_optional(repository.pool()).await.map_err(|_| {
            RuntimeError::new("storage_error", "acknowledged delegation request lookup failed")
        })?;
    let Some((original, submitted)) = request else {
        // A stale/cross-task link cannot fall back to a model-added permission.
        return Ok(String::new());
    };
    if crate::chatgpt_routing::request_id(&submitted) == Some(request_id) {
        // Routed ChatUI accepts a shortened MCP echo to preserve conversation identity.
        // That echo must neither remove the user's delegation request nor add permission
        // the original user never gave. Keep the echoed event intact for diagnostics.
        return Ok(original);
    }
    // Legacy/unrouted MCP turns keep their existing exact-message behavior.
    Ok(echoed.to_owned())
}

#[cfg(test)]
#[path = "subagent_request_source_tests.rs"]
mod tests;
