mod operations;

use crate::{
    PolicyEngine, RuntimeConfig, RuntimeError, RuntimeResult, SharedEventSink, ShellEvent,
    ShellSessionInfo,
};
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
    events: VecDeque<ShellEvent>,
    bytes: usize,
    latest: u64,
    replay_truncated: bool,
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
