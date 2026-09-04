use std::{fmt, sync::mpsc};

use chatcmd_runtime::{MutationJournalSink, RuntimeError, RuntimeResult};
use chatcmd_storage::SqliteRepository;

enum Command {
    Upsert {
        json: String,
        ack: mpsc::Sender<Result<(), String>>,
    },
    Remove {
        operation_id: String,
        ack: mpsc::Sender<Result<(), String>>,
    },
    List {
        ack: mpsc::Sender<Result<Vec<String>, String>>,
    },
}

#[derive(Clone)]
pub(crate) struct SqliteMutationJournalSink {
    sender: mpsc::Sender<Command>,
}

impl fmt::Debug for SqliteMutationJournalSink {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SqliteMutationJournalSink")
            .finish_non_exhaustive()
    }
}

impl SqliteMutationJournalSink {
    pub(crate) fn start(repository: SqliteRepository) -> anyhow::Result<Self> {
        let (sender, receiver) = mpsc::channel::<Command>();
        std::thread::Builder::new()
            .name("chatcmd-fs-journal".to_owned())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        tracing::error!(error = ?error, "filesystem journal runtime failed to start");
                        return;
                    }
                };
                while let Ok(command) = receiver.recv() {
                    match command {
                        Command::Upsert { json, ack } => {
                            let result = runtime
                                .block_on(repository.upsert_filesystem_operation_journal_json(&json))
                                .map_err(|error| error.to_string());
                            let _ = ack.send(result);
                        }
                        Command::Remove { operation_id, ack } => {
                            let result = runtime
                                .block_on(repository.remove_filesystem_operation_journal(&operation_id))
                                .map_err(|error| error.to_string());
                            let _ = ack.send(result);
                        }
                        Command::List { ack } => {
                            let result = runtime
                                .block_on(repository.list_filesystem_operation_journal_json())
                                .map_err(|error| error.to_string());
                            let _ = ack.send(result);
                        }
                    }
                }
            })?;
        Ok(Self { sender })
    }

    fn request(
        &self,
        command: impl FnOnce(mpsc::Sender<Result<(), String>>) -> Command,
    ) -> RuntimeResult<()> {
        let (ack_tx, ack_rx) = mpsc::channel();
        self.sender
            .send(command(ack_tx))
            .map_err(|error| RuntimeError::new("journal_persistence_failed", error.to_string()))?;
        ack_rx
            .recv()
            .map_err(|error| RuntimeError::new("journal_persistence_failed", error.to_string()))?
            .map_err(|error| RuntimeError::new("journal_persistence_failed", error))
    }
}

impl MutationJournalSink for SqliteMutationJournalSink {
    fn upsert_json(&self, journal_json: &str) -> RuntimeResult<()> {
        self.request(|ack| Command::Upsert {
            json: journal_json.to_owned(),
            ack,
        })
    }

    fn remove(&self, operation_id: &str) -> RuntimeResult<()> {
        self.request(|ack| Command::Remove {
            operation_id: operation_id.to_owned(),
            ack,
        })
    }

    fn list_json(&self) -> RuntimeResult<Vec<String>> {
        let (ack_tx, ack_rx) = mpsc::channel();
        self.sender
            .send(Command::List { ack: ack_tx })
            .map_err(|error| RuntimeError::new("journal_persistence_failed", error.to_string()))?;
        ack_rx
            .recv()
            .map_err(|error| RuntimeError::new("journal_persistence_failed", error.to_string()))?
            .map_err(|error| RuntimeError::new("journal_persistence_failed", error))
    }
}
