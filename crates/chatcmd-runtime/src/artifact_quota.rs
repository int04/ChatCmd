//! Shared cumulative quota and startup retention for process artifacts.

use crate::{RuntimeError, RuntimeResult};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};
use tokio::sync::OnceCell;

const ARTIFACT_PREFIX: &str = "process-";
const ARTIFACT_SUFFIX: &str = ".output";

#[derive(Clone)]
pub(super) struct ArtifactQuota {
    directory: Arc<PathBuf>,
    max_total_bytes: u64,
    retention: Duration,
    initial_bytes: Arc<OnceCell<u64>>,
    usage: Arc<Mutex<QuotaUsage>>,
}

#[derive(Default)]
struct QuotaUsage {
    initialized: bool,
    committed_bytes: u64,
    reserved_bytes: u64,
}

pub(super) struct ArtifactReservation {
    usage: Arc<Mutex<QuotaUsage>>,
    reserved_bytes: u64,
    committed: bool,
}

impl ArtifactQuota {
    pub(super) fn new(directory: PathBuf, max_total_bytes: u64, retention: Duration) -> Self {
        Self {
            directory: Arc::new(directory),
            max_total_bytes,
            retention,
            initial_bytes: Arc::new(OnceCell::new()),
            usage: Arc::new(Mutex::new(QuotaUsage::default())),
        }
    }

    pub(super) async fn reserve(&self, requested_bytes: u64) -> RuntimeResult<ArtifactReservation> {
        let initial_bytes = *self
            .initial_bytes
            .get_or_try_init(|| async {
                scan_and_prune(
                    self.directory.as_ref(),
                    self.max_total_bytes,
                    self.retention,
                )
                .await
            })
            .await?;
        let mut usage = self.usage.lock().map_err(lock_error)?;
        if !usage.initialized {
            usage.initialized = true;
            usage.committed_bytes = initial_bytes;
        }
        let allocated = usage.committed_bytes.saturating_add(usage.reserved_bytes);
        if requested_bytes > self.max_total_bytes.saturating_sub(allocated) {
            return Err(RuntimeError::new(
                "artifact_quota_exceeded",
                "cumulative process artifact quota is exhausted",
            ));
        }
        usage.reserved_bytes = usage.reserved_bytes.saturating_add(requested_bytes);
        Ok(ArtifactReservation {
            usage: self.usage.clone(),
            reserved_bytes: requested_bytes,
            committed: false,
        })
    }
}

impl ArtifactReservation {
    pub(super) fn commit(mut self, retained_bytes: u64) {
        if let Ok(mut usage) = self.usage.lock() {
            usage.reserved_bytes = usage.reserved_bytes.saturating_sub(self.reserved_bytes);
            usage.committed_bytes = usage
                .committed_bytes
                .saturating_add(retained_bytes.min(self.reserved_bytes));
            self.committed = true;
        }
    }
}

impl Drop for ArtifactReservation {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        if let Ok(mut usage) = self.usage.lock() {
            usage.reserved_bytes = usage.reserved_bytes.saturating_sub(self.reserved_bytes);
        }
    }
}

async fn scan_and_prune(
    directory: &Path,
    max_total_bytes: u64,
    retention: Duration,
) -> RuntimeResult<u64> {
    tokio::fs::create_dir_all(directory)
        .await
        .map_err(io_error)?;
    let mut reader = tokio::fs::read_dir(directory).await.map_err(io_error)?;
    let now = SystemTime::now();
    let mut retained = Vec::new();
    while let Some(entry) = reader.next_entry().await.map_err(io_error)? {
        let path = entry.path();
        if !is_managed_artifact(&path) {
            continue;
        }
        let metadata = entry.metadata().await.map_err(io_error)?;
        if !metadata.is_file() {
            continue;
        }
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if now.duration_since(modified).unwrap_or_default() > retention {
            tokio::fs::remove_file(path).await.map_err(io_error)?;
        } else {
            retained.push((modified, metadata.len(), path));
        }
    }
    retained.sort_by_key(|(modified, _, _)| *modified);
    let mut total = retained
        .iter()
        .fold(0_u64, |sum, (_, bytes, _)| sum.saturating_add(*bytes));
    for (_, bytes, path) in retained {
        if total <= max_total_bytes {
            break;
        }
        tokio::fs::remove_file(path).await.map_err(io_error)?;
        total = total.saturating_sub(bytes);
    }
    Ok(total)
}

fn is_managed_artifact(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| name.starts_with(ARTIFACT_PREFIX) && name.ends_with(ARTIFACT_SUFFIX))
}

fn io_error(error: std::io::Error) -> RuntimeError {
    RuntimeError::new("artifact_storage_failed", error.to_string())
}

fn lock_error<T>(_: std::sync::PoisonError<T>) -> RuntimeError {
    RuntimeError::new(
        "artifact_quota_unavailable",
        "process artifact quota lock is poisoned",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reservations_enforce_cumulative_limit_and_release_unused_bytes() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let quota = ArtifactQuota::new(directory.path().to_owned(), 10, Duration::from_secs(3_600));
        let first = quota.reserve(7).await.expect("first reservation");
        let error = match quota.reserve(4).await {
            Err(error) => error,
            Ok(_) => panic!("quota reservation unexpectedly succeeded"),
        };
        assert_eq!(error.code, "artifact_quota_exceeded");
        drop(first);
        let second = quota.reserve(10).await.expect("released reservation");
        second.commit(6);
        let error = match quota.reserve(5).await {
            Err(error) => error,
            Ok(_) => panic!("committed quota reservation unexpectedly succeeded"),
        };
        assert_eq!(error.code, "artifact_quota_exceeded");
        quota.reserve(4).await.expect("remaining capacity");
    }

    #[tokio::test]
    async fn startup_prunes_oldest_managed_artifacts_but_keeps_other_files() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let first = directory.path().join("process-first.output");
        let second = directory.path().join("process-second.output");
        let unrelated = directory.path().join("keep.txt");
        tokio::fs::write(&first, b"123456").await.expect("first");
        tokio::time::sleep(Duration::from_millis(20)).await;
        tokio::fs::write(&second, b"123456").await.expect("second");
        tokio::fs::write(&unrelated, b"unrelated")
            .await
            .expect("unrelated");

        let quota = ArtifactQuota::new(directory.path().to_owned(), 8, Duration::from_secs(3_600));
        quota.reserve(2).await.expect("capacity after pruning");

        assert!(!first.exists());
        assert!(second.exists());
        assert!(unrelated.exists());
    }
}
