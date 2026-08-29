use super::{ShellRuntime, find_session, last_sequence};
use crate::{RuntimeResult, ShellReadResult};
use std::time::Duration;

impl ShellRuntime {
    pub async fn read_when_available(
        &self,
        session_id: &str,
        after_sequence: u64,
        max_events: usize,
        timeout: Duration,
    ) -> RuntimeResult<ShellReadResult> {
        let session = find_session(&self.inner, session_id)?;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if last_sequence(&session)? > after_sequence
                || session.exited.load(std::sync::atomic::Ordering::Acquire)
            {
                return self.read(session_id, after_sequence, max_events).await;
            }
            if tokio::time::Instant::now() >= deadline {
                return self.read(session_id, after_sequence, max_events).await;
            }
            tokio::select! {
                () = session.notify.notified() => {},
                () = tokio::time::sleep_until(deadline) => {},
            }
        }
    }
}
