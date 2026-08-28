use super::*;

pub(super) async fn delete_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, Problem> {
    let task_id = TaskId::new(&id).map_err(|_| bad_id())?;
    let task = state
        .repository
        .task(&task_id)
        .await
        .map_err(storage_problem)?
        .ok_or_else(not_found)?;

    if matches!(task.status, TaskStatus::Pending | TaskStatus::Running) {
        return Err(active_task_problem());
    }

    let descendant_ids = sqlx::query_scalar::<_, String>(
        "WITH RECURSIVE descendants(id) AS (\
            SELECT child_task_id FROM subagent_runs WHERE parent_task_id=? AND child_task_id IS NOT NULL \
            UNION \
            SELECT r.child_task_id FROM subagent_runs r JOIN descendants d ON r.parent_task_id=d.id WHERE r.child_task_id IS NOT NULL\
         ) SELECT id FROM descendants",
    )
    .bind(task_id.as_str())
    .fetch_all(state.repository.pool())
    .await
    .map_err(db_problem)?;

    for child_id in &descendant_ids {
        let status = sqlx::query_scalar::<_, String>("SELECT status FROM tasks WHERE id=?")
            .bind(child_id)
            .fetch_optional(state.repository.pool())
            .await
            .map_err(db_problem)?;
        if status.as_deref().is_some_and(|value| matches!(value, "pending" | "running")) {
            return Err(Problem::new(
                StatusCode::CONFLICT,
                "Subagent task is still active",
                "Không thể xóa đoạn trò chuyện vì vẫn còn agent con đang chạy. Hãy đợi agent con kết thúc hoặc dừng nó trước.",
            ));
        }
    }

    let mut transaction = state.repository.pool().begin().await.map_err(db_problem)?;
    for child_id in descendant_ids.iter().rev() {
        delete_task_data(&mut transaction, child_id).await?;
    }
    delete_task_data(&mut transaction, task_id.as_str()).await?;
    transaction.commit().await.map_err(db_problem)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_task_data(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    task_id: &str,
) -> Result<(), Problem> {
    for table in [
        "approvals",
        "artifact_registry",
        "task_execution_modes",
        "turn_bindings",
        "timeline_events",
        "task_sessions",
        "chatgpt_bridge_requests",
    ] {
        let statement = format!("DELETE FROM {table} WHERE task_id=?");
        sqlx::query(&statement)
            .bind(task_id)
            .execute(&mut **transaction)
            .await
            .map_err(db_problem)?;
    }

    sqlx::query("DELETE FROM subagent_runs WHERE child_task_id=?")
        .bind(task_id)
        .execute(&mut **transaction)
        .await
        .map_err(db_problem)?;

    sqlx::query("UPDATE terminal_sessions SET task_id=NULL WHERE task_id=?")
        .bind(task_id)
        .execute(&mut **transaction)
        .await
        .map_err(db_problem)?;
    sqlx::query("UPDATE terminal_event_chunks SET task_id=NULL WHERE task_id=?")
        .bind(task_id)
        .execute(&mut **transaction)
        .await
        .map_err(db_problem)?;

    let affected = sqlx::query("DELETE FROM tasks WHERE id=?")
        .bind(task_id)
        .execute(&mut **transaction)
        .await
        .map_err(db_problem)?
        .rows_affected();
    if affected == 0 {
        return Err(not_found());
    }
    Ok(())
}

fn active_task_problem() -> Problem {
    Problem::new(
        StatusCode::CONFLICT,
        "Task is still active",
        "Chỉ có thể xóa đoạn trò chuyện sau khi task đã kết thúc.",
    )
}
