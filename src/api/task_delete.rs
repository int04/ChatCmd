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
        return Err(Problem::new(
            StatusCode::CONFLICT,
            "Task is still active",
            "Chỉ có thể xóa đoạn trò chuyện sau khi task đã kết thúc.",
        ));
    }

    let mut transaction = state.repository.pool().begin().await.map_err(db_problem)?;
    for table in [
        "approvals",
        "artifact_registry",
        "task_execution_modes",
        "turn_bindings",
        "timeline_events",
        "task_sessions",
    ] {
        let statement = format!("DELETE FROM {table} WHERE task_id=?");
        sqlx::query(&statement)
            .bind(task_id.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(db_problem)?;
    }

    let affected = sqlx::query("DELETE FROM tasks WHERE id=?")
        .bind(task_id.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(db_problem)?
        .rows_affected();
    if affected == 0 {
        return Err(not_found());
    }

    transaction.commit().await.map_err(db_problem)?;
    Ok(StatusCode::NO_CONTENT)
}
