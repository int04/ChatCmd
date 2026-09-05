//! Durable ownership and idempotency registry for command executions.

use crate::{
    CommandExecutionResult, CommandIdentity, CommandTerminalState, RuntimeError, RuntimeResult,
    command_execution_journal::CommandExecutionJournal,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::Notify;

const JOURNAL_SCHEMA_VERSION: u16 = 1;
const MAX_RECORDS: usize = 512;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ExecutionKey {
    pub(super) task_id: String,
    pub(super) agent_id: String,
    pub(super) idempotency_key: String,
}

#[derive(Clone)]
pub(super) struct CommandExecutionRegistry {
    records: Arc<Mutex<ExecutionRecords>>,
    journal: CommandExecutionJournal,
    startup_error: Option<RuntimeError>,
}

struct ExecutionRecords {
    entries: HashMap<ExecutionKey, ExecutionEntry>,
    by_id: HashMap<String, ExecutionKey>,
    completed_order: VecDeque<ExecutionKey>,
}

struct ExecutionEntry {
    execution_id: String,
    request_digest: String,
    state: ExecutionState,
}

enum ExecutionState {
    Running(Arc<Notify>),
    Finished(Box<CommandExecutionResult>),
}

pub(super) enum Claim {
    Run { execution_id: String },
    Wait { notify: Arc<Notify> },
    Finished(Box<CommandExecutionResult>),
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase", deny_unknown_fields)]
enum PersistedExecutionRecord {
    Running {
        schema_version: u16,
        key: ExecutionKey,
        execution_id: String,
        request_digest: String,
        command: CommandIdentity,
        cwd: PathBuf,
        started_at_unix_ms: u64,
    },
    Finished {
        schema_version: u16,
        key: ExecutionKey,
        request_digest: String,
        result: Box<CommandExecutionResult>,
    },
}

impl CommandExecutionRegistry {
    pub(super) fn new(artifact_directory: &std::path::Path) -> Self {
        let journal = CommandExecutionJournal::new(artifact_directory);
        match load_records(&journal) {
            Ok(records) => Self {
                records: Arc::new(Mutex::new(records)),
                journal,
                startup_error: None,
            },
            Err(error) => Self {
                records: Arc::new(Mutex::new(empty_records())),
                journal,
                startup_error: Some(error),
            },
        }
    }

    pub(super) async fn claim(
        &self,
        key: &ExecutionKey,
        request_digest: &str,
        command: CommandIdentity,
        cwd: PathBuf,
    ) -> RuntimeResult<Claim> {
        self.ensure_available()?;
        let execution_id = {
            let mut records = self.records.lock().map_err(lock_error)?;
            if let Some(entry) = records.entries.get(key) {
                if entry.request_digest != request_digest {
                    return Err(RuntimeError::new(
                        "idempotency_conflict",
                        "idempotency key was already used for a different command",
                    ));
                }
                return Ok(match &entry.state {
                    ExecutionState::Running(notify) => Claim::Wait {
                        notify: notify.clone(),
                    },
                    ExecutionState::Finished(result) => Claim::Finished(result.clone()),
                });
            }
            evict_completed(&mut records, &self.journal)?;
            if records.entries.len() >= MAX_RECORDS {
                return Err(RuntimeError::busy("command execution registry is full"));
            }
            let execution_id = uuid::Uuid::new_v4().to_string();
            records.by_id.insert(execution_id.clone(), key.clone());
            records.entries.insert(
                key.clone(),
                ExecutionEntry {
                    execution_id: execution_id.clone(),
                    request_digest: request_digest.to_owned(),
                    state: ExecutionState::Running(Arc::new(Notify::new())),
                },
            );
            execution_id
        };
        let record = PersistedExecutionRecord::Running {
            schema_version: JOURNAL_SCHEMA_VERSION,
            key: key.clone(),
            execution_id: execution_id.clone(),
            request_digest: request_digest.to_owned(),
            command,
            cwd,
            started_at_unix_ms: now_unix_ms(),
        };
        let storage_key = storage_key(key)?;
        if let Err(error) = self.journal.write_running(&storage_key, &record).await {
            self.rollback_claim(key, &execution_id)?;
            return Err(error);
        }
        Ok(Claim::Run { execution_id })
    }

    pub(super) fn finished(
        &self,
        key: &ExecutionKey,
    ) -> RuntimeResult<Option<CommandExecutionResult>> {
        self.ensure_available()?;
        let records = self.records.lock().map_err(lock_error)?;
        Ok(records
            .entries
            .get(key)
            .and_then(|entry| match &entry.state {
                ExecutionState::Running(_) => None,
                ExecutionState::Finished(result) => Some((**result).clone()),
            }))
    }

    pub(super) async fn finish(
        &self,
        key: &ExecutionKey,
        result: CommandExecutionResult,
    ) -> RuntimeResult<()> {
        self.ensure_available()?;
        let request_digest = {
            let records = self.records.lock().map_err(lock_error)?;
            let entry = records.entries.get(key).ok_or_else(not_found)?;
            if matches!(entry.state, ExecutionState::Finished(_)) {
                return Ok(());
            }
            entry.request_digest.clone()
        };
        let record = PersistedExecutionRecord::Finished {
            schema_version: JOURNAL_SCHEMA_VERSION,
            key: key.clone(),
            request_digest,
            result: Box::new(result.clone()),
        };
        let persist_result = self
            .journal
            .write_finished(&storage_key(key)?, &record)
            .await;
        let mut records = self.records.lock().map_err(lock_error)?;
        let entry = records.entries.get_mut(key).ok_or_else(not_found)?;
        let notify = match &entry.state {
            ExecutionState::Running(notify) => notify.clone(),
            ExecutionState::Finished(_) => return persist_result,
        };
        debug_assert_eq!(entry.execution_id, result.execution_id);
        entry.state = ExecutionState::Finished(Box::new(result));
        records.completed_order.push_back(key.clone());
        notify.notify_waiters();
        persist_result
    }

    pub(super) fn result(
        &self,
        task_id: &str,
        agent_id: &str,
        execution_id: &str,
    ) -> RuntimeResult<CommandExecutionResult> {
        self.ensure_available()?;
        let records = self.records.lock().map_err(lock_error)?;
        let key = records.by_id.get(execution_id).ok_or_else(not_found)?;
        if key.task_id != task_id || key.agent_id != agent_id {
            return Err(not_found());
        }
        match &records.entries.get(key).ok_or_else(not_found)?.state {
            ExecutionState::Running(_) => Err(RuntimeError::new(
                "execution_running",
                "command execution has not reached a terminal state",
            )),
            ExecutionState::Finished(result) => Ok((**result).clone()),
        }
    }

    fn rollback_claim(&self, key: &ExecutionKey, execution_id: &str) -> RuntimeResult<()> {
        let mut records = self.records.lock().map_err(lock_error)?;
        if records
            .entries
            .get(key)
            .is_some_and(|entry| entry.execution_id == execution_id)
        {
            if let Some(ExecutionEntry {
                state: ExecutionState::Running(notify),
                ..
            }) = records.entries.remove(key)
            {
                notify.notify_waiters();
            }
            records.by_id.remove(execution_id);
        }
        Ok(())
    }

    fn ensure_available(&self) -> RuntimeResult<()> {
        self.startup_error.clone().map_or(Ok(()), Err)
    }
}

fn load_records(journal: &CommandExecutionJournal) -> RuntimeResult<ExecutionRecords> {
    let persisted = journal.load::<PersistedExecutionRecord>()?;
    let mut records = empty_records();
    for (storage_key, record) in persisted.finished {
        let PersistedExecutionRecord::Finished {
            schema_version,
            key,
            request_digest,
            result,
        } = record
        else {
            return Err(corrupt("finished journal file contains a running record"));
        };
        validate_schema(schema_version)?;
        validate_storage_binding(&storage_key, &key)?;
        insert_finished(&mut records, key, request_digest, result);
    }
    for (storage_key, record) in persisted.running {
        let PersistedExecutionRecord::Running {
            schema_version,
            key,
            execution_id,
            request_digest,
            command,
            cwd,
            started_at_unix_ms,
        } = record
        else {
            return Err(corrupt("running journal file contains a finished record"));
        };
        validate_schema(schema_version)?;
        validate_storage_binding(&storage_key, &key)?;
        let result = Box::new(orphaned_result(
            execution_id,
            command,
            cwd,
            started_at_unix_ms,
        ));
        journal.write_finished_blocking(
            &storage_key,
            &PersistedExecutionRecord::Finished {
                schema_version: JOURNAL_SCHEMA_VERSION,
                key: key.clone(),
                request_digest: request_digest.clone(),
                result: result.clone(),
            },
        )?;
        insert_finished(&mut records, key, request_digest, result);
    }
    while records.entries.len() > MAX_RECORDS {
        evict_completed(&mut records, journal)?;
    }
    Ok(records)
}

fn insert_finished(
    records: &mut ExecutionRecords,
    key: ExecutionKey,
    request_digest: String,
    result: Box<CommandExecutionResult>,
) {
    records
        .by_id
        .insert(result.execution_id.clone(), key.clone());
    records.completed_order.push_back(key.clone());
    records.entries.insert(
        key,
        ExecutionEntry {
            execution_id: result.execution_id.clone(),
            request_digest,
            state: ExecutionState::Finished(result),
        },
    );
}

fn orphaned_result(
    execution_id: String,
    command: CommandIdentity,
    cwd: PathBuf,
    started_at_unix_ms: u64,
) -> CommandExecutionResult {
    CommandExecutionResult {
        execution_id,
        terminal_state: CommandTerminalState::Unknown,
        command,
        cwd,
        exit_code: None,
        signal: None,
        timed_out: false,
        cancelled: false,
        started_at_unix_ms,
        finished_at_unix_ms: now_unix_ms(),
        elapsed_ms: 0,
        stdout: String::new(),
        stderr: "command outcome is unknown because the host restarted during execution".into(),
        stdout_bytes: 0,
        stderr_bytes: 0,
        truncated: false,
        truncation_reason: Some("hostRestarted".into()),
        artifact_ref: None,
        artifact_bytes: 0,
        artifact_sha256: None,
        source_state_before: None,
        source_state_after: None,
        reused: false,
    }
}

fn evict_completed(
    records: &mut ExecutionRecords,
    journal: &CommandExecutionJournal,
) -> RuntimeResult<()> {
    while records.entries.len() >= MAX_RECORDS {
        let Some(key) = records.completed_order.pop_front() else {
            break;
        };
        if let Some(entry) = records.entries.remove(&key) {
            records.by_id.remove(&entry.execution_id);
            journal.remove(&storage_key(&key)?)?;
        }
    }
    Ok(())
}

fn storage_key(key: &ExecutionKey) -> RuntimeResult<String> {
    let bytes = serde_json::to_vec(key).map_err(|error| {
        RuntimeError::new("execution_journal_failed", format!("encode key: {error}"))
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn validate_storage_binding(storage_key_value: &str, key: &ExecutionKey) -> RuntimeResult<()> {
    if storage_key(key)? == storage_key_value {
        Ok(())
    } else {
        Err(corrupt("command execution journal key binding is invalid"))
    }
}

fn validate_schema(schema_version: u16) -> RuntimeResult<()> {
    if schema_version == JOURNAL_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(corrupt("command execution journal schema is unsupported"))
    }
}

fn empty_records() -> ExecutionRecords {
    ExecutionRecords {
        entries: HashMap::new(),
        by_id: HashMap::new(),
        completed_order: VecDeque::new(),
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn lock_error<T>(_: std::sync::PoisonError<T>) -> RuntimeError {
    RuntimeError::new(
        "command_registry_unavailable",
        "command execution registry lock is poisoned",
    )
}

fn not_found() -> RuntimeError {
    RuntimeError::new("execution_not_found", "command execution was not found")
}

fn corrupt(message: &str) -> RuntimeError {
    RuntimeError::new("execution_journal_corrupt", message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn orphaned_running_record_recovers_as_unknown_without_rerun() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let key = ExecutionKey {
            task_id: "task".into(),
            agent_id: "agent".into(),
            idempotency_key: "key".into(),
        };
        let command = CommandIdentity {
            executable: "test".into(),
            argument_count: 0,
            arguments_sha256: "sha256:test".into(),
        };
        let first = CommandExecutionRegistry::new(directory.path());
        let execution_id = match first
            .claim(
                &key,
                "sha256:request",
                command.clone(),
                directory.path().into(),
            )
            .await
            .expect("initial claim")
        {
            Claim::Run { execution_id } => execution_id,
            _ => panic!("initial claim was not runnable"),
        };
        drop(first);

        let recovered = CommandExecutionRegistry::new(directory.path());
        let result = match recovered
            .claim(&key, "sha256:request", command, directory.path().into())
            .await
            .expect("recovered claim")
        {
            Claim::Finished(result) => result,
            _ => panic!("orphan was not terminal after restart"),
        };
        assert_eq!(result.execution_id, execution_id);
        assert_eq!(result.terminal_state, CommandTerminalState::Unknown);
        assert_eq!(result.truncation_reason.as_deref(), Some("hostRestarted"));
    }

    #[tokio::test]
    async fn corrupt_durable_record_fails_closed() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let journal_directory = directory.path().join("executions-v1");
        std::fs::create_dir_all(&journal_directory).expect("journal directory");
        std::fs::write(
            journal_directory.join(format!("{}.running.json", "b".repeat(64))),
            b"{",
        )
        .expect("corrupt record");
        let registry = CommandExecutionRegistry::new(directory.path());
        assert_eq!(
            registry.ensure_available().expect_err("fail closed").code,
            "execution_journal_corrupt"
        );
    }
}
