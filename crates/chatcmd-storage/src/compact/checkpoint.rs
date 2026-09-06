use super::validation::{validate_checkpoint, validate_operation};
use super::{
    CompactCheckpoint, CompactJob, CompactPhase, conflict, db, invalid, missing, openai_scope,
    validate_conversation,
};
use crate::SqliteRepository;
use chatcmd_core::StorageError;
use sqlx::{Row, SqliteConnection};

impl SqliteRepository {
    /// Compare-and-swap: even an identical retry with a stale revision is a 409.
    /// The caller recovers an uncertain response using compact_job, never by guessing.
    pub async fn compact_checkpoint(
        &self,
        id: &str,
        input: &CompactCheckpoint,
        operation_id: &str,
        now: i64,
    ) -> Result<CompactJob, StorageError> {
        validate_checkpoint(input)?;
        validate_operation(operation_id)?;
        let mut tx = self
            .pool()
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(db)?;
        let mut job: CompactJob = sqlx::query_as("SELECT * FROM chatgpt_compact_jobs WHERE id=?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?
            .ok_or_else(missing)?;
        if job.revision != input.expected_revision {
            return Err(conflict(
                "stale compact revision; reload the job before retrying",
            ));
        }
        if job.phase.is_terminal() {
            let saved = terminal_detail(&mut tx, &job, input, operation_id, now).await?;
            tx.commit().await.map_err(db)?;
            return Ok(saved);
        }
        let phase = input.phase.unwrap_or(job.phase);
        if phase.rank() < job.phase.rank() {
            return Err(conflict("compact phases cannot move backwards"));
        }
        if matches!(
            phase,
            CompactPhase::OpeningNewChat | CompactPhase::Completed
        ) && job
            .handoff_text
            .as_ref()
            .is_none_or(|text| text.trim().is_empty())
        {
            return Err(conflict(
                "save the handoff in a prior checkpoint before opening a new chat",
            ));
        }
        if phase == CompactPhase::Completed && job.phase != CompactPhase::OpeningNewChat {
            return Err(conflict(
                "completion requires an opening_new_chat checkpoint",
            ));
        }
        if let Some(text) = &input.handoff_text {
            if job.phase.rank() >= CompactPhase::OpeningNewChat.rank()
                && job.handoff_text.as_ref() != Some(text)
            {
                return Err(conflict(
                    "the handoff cannot change after opening the new chat",
                ));
            }
            job.handoff_text = Some(text.clone());
        }
        if let Some(detail) = &input.detail {
            job.detail = detail.clone();
        }
        let destination_id = input
            .new_conversation_id
            .as_ref()
            .or(job.new_conversation_id.as_ref())
            .cloned();
        let destination_url = input
            .new_conversation_url
            .as_ref()
            .or(job.new_conversation_url.as_ref())
            .cloned();
        match (&destination_id, &destination_url) {
            (Some(new_id), Some(url)) => {
                validate_conversation(new_id, url)?;
                if new_id == &job.old_conversation_id {
                    return Err(conflict("a new conversation is required"));
                }
                if job
                    .new_conversation_id
                    .as_ref()
                    .is_some_and(|id| id != new_id)
                    || job
                        .new_conversation_url
                        .as_ref()
                        .is_some_and(|old_url| old_url != url)
                {
                    return Err(conflict("the reserved destination cannot be rebound"));
                }
                if phase != CompactPhase::Cancelled {
                    ensure_destination(&mut tx, &job, new_id).await?;
                }
            }
            (None, None) if phase != CompactPhase::Completed => {}
            _ => {
                return Err(invalid(
                    "a matching newConversationId and newConversationUrl pair is required",
                ));
            }
        }
        // Re-read under the writer reservation, not using the browser's task identity.
        let binding_matches: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM chatgpt_conversations c JOIN tasks t ON t.id=c.task_id WHERE c.task_id=? AND c.conversation_id=? AND c.conversation_url=? AND t.conversation_scope_hash IS ?)")
            .bind(&job.task_id).bind(&job.old_conversation_id).bind(&job.old_conversation_url).bind(&job.old_scope_hash)
            .fetch_one(&mut *tx).await.map_err(db)?;
        if !binding_matches && phase != CompactPhase::Cancelled {
            return Err(conflict("the source task binding changed during compact"));
        }
        let revision = job
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("revision overflow"))?;
        let timestamp = now.max(job.updated_at_ms);
        let finished_at = phase.is_terminal().then_some(timestamp);
        let new_scope = destination_id.as_deref().map(openai_scope);
        let changed = sqlx::query("UPDATE chatgpt_compact_jobs SET phase=?,revision=?,handoff_text=?,new_conversation_id=?,new_conversation_url=?,new_scope_hash=?,detail=?,updated_at_ms=?,completed_at_ms=? WHERE id=? AND revision=? AND phase NOT IN ('completed','cancelled')")
            .bind(phase.as_str()).bind(revision).bind(&job.handoff_text).bind(&destination_id).bind(&destination_url)
            .bind(&new_scope).bind(&job.detail).bind(timestamp).bind(finished_at).bind(id).bind(input.expected_revision)
            .execute(&mut *tx).await.map_err(db)?.rows_affected();
        if changed != 1 {
            return Err(conflict("stale compact revision"));
        }
        sqlx::query("INSERT INTO chatgpt_compact_operations(operation_id,job_id,revision,phase,created_at_ms) VALUES(?,?,?,?,?)")
            .bind(operation_id).bind(id).bind(revision).bind(phase.as_str()).bind(timestamp)
            .execute(&mut *tx).await.map_err(db)?;
        job.new_conversation_id = destination_id;
        job.new_conversation_url = destination_url;
        if phase == CompactPhase::Completed {
            complete(&mut tx, &job, timestamp).await?;
        }
        let saved = sqlx::query_as("SELECT * FROM chatgpt_compact_jobs WHERE id=?")
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(saved)
    }
}

async fn ensure_destination(
    conn: &mut SqliteConnection,
    job: &CompactJob,
    id: &str,
) -> Result<(), StorageError> {
    let scope = openai_scope(id);
    let occupied: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM chatgpt_conversations WHERE conversation_id=? AND task_id<>?) OR EXISTS(SELECT 1 FROM tasks WHERE conversation_scope_hash=? AND id<>?) OR EXISTS(SELECT 1 FROM chatgpt_compact_archives WHERE conversation_id=? OR scope_hash=?) OR EXISTS(SELECT 1 FROM chatgpt_compact_jobs WHERE new_conversation_id=? AND id<>? AND phase NOT IN ('completed','cancelled'))")
        .bind(id).bind(&job.task_id).bind(&scope).bind(&job.task_id).bind(id).bind(&scope).bind(id).bind(&job.id)
        .fetch_one(conn).await.map_err(db)?;
    if occupied {
        return Err(conflict(
            "destination conversation is already owned or archived",
        ));
    }
    Ok(())
}

async fn complete(
    conn: &mut SqliteConnection,
    job: &CompactJob,
    now: i64,
) -> Result<(), StorageError> {
    let new_id = job
        .new_conversation_id
        .as_deref()
        .ok_or_else(|| invalid("destination missing"))?;
    let new_url = job
        .new_conversation_url
        .as_deref()
        .ok_or_else(|| invalid("destination missing"))?;
    let row = sqlx::query("SELECT COALESCE(t.active_session_id,j.old_active_session_id) AS session_id FROM tasks t JOIN chatgpt_compact_jobs j ON j.task_id=t.id WHERE j.id=?")
        .bind(&job.id).fetch_one(&mut *conn).await.map_err(db)?;
    sqlx::query("INSERT INTO chatgpt_compact_archives(conversation_id,scope_hash,previous_scope_hash,old_session_id,task_id,job_id,created_at_ms) VALUES(?,?,?,?,?,?,?)")
        .bind(&job.old_conversation_id).bind(openai_scope(&job.old_conversation_id)).bind(&job.old_scope_hash)
        .bind(row.get::<Option<String>,_>("session_id")).bind(&job.task_id).bind(&job.id).bind(now)
        .execute(&mut *conn).await.map_err(db)?;
    sqlx::query("UPDATE chatgpt_bridge_requests SET status='stopped',updated_at_ms=?,completed_at_ms=COALESCE(completed_at_ms,?) WHERE task_id=? AND status IN ('queued','running','stop_requested')")
        .bind(now).bind(now).bind(&job.task_id).execute(&mut *conn).await.map_err(db)?;
    sqlx::query("INSERT OR IGNORE INTO chatgpt_compact_obsolete_requests(request_id,job_id) SELECT id,? FROM chatgpt_bridge_requests WHERE task_id=?")
        .bind(&job.id).bind(&job.task_id).execute(&mut *conn).await.map_err(db)?;
    sqlx::query("UPDATE chatgpt_conversations SET conversation_id=?,conversation_url=?,active_request_id=NULL,updated_at_ms=? WHERE task_id=? AND conversation_id=?")
        .bind(new_id).bind(new_url).bind(now).bind(&job.task_id).bind(&job.old_conversation_id)
        .execute(&mut *conn).await.map_err(db)?;
    // Never replace the task: title, project, agent, permissions and history remain intact.
    sqlx::query("UPDATE tasks SET conversation_scope_hash=?,active_session_id=NULL,generation=generation+1,status=CASE WHEN status='running' THEN 'interrupted' ELSE status END,updated_at_ms=? WHERE id=?")
        .bind(openai_scope(new_id)).bind(now).bind(&job.task_id).execute(&mut *conn).await.map_err(db)?;
    sqlx::query("UPDATE task_sessions SET status='interrupted',updated_at_ms=? WHERE task_id=? AND status IN ('starting','running')")
        .bind(now).bind(&job.task_id).execute(conn).await.map_err(db)?;
    Ok(())
}

// Terminal binding/handoff state is immutable, but same-phase detail updates are
// still legal under CAS (for example clearing a worker diagnostic after recovery).
async fn terminal_detail(
    conn: &mut SqliteConnection,
    job: &CompactJob,
    input: &CompactCheckpoint,
    operation_id: &str,
    now: i64,
) -> Result<CompactJob, StorageError> {
    if input.phase.is_some_and(|phase| phase != job.phase)
        || input.handoff_text.is_some()
        || input.new_conversation_id.is_some()
        || input.new_conversation_url.is_some()
    {
        return Err(conflict("terminal compact binding cannot change"));
    }
    let detail = input
        .detail
        .as_ref()
        .ok_or_else(|| conflict("compact job is already terminal"))?;
    let revision = job.revision + 1;
    let timestamp = now.max(job.updated_at_ms);
    sqlx::query("UPDATE chatgpt_compact_jobs SET detail=?,revision=?,updated_at_ms=? WHERE id=? AND revision=?")
        .bind(detail).bind(revision).bind(timestamp).bind(&job.id).bind(job.revision)
        .execute(&mut *conn).await.map_err(db)?;
    sqlx::query("INSERT INTO chatgpt_compact_operations(operation_id,job_id,revision,phase,created_at_ms) VALUES(?,?,?,?,?)")
        .bind(operation_id).bind(&job.id).bind(revision).bind(job.phase.as_str()).bind(timestamp)
        .execute(&mut *conn).await.map_err(db)?;
    sqlx::query_as("SELECT * FROM chatgpt_compact_jobs WHERE id=?")
        .bind(&job.id)
        .fetch_one(conn)
        .await
        .map_err(db)
}
