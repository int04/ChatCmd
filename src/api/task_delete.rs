use super::*;

pub(super) async fn delete_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, Problem> {
    let task_id = TaskId::new(&id).map_err(|_| bad_id())?;
    delete_task_by_id(&state, &task_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn delete_task_by_id(
    state: &Arc<AppState>,
    task_id: &TaskId,
) -> Result<(), Problem> {
    let task = state
        .repository
        .task(task_id)
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
        if status
            .as_deref()
            .is_some_and(|value| matches!(value, "pending" | "running"))
        {
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
    for child_id in &descendant_ids {
        let _ = state.blob_store.cleanup_task(child_id);
    }
    let _ = state.blob_store.cleanup_task(task_id.as_str());
    Ok(())
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

const DATA_RETENTION_SETTING: &str = "ui_dataRetention";
const DATA_CLEANUP_LAST_RUN_SETTING: &str = "data_cleanup_last_run_ms";
const DEFAULT_DATA_RETENTION: &str = "1d";

pub(super) async fn delete_all_user_data(
    State(state): State<Arc<AppState>>,
) -> Result<StatusCode, Problem> {
    cleanup_user_generated_data(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) fn start_data_cleanup_scheduler(state: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            if let Err(error) = run_scheduled_cleanup_if_due(&state).await {
                tracing::warn!(error = %error.detail, "automatic data cleanup failed");
            }
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }
    });
}

async fn run_scheduled_cleanup_if_due(state: &Arc<AppState>) -> Result<(), Problem> {
    let retention = state
        .repository
        .setting(DATA_RETENTION_SETTING)
        .await
        .map_err(storage_problem)?
        .and_then(|setting| serde_json::from_str::<String>(&setting.value_json).ok())
        .unwrap_or_else(|| DEFAULT_DATA_RETENTION.to_owned());
    let Some(retention_ms) = retention_ms(&retention) else {
        return Ok(());
    };

    let now = now_ms();
    let cutoff_ms = now.saturating_sub(retention_ms);
    cleanup_expired_user_generated_data(state, cutoff_ms).await?;
    persist_cleanup_last_run(state, now).await?;
    Ok(())
}

fn retention_ms(value: &str) -> Option<i64> {
    let hours = match value {
        "1h" => 1,
        "5h" => 5,
        "10h" => 10,
        "1d" => 24,
        "3d" => 72,
        "5d" => 120,
        "10d" => 240,
        "off" => return None,
        _ => 24,
    };
    Some(hours * 60 * 60 * 1_000)
}

async fn persist_cleanup_last_run(state: &Arc<AppState>, timestamp_ms: i64) -> Result<(), Problem> {
    state
        .repository
        .set_setting(&Setting {
            key: DATA_CLEANUP_LAST_RUN_SETTING.to_owned(),
            value_json: timestamp_ms.to_string(),
            updated_at_ms: now_ms(),
        })
        .await
        .map_err(storage_problem)
}

async fn cleanup_expired_user_generated_data(
    state: &Arc<AppState>,
    cutoff_ms: i64,
) -> Result<(), Problem> {
    let task_ids = sqlx::query_scalar::<_, String>(
        "SELECT id FROM tasks \
         WHERE created_at_ms < ? \
           AND status NOT IN ('pending','running') \
           AND NOT EXISTS (\
               SELECT 1 FROM terminal_sessions s \
               WHERE s.task_id=tasks.id AND s.status IN ('starting','running')\
           ) \
           AND NOT EXISTS (\
               SELECT 1 FROM chatgpt_bridge_requests r \
               WHERE r.task_id=tasks.id AND r.status IN ('queued','running','stop_requested')\
           ) \
         ORDER BY created_at_ms, id",
    )
    .bind(cutoff_ms)
    .fetch_all(state.repository.pool())
    .await
    .map_err(db_problem)?;

    let mut transaction = state.repository.pool().begin().await.map_err(db_problem)?;
    let mut deleted_any = false;

    for task_id in &task_ids {
        deleted_any |= delete_expired_task_data(&mut transaction, task_id).await?;
    }

    let stale_bridge_requests = sqlx::query(
        "DELETE FROM chatgpt_bridge_requests \
         WHERE task_id IS NULL \
           AND created_at_ms < ? \
           AND status NOT IN ('queued','running','stop_requested')",
    )
    .bind(cutoff_ms)
    .execute(&mut *transaction)
    .await
    .map_err(db_problem)?
    .rows_affected();
    deleted_any |= stale_bridge_requests > 0;

    let stale_terminal_events = sqlx::query(
        "DELETE FROM terminal_event_chunks \
         WHERE session_id IN (\
             SELECT id FROM terminal_sessions \
             WHERE task_id IS NULL \
               AND created_at_ms < ? \
               AND status NOT IN ('starting','running')\
         )",
    )
    .bind(cutoff_ms)
    .execute(&mut *transaction)
    .await
    .map_err(db_problem)?
    .rows_affected();
    deleted_any |= stale_terminal_events > 0;

    let stale_terminals = sqlx::query(
        "DELETE FROM terminal_sessions \
         WHERE task_id IS NULL \
           AND created_at_ms < ? \
           AND status NOT IN ('starting','running')",
    )
    .bind(cutoff_ms)
    .execute(&mut *transaction)
    .await
    .map_err(db_problem)?
    .rows_affected();
    deleted_any |= stale_terminals > 0;

    transaction.commit().await.map_err(db_problem)?;
    for task_id in &task_ids {
        let _ = state.blob_store.cleanup_task(task_id);
    }

    if deleted_any {
        compact_database(state).await?;
    }
    Ok(())
}

async fn delete_expired_task_data(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    task_id: &str,
) -> Result<bool, Problem> {
    sqlx::query("DELETE FROM chatgpt_conversations WHERE task_id=?")
        .bind(task_id)
        .execute(&mut **transaction)
        .await
        .map_err(db_problem)?;

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
            .bind(task_id)
            .execute(&mut **transaction)
            .await
            .map_err(db_problem)?;
    }

    sqlx::query("DELETE FROM subagent_runs WHERE parent_task_id=? OR child_task_id=?")
        .bind(task_id)
        .bind(task_id)
        .execute(&mut **transaction)
        .await
        .map_err(db_problem)?;

    sqlx::query("DELETE FROM chatgpt_bridge_requests WHERE task_id=?")
        .bind(task_id)
        .execute(&mut **transaction)
        .await
        .map_err(db_problem)?;

    sqlx::query(
        "DELETE FROM terminal_event_chunks \
         WHERE task_id=? \
            OR session_id IN (SELECT id FROM terminal_sessions WHERE task_id=?)",
    )
    .bind(task_id)
    .bind(task_id)
    .execute(&mut **transaction)
    .await
    .map_err(db_problem)?;

    sqlx::query("DELETE FROM terminal_sessions WHERE task_id=?")
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
    Ok(affected > 0)
}

async fn compact_database(state: &Arc<AppState>) -> Result<(), Problem> {
    sqlx::query("VACUUM")
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    Ok(())
}

async fn cleanup_user_generated_data(state: &Arc<AppState>) -> Result<(), Problem> {
    let active_tasks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE status='running'")
        .fetch_one(state.repository.pool())
        .await
        .map_err(db_problem)?;
    let active_terminals: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM terminal_sessions WHERE status IN ('starting','running')",
    )
    .fetch_one(state.repository.pool())
    .await
    .map_err(db_problem)?;
    if active_tasks > 0 || active_terminals > 0 {
        return Err(Problem::new(
            StatusCode::CONFLICT,
            "User data is still active",
            "Không thể xóa toàn bộ dữ liệu khi vẫn còn công việc hoặc terminal đang chạy. Hãy dừng hoặc chờ các công việc hiện tại kết thúc trước.",
        ));
    }

    let mut transaction = state.repository.pool().begin().await.map_err(db_problem)?;

    // Usage-generated data only. System/configuration tables such as settings,
    // agents, tools, local device identity and workspace_projects are preserved.
    for table in [
        "chatgpt_conversations",
        "approvals",
        "artifact_registry",
        "task_execution_modes",
        "turn_bindings",
        "timeline_events",
        "task_sessions",
        "subagent_runs",
        "chatgpt_bridge_requests",
        "terminal_event_chunks",
        "terminal_sessions",
        "tasks",
    ] {
        let statement = format!("DELETE FROM {table}");
        sqlx::query(&statement)
            .execute(&mut *transaction)
            .await
            .map_err(db_problem)?;
    }

    transaction.commit().await.map_err(db_problem)?;
    let _ = state.blob_store.cleanup_all();

    // VACUUM must run outside a transaction. This returns free pages to the OS so
    // the SQLite file shrinks after both manual and scheduled cleanup.
    sqlx::query("VACUUM")
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(state.repository.pool())
        .await
        .map_err(db_problem)?;
    persist_cleanup_last_run(state, now_ms()).await?;

    Ok(())
}

fn active_task_problem() -> Problem {
    Problem::new(
        StatusCode::CONFLICT,
        "Task is still active",
        "Chỉ có thể xóa đoạn trò chuyện sau khi task đã kết thúc.",
    )
}
