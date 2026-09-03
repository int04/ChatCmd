use serde_json::{Value, json};
use sqlx::Row as _;

use crate::websocket::{AppEvent, AppState};

use super::{Problem, db_problem, iso_ms, now_ms};

#[derive(Debug)]
pub(super) struct SubagentParent {
    pub(super) task_id: String,
    pub(super) turn_id: String,
    pub(super) name: String,
}

pub(super) async fn task_subagent_data(
    state: &AppState,
    task_id: &str,
) -> Result<(Option<SubagentParent>, Vec<Value>), Problem> {
    ensure_reserved_subagent_tasks(state, task_id).await?;
    let parent_row = sqlx::query(
        "SELECT parent_task_id,parent_turn_id,name FROM subagent_runs WHERE child_task_id=? LIMIT 1",
    )
    .bind(task_id)
    .fetch_optional(state.repository.pool())
    .await
    .map_err(db_problem)?;
    let parent = parent_row.map(|row| SubagentParent {
        task_id: row.get("parent_task_id"),
        turn_id: row.get("parent_turn_id"),
        name: row.get("name"),
    });

    let rows = sqlx::query(
        "SELECT r.id,r.parent_turn_id,r.child_task_id,r.name,r.request,r.status AS registered_status,r.created_at_ms,r.updated_at_ms,r.completed_at_ms,r.worker_id,r.attempt,r.lease_expires_at_ms,r.last_heartbeat_at_ms,r.max_runtime_ms,r.started_at_ms,r.terminal_reason,t.status AS task_status FROM subagent_runs r LEFT JOIN tasks t ON t.id=r.child_task_id WHERE r.parent_task_id=? ORDER BY r.created_at_ms,r.id",
    )
    .bind(task_id)
    .fetch_all(state.repository.pool())
    .await
    .map_err(db_problem)?;

    let runs = rows
        .iter()
        .map(|row| {
            let registered = row.get::<String, _>("registered_status");
            let task_status = row.get::<Option<String>, _>("task_status");
            let status = effective_status(&registered, task_status.as_deref()).to_owned();
            let created_at = row.get::<i64, _>("created_at_ms");
            let updated_at = row.get::<i64, _>("updated_at_ms");
            let completed_at = row.get::<Option<i64>, _>("completed_at_ms");
            json!({
                "id": row.get::<String, _>("id"),
                "parentTurnId": row.get::<String, _>("parent_turn_id"),
                "taskId": row.get::<Option<String>, _>("child_task_id"),
                "name": row.get::<String, _>("name"),
                "request": row.get::<String, _>("request"),
                "status": status,
                "createdAtUtc": iso_ms(created_at),
                "updatedAtUtc": iso_ms(updated_at),
                "completedAtUtc": completed_at.map(iso_ms),
                "workerId": row.get::<Option<String>, _>("worker_id"),
                "attempt": row.get::<i64, _>("attempt"),
                "leaseExpiresAtUtc": row.get::<Option<i64>, _>("lease_expires_at_ms").map(iso_ms),
                "lastHeartbeatAtUtc": row.get::<Option<i64>, _>("last_heartbeat_at_ms").map(iso_ms),
                "maxRuntimeMs": row.get::<i64, _>("max_runtime_ms"),
                "startedAtUtc": row.get::<Option<i64>, _>("started_at_ms").map(iso_ms),
                "terminalReason": row.get::<Option<String>, _>("terminal_reason")
            })
        })
        .collect();
    Ok((parent, runs))
}

pub(super) async fn pending_subagent_approvals(
    state: &AppState,
    parent_task_id: &str,
) -> Result<Vec<Value>, Problem> {
    let rows = sqlx::query(
        "SELECT a.id AS activity_id,a.task_id AS child_task_id,a.request_json,a.created_at_ms,r.id AS subagent_id,r.parent_turn_id,r.name FROM approvals a JOIN subagent_runs r ON r.child_task_id=a.task_id WHERE r.parent_task_id=? AND a.state='pending' ORDER BY a.created_at_ms,a.id",
    )
    .bind(parent_task_id)
    .fetch_all(state.repository.pool())
    .await
    .map_err(db_problem)?;
    Ok(rows
        .iter()
        .map(|row| {
            let request = serde_json::from_str::<Value>(&row.get::<String, _>("request_json"))
                .unwrap_or(Value::Null);
            json!({
                "activityId": row.get::<String, _>("activity_id"),
                "childTaskId": row.get::<String, _>("child_task_id"),
                "subagentId": row.get::<String, _>("subagent_id"),
                "agentName": row.get::<String, _>("name"),
                "parentTurnId": row.get::<String, _>("parent_turn_id"),
                "childTurnId": request.get("turnId").cloned().unwrap_or(Value::Null),
                "tool": request.get("tool").cloned().unwrap_or(Value::Null),
                "input": request.get("input").cloned().unwrap_or(Value::Null),
                "createdAtUtc": iso_ms(row.get::<i64, _>("created_at_ms"))
            })
        })
        .collect())
}

pub(super) async fn publish_subagent_approval_resolved(
    state: &AppState,
    child_task_id: &str,
    activity_id: &str,
    decision: &str,
) -> Result<(), Problem> {
    let row = sqlx::query(
        "SELECT id,parent_task_id,parent_turn_id,name FROM subagent_runs WHERE child_task_id=? LIMIT 1",
    )
    .bind(child_task_id)
    .fetch_optional(state.repository.pool())
    .await
    .map_err(db_problem)?;
    let Some(row) = row else {
        return Ok(());
    };
    let mut event = AppEvent::new(
        "subagent.approval_resolved",
        json!({
            "subagentId": row.get::<String, _>("id"),
            "childTaskId": child_task_id,
            "activityId": activity_id,
            "agentName": row.get::<String, _>("name"),
            "decision": decision,
        }),
    );
    event.task_id = Some(row.get::<String, _>("parent_task_id"));
    event.session_id = Some(child_task_id.to_owned());
    event.turn_id = Some(row.get::<String, _>("parent_turn_id"));
    state.publish(event);
    Ok(())
}

async fn ensure_reserved_subagent_tasks(
    state: &AppState,
    parent_task_id: &str,
) -> Result<(), Problem> {
    let rows = sqlx::query(
        "SELECT id,name,created_at_ms FROM subagent_runs WHERE parent_task_id=? AND child_task_id IS NULL ORDER BY created_at_ms,id",
    )
    .bind(parent_task_id)
    .fetch_all(state.repository.pool())
    .await
    .map_err(db_problem)?;
    for row in rows {
        let subagent_id = row.get::<String, _>("id");
        let child_task_id = child_task_id_for_subagent(&subagent_id);
        let name = row.get::<String, _>("name");
        let created_at = row.get::<i64, _>("created_at_ms");
        let now = now_ms();
        sqlx::query("INSERT INTO tasks(id,agent_id,device_id,conversation_scope_hash,title,source,project_folder,status,active_session_id,generation,stopped_at_ms,created_at_ms,updated_at_ms) SELECT ?,agent_id,device_id,NULL,?,'mcp',project_folder,'pending',NULL,1,NULL,?,? FROM tasks WHERE id=? ON CONFLICT(id) DO NOTHING")
            .bind(&child_task_id)
            .bind(&name)
            .bind(created_at)
            .bind(now)
            .bind(parent_task_id)
            .execute(state.repository.pool())
            .await
            .map_err(db_problem)?;
        sqlx::query("UPDATE subagent_runs SET child_task_id=?,updated_at_ms=? WHERE id=? AND child_task_id IS NULL")
            .bind(&child_task_id)
            .bind(now)
            .bind(&subagent_id)
            .execute(state.repository.pool())
            .await
            .map_err(db_problem)?;
    }
    Ok(())
}

fn child_task_id_for_subagent(subagent_id: &str) -> String {
    format!("task-{subagent_id}")
}

pub(super) async fn mark_child_subagent_terminal(
    state: &AppState,
    child_task_id: &str,
    status: &str,
) -> Result<(), Problem> {
    let status = match status {
        "completed" | "failed" | "stopped" | "timedOut" | "interrupted" => status,
        _ => return Ok(()),
    };
    let row = sqlx::query(
        "SELECT id,parent_task_id,parent_turn_id,name FROM subagent_runs WHERE child_task_id=? LIMIT 1",
    )
    .bind(child_task_id)
    .fetch_optional(state.repository.pool())
    .await
    .map_err(db_problem)?;
    let Some(row) = row else {
        return Ok(());
    };

    let now = now_ms();
    let affected = sqlx::query(
        "UPDATE subagent_runs SET status=?,terminal_reason=NULL,lease_expires_at_ms=NULL,updated_at_ms=?,completed_at_ms=? WHERE child_task_id=? AND status IN ('pending','running')",
    )
    .bind(status)
    .bind(now)
    .bind(now)
    .bind(child_task_id)
    .execute(state.repository.pool())
    .await
    .map_err(db_problem)?
    .rows_affected();
    if affected == 0 {
        return Ok(());
    }

    let subagent_id = row.get::<String, _>("id");
    let parent_task_id = row.get::<String, _>("parent_task_id");
    let parent_turn_id = row.get::<String, _>("parent_turn_id");
    let name = row.get::<String, _>("name");
    let mut event = AppEvent::new(
        "subagent.status",
        json!({
            "subagentId": subagent_id,
            "childTaskId": child_task_id,
            "name": name,
            "status": status
        }),
    );
    event.task_id = Some(parent_task_id);
    event.session_id = Some(child_task_id.to_owned());
    event.turn_id = Some(parent_turn_id);
    state.publish(event);
    Ok(())
}

pub(super) async fn interrupt_active_child_subagents(
    state: &AppState,
    parent_task_id: &str,
) -> Result<(), Problem> {
    let rows = sqlx::query("SELECT id,parent_turn_id,child_task_id,name,attempt FROM subagent_runs WHERE parent_task_id=? AND status IN ('pending','running')")
        .bind(parent_task_id)
        .fetch_all(state.repository.pool())
        .await
        .map_err(db_problem)?;
    let now = now_ms();
    for row in rows {
        let id = row.get::<String, _>("id");
        let child_task_id = row.get::<Option<String>, _>("child_task_id");
        let affected = sqlx::query("UPDATE subagent_runs SET status='interrupted',terminal_reason='parent task stopped',lease_expires_at_ms=NULL,updated_at_ms=?,completed_at_ms=? WHERE id=? AND status IN ('pending','running')")
            .bind(now).bind(now).bind(&id).execute(state.repository.pool()).await.map_err(db_problem)?.rows_affected();
        if affected == 0 {
            continue;
        }
        if let Some(child_task_id) = child_task_id.as_deref() {
            state.activities.cancel_task(child_task_id);
            sqlx::query("UPDATE tasks SET status=CASE WHEN status IN ('pending','running') THEN 'interrupted' ELSE status END,active_session_id=NULL,updated_at_ms=? WHERE id=?")
                .bind(now).bind(child_task_id).execute(state.repository.pool()).await.map_err(db_problem)?;
            sqlx::query("UPDATE approvals SET state='cancelled',decision_json='{\"reason\":\"parent task stopped\"}',resolved_at_ms=? WHERE task_id=? AND state='pending'")
                .bind(now).bind(child_task_id).execute(state.repository.pool()).await.map_err(db_problem)?;
        }
        let mut event = AppEvent::new(
            "subagent.status",
            json!({
                "subagentId": id,
                "childTaskId": child_task_id,
                "name": row.get::<String, _>("name"),
                "status": "interrupted",
                "reason": "parent task stopped",
                "attempt": row.get::<i64, _>("attempt")
            }),
        );
        event.task_id = Some(parent_task_id.to_owned());
        event.turn_id = Some(row.get::<String, _>("parent_turn_id"));
        state.publish(event);
    }
    Ok(())
}

fn effective_status<'a>(registered: &'a str, task_status: Option<&'a str>) -> &'a str {
    if registered == "timedOut" {
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
