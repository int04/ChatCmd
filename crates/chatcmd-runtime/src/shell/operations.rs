use super::*;
use crate::{
    OperationContext, PolicyContext, ShellCreateRequest, ShellReadResult, ShellSignal,
    ShellWaitResult, ShellWriteRequest, TimelineEvent,
};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};
use uuid::Uuid;

impl ShellRuntime {
    #[must_use]
    pub fn new(config: RuntimeConfig, policy: PolicyEngine, events: SharedEventSink) -> Self {
        let concurrency = config.max_concurrent_operations.max(1);
        Self {
            inner: Arc::new(ShellRuntimeInner {
                config,
                sessions: Mutex::new(HashMap::new()),
                retired_sessions: Mutex::new(VecDeque::new()),
                completed_requests: Mutex::new(HashMap::new()),
                in_flight_requests: Mutex::new(HashSet::new()),
                admission: AdmissionController::new(concurrency, 2, 64 * 1024 * 1024),
                policy,
                events,
            }),
        }
    }

    fn permit(
        &self,
        actor: &str,
        weight: u32,
        memory: u64,
    ) -> RuntimeResult<crate::AdmissionPermit> {
        self.inner.admission.try_admit(actor, weight, memory)
    }

    fn operation_tracker(
        &self,
        context: &OperationContext,
        timeout: Duration,
        max_bytes_read: Option<u64>,
        max_bytes_written: Option<u64>,
        max_output_bytes: Option<u64>,
    ) -> BudgetTracker {
        BudgetTracker::new(
            context.cancellation.clone(),
            ToolBudget {
                max_bytes_read,
                max_bytes_written,
                max_output_bytes,
                memory_reservation_bytes: Some(4 * 1024 * 1024),
                ..ToolBudget::default()
            }
            .with_timeout(timeout),
        )
    }

    pub async fn create(
        &self,
        context: &OperationContext,
        request: ShellCreateRequest,
    ) -> RuntimeResult<ShellSessionInfo> {
        self.create_with_additional_scopes(context, request, &[])
            .await
    }

    pub async fn create_with_additional_scopes(
        &self,
        context: &OperationContext,
        request: ShellCreateRequest,
        additional_scopes: &[PathBuf],
    ) -> RuntimeResult<ShellSessionInfo> {
        let _permit = self.permit(&context.agent_id, 2, 4 * 1024 * 1024)?;
        let tracker = self.operation_tracker(context, Duration::from_secs(30), None, None, None);
        tracker.set_phase("creatingShell");
        tracker.checkpoint()?;
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
        let cwd = self.resolve_cwd(request.working_directory.as_deref(), additional_scopes)?;
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
                dropped_bytes: 0,
                dropped_events: 0,
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
        let session_for_coalescer = session.clone();
        let inner_for_reader = self.inner.clone();
        let max_bytes = self.inner.config.max_replay_bytes.max(4096);
        let max_events = self.inner.config.max_replay_events.max(1);
        let max_chunk_bytes = self
            .inner
            .config
            .shell_output_chunk_bytes
            .clamp(256, 64 * 1024);
        let max_latency =
            Duration::from_millis(self.inner.config.shell_output_max_latency_ms.clamp(1, 100));
        let (raw_tx, raw_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(128);
        let reader_session = session.clone();
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
                            if raw_tx.send(bytes[..count].to_vec()).is_err() {
                                break;
                            }
                        }
                    }
                }
                drop(raw_tx);
                reader_session.notify.notify_waiters();
            })
            .map_err(|error| RuntimeError::new("pty_reader_failed", error.to_string()))?;
        std::thread::Builder::new()
            .name(format!("chatcmd-pty-coalesce-{}", session.id))
            .spawn(move || {
                let mut coalescer = TerminalOutputCoalescer::new(max_chunk_bytes);
                loop {
                    match raw_rx.recv_timeout(max_latency) {
                        Ok(bytes) => {
                            for chunk in coalescer.push(&bytes) {
                                append_output(&session_for_coalescer, chunk, max_bytes, max_events);
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            if let Some(chunk) = coalescer.flush() {
                                append_output(&session_for_coalescer, chunk, max_bytes, max_events);
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
                if let Some(chunk) = coalescer.flush() {
                    append_output(&session_for_coalescer, chunk, max_bytes, max_events);
                }
                session_for_coalescer.notify.notify_waiters();
                if try_wait(&session_for_coalescer).ok().flatten().is_some() {
                    let _ = retire_session(&inner_for_reader, &session_for_coalescer.id);
                }
            })
            .map_err(|error| RuntimeError::new("pty_reader_failed", error.to_string()))?;
        spawn_session_reaper(self.inner.clone(), session.clone())?;
        if cfg!(windows) {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        if let Err(error) = tracker.checkpoint() {
            let _ = kill_tree(&session);
            let _ = retire_session(&self.inner, &session.id);
            return Err(error);
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
        let _permit = self.permit(&context.agent_id, 1, 512 * 1024)?;
        let tracker = self.operation_tracker(
            context,
            Duration::from_secs(30),
            None,
            Some(u64::try_from(self.inner.config.max_shell_paste_input_bytes).unwrap_or(u64::MAX)),
            None,
        );
        tracker.set_phase("writingShellInput");
        tracker.checkpoint()?;
        if let Some(value) = self.cached(&request.request_id)? {
            return serde_json::from_value(value).map_err(|error| {
                RuntimeError::new("idempotency_cache_corrupt", error.to_string())
            });
        }
        let _request_guard = self.begin_request(&request.request_id)?;
        if context.cancellation.is_cancelled() {
            return Err(RuntimeError::new("cancelled", "operation was cancelled"));
        }
        let received_bytes = request.text.len();
        let max_input_bytes = match request.input_kind {
            crate::ShellInputKind::Interactive => {
                self.inner.config.max_shell_interactive_input_bytes
            }
            crate::ShellInputKind::Paste => self.inner.config.max_shell_paste_input_bytes,
        };
        if received_bytes > max_input_bytes {
            return Err(RuntimeError::new(
                "shellInputTooLarge",
                format!(
                    "shell_write is for interactive input; use fs_write_text/fs_write_raw with contentRef (maxBytes={max_input_bytes}, receivedBytes={received_bytes})"
                ),
            ));
        }
        if request.text.as_bytes().contains(&0) {
            return Err(RuntimeError::new(
                "shellInputInvalid",
                "shell_write does not accept NUL bytes",
            ));
        }
        let session = self.session(&request.session_id)?;
        let mut data = request.text.into_bytes();
        if request.append_new_line {
            data.extend_from_slice(if cfg!(windows) { b"\r\n" } else { b"\n" });
        }
        let count = data.len();
        tracker.consume_write_bytes(u64::try_from(count).unwrap_or(u64::MAX))?;
        {
            let mut writer = session.writer.lock().map_err(lock_error)?;
            let writer = writer
                .as_mut()
                .ok_or_else(|| RuntimeError::new("session_closed", "terminal input is closed"))?;
            writer.write_all(&data).map_err(io_error)?;
            writer.flush().map_err(io_error)?;
        }
        tracker.checkpoint()?;
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
        let context = OperationContext::new("shell-read-compat", "shell-system", "shell_read");
        self.read_with_context(&context, session_id, after_sequence, max_events)
            .await
            .map(|(result, _)| result)
    }

    pub async fn read_with_context(
        &self,
        context: &OperationContext,
        session_id: &str,
        after_sequence: u64,
        max_events: usize,
    ) -> RuntimeResult<(ShellReadResult, ToolUsage)> {
        let _permit = self.permit(&context.agent_id, 1, 2 * 1024 * 1024)?;
        let tracker = self.operation_tracker(
            context,
            Duration::from_secs(30),
            Some(u64::try_from(self.inner.config.max_replay_bytes).unwrap_or(u64::MAX)),
            None,
            Some(u64::try_from(self.inner.config.max_replay_bytes).unwrap_or(u64::MAX)),
        );
        tracker.set_phase("readingShellOutput");
        tracker.checkpoint()?;
        let session = self.session(session_id)?;
        let output = session.output.lock().map_err(lock_error)?;
        let oldest = output
            .events
            .front()
            .map_or(output.latest.saturating_add(1), |stored| {
                stored.event.sequence
            });
        let events: Vec<_> = output
            .events
            .iter()
            .filter(|stored| stored.event.sequence > after_sequence)
            .take(max_events.clamp(1, 2000))
            .map(|stored| stored.event.clone())
            .collect();
        let output_bytes = events.iter().fold(0_u64, |total, event| {
            total.saturating_add(u64::try_from(event.data.len()).unwrap_or(u64::MAX))
        });
        tracker.consume_read_bytes(output_bytes)?;
        tracker.reserve_output(output_bytes)?;
        tracker.checkpoint()?;
        let result = ShellReadResult {
            session_id: session_id.into(),
            oldest_available_sequence: oldest,
            latest_available_sequence: output.latest,
            replay_truncated: output.replay_truncated || after_sequence.saturating_add(1) < oldest,
            dropped_bytes: output.dropped_bytes,
            dropped_events: output.dropped_events,
            events,
        };
        drop(output);
        Ok((result, tracker.finish_usage().into()))
    }

    pub async fn wait(
        &self,
        session_id: &str,
        timeout: Duration,
    ) -> RuntimeResult<ShellWaitResult> {
        let context = OperationContext::new("shell-wait-compat", "shell-system", "shell_wait");
        self.wait_with_context(&context, session_id, timeout)
            .await
            .map(|(result, _)| result)
    }

    pub async fn wait_with_context(
        &self,
        context: &OperationContext,
        session_id: &str,
        timeout: Duration,
    ) -> RuntimeResult<(ShellWaitResult, ToolUsage)> {
        let _permit = self.permit(&context.agent_id, 1, 64 * 1024)?;
        let effective_timeout = timeout
            .min(Duration::from_secs(5 * 60))
            .max(Duration::from_millis(1));
        let tracker = BudgetTracker::new(
            context.cancellation.clone(),
            ToolBudget {
                memory_reservation_bytes: Some(4 * 1024 * 1024),
                ..ToolBudget::default()
            },
        );
        tracker.set_phase("waitingForShell");
        tracker.checkpoint()?;
        let session = self.session(session_id)?;
        let deadline = tokio::time::Instant::now() + effective_timeout;
        loop {
            tracker.checkpoint()?;
            if let Some(code) = try_wait(&session)? {
                let result = ShellWaitResult {
                    session_id: session_id.into(),
                    completed: true,
                    wait_timed_out: false,
                    exit_code: Some(code),
                    last_sequence: last_sequence(&session)?,
                };
                retire_session(&self.inner, session_id)?;
                return Ok((result, tracker.finish_usage().into()));
            }
            if tokio::time::Instant::now() >= deadline {
                let result = ShellWaitResult {
                    session_id: session_id.into(),
                    completed: false,
                    wait_timed_out: true,
                    exit_code: None,
                    last_sequence: last_sequence(&session)?,
                };
                return Ok((result, tracker.finish_usage().into()));
            }
            tokio::select! {
                () = context.cancellation.cancelled() => tracker.checkpoint()?,
                () = session.notify.notified() => {},
                () = tokio::time::sleep(Duration::from_millis(25)) => {}
            }
        }
    }

    pub async fn signal(
        &self,
        context: &OperationContext,
        session_id: &str,
        signal: ShellSignal,
    ) -> RuntimeResult<()> {
        let _permit = self.permit(&context.agent_id, 1, 64 * 1024)?;
        let tracker =
            self.operation_tracker(context, Duration::from_secs(30), None, Some(16), None);
        tracker.set_phase("signallingShell");
        tracker.checkpoint()?;
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
        tracker.consume_write_bytes(u64::try_from(bytes.len()).unwrap_or(u64::MAX))?;
        let mut writer = session.writer.lock().map_err(lock_error)?;
        writer
            .as_mut()
            .ok_or_else(|| RuntimeError::new("session_closed", "terminal input is closed"))?
            .write_all(bytes)
            .map_err(io_error)?;
        tracker.checkpoint()?;
        self.emit(context, "completed", None);
        Ok(())
    }

    pub async fn resize(
        &self,
        session_id: &str,
        columns: u16,
        rows: u16,
    ) -> RuntimeResult<ShellSessionInfo> {
        let context = OperationContext::new("shell-resize-compat", "shell-system", "shell_resize");
        self.resize_with_context(&context, session_id, columns, rows)
            .await
    }

    pub async fn resize_with_context(
        &self,
        context: &OperationContext,
        session_id: &str,
        columns: u16,
        rows: u16,
    ) -> RuntimeResult<ShellSessionInfo> {
        let _permit = self.permit(&context.agent_id, 1, 64 * 1024)?;
        let tracker = self.operation_tracker(context, Duration::from_secs(30), None, None, None);
        tracker.set_phase("resizingShell");
        tracker.checkpoint()?;
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
        tracker.checkpoint()?;
        session_info(&session)
    }

    pub async fn close(
        &self,
        context: &OperationContext,
        session_id: &str,
        force: bool,
    ) -> RuntimeResult<()> {
        let _permit = self.permit(&context.agent_id, 1, 64 * 1024)?;
        let tracker =
            self.operation_tracker(context, Duration::from_secs(30), None, Some(16), None);
        tracker.set_phase("closingShell");
        tracker.checkpoint()?;
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
        tracker.checkpoint()?;
        retire_session(&self.inner, session_id)?;
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

    fn resolve_cwd(
        &self,
        requested: Option<&Path>,
        additional_scopes: &[PathBuf],
    ) -> RuntimeResult<PathBuf> {
        let path = requested
            .or_else(|| self.inner.config.roots.first().map(PathBuf::as_path))
            .ok_or_else(|| {
                RuntimeError::new(
                    "workspace_root_required",
                    "no working directory or configured root",
                )
            })?;
        let requested_absolute = path.is_absolute();
        let canonical = path.canonicalize().map_err(io_error)?;
        if requested_absolute {
            return Ok(canonical);
        }
        let configured = self
            .inner
            .config
            .roots
            .iter()
            .filter_map(|root| root.canonicalize().ok())
            .any(|root| canonical.starts_with(root));
        let user_granted = additional_scopes
            .iter()
            .filter_map(|scope| scope.canonicalize().ok())
            .filter(|scope| scope.parent().is_some())
            .any(|scope| canonical.starts_with(scope));
        if !self.inner.config.roots.is_empty() && !configured && !user_granted {
            return Err(RuntimeError::new(
                "path_outside_allowed_scope",
                "working directory is outside configured roots and user-provided task path grants",
            ));
        }
        Ok(canonical)
    }

    fn session(&self, id: &str) -> RuntimeResult<Arc<Session>> {
        find_session(&self.inner, id)
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
