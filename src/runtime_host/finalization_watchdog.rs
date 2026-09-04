use std::{sync::Arc, time::Duration};

use chatcmd_runtime::{RuntimeError, RuntimeResult};
use serde_json::json;
use sqlx::Row as _;
use tokio::task::JoinHandle;
use tracing::warn;
use uuid::Uuid;

use super::{RuntimeHost, now_ms};

const DEFAULT_FINALIZATION_GRACE_SECONDS: u64 = 120;
const MIN_FINALIZATION_GRACE_SECONDS: u64 = 30;
const MAX_FINALIZATION_GRACE_SECONDS: u64 = 3_600;
const WATCHDOG_INTERVAL_SECONDS: u64 = 5;
const STALE_TASK_LIMIT: i64 = 100;

impl RuntimeHost {
    pub(crate) fn start_finalization_watchdog(self: &Arc<Self>) -> JoinHandle<()> {
        let host = Arc::clone(self);
        tokio::spawn(async move {
            let grace = finalization_grace();
            let mut interval =
                tokio::time::interval(Duration::from_secs(WATCHDOG_INTERVAL_SECONDS));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                if let Err(error) = host.expire_stale_subagents(None).await {
                    warn!(code = %error.code, message = %error.message, "sub-agent lease watchdog failed");
                }
                if let Err(error) = host.auto_finalize_stale_turns(grace).await {
                    warn!(code = %error.code, message = %error.message, "finalization watchdog failed");
                }
            }
        })
    }

    async fn auto_finalize_stale_turns(&self, grace: Duration) -> RuntimeResult<()> {
        let grace_ms = i64::try_from(grace.as_millis()).unwrap_or(i64::MAX);
        let cutoff = now_ms().saturating_sub(grace_ms);
        let rows = sqlx::query(
            "SELECT id,active_session_id,updated_at_ms FROM tasks WHERE status='running' AND updated_at_ms<=? ORDER BY updated_at_ms,id LIMIT ?",
        )
        .bind(cutoff)
        .bind(STALE_TASK_LIMIT)
        .fetch_all(self.repository.pool())
        .await
        .map_err(watchdog_storage_error)?;

        for row in rows {
            let task_id = row.get::<String, _>("id");
            let active_session_id = row.get::<Option<String>, _>("active_session_id");
            let Some(latest) = sqlx::query(
                "SELECT turn_id,session_id,created_at_ms FROM timeline_events WHERE task_id=? AND turn_id IS NOT NULL ORDER BY created_at_ms DESC,event_id DESC LIMIT 1",
            )
            .bind(&task_id)
            .fetch_optional(self.repository.pool())
            .await
            .map_err(watchdog_storage_error)?
            else {
                continue;
            };
            let Some(turn_id) = latest.get::<Option<String>, _>("turn_id") else {
                continue;
            };
            if latest.get::<i64, _>("created_at_ms") > cutoff
                || self.activities.has_active_turn(&task_id, &turn_id)
                || self.has_active_subagent_work(&task_id, &turn_id).await?
            {
                continue;
            }
            let session_id = latest
                .get::<Option<String>, _>("session_id")
                .or_else(|| active_session_id.clone());
            self.auto_finalize_turn(&task_id, &turn_id, session_id.as_deref(), cutoff)
                .await?;
        }
        Ok(())
    }

    async fn has_active_subagent_work(&self, task_id: &str, turn_id: &str) -> RuntimeResult<bool> {
        self.expire_stale_subagents(None).await?;
        let active = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM subagent_runs WHERE status IN ('pending','running') AND ((parent_task_id=? AND parent_turn_id=?) OR child_task_id=?))",
        )
        .bind(task_id)
        .bind(turn_id)
        .bind(task_id)
        .fetch_one(self.repository.pool())
        .await
        .map_err(watchdog_storage_error)?;
        Ok(active != 0)
    }

    async fn auto_finalize_turn(
        &self,
        task_id: &str,
        turn_id: &str,
        session_id: Option<&str>,
        cutoff: i64,
    ) -> RuntimeResult<()> {
        let now = now_ms();
        let event_id = format!("finalization-watchdog-{}", Uuid::new_v4());
        let payload = json!({
            "tool": "agent_turn_complete",
            "status": "completed",
            "autoFinalized": true,
            "finalizerMissing": true,
            "reason": "finalizer_timeout"
        });
        let mut tx = self
            .repository
            .pool()
            .begin()
            .await
            .map_err(watchdog_storage_error)?;
        let updated = sqlx::query(
            "UPDATE tasks SET status='completed',updated_at_ms=? WHERE id=? AND status='running' AND updated_at_ms<=?",
        )
        .bind(now)
        .bind(task_id)
        .bind(cutoff)
        .execute(&mut *tx)
        .await
        .map_err(watchdog_storage_error)?;
        if updated.rows_affected() == 0 {
            tx.rollback().await.map_err(watchdog_storage_error)?;
            return Ok(());
        }
        sqlx::query(
            "INSERT INTO timeline_events(event_id,task_id,turn_id,session_id,actor,kind,idempotency_key,payload_json,metadata_json,created_at_ms) VALUES(?,?,?,?,?,'status',?,?,NULL,?)",
        )
        .bind(&event_id)
        .bind(task_id)
        .bind(turn_id)
        .bind(session_id)
        .bind("assistant")
        .bind(&event_id)
        .bind(payload.to_string())
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(watchdog_storage_error)?;
        tx.commit().await.map_err(watchdog_storage_error)?;

        if let Err(error) = self
            .reconcile_orphaned_tool_calls(task_id, turn_id, session_id, "finalizer_timeout", now)
            .await
        {
            tracing::warn!(
                task_id,
                turn_id,
                error = ?error,
                "failed to reconcile orphaned tool calls after watchdog completion"
            );
        }
        self.publish_event(
            event_id,
            "status",
            Some(task_id.to_owned()),
            session_id.map(str::to_owned),
            Some(turn_id.to_owned()),
            payload,
        );
        Ok(())
    }
}

fn finalization_grace() -> Duration {
    let seconds = std::env::var("CHATCMD_FINALIZATION_GRACE_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_FINALIZATION_GRACE_SECONDS)
        .clamp(
            MIN_FINALIZATION_GRACE_SECONDS,
            MAX_FINALIZATION_GRACE_SECONDS,
        );
    Duration::from_secs(seconds)
}

fn watchdog_storage_error(_: sqlx::Error) -> RuntimeError {
    RuntimeError::new(
        "storage_error",
        "finalization watchdog storage operation failed",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_grace_is_safe_for_slow_final_responses() {
        assert_eq!(DEFAULT_FINALIZATION_GRACE_SECONDS, 120);
        const { assert!(DEFAULT_FINALIZATION_GRACE_SECONDS >= MIN_FINALIZATION_GRACE_SECONDS) };
    }
}
