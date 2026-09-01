use chatcmd_storage::SqliteRepository;
use sqlx::Row as _;

#[derive(Clone, Debug)]
pub(crate) struct QueuedChatGptMessage {
    pub id: String,
    pub task_id: String,
    pub content: String,
    pub mode: String,
    pub sort_order: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

pub(crate) async fn list_messages(
    repository: &SqliteRepository,
    task_id: &str,
) -> Result<Vec<QueuedChatGptMessage>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id,task_id,content,mode,sort_order,created_at_ms,updated_at_ms FROM chatgpt_message_queue WHERE task_id=? ORDER BY sort_order,created_at_ms,id",
    )
    .bind(task_id)
    .fetch_all(repository.pool())
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| QueuedChatGptMessage {
            id: row.get("id"),
            task_id: row.get("task_id"),
            content: row.get("content"),
            mode: row.get("mode"),
            sort_order: row.get("sort_order"),
            created_at_ms: row.get("created_at_ms"),
            updated_at_ms: row.get("updated_at_ms"),
        })
        .collect())
}

pub(crate) async fn immediate_allowed(
    repository: &SqliteRepository,
    task_id: &str,
) -> Result<bool, sqlx::Error> {
    let value = sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM chatgpt_conversations c JOIN tasks t ON t.id=c.task_id JOIN chatgpt_bridge_requests r ON r.id=c.active_request_id WHERE c.task_id=? AND t.status='running' AND r.status IN ('queued','running','stop_requested'))",
    )
    .bind(task_id)
    .fetch_one(repository.pool())
    .await?;
    Ok(value != 0)
}

pub(crate) async fn demote_immediate_if_inactive(
    repository: &SqliteRepository,
    task_id: &str,
    now_ms: i64,
) -> Result<u64, sqlx::Error> {
    if immediate_allowed(repository, task_id).await? {
        return Ok(0);
    }
    Ok(sqlx::query(
        "UPDATE chatgpt_message_queue SET mode='queued',updated_at_ms=? WHERE task_id=? AND mode='immediate'",
    )
    .bind(now_ms)
    .bind(task_id)
    .execute(repository.pool())
    .await?
    .rows_affected())
}

pub(crate) async fn demote_all_immediate(
    repository: &SqliteRepository,
    task_id: &str,
    now_ms: i64,
) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query(
        "UPDATE chatgpt_message_queue SET mode='queued',updated_at_ms=? WHERE task_id=? AND mode='immediate'",
    )
    .bind(now_ms)
    .bind(task_id)
    .execute(repository.pool())
    .await?
    .rows_affected())
}

pub(crate) async fn claim_immediate(
    repository: &SqliteRepository,
    task_id: &str,
) -> Result<Vec<QueuedChatGptMessage>, sqlx::Error> {
    let mut transaction = repository.pool().begin().await?;
    let rows = sqlx::query(
        "SELECT id,task_id,content,mode,sort_order,created_at_ms,updated_at_ms FROM chatgpt_message_queue WHERE task_id=? AND mode='immediate' ORDER BY sort_order,created_at_ms,id",
    )
    .bind(task_id)
    .fetch_all(&mut *transaction)
    .await?;
    if rows.is_empty() {
        transaction.commit().await?;
        return Ok(Vec::new());
    }
    sqlx::query("DELETE FROM chatgpt_message_queue WHERE task_id=? AND mode='immediate'")
        .bind(task_id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(rows
        .into_iter()
        .map(|row| QueuedChatGptMessage {
            id: row.get("id"),
            task_id: row.get("task_id"),
            content: row.get("content"),
            mode: row.get("mode"),
            sort_order: row.get("sort_order"),
            created_at_ms: row.get("created_at_ms"),
            updated_at_ms: row.get("updated_at_ms"),
        })
        .collect())
}
