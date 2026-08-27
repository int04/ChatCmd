use std::time::Duration;

use chatcmd_core::{StorageError, TerminalEventChunk};
use tokio::sync::{mpsc, oneshot};

use crate::{SqliteRepository, repository::backend};

/// Bounded single-writer queue settings.
#[derive(Debug, Clone, Copy)]
pub struct EventWriterOptions {
    pub queue_capacity: usize,
    pub max_batch_events: usize,
    pub max_batch_delay: Duration,
}

impl Default for EventWriterOptions {
    fn default() -> Self {
        Self {
            queue_capacity: 64,
            max_batch_events: 250,
            max_batch_delay: Duration::from_millis(50),
        }
    }
}

struct WriteRequest {
    chunks: Vec<TerminalEventChunk>,
    response: oneshot::Sender<Result<usize, StorageError>>,
}

/// Cloneable producer for the bounded SQLite event writer.
#[derive(Debug, Clone)]
pub struct EventWriter {
    sender: mpsc::Sender<WriteRequest>,
    max_batch_events: usize,
}

impl EventWriter {
    /// Starts one async writer. Batches never exceed 250 events or 50 ms.
    #[must_use]
    pub fn start(repository: SqliteRepository, options: EventWriterOptions) -> Self {
        let max_batch_events = options.max_batch_events.clamp(1, 250);
        let max_batch_delay = options.max_batch_delay.min(Duration::from_millis(50));
        let (sender, receiver) = mpsc::channel(options.queue_capacity.max(1));
        tokio::spawn(run_writer(
            repository,
            receiver,
            max_batch_events,
            max_batch_delay,
        ));
        Self {
            sender,
            max_batch_events,
        }
    }

    /// Enqueues without waiting. A full queue returns explicit backpressure.
    pub fn try_enqueue(
        &self,
        chunks: Vec<TerminalEventChunk>,
    ) -> Result<oneshot::Receiver<Result<usize, StorageError>>, StorageError> {
        if chunks.len() > self.max_batch_events {
            return Err(StorageError::InvalidData(format!(
                "event request exceeds {} chunks",
                self.max_batch_events
            )));
        }
        let (response, receiver) = oneshot::channel();
        self.sender
            .try_send(WriteRequest { chunks, response })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => StorageError::Backpressure,
                mpsc::error::TrySendError::Closed(_) => StorageError::WriterClosed,
            })?;
        Ok(receiver)
    }
}

async fn run_writer(
    repository: SqliteRepository,
    mut receiver: mpsc::Receiver<WriteRequest>,
    max_batch_events: usize,
    max_batch_delay: Duration,
) {
    let mut carry = None;
    loop {
        let first = match carry.take() {
            Some(request) => request,
            None => match receiver.recv().await {
                Some(request) => request,
                None => break,
            },
        };
        let deadline = tokio::time::Instant::now() + max_batch_delay;
        let mut event_count = first.chunks.len();
        let mut requests = vec![first];
        while event_count < max_batch_events {
            match tokio::time::timeout_at(deadline, receiver.recv()).await {
                Ok(Some(request)) => {
                    if event_count + request.chunks.len() > max_batch_events {
                        carry = Some(request);
                        break;
                    }
                    event_count += request.chunks.len();
                    requests.push(request);
                }
                Ok(None) | Err(_) => break,
            }
        }
        persist_requests(&repository, requests).await;
    }
}

async fn persist_requests(repository: &SqliteRepository, requests: Vec<WriteRequest>) {
    let mut transaction = match repository.pool().begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            respond_all(requests, format!("begin writer batch: {error}"));
            return;
        }
    };
    let mut results = Vec::with_capacity(requests.len());
    let mut failure = None;
    for request in &requests {
        match SqliteRepository::insert_terminal_chunks_tx(&mut transaction, &request.chunks).await {
            Ok(inserted) => results.push(inserted),
            Err(error) => {
                failure = Some(error.to_string());
                break;
            }
        }
    }
    if let Some(error) = failure {
        let _ = transaction.rollback().await;
        respond_all(requests, error);
        return;
    }
    if let Err(error) = transaction.commit().await {
        respond_all(requests, format!("commit writer batch: {error}"));
        return;
    }
    for (request, inserted) in requests.into_iter().zip(results) {
        let _ = request.response.send(Ok(inserted));
    }
}

fn respond_all(requests: Vec<WriteRequest>, message: String) {
    for request in requests {
        let _ = request
            .response
            .send(Err(backend("write event batch", &message)));
    }
}
