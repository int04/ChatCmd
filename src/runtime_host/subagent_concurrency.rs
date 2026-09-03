use std::time::Duration;

use chatcmd_core::SettingsStore as _;
use chatcmd_runtime::{RuntimeError, RuntimeResult};
use tokio::time::sleep;

use super::RuntimeHost;

pub(super) const DEFAULT_SUBAGENT_CONCURRENCY: i64 = 2;
pub(super) const MIN_SUBAGENT_CONCURRENCY: i64 = 1;
pub(super) const MAX_SUBAGENT_CONCURRENCY: i64 = 5;
const SLOT_RECHECK_DELAY: Duration = Duration::from_millis(100);

impl RuntimeHost {
    pub(super) async fn subagent_concurrency_limit(&self) -> RuntimeResult<i64> {
        let setting = self
            .repository
            .setting("ui_subagentConcurrency")
            .await
            .map_err(|_| {
                RuntimeError::new("storage_error", "sub-agent concurrency setting unavailable")
            })?;
        let value = setting
            .and_then(|setting| serde_json::from_str::<i64>(&setting.value_json).ok())
            .unwrap_or(DEFAULT_SUBAGENT_CONCURRENCY);
        Ok(value.clamp(MIN_SUBAGENT_CONCURRENCY, MAX_SUBAGENT_CONCURRENCY))
    }

    pub(super) async fn active_subagent_count(&self) -> RuntimeResult<i64> {
        self.expire_stale_subagents(None).await?;
        let now = super::now_ms();
        let native_cutoff = now.saturating_sub(60_000);
        let fallback_cutoff = now.saturating_sub(180_000);
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM subagent_runs WHERE (status='running' AND lease_expires_at_ms>?) OR (status='pending' AND ((fallback_state='none' AND created_at_ms>?) OR (fallback_state IN ('requested','started') AND updated_at_ms>?)))",
        )
        .bind(now)
        .bind(native_cutoff)
        .bind(fallback_cutoff)
        .fetch_one(self.repository.pool())
        .await
        .map_err(|_| RuntimeError::new("storage_error", "active sub-agent count unavailable"))
    }

    pub(super) async fn wait_before_retrying_subagent_slot(&self) {
        sleep(SLOT_RECHECK_DELAY).await;
    }
}
