//! Durable, write-once lifecycle records for command idempotency.

use crate::{RuntimeError, RuntimeResult};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    collections::HashSet,
    fs::OpenOptions,
    io::Write as _,
    path::{Path, PathBuf},
};

const MAX_RECORD_BYTES: u64 = 8 * 1024 * 1024;
const MAX_JOURNAL_FILES: usize = 2_048;

#[derive(Clone)]
pub(super) struct CommandExecutionJournal {
    directory: PathBuf,
}

pub(super) struct JournalRecords<T> {
    pub(super) running: Vec<(String, T)>,
    pub(super) finished: Vec<(String, T)>,
}

impl CommandExecutionJournal {
    pub(super) fn new(artifact_directory: &Path) -> Self {
        Self {
            directory: artifact_directory.join("executions-v1"),
        }
    }

    pub(super) fn load<T: DeserializeOwned>(&self) -> RuntimeResult<JournalRecords<T>> {
        std::fs::create_dir_all(&self.directory).map_err(io_error)?;
        let mut finished_keys = HashSet::new();
        let mut finished_paths = Vec::new();
        let mut running_paths = Vec::new();
        for entry in std::fs::read_dir(&self.directory).map_err(io_error)? {
            let path = entry.map_err(io_error)?.path();
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if let Some(key) = name.strip_suffix(".finished.json") {
                validate_storage_key(key)?;
                finished_keys.insert(key.to_owned());
                finished_paths.push((key.to_owned(), path));
            } else if let Some(key) = name.strip_suffix(".running.json") {
                validate_storage_key(key)?;
                running_paths.push((key.to_owned(), path));
            }
        }
        if finished_paths.len().saturating_add(running_paths.len()) > MAX_JOURNAL_FILES {
            return Err(RuntimeError::new(
                "execution_journal_full",
                "command execution journal contains too many records",
            ));
        }
        let finished = finished_paths
            .into_iter()
            .map(|(key, path)| Ok((key, read_record(&path)?)))
            .collect::<RuntimeResult<Vec<_>>>()?;
        let running = running_paths
            .into_iter()
            .filter(|(key, _)| !finished_keys.contains(key))
            .map(|(key, path)| Ok((key, read_record(&path)?)))
            .collect::<RuntimeResult<Vec<_>>>()?;
        Ok(JournalRecords { running, finished })
    }

    pub(super) async fn write_running<T: Serialize>(
        &self,
        storage_key: &str,
        record: &T,
    ) -> RuntimeResult<()> {
        self.write_once(storage_key, "running", record).await
    }

    pub(super) async fn write_finished<T: Serialize>(
        &self,
        storage_key: &str,
        record: &T,
    ) -> RuntimeResult<()> {
        self.write_once(storage_key, "finished", record).await
    }

    pub(super) fn write_finished_blocking<T: Serialize>(
        &self,
        storage_key: &str,
        record: &T,
    ) -> RuntimeResult<()> {
        let bytes = encode_record(record)?;
        write_once_blocking(&self.record_path(storage_key, "finished")?, &bytes)
    }

    pub(super) fn remove(&self, storage_key: &str) -> RuntimeResult<()> {
        for state in ["running", "finished"] {
            let path = self.record_path(storage_key, state)?;
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(io_error(error)),
            }
        }
        Ok(())
    }

    async fn write_once<T: Serialize>(
        &self,
        storage_key: &str,
        state: &str,
        record: &T,
    ) -> RuntimeResult<()> {
        let bytes = encode_record(record)?;
        let path = self.record_path(storage_key, state)?;
        tokio::task::spawn_blocking(move || write_once_blocking(&path, &bytes))
            .await
            .map_err(|error| RuntimeError::new("execution_journal_failed", error.to_string()))?
    }

    fn record_path(&self, storage_key: &str, state: &str) -> RuntimeResult<PathBuf> {
        validate_storage_key(storage_key)?;
        Ok(self.directory.join(format!("{storage_key}.{state}.json")))
    }
}

fn encode_record<T: Serialize>(record: &T) -> RuntimeResult<Vec<u8>> {
    let bytes = serde_json::to_vec(record).map_err(|error| {
        RuntimeError::new(
            "execution_journal_failed",
            format!("encode record: {error}"),
        )
    })?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_RECORD_BYTES {
        return Err(RuntimeError::new(
            "execution_record_too_large",
            "command execution record exceeds its durable size limit",
        ));
    }
    Ok(bytes)
}

fn read_record<T: DeserializeOwned>(path: &Path) -> RuntimeResult<T> {
    let metadata = std::fs::metadata(path).map_err(io_error)?;
    if metadata.len() > MAX_RECORD_BYTES {
        return Err(RuntimeError::new(
            "execution_record_too_large",
            "persisted command execution record exceeds its size limit",
        ));
    }
    let bytes = std::fs::read(path).map_err(io_error)?;
    serde_json::from_slice(&bytes).map_err(|error| {
        RuntimeError::new(
            "execution_journal_corrupt",
            format!("persisted command execution record is invalid: {error}"),
        )
    })
}

fn write_once_blocking(path: &Path, bytes: &[u8]) -> RuntimeResult<()> {
    let parent = path.parent().ok_or_else(|| {
        RuntimeError::new("execution_journal_failed", "journal path has no parent")
    })?;
    std::fs::create_dir_all(parent).map_err(io_error)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io_error)?;
    file.write_all(bytes).map_err(io_error)?;
    file.sync_all().map_err(io_error)
}

fn validate_storage_key(key: &str) -> RuntimeResult<()> {
    if key.len() == 64 && key.bytes().all(|value| value.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(RuntimeError::new(
            "execution_journal_corrupt",
            "command execution journal contains an invalid record name",
        ))
    }
}

fn io_error(error: std::io::Error) -> RuntimeError {
    RuntimeError::new("execution_journal_failed", error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
    struct TestRecord {
        value: String,
    }

    #[tokio::test]
    async fn records_are_write_once_and_finished_supersedes_running() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let journal = CommandExecutionJournal::new(directory.path());
        let key = "a".repeat(64);
        journal
            .write_running(
                &key,
                &TestRecord {
                    value: "run".into(),
                },
            )
            .await
            .expect("running record");
        journal
            .write_finished(
                &key,
                &TestRecord {
                    value: "done".into(),
                },
            )
            .await
            .expect("finished record");

        let records = journal.load::<TestRecord>().expect("load records");
        assert!(records.running.is_empty());
        assert_eq!(records.finished[0].1.value, "done");
        assert!(
            journal
                .write_finished(
                    &key,
                    &TestRecord {
                        value: "again".into()
                    }
                )
                .await
                .is_err()
        );
    }
}
