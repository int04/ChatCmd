use super::{
    CompactJob, conflict, db, missing,
    validation::{validate_conversation, validate_operation},
};
use crate::SqliteRepository;
use chatcmd_core::StorageError;
use sqlx::Row;
use uuid::Uuid;

impl SqliteRepository {
    /// Reserve a job and snapshot the old binding in one writer transaction.
    /// Repeated starts return the active job even while a request is running.
    pub async fn compact_start(
        &self,
        task_id: &str,
        operation_id: &str,
        now: i64,
    ) -> Result<CompactJob, StorageError> {
        self.compact_start_with_continuation(task_id, operation_id, false, now)
            .await
    }

    /// Reserve an immutable continuation choice with the initial job. A repeated
    /// start returns the existing choice and never opts in an already-running job.
    pub async fn compact_start_with_continuation(
        &self,
        task_id: &str,
        operation_id: &str,
        continue_after_compact: bool,
        now: i64,
    ) -> Result<CompactJob, StorageError> {
        validate_operation(operation_id)?;
        let mut tx = self
            .pool()
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(db)?;
        let replay: Option<CompactJob> =
            sqlx::query_as("SELECT * FROM chatgpt_compact_jobs WHERE start_operation_id=?")
                .bind(operation_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db)?;
        if let Some(job) = replay {
            if job.task_id != task_id {
                return Err(conflict("operation id belongs to another task"));
            }
            tx.commit().await.map_err(db)?;
            return Ok(job);
        }
        let active: Option<CompactJob> = sqlx::query_as("SELECT * FROM chatgpt_compact_jobs WHERE task_id=? AND phase NOT IN ('completed','cancelled')")
            .bind(task_id).fetch_optional(&mut *tx).await.map_err(db)?;
        if let Some(job) = active {
            tx.commit().await.map_err(db)?;
            return Ok(job);
        }
        let row = sqlx::query("SELECT c.conversation_id,c.conversation_url,c.model,t.conversation_scope_hash,t.active_session_id,t.project_folder,a.name AS agent_name,r.id AS request_id,CASE WHEN r.id IS NULL THEN NULL ELSE json_object('id',r.id,'taskId',r.task_id,'turnId',r.turn_id,'agentId',r.agent_id,'model',r.model,'userContent',r.user_content,'submittedContent',r.submitted_content,'projectFolder',r.project_folder,'status',r.status,'conversationId',r.conversation_id,'conversationUrl',r.conversation_url,'createdAtMs',r.created_at_ms) END AS request_params FROM chatgpt_conversations c JOIN tasks t ON t.id=c.task_id LEFT JOIN mcp_agents a ON a.id=t.agent_id LEFT JOIN chatgpt_bridge_requests r ON r.id=COALESCE(c.active_request_id,(SELECT id FROM chatgpt_bridge_requests WHERE task_id=t.id AND conversation_id=c.conversation_id ORDER BY created_at_ms DESC,id DESC LIMIT 1)) WHERE c.task_id=? AND t.source='chatgpt_web'")
            .bind(task_id).fetch_optional(&mut *tx).await.map_err(db)?.ok_or_else(missing)?;
        let conversation_id: String = row.get("conversation_id");
        let conversation_url: String = row.get("conversation_url");
        validate_conversation(&conversation_id, &conversation_url)?;
        let id = Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO chatgpt_compact_jobs(id,task_id,phase,revision,start_operation_id,old_conversation_id,old_conversation_url,old_model,old_request_id,old_scope_hash,old_active_session_id,old_request_params_json,agent_name,project_folder,continue_after_compact,created_at_ms,updated_at_ms) VALUES(?,?,'preparing',0,?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&id).bind(task_id).bind(operation_id).bind(&conversation_id).bind(&conversation_url)
            .bind(row.get::<String,_>("model")).bind(row.get::<Option<String>,_>("request_id"))
            .bind(row.get::<Option<String>,_>("conversation_scope_hash"))
            .bind(row.get::<Option<String>,_>("active_session_id"))
            .bind(row.get::<Option<String>,_>("request_params"))
            .bind(row.get::<Option<String>,_>("agent_name"))
            .bind(row.get::<Option<String>,_>("project_folder")).bind(continue_after_compact).bind(now).bind(now)
            .execute(&mut *tx).await.map_err(db)?;
        sqlx::query("INSERT INTO chatgpt_compact_operations(operation_id,job_id,revision,phase,created_at_ms) VALUES(?,?,0,'preparing',?)")
            .bind(operation_id).bind(&id).bind(now).execute(&mut *tx).await.map_err(db)?;
        // Existing browser work is asked to stop; it is not assumed to have stopped yet.
        sqlx::query("UPDATE chatgpt_bridge_requests SET status='stop_requested',updated_at_ms=? WHERE task_id=? AND status IN ('queued','running')")
            .bind(now).bind(task_id).execute(&mut *tx).await.map_err(db)?;
        sqlx::query("UPDATE chatgpt_message_queue SET mode='queued',updated_at_ms=? WHERE task_id=? AND mode='immediate'")
            .bind(now).bind(task_id).execute(&mut *tx).await.map_err(db)?;
        let job = sqlx::query_as("SELECT * FROM chatgpt_compact_jobs WHERE id=?")
            .bind(&id)
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(job)
    }
}
