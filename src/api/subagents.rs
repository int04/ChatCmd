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
        "SELECT r.id,r.parent_turn_id,r.child_task_id,r.name,r.request,r.status AS registered_status,r.created_at_ms,r.updated_at_ms,r.completed_at_ms,t.status AS task_status FROM subagent_runs r LEFT JOIN tasks t ON t.id=r.child_task_id WHERE r.parent_task_id=? ORDER BY r.created_at_ms,r.id",
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
                "completedAtUtc": completed_at.map(iso_ms)
            })
        })
        .collect();
    Ok((parent, runs))
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
        sqlx::query("INSERT INTO tasks(id,agent_id,device_id,conversation_scope_hash,title,source,status,active_session_id,generation,stopped_at_ms,created_at_ms,updated_at_ms) SELECT ?,agent_id,device_id,NULL,?,'mcp','pending',NULL,1,NULL,?,? FROM tasks WHERE id=? ON CONFLICT(id) DO NOTHING")
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
        "completed" | "failed" | "stopped" | "interrupted" => status,
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
    sqlx::query(
        "UPDATE subagent_runs SET status=?,updated_at_ms=?,completed_at_ms=? WHERE child_task_id=?",
    )
    .bind(status)
    .bind(now)
    .bind(now)
    .bind(child_task_id)
    .execute(state.repository.pool())
    .await
    .map_err(db_problem)?;

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

fn effective_status<'a>(registered: &'a str, task_status: Option<&'a str>) -> &'a str {
    match task_status {
        Some("completed") => "completed",
        Some("failed") => "failed",
        Some("stopped") => "stopped",
        Some("interrupted") => "interrupted",
        Some("running") if registered == "pending" => "running",
        _ => registered,
    }
}
