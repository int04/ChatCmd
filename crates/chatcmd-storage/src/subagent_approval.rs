//! Durable diagnostics for optional sub-agent grant inheritance, not authorization decisions.
use serde_json::{Value, json};
use sqlx::{Row as _, SqlitePool};

pub fn event_id(subagent_id: &str, attempt: i64) -> String {
    format!("subagent-grant:{subagent_id}:{attempt}")
}

/// Unknown legacy state must not be presented as an inherited grant.
pub fn status_value(raw: Option<&str>, requested: bool) -> Value {
    raw.and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or_else(|| json!({"status": if requested { "unknown" } else { "notRequested" }}))
}

/// Read only the diagnostic for this child's current worker attempt.
pub async fn status(pool: &SqlitePool, child_task_id: &str) -> Result<Option<Value>, sqlx::Error> {
    let row = sqlx::query("SELECT r.requested_approval_grant_json IS NOT NULL AS requested,json_extract(e.payload_json,'$.subagentApproval') AS diagnostic FROM subagent_runs r LEFT JOIN timeline_events e ON e.event_id='subagent-grant:'||r.id||':'||r.attempt AND e.task_id=r.child_task_id AND e.actor='system' AND e.kind='status' WHERE r.child_task_id=? LIMIT 1")
        .bind(child_task_id).fetch_optional(pool).await?;
    Ok(row.map(|row| {
        status_value(
            row.get::<Option<String>, _>("diagnostic").as_deref(),
            row.get("requested"),
        )
    }))
}
