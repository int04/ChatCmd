//! Shared descendant traversal for the root conversation, approvals and completion gate.
//! UNION deduplicates identifiers, so even a malformed legacy cycle terminates.
use sqlx::{SqlitePool, sqlite::SqliteRow};

pub const BROWSER_HEARTBEAT_LEASE_MS: i64 = 180_000;

pub const DESCENDANTS_CTE: &str = "WITH RECURSIVE descendants(id,root_turn_id) AS (
    SELECT id,parent_turn_id FROM subagent_runs WHERE parent_task_id=?
    UNION
    SELECT child.id,ancestor.root_turn_id FROM subagent_runs child
    JOIN subagent_runs parent ON child.parent_task_id=parent.child_task_id
    JOIN descendants ancestor ON ancestor.id=parent.id
)";

/// Returns all descendants while preserving both the direct parent and root turn.
pub async fn descendant_runs(
    pool: &SqlitePool,
    task_id: &str,
    turn_id: Option<&str>,
) -> Result<Vec<SqliteRow>, sqlx::Error> {
    let sql = format!("{DESCENDANTS_CTE}
        SELECT r.id,r.parent_task_id,r.parent_turn_id,d.root_turn_id,r.child_task_id,r.name,r.request,
        r.status AS registered_status,r.created_at_ms,r.updated_at_ms,r.completed_at_ms,r.worker_id,
        r.attempt,r.lease_expires_at_ms,r.last_heartbeat_at_ms,r.max_runtime_ms,r.started_at_ms,
        COALESCE(r.terminal_reason,CASE WHEN r.status='failed' THEN r.fallback_error END) AS terminal_reason,
        t.status AS task_status,p.title AS parent_name,
        r.requested_approval_grant_json IS NOT NULL AS approval_grant_requested,
        json_extract(g.payload_json,'$.subagentApproval') AS approval_grant_json
        FROM descendants d JOIN subagent_runs r ON r.id=d.id
        LEFT JOIN tasks t ON t.id=r.child_task_id LEFT JOIN tasks p ON p.id=r.parent_task_id
        LEFT JOIN timeline_events g ON g.event_id='subagent-grant:'||r.id||':'||r.attempt AND g.task_id=r.child_task_id AND g.actor='system' AND g.kind='status'
        WHERE (? IS NULL OR d.root_turn_id=?) ORDER BY r.created_at_ms,r.id");
    sqlx::query(&sql)
        .bind(task_id)
        .bind(turn_id)
        .bind(turn_id)
        .fetch_all(pool)
        .await
}
