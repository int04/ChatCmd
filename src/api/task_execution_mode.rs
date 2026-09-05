use chatcmd_core::{ExecutionMode, TaskStore as _};
use serde::Deserialize;

use super::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskExecutionModeRequest {
    mode: String,
}

pub(super) async fn task_execution_mode(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Problem> {
    let task_id = TaskId::new(&id).map_err(|_| bad_id())?;
    if state
        .repository
        .task(&task_id)
        .await
        .map_err(storage_problem)?
        .is_none()
    {
        return Err(not_found());
    }
    let overridden = sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM task_execution_modes WHERE task_id=? LIMIT 1)",
    )
    .bind(task_id.as_str())
    .fetch_one(state.repository.pool())
    .await
    .map_err(db_problem)?
        == 1;
    let mode = state
        .repository
        .execution_mode(Some(&task_id))
        .await
        .map_err(storage_problem)?;
    Ok(Json(
        json!({ "mode": execution_mode_name(mode), "overridden": overridden }),
    ))
}

pub(super) async fn set_task_execution_mode(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(request): Json<TaskExecutionModeRequest>,
) -> Result<Json<Value>, Problem> {
    let task_id = TaskId::new(&id).map_err(|_| bad_id())?;
    if state
        .repository
        .task(&task_id)
        .await
        .map_err(storage_problem)?
        .is_none()
    {
        return Err(not_found());
    }
    let mode = match request.mode.as_str() {
        "approval" => ExecutionMode::Approval,
        "allowAll" | "allow" => ExecutionMode::Allow,
        _ => {
            return Err(Problem::new(
                StatusCode::BAD_REQUEST,
                "Invalid command execution mode",
                "Mode must be 'approval' or 'allowAll'.",
            ));
        }
    };
    persist_user_execution_mode(&state, &task_id, mode).await?;
    Ok(Json(
        json!({ "mode": execution_mode_name(mode), "overridden": true }),
    ))
}

async fn persist_user_execution_mode(
    state: &AppState,
    task_id: &TaskId,
    mode: ExecutionMode,
) -> Result<(), Problem> {
    let now = now_ms();
    let decision_id = uuid::Uuid::new_v4().to_string();
    let payload = json!({
        "decisionId": decision_id,
        "source": "authenticatedLocalUi",
        "mode": execution_mode_name(mode),
    });
    let mut transaction = state.repository.pool().begin().await.map_err(db_problem)?;
    sqlx::query("INSERT INTO task_execution_modes(task_id,mode,updated_at_ms) VALUES(?,?,?) ON CONFLICT(task_id) DO UPDATE SET mode=excluded.mode,updated_at_ms=excluded.updated_at_ms")
        .bind(task_id.as_str())
        .bind(mode.as_str())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(db_problem)?;
    sqlx::query("WITH RECURSIVE task_tree(id) AS (SELECT ? UNION ALL SELECT child_task_id FROM subagent_runs JOIN task_tree ON parent_task_id=task_tree.id) UPDATE approvals SET state='cancelled',decision_json=json_object('reason','execution policy changed'),resolved_at_ms=? WHERE task_id IN (SELECT id FROM task_tree) AND state='pending'")
        .bind(task_id.as_str())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(db_problem)?;
    sqlx::query("WITH RECURSIVE grants(id) AS (SELECT id FROM approval_grants WHERE task_id=? UNION ALL SELECT child.id FROM approval_grants child JOIN grants parent ON child.inherited_from=parent.id) UPDATE approval_grants SET state='revoked',updated_at_ms=? WHERE id IN (SELECT id FROM grants) AND state='active'")
        .bind(task_id.as_str())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(db_problem)?;
    sqlx::query("INSERT INTO timeline_events(event_id,task_id,turn_id,session_id,actor,kind,idempotency_key,payload_json,metadata_json,created_at_ms) VALUES(?,?,NULL,NULL,'user','status',?,?,NULL,?)")
        .bind(&decision_id)
        .bind(task_id.as_str())
        .bind(&decision_id)
        .bind(payload.to_string())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(db_problem)?;
    transaction.commit().await.map_err(db_problem)?;

    let mut event = AppEvent::new("execution_mode.changed", payload);
    event.id = decision_id;
    event.task_id = Some(task_id.as_str().to_owned());
    state.publish(event);
    Ok(())
}

pub(super) fn execution_mode_name(mode: ExecutionMode) -> &'static str {
    match mode {
        ExecutionMode::Allow => "allowAll",
        ExecutionMode::Approval | ExecutionMode::Deny => "approval",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_host::user_message_tests;

    #[tokio::test]
    async fn parent_policy_change_cancels_child_pending_and_revokes_inherited_grants() {
        let (host, agent_id, directory) = user_message_tests::test_host().await;
        let state = host.test_app_state(directory.path().join("chatcmd.db").display().to_string());
        let device_id: String = sqlx::query_scalar("SELECT device_id FROM local_device LIMIT 1")
            .fetch_one(state.repository.pool())
            .await
            .expect("device");
        let now = now_ms();
        for task in ["policy-root", "policy-child"] {
            sqlx::query("INSERT INTO tasks(id,agent_id,device_id,title,source,status,generation,created_at_ms,updated_at_ms) VALUES(?,?,?,?,'mcp','running',1,?,?)")
                .bind(task).bind(&agent_id).bind(&device_id).bind(task).bind(now).bind(now)
                .execute(state.repository.pool()).await.expect("task");
        }
        sqlx::query("INSERT INTO subagent_runs(id,parent_task_id,parent_turn_id,child_task_id,name,request,status,created_at_ms,updated_at_ms) VALUES('policy-run','policy-root','policy-turn','policy-child','child','read','pending',?,?)")
            .bind(now).bind(now).execute(state.repository.pool()).await.expect("lineage");
        sqlx::query("INSERT INTO approvals(id,task_id,state,request_json,created_at_ms) VALUES('policy-pending','policy-child','pending','{}',?)")
            .bind(now).execute(state.repository.pool()).await.expect("pending approval");
        let expires = now.saturating_add(60_000);
        sqlx::query("INSERT INTO approval_grants(id,owner_agent_id,task_id,allowed_tools_json,path_scopes_json,option_constraints_json,max_calls,expires_at_ms,catalog_hash,state,created_at_ms,updated_at_ms) VALUES('policy-parent-grant',?,'policy-root','[\"fs_stat\"]','[]','{}',1,?,'catalog','active',?,?)")
            .bind(&agent_id).bind(expires).bind(now).bind(now)
            .execute(state.repository.pool()).await.expect("parent grant");
        sqlx::query("INSERT INTO approval_grants(id,owner_agent_id,task_id,allowed_tools_json,path_scopes_json,option_constraints_json,max_calls,expires_at_ms,inherited_from,catalog_hash,state,created_at_ms,updated_at_ms) VALUES('policy-child-grant',?,'policy-child','[\"fs_stat\"]','[]','{}',1,?,'policy-parent-grant','catalog','active',?,?)")
            .bind(&agent_id).bind(expires).bind(now).bind(now)
            .execute(state.repository.pool()).await.expect("child grant");

        persist_user_execution_mode(
            &state,
            &TaskId::new("policy-root").expect("root"),
            ExecutionMode::Approval,
        )
        .await
        .expect("policy change");

        let approval: String =
            sqlx::query_scalar("SELECT state FROM approvals WHERE id='policy-pending'")
                .fetch_one(state.repository.pool())
                .await
                .expect("approval state");
        assert_eq!(approval, "cancelled");
        let grants: Vec<String> = sqlx::query_scalar("SELECT state FROM approval_grants WHERE id IN ('policy-parent-grant','policy-child-grant') ORDER BY id")
            .fetch_all(state.repository.pool()).await.expect("grant states");
        assert_eq!(grants, vec!["revoked", "revoked"]);
    }
}
