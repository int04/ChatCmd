use super::{CompactJob, CompactTaskJobs, SUMMARY_COLUMNS, db, missing};
use crate::SqliteRepository;
use chatcmd_core::StorageError;

impl SqliteRepository {
    /// The single-job endpoint is the recovery channel and includes the handoff.
    pub async fn compact_job(&self, id: &str) -> Result<CompactJob, StorageError> {
        sqlx::query_as("SELECT * FROM chatgpt_compact_jobs WHERE id=?")
            .bind(id)
            .fetch_optional(self.pool())
            .await
            .map_err(db)?
            .ok_or_else(missing)
    }

    pub async fn compact_task_jobs(&self, task_id: &str) -> Result<CompactTaskJobs, StorageError> {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tasks WHERE id=?)")
            .bind(task_id)
            .fetch_one(self.pool())
            .await
            .map_err(db)?;
        if !exists {
            return Err(missing());
        }
        let query = format!(
            "SELECT {SUMMARY_COLUMNS} FROM chatgpt_compact_jobs WHERE task_id=? ORDER BY created_at_ms DESC,id DESC"
        );
        let jobs: Vec<CompactJob> = sqlx::query_as(&query)
            .bind(task_id)
            .fetch_all(self.pool())
            .await
            .map_err(db)?;
        let mut result = CompactTaskJobs {
            active: None,
            history: Vec::new(),
        };
        for job in jobs {
            if job.phase.is_terminal() {
                result.history.push(job);
            } else {
                result.active = Some(job);
            }
        }
        Ok(result)
    }

    pub async fn compact_pending(&self) -> Result<Vec<CompactJob>, StorageError> {
        let query = format!(
            "SELECT {SUMMARY_COLUMNS} FROM chatgpt_compact_jobs WHERE phase NOT IN ('completed','cancelled') ORDER BY created_at_ms,id"
        );
        sqlx::query_as(&query)
            .fetch_all(self.pool())
            .await
            .map_err(db)
    }

    pub async fn compact_is_active(&self, task_id: &str) -> Result<bool, StorageError> {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM chatgpt_compact_jobs WHERE task_id=? AND phase NOT IN ('completed','cancelled'))")
            .bind(task_id).fetch_one(self.pool()).await.map_err(db)
    }

    /// Useful to fail closed before MCP identity selection. The database triggers
    /// remain the final race-safe guard for callers that have already read state.
    pub async fn compact_scope_archived(&self, scope: &str) -> Result<bool, StorageError> {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM chatgpt_compact_archives WHERE scope_hash=? OR previous_scope_hash=?)")
            .bind(scope).bind(scope).fetch_one(self.pool()).await.map_err(db)
    }
}
