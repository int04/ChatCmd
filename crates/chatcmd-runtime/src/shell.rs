mod live;
mod operations;

use crate::{
    PolicyEngine, RuntimeConfig, RuntimeError, RuntimeResult, SharedEventSink, ShellEvent,
    ShellSessionInfo,
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use portable_pty::{Child, MasterPty};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::Write,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI32, AtomicU16, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Notify, Semaphore};

#[derive(Clone)]
pub struct ShellRuntime {
    inner: Arc<ShellRuntimeInner>,
}

struct ShellRuntimeInner {
    config: RuntimeConfig,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    retired_sessions: Mutex<VecDeque<Arc<Session>>>,
    completed_requests: Mutex<HashMap<String, serde_json::Value>>,
    in_flight_requests: Mutex<HashSet<String>>,
    operations: Arc<Semaphore>,
    policy: PolicyEngine,
    events: SharedEventSink,
}

struct Session {
    id: String,
    executable: String,
    cwd: PathBuf,
    created_at: u128,
    process_id: Option<u32>,
    columns: AtomicU16,
    rows: AtomicU16,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    output: Mutex<OutputBuffer>,
    notify: Notify,
    exited: AtomicBool,
    exit_code: AtomicI32,
}

struct OutputBuffer {
    events: VecDeque<StoredEvent>,
    bytes: usize,
    latest: u64,
    replay_truncated: bool,
    dropped_bytes: u64,
    dropped_events: u64,
}

struct StoredEvent {
    event: ShellEvent,
    bytes: usize,
}

impl OutputBuffer {
    fn push(&mut self, event: ShellEvent, byte_count: usize, max_bytes: usize, max_events: usize) {
        self.bytes = self.bytes.saturating_add(byte_count);
        self.events.push_back(StoredEvent {
            event,
            bytes: byte_count,
        });
        while self.bytes > max_bytes || self.events.len() > max_events {
            let Some(removed) = self.events.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(removed.bytes);
            self.dropped_bytes = self
                .dropped_bytes
                .saturating_add(u64::try_from(removed.bytes).unwrap_or(u64::MAX));
            self.dropped_events = self.dropped_events.saturating_add(1);
            self.replay_truncated = true;
        }
    }
}

struct TerminalOutputCoalescer {
    pending: Vec<u8>,
    max_chunk_bytes: usize,
}

impl TerminalOutputCoalescer {
    fn new(max_chunk_bytes: usize) -> Self {
        Self {
            pending: Vec::with_capacity(max_chunk_bytes),
            max_chunk_bytes: max_chunk_bytes.max(256),
        }
    }

    fn push(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.pending.extend_from_slice(bytes);
        let mut chunks = Vec::new();
        while self.pending.len() >= self.max_chunk_bytes {
            chunks.push(self.pending.drain(..self.max_chunk_bytes).collect());
        }
        chunks
    }

    fn flush(&mut self) -> Option<Vec<u8>> {
        (!self.pending.is_empty()).then(|| std::mem::take(&mut self.pending))
    }
}

fn encode_output(bytes: &[u8]) -> (String, String) {
    match std::str::from_utf8(bytes) {
        Ok(text) => (text.to_owned(), "utf-8".to_owned()),
        Err(_) => (BASE64.encode(bytes), "base64".to_owned()),
    }
}

fn append_output(
    session: &Session,
    bytes: Vec<u8>,
    max_replay_bytes: usize,
    max_replay_events: usize,
) {
    let byte_count = bytes.len();
    let (data, encoding) = encode_output(&bytes);
    if let Ok(mut output) = session.output.lock() {
        output.latest = output.latest.saturating_add(1);
        let event = ShellEvent {
            sequence: output.latest,
            timestamp_unix_ms: now_ms(),
            event_type: "output".into(),
            stream: "pty".into(),
            data,
            encoding,
        };
        output.push(event, byte_count, max_replay_bytes, max_replay_events);
    }
    session.notify.notify_waiters();
}

fn default_shell() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from("powershell.exe")
    } else {
        std::env::var_os("SHELL")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/bin/sh"))
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_millis())
}

fn last_sequence(session: &Session) -> RuntimeResult<u64> {
    Ok(session.output.lock().map_err(lock_error)?.latest)
}

fn try_wait(session: &Session) -> RuntimeResult<Option<i32>> {
    let mut child = session.child.lock().map_err(lock_error)?;
    match child.try_wait().map_err(io_error)? {
        Some(status) => {
            let code = status.exit_code() as i32;
            session.exit_code.store(code, Ordering::Release);
            session.exited.store(true, Ordering::Release);
            Ok(Some(code))
        }
        None => Ok(None),
    }
}

fn session_info(session: &Arc<Session>) -> RuntimeResult<ShellSessionInfo> {
    let exit = try_wait(session)?;
    Ok(ShellSessionInfo {
        session_id: session.id.clone(),
        status: if exit.is_some() || session.exited.load(Ordering::Acquire) {
            "exited".into()
        } else {
            "running".into()
        },
        process_id: session.process_id,
        executable: session.executable.clone(),
        initial_working_directory: session.cwd.clone(),
        columns: session.columns.load(Ordering::Acquire),
        rows: session.rows.load(Ordering::Acquire),
        created_at_unix_ms: session.created_at,
        exit_code: exit.or_else(|| {
            let value = session.exit_code.load(Ordering::Acquire);
            (value != i32::MIN).then_some(value)
        }),
        last_sequence: last_sequence(session)?,
    })
}

fn find_session(inner: &ShellRuntimeInner, id: &str) -> RuntimeResult<Arc<Session>> {
    if let Some(session) = inner.sessions.lock().map_err(lock_error)?.get(id).cloned() {
        return Ok(session);
    }
    inner
        .retired_sessions
        .lock()
        .map_err(lock_error)?
        .iter()
        .find(|session| session.id == id)
        .cloned()
        .ok_or_else(|| RuntimeError::new("session_not_found", "terminal session was not found"))
}

fn retire_session(inner: &ShellRuntimeInner, session_id: &str) -> RuntimeResult<()> {
    let retired = inner
        .sessions
        .lock()
        .map_err(lock_error)?
        .remove(session_id);
    let Some(session) = retired else {
        return Ok(());
    };
    if let Ok(mut writer) = session.writer.lock() {
        *writer = None;
    }
    let mut retired_sessions = inner.retired_sessions.lock().map_err(lock_error)?;
    retired_sessions.push_back(session);
    while retired_sessions.len() > 64 {
        retired_sessions.pop_front();
    }
    Ok(())
}

fn spawn_session_reaper(inner: Arc<ShellRuntimeInner>, session: Arc<Session>) -> RuntimeResult<()> {
    std::thread::Builder::new()
        .name(format!("chatcmd-reaper-{}", session.id))
        .spawn(move || {
            loop {
                if try_wait(&session).ok().flatten().is_some() {
                    let _ = retire_session(&inner, &session.id);
                    session.notify.notify_waiters();
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        })
        .map(|_| ())
        .map_err(|error| RuntimeError::new("shell_reaper_failed", error.to_string()))
}

fn kill_tree(session: &Session) -> RuntimeResult<()> {
    if cfg!(windows)
        && let Some(pid) = session.process_id
    {
        let _ = std::process::Command::new("taskkill.exe")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
    session
        .child
        .lock()
        .map_err(lock_error)?
        .kill()
        .map_err(io_error)
}

fn lock_error<T>(_error: std::sync::PoisonError<T>) -> RuntimeError {
    RuntimeError::new(
        "runtime_state_corrupt",
        "runtime synchronization state is unavailable",
    )
}

fn io_error(error: std::io::Error) -> RuntimeError {
    RuntimeError::new("io_error", error.to_string())
}

fn pty_error(error: anyhow::Error) -> RuntimeError {
    RuntimeError::new("pty_error", error.to_string())
}

fn redact(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    if lower.contains("bearer ")
        || lower.contains("authorization")
        || lower.contains("token=")
        || lower.contains("/mcp/")
    {
        "[REDACTED]".into()
    } else {
        value.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalescer_bounds_chunks_and_preserves_a_million_tiny_reads() {
        let input = (0..1_000_000)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        let mut coalescer = TerminalOutputCoalescer::new(8 * 1024);
        let mut chunks = Vec::new();
        for byte in &input {
            chunks.extend(coalescer.push(std::slice::from_ref(byte)));
        }
        if let Some(chunk) = coalescer.flush() {
            chunks.push(chunk);
        }
        assert!(chunks.iter().all(|chunk| chunk.len() <= 8 * 1024));
        assert!(chunks.len() < 125);
        assert_eq!(chunks.concat(), input);
    }

    #[test]
    fn encoded_output_round_trips_utf8_ansi_cr_and_binary() {
        let vectors: [&[u8]; 4] = [
            "xin chào".as_bytes(),
            b"\x1b[31mred\x1b[0m",
            b"10%\r20%\r",
            b"\x00\xff\x80\n",
        ];
        for input in vectors {
            let (data, encoding) = encode_output(input);
            let decoded = if encoding == "base64" {
                BASE64.decode(data).expect("valid base64")
            } else {
                data.into_bytes()
            };
            assert_eq!(decoded, input);
        }
    }

    #[test]
    fn replay_evicts_oldest_by_bytes_and_events_with_gap_counters() {
        let mut output = OutputBuffer {
            events: VecDeque::new(),
            bytes: 0,
            latest: 0,
            replay_truncated: false,
            dropped_bytes: 0,
            dropped_events: 0,
        };
        for sequence in 1..=4 {
            output.latest = sequence;
            output.push(
                ShellEvent {
                    sequence,
                    timestamp_unix_ms: 0,
                    event_type: "output".to_owned(),
                    stream: "pty".to_owned(),
                    data: "1234".to_owned(),
                    encoding: "utf-8".to_owned(),
                },
                4,
                10,
                2,
            );
        }
        assert_eq!(output.events.len(), 2);
        assert_eq!(
            output.events.front().map(|stored| stored.event.sequence),
            Some(3)
        );
        assert_eq!(output.dropped_events, 2);
        assert_eq!(output.dropped_bytes, 8);
        assert!(output.replay_truncated);
    }
}
