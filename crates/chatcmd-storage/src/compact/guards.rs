use super::{conflict, db};
use chatcmd_core::StorageError;
use sqlx::SqliteConnection;

/// Call inside the same writer transaction as callback persistence. This is
/// deliberately scoped to compact jobs: ordinary late identity recovery remains valid.
pub async fn guard_callback(
    conn: &mut SqliteConnection,
    request_id: &str,
    conversation_id: Option<&str>,
) -> Result<(), StorageError> {
    let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM chatgpt_compact_obsolete_requests WHERE request_id=?) OR EXISTS(SELECT 1 FROM chatgpt_compact_archives WHERE conversation_id=?) OR EXISTS(SELECT 1 FROM chatgpt_bridge_requests r JOIN chatgpt_compact_jobs j ON j.task_id=r.task_id WHERE r.id=? AND j.phase NOT IN ('completed','cancelled')) OR EXISTS(SELECT 1 FROM chatgpt_bridge_requests r JOIN chatgpt_conversations c ON c.task_id=r.task_id WHERE r.id=? AND EXISTS(SELECT 1 FROM chatgpt_compact_jobs j WHERE j.task_id=r.task_id AND j.phase='completed') AND ((? IS NOT NULL AND c.conversation_id<>?) OR (r.conversation_id IS NOT NULL AND r.conversation_id<>c.conversation_id)))")
        .bind(request_id).bind(conversation_id).bind(request_id).bind(request_id)
        .bind(conversation_id).bind(conversation_id).fetch_one(conn).await.map_err(db)?;
    if blocked {
        return Err(conflict(
            "compact owns this binding; obsolete browser callbacks are rejected",
        ));
    }
    Ok(())
}

/// Before finding a prior native request or creating a recorder task. Merely
/// reopening archived history must not enroll it again. Reserved seed chats are
/// also invisible to ordinary capture until the compact worker confirms completion.
pub async fn guard_native(
    conn: &mut SqliteConnection,
    conversation_id: &str,
) -> Result<(), StorageError> {
    let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM chatgpt_compact_archives WHERE conversation_id=?) OR EXISTS(SELECT 1 FROM chatgpt_compact_jobs WHERE phase NOT IN ('completed','cancelled') AND (old_conversation_id=? OR new_conversation_id=?))")
        .bind(conversation_id).bind(conversation_id).bind(conversation_id)
        .fetch_one(conn).await.map_err(db)?;
    if blocked {
        return Err(conflict(
            "archived or compact-owned conversations cannot enroll browser turns",
        ));
    }
    Ok(())
}
