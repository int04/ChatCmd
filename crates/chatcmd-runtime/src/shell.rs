use crate::{
    OperationContext, PolicyContext, PolicyEngine, RuntimeConfig, RuntimeError, RuntimeResult,
    SharedEventSink, ShellCreateRequest, ShellEvent, ShellReadResult, ShellSessionInfo,
    ShellSignal, ShellWaitResult, ShellWriteRequest, TimelineEvent,
};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI32, AtomicU16, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Notify, Semaphore};
use uuid::Uuid;

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

impl ShellRuntime {
    #[must_use]
    pub fn new(config: RuntimeConfig, policy: PolicyEngine, events: SharedEventSink) -> Self {
        let concurrency = config.max_concurrent_operations.max(1);
        Self {
            inner: Arc::new(ShellRuntimeInner {
                config,
                sessions: Mutex::new(HashMap::new()),
                completed_requests: Mutex::new(HashMap::new()),
                in_flight_requests: Mutex::new(HashSet::new()),
                operations: Arc::new(Semaphore::new(concurrency)),
                policy,
                events,
            }),
        }
    }

    async fn permit(&self) -> RuntimeResult<tokio::sync::OwnedSemaphorePermit> {
        self.inner
            .operations
            .clone()
            .try_acquire_owned()
            .map_err(|_| RuntimeError::busy("local device operation limit reached"))
    }

    pub async fn create(
        &self,
        context: &OperationContext,
        request: ShellCreateRequest,
    ) -> RuntimeResult<ShellSessionInfo> {
        let _permit = self.permit().await?;
        if let Some(value) = self.cached(&request.request_id)? {
            return serde_json::from_value(value).map_err(|error| {
                RuntimeError::new("idempotency_cache_corrupt", error.to_string())
            });
        }
        let _request_guard = self.begin_request(&request.request_id)?;
        if context.cancellation.is_cancelled() {
            return Err(RuntimeError::new("cancelled", "operation was cancelled"));
        }
        {
            let mut sessions = self.inner.sessions.lock().map_err(lock_error)?;
            sessions.retain(|_, session| try_wait(session).map_or(true, |exit| exit.is_none()));
            if sessions.len() >= self.inner.config.max_sessions {
                return Err(RuntimeError::busy("active terminal session limit reached"));
            }
        }
        let cwd = self.resolve_cwd(request.working_directory.as_deref())?;
        let executable = request
            .executable
            .clone()
            .or_else(|| self.inner.config.default_shell.clone())
            .unwrap_or_else(default_shell);
        let columns = request.columns.unwrap_or(120).clamp(1, 500);
        let rows = request.rows.unwrap_or(30).clamp(1, 300);
        let mut command = CommandBuilder::new(&executable);
        command.cwd(&cwd);
        if cfg!(windows)
            && request.arguments.is_empty()
            && executable
                .file_stem()
                .is_some_and(|name| name.eq_ignore_ascii_case("powershell"))
        {
            command.args(["-NoLogo", "-NoProfile"]);
        }
        for argument in &request.arguments {
            command.arg(argument);
        }
        for (key, value) in &request.environment {
            command.env(key, value);
        }
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols: columns,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(pty_error)?;
        let child = pair.slave.spawn_command(command).map_err(pty_error)?;
        drop(pair.slave);
        let process_id = child.process_id();
        let mut reader = pair.master.try_clone_reader().map_err(pty_error)?;
        let writer = pair.master.take_writer().map_err(pty_error)?;
        let id = Uuid::new_v4().to_string();
        let session = Arc::new(Session {
            id: id.clone(),
            executable: executable.to_string_lossy().into_owned(),
            cwd,
            created_at: now_ms(),
            process_id,
            columns: AtomicU16::new(columns),
            rows: AtomicU16::new(rows),
            master: Mutex::new(pair.master),
            writer: Mutex::new(Some(writer)),
            child: Mutex::new(child),
            output: Mutex::new(OutputBuffer {
                events: VecDeque::new(),
                bytes: 0,
                latest: 0,
                replay_truncated: false,
            }),
            notify: Notify::new(),
            exited: AtomicBool::new(false),
            exit_code: AtomicI32::new(i32::MIN),
        });
        self.inner
            .sessions
            .lock()
            .map_err(lock_error)?
            .insert(id, session.clone());
        let session_for_reader = session.clone();
        let max_bytes = self.inner.config.max_replay_bytes.max(4096);
        std::thread::Builder::new()
            .name(format!("chatcmd-pty-{}", session.id))
            .spawn(move || {
                let mut bytes = [0_u8; 8192];
                loop {
                    match reader.read(&mut bytes) {
                        Ok(0) | Err(_) => break,
                        Ok(count) => {
                            if bytes[..count].windows(4).any(|window| window == b"\x1b[6n")
                                && let Ok(mut writer) = session_for_reader.writer.lock()
                                && let Some(writer) = writer.as_mut()
                            {
                                let _ = writer.write_all(b"\x1b[1;1R");
                                let _ = writer.flush();
                            }
                            if let Ok(mut output) = session_for_reader.output.lock() {
                                output.latest = output.latest.saturating_add(1);
                                let event = ShellEvent {
                                    sequence: output.latest,
                                    timestamp_unix_ms: now_ms(),
                                    event_type: "output".into(),
                                    stream: "pty".into(),
                                    data: String::from_utf8_lossy(&bytes[..count]).into_owned(),
                                };
                                output.bytes = output.bytes.saturating_add(event.data.len());
                                output.events.push_back(event);
                                while output.bytes > max_bytes {
                                    if let Some(removed) = output.events.pop_front() {
                                        output.bytes =
                                            output.bytes.saturating_sub(removed.data.len());
                                        output.replay_truncated = true;
                                    } else {
                                        break;
                                    }
                                }
                            }
                            session_for_reader.notify.notify_waiters();
                        }
                    }
                }
                session_for_reader.notify.notify_waiters();
            })
            .map_err(|error| RuntimeError::new("pty_reader_failed", error.to_string()))?;
        if cfg!(windows) {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let info = session_info(&session)?;
        self.store_cached(&request.request_id, &info)?;
        self.emit(context, "completed", None);
        Ok(info)
    }

    pub async fn write(
        &self,
        context: &OperationContext,
        request: ShellWriteRequest,
    ) -> RuntimeResult<usize> {
        let _permit = self.permit().await?;
        if let Some(value) = self.cached(&request.request_id)? {
            return serde_json::from_value(value).map_err(|error| {
                RuntimeError::new("idempotency_cache_corrupt", error.to_string())
            });
        }
        let _request_guard = self.begin_request(&request.request_id)?;
        if context.cancellation.is_cancelled() {
            return Err(RuntimeError::new("cancelled", "operation was cancelled"));
        }
        let session = self.session(&request.session_id)?;
        let mut data = request.text.into_bytes();
        if request.append_new_line {
            data.extend_from_slice(if cfg!(windows) { b"\r\n" } else { b"\n" });
        }
        let count = data.len();
        {
            let mut writer = session.writer.lock().map_err(lock_error)?;
            let writer = writer
                .as_mut()
                .ok_or_else(|| RuntimeError::new("session_closed", "terminal input is closed"))?;
            writer.write_all(&data).map_err(io_error)?;
            writer.flush().map_err(io_error)?;
        }
        self.store_cached(&request.request_id, &count)?;
        self.emit(context, "completed", None);
        Ok(count)
    }

    pub async fn read(
        &self,
        session_id: &str,
        after_sequence: u64,
        max_events: usize,
    ) -> RuntimeResult<ShellReadResult> {
        let _permit = self.permit().await?;
        let session = self.session(session_id)?;
        let output = session.output.lock().map_err(lock_error)?;
        let oldest = output
            .events
            .front()
            .map_or(output.latest.saturating_add(1), |event| event.sequence);
        let events = output
            .events
            .iter()
            .filter(|event| event.sequence > after_sequence)
            .take(max_events.clamp(1, 2000))
            .cloned()
            .collect();
        Ok(ShellReadResult {
            session_id: session_id.into(),
            oldest_available_sequence: oldest,
            latest_available_sequence: output.latest,
            replay_truncated: output.replay_truncated || after_sequence.saturating_add(1) < oldest,
            events,
        })
    }

    pub async fn wait(
        &self,
        session_id: &str,
        timeout: Duration,
    ) -> RuntimeResult<ShellWaitResult> {
        let _permit = self.permit().await?;
        let session = self.session(session_id)?;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(code) = try_wait(&session)? {
                return Ok(ShellWaitResult {
                    session_id: session_id.into(),
                    completed: true,
                    wait_timed_out: false,
                    exit_code: Some(code),
                    last_sequence: last_sequence(&session)?,
                });
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(ShellWaitResult {
                    session_id: session_id.into(),
                    completed: false,
                    wait_timed_out: true,
                    exit_code: None,
                    last_sequence: last_sequence(&session)?,
                });
            }
            tokio::select! { () = session.notify.notified() => {}, () = tokio::time::sleep(Duration::from_millis(25)) => {} }
        }
    }

    pub async fn signal(
        &self,
        context: &OperationContext,
        session_id: &str,
        signal: ShellSignal,
    ) -> RuntimeResult<()> {
        let _permit = self.permit().await?;
        let session = self.session(session_id)?;
        let bytes: &[u8] = match signal {
            ShellSignal::CtrlC => b"\x03",
            ShellSignal::CtrlBreak => b"\x1c",
            ShellSignal::Eof => {
                if cfg!(windows) {
                    b"\x1a"
                } else {
                    b"\x04"
                }
            }
        };
        let mut writer = session.writer.lock().map_err(lock_error)?;
        writer
            .as_mut()
            .ok_or_else(|| RuntimeError::new("session_closed", "terminal input is closed"))?
            .write_all(bytes)
            .map_err(io_error)?;
        self.emit(context, "completed", None);
        Ok(())
    }

    pub async fn resize(
        &self,
        session_id: &str,
        columns: u16,
        rows: u16,
    ) -> RuntimeResult<ShellSessionInfo> {
        let _permit = self.permit().await?;
        let session = self.session(session_id)?;
        let columns = columns.clamp(1, 500);
        let rows = rows.clamp(1, 300);
        session
            .master
            .lock()
            .map_err(lock_error)?
            .resize(PtySize {
                rows,
                cols: columns,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(pty_error)?;
        session.columns.store(columns, Ordering::Release);
        session.rows.store(rows, Ordering::Release);
        session_info(&session)
    }

    pub async fn close(
        &self,
        context: &OperationContext,
        session_id: &str,
        force: bool,
    ) -> RuntimeResult<()> {
        let _permit = self.permit().await?;
        if force {
            self.inner
                .policy
                .authorize(&PolicyContext {
                    agent_id: context.agent_id.clone(),
                    tool_name: "shell_close".into(),
                    root: None,
                    destructive: true,
                })
                .await?;
        }
        let session = self.session(session_id)?;
        if force {
            kill_tree(&session)?;
        } else if let Ok(mut writer) = session.writer.lock()
            && let Some(writer) = writer.as_mut()
        {
            let _ = writer.write_all(if cfg!(windows) { b"exit\r" } else { b"exit\n" });
            let _ = writer.flush();
        }
        if let Ok(mut writer) = session.writer.lock() {
            *writer = None;
        }
        self.emit(context, "completed", None);
        Ok(())
    }

    pub async fn list(&self) -> RuntimeResult<Vec<ShellSessionInfo>> {
        let sessions: Vec<_> = self
            .inner
            .sessions
            .lock()
            .map_err(lock_error)?
            .values()
            .cloned()
            .collect();
        sessions.iter().map(session_info).collect()
    }

    pub async fn inspect(&self, session_id: &str) -> RuntimeResult<ShellSessionInfo> {
        session_info(&self.session(session_id)?)
    }

    fn resolve_cwd(&self, requested: Option<&Path>) -> RuntimeResult<PathBuf> {
        let path = requested
            .or_else(|| self.inner.config.roots.first().map(PathBuf::as_path))
            .ok_or_else(|| {
                RuntimeError::new(
                    "workspace_root_required",
                    "no working directory or configured root",
                )
            })?;
        let canonical = path.canonicalize().map_err(io_error)?;
        if !self.inner.config.roots.is_empty()
            && !self
                .inner
                .config
                .roots
                .iter()
                .filter_map(|root| root.canonicalize().ok())
                .any(|root| canonical.starts_with(root))
        {
            return Err(RuntimeError::new(
                "path_outside_allowed_scope",
                "working directory is outside configured roots",
            ));
        }
        Ok(canonical)
    }

    fn session(&self, id: &str) -> RuntimeResult<Arc<Session>> {
        self.inner
            .sessions
            .lock()
            .map_err(lock_error)?
            .get(id)
            .cloned()
            .ok_or_else(|| RuntimeError::new("session_not_found", "terminal session was not found"))
    }
    fn cached(&self, request_id: &str) -> RuntimeResult<Option<serde_json::Value>> {
        Ok(self
            .inner
            .completed_requests
            .lock()
            .map_err(lock_error)?
            .get(request_id)
            .cloned())
    }
    fn begin_request(&self, request_id: &str) -> RuntimeResult<RequestGuard> {
        if request_id.is_empty() {
            return Err(RuntimeError::new(
                "request_id_required",
                "request id cannot be empty",
            ));
        }
        let mut requests = self.inner.in_flight_requests.lock().map_err(lock_error)?;
        if !requests.insert(request_id.to_owned()) {
            return Err(RuntimeError::busy("duplicate request is still in progress"));
        }
        Ok(RequestGuard {
            inner: self.inner.clone(),
            request_id: request_id.to_owned(),
        })
    }
    fn store_cached<T: serde::Serialize>(&self, request_id: &str, value: &T) -> RuntimeResult<()> {
        let mut completed = self.inner.completed_requests.lock().map_err(lock_error)?;
        if completed.len() >= 4096 {
            completed.clear();
        }
        completed.insert(
            request_id.into(),
            serde_json::to_value(value)
                .map_err(|error| RuntimeError::new("serialization_failed", error.to_string()))?,
        );
        Ok(())
    }
    fn emit(&self, context: &OperationContext, status: &str, message: Option<String>) {
        self.inner.events.emit(TimelineEvent {
            event_type: "runtime_operation".into(),
            request_id: Some(context.request_id.clone()),
            task_id: context.task_id.clone(),
            turn_id: context.turn_id.clone(),
            tool_name: Some(context.tool_name.clone()),
            status: status.into(),
            message: message.map(|value| redact(&value)),
            metadata: BTreeMap::new(),
        });
    }
}

struct RequestGuard {
    inner: Arc<ShellRuntimeInner>,
    request_id: String,
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        if let Ok(mut requests) = self.inner.in_flight_requests.lock() {
            requests.remove(&self.request_id);
        }
    }
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
    if lower.contains("bearer ") || lower.contains("authorization") || lower.contains("token=") {
        "[REDACTED]".into()
    } else {
        value.into()
    }
}
