use std::time::Duration;

use chatcmd_runtime::{OperationContext, RuntimeError, RuntimeResult};
use serde_json::{Value, json};
use sqlx::Row as _;
use tokio::time::{Instant, sleep};
use uuid::Uuid;

use super::approval::SubagentGrantInheritance;
use super::inputs::SubagentApprovalGrantInput;
use super::{RuntimeHost, now_ms};

const SUBAGENT_MARKER_PREFIX: &str = "CMDGPT_SUBAGENT_ID=";
const MAX_SUBAGENT_NAME_CHARS: usize = 120;
const MAX_SUBAGENT_REQUEST_CHARS: usize = 20_000;
const UNCLAIMED_SUBAGENT_TIMEOUT_MS: i64 = 60_000;
const EXTENSION_UNCLAIMED_SUBAGENT_TIMEOUT_MS: i64 = 180_000;
const DEFAULT_SUBAGENT_LEASE_MS: i64 = 60_000;
const MIN_SUBAGENT_LEASE_MS: i64 = 15_000;
const MAX_SUBAGENT_LEASE_MS: i64 = 600_000;
const DEFAULT_SUBAGENT_MAX_RUNTIME_MS: i64 = 1_800_000;
const MIN_SUBAGENT_MAX_RUNTIME_MS: i64 = 60_000;
const MAX_SUBAGENT_MAX_RUNTIME_MS: i64 = 86_400_000;
const SUBAGENT_WATCHDOG_BATCH: i64 = 100;

mod coordination;
mod grant_bootstrap;
mod lifecycle;
mod registration;
mod reports;
mod watchdog;

fn subagent_row_value(row: &sqlx::sqlite::SqliteRow) -> Value {
    let registered = row.get::<String, _>("registered_status");
    let task_status = row.get::<Option<String>, _>("task_status");
    let status = effective_status(&registered, task_status.as_deref());
    json!({
        "id": row.get::<String, _>("id"),
        "parentTurnId": row.get::<String, _>("parent_turn_id"),
        "parentTaskId": row.get::<String, _>("parent_task_id"),
        "rootTurnId": row.get::<String, _>("root_turn_id"),
        "parentName": row.get::<Option<String>, _>("parent_name"),
        "taskId": row.get::<Option<String>, _>("child_task_id"),
        "name": row.get::<String, _>("name"),
        "request": row.get::<String, _>("request"),
        "status": status,
        "approvalGrant": chatcmd_storage::subagent_approval::status_value(row.get::<Option<String>, _>("approval_grant_json").as_deref(), row.get("approval_grant_requested")),
        "createdAtMs": row.get::<i64, _>("created_at_ms"),
        "updatedAtMs": row.get::<i64, _>("updated_at_ms"),
        "completedAtMs": row.get::<Option<i64>, _>("completed_at_ms")
        ,"workerId": row.get::<Option<String>, _>("worker_id")
        ,"attempt": row.get::<i64, _>("attempt")
        ,"leaseExpiresAtMs": row.get::<Option<i64>, _>("lease_expires_at_ms")
        ,"lastHeartbeatAtMs": row.get::<Option<i64>, _>("last_heartbeat_at_ms")
        ,"maxRuntimeMs": row.get::<i64, _>("max_runtime_ms")
        ,"startedAtMs": row.get::<Option<i64>, _>("started_at_ms")
        ,"terminalReason": row.get::<Option<String>, _>("terminal_reason")
    })
}

fn effective_status<'a>(registered: &'a str, task_status: Option<&'a str>) -> &'a str {
    if matches!(
        registered,
        "completed" | "failed" | "stopped" | "timedOut" | "interrupted"
    ) {
        return registered;
    }
    match task_status {
        Some("completed") => "completed",
        Some("failed") => "failed",
        Some("stopped") => "stopped",
        Some("interrupted") => "interrupted",
        Some("running") if registered == "pending" => "running",
        _ => registered,
    }
}

fn normalize_terminal_status(status: &str) -> &str {
    match status {
        "completed" => "completed",
        "failed" => "failed",
        "stopped" => "stopped",
        "interrupted" => "interrupted",
        "timedOut" => "timedOut",
        _ => "completed",
    }
}

fn subagent_lease_ms() -> i64 {
    configured_duration_ms(
        "CHATCMD_SUBAGENT_LEASE_MS",
        DEFAULT_SUBAGENT_LEASE_MS,
        MIN_SUBAGENT_LEASE_MS,
        MAX_SUBAGENT_LEASE_MS,
    )
}

fn subagent_max_runtime_ms() -> i64 {
    configured_duration_ms(
        "CHATCMD_SUBAGENT_MAX_RUNTIME_MS",
        DEFAULT_SUBAGENT_MAX_RUNTIME_MS,
        MIN_SUBAGENT_MAX_RUNTIME_MS,
        MAX_SUBAGENT_MAX_RUNTIME_MS,
    )
}

fn configured_duration_ms(name: &str, default: i64, minimum: i64, maximum: i64) -> i64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(default)
        .clamp(minimum, maximum)
}

fn is_pending_status(status: Option<&str>) -> bool {
    matches!(status, Some("pending" | "running"))
}

fn required_context_value<'a>(value: Option<&'a str>, field: &str) -> RuntimeResult<&'a str> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| RuntimeError::new("invalid_context", format!("{field} is required")))
}

fn validate_text<'a>(field: &str, value: &'a str, max_chars: usize) -> RuntimeResult<&'a str> {
    let value = value.trim();
    if value.is_empty() {
        return Err(RuntimeError::new(
            "invalid_arguments",
            format!("{field} must not be empty"),
        ));
    }
    if value.chars().count() > max_chars {
        return Err(RuntimeError::new(
            "invalid_arguments",
            format!("{field} exceeds {max_chars} characters"),
        ));
    }
    Ok(value)
}

fn validate_subagent_approval_request(
    request: Option<&SubagentApprovalGrantInput>,
) -> RuntimeResult<()> {
    let Some(request) = request else {
        return Ok(());
    };
    super::approval::validate_subagent_grant_request(request).map(|_| ())
}

fn subagent_id_for_registration(
    parent_task_id: &str,
    parent_turn_id: &str,
    name: &str,
    request: &str,
    approval_grant_json: Option<&str>,
) -> String {
    let material = format!(
        "{parent_task_id}\0{parent_turn_id}\0{name}\0{request}\0{}",
        approval_grant_json.unwrap_or_default()
    );
    format!(
        "subagent-{}",
        Uuid::new_v5(&Uuid::NAMESPACE_OID, material.as_bytes())
    )
}

fn subagent_registration_value(
    subagent_id: &str,
    child_task_id: &str,
    name: &str,
    status: &str,
    duplicate: bool,
) -> Value {
    json!({
        "subagentId": subagent_id,
        "taskId": child_task_id,
        "childTaskId": child_task_id,
        "name": name,
        "status": status,
        "duplicate": duplicate,
        "delegationMarker": format!("{SUBAGENT_MARKER_PREFIX}{subagent_id}"),
        "instruction": "Include delegationMarker verbatim in the child agent request. The child must preserve it in its first agent_user_message call."
    })
}

fn child_task_id_for_subagent(subagent_id: &str) -> String {
    format!("task-{subagent_id}")
}

fn extract_subagent_id(message: &str) -> Option<String> {
    message.lines().find_map(|line| {
        let marker = line.find(SUBAGENT_MARKER_PREFIX)?;
        let tail = &line[marker + SUBAGENT_MARKER_PREFIX.len()..];
        let id = tail
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
            .collect::<String>();
        id.starts_with("subagent-").then_some(id)
    })
}

#[cfg(test)]
mod tests {
    use super::{child_task_id_for_subagent, extract_subagent_id, subagent_id_for_registration};

    #[test]
    fn registration_id_is_stable_for_semantic_retry() {
        let first = subagent_id_for_registration("parent", "turn", "Reader", "Read lib.rs", None);
        assert_eq!(
            first,
            subagent_id_for_registration("parent", "turn", "Reader", "Read lib.rs", None)
        );
        assert_ne!(
            first,
            subagent_id_for_registration("parent", "turn", "Reader", "Read another.rs", None)
        );
        assert_ne!(
            first,
            subagent_id_for_registration(
                "parent",
                "turn",
                "Reader",
                "Read lib.rs",
                Some("{\"allowedTools\":[\"fs_read_text\"]}")
            )
        );
    }

    #[test]
    fn child_task_id_is_stable_from_subagent_id() {
        assert_eq!(
            child_task_id_for_subagent("subagent-1234-abcd"),
            "task-subagent-1234-abcd"
        );
    }

    #[test]
    fn extracts_subagent_marker_from_delegated_prompt() {
        assert_eq!(
            extract_subagent_id(
                "Please inspect this.\nCMDGPT_SUBAGENT_ID=subagent-1234-abcd\nKeep going."
            ),
            Some("subagent-1234-abcd".to_owned())
        );
    }

    #[test]
    fn ignores_unrelated_text() {
        assert_eq!(extract_subagent_id("normal delegated request"), None);
    }
}
