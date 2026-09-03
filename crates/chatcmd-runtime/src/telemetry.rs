//! Privacy-preserving, bounded telemetry for tool execution.

use crate::{OperationContext, ToolUsage};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    fmt::Write as _,
    sync::{Arc, Mutex},
    time::Instant,
};
use tracing::{Span, info_span};

const DEFAULT_HISTORY_LIMIT: usize = 1_024;
const DEFAULT_ACTIVE_SNAPSHOT_LIMIT: usize = 256;
const COMPLETED_ID_LIMIT: usize = 4_096;
const HISTOGRAM_BOUNDS_MS: [u64; 8] = [1, 5, 10, 50, 100, 500, 1_000, u64::MAX];

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum ToolClass {
    Read,
    Search,
    Mutation,
    Git,
    Shell,
    Agent,
    Blob,
    Process,
    Other,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ToolPhase {
    Queued,
    Authorizing,
    ResolvingPath,
    Scanning,
    Reading,
    Staging,
    Verifying,
    Syncing,
    Committing,
    RollingBack,
    CleaningUp,
    ProcessStarting,
    ProcessRunning,
    ProcessStopping,
    ArtifactWriting,
    WaitingApproval,
    WaitingSubagent,
}

impl ToolPhase {
    const fn label(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Authorizing => "authorizing",
            Self::ResolvingPath => "resolvingPath",
            Self::Scanning => "scanning",
            Self::Reading => "reading",
            Self::Staging => "staging",
            Self::Verifying => "verifying",
            Self::Syncing => "syncing",
            Self::Committing => "committing",
            Self::RollingBack => "rollingBack",
            Self::CleaningUp => "cleaningUp",
            Self::ProcessStarting => "processStarting",
            Self::ProcessRunning => "processRunning",
            Self::ProcessStopping => "processStopping",
            Self::ArtifactWriting => "artifactWriting",
            Self::WaitingApproval => "waitingApproval",
            Self::WaitingSubagent => "waitingSubagent",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum ToolStatus {
    Success,
    Failure,
    Cancelled,
    Timeout,
}

impl ToolStatus {
    const fn label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Cancelled => "cancelled",
            Self::Timeout => "timeout",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolMetric {
    pub calls: u64,
    pub duration_ms: u64,
    pub max_duration_ms: u64,
    pub duration_ms_histogram: [u64; 8],
    pub queue_wait_ms: u64,
    pub queue_wait_ms_histogram: [u64; 8],
    pub files_scanned: u64,
    pub entries_scanned: u64,
    pub bytes_read: u64,
    pub bytes_written: u64,
    pub output_bytes: u64,
    pub progress_events: u64,
    pub retries: u64,
    pub truncations: u64,
    pub artifact_externalizations: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolMetricView {
    pub tool: String,
    pub class: ToolClass,
    pub status: ToolStatus,
    #[serde(flatten)]
    pub metric: ToolMetric,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActiveToolOperation {
    pub operation_id: String,
    pub tool: String,
    pub class: ToolClass,
    pub phase: ToolPhase,
    pub elapsed_ms: u64,
    pub request_correlation: String,
    pub task_correlation: Option<String>,
    pub turn_correlation: Option<String>,
    pub session_correlation: Option<String>,
    pub usage: ToolUsage,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProgressMetrics {
    pub emitted: u64,
    pub coalesced: u64,
    pub dropped: u64,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SubagentLeaseMetrics {
    pub heartbeat: u64,
    pub hard_deadline: u64,
    pub worker_restart: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TelemetrySnapshot {
    pub enabled: bool,
    pub active_operations: Vec<ActiveToolOperation>,
    pub active_operations_total: usize,
    pub active_operations_omitted: usize,
    pub metrics: Vec<ToolMetricView>,
    pub progress_events: ProgressMetrics,
    pub artifact_bytes: u64,
    pub blob_bytes: u64,
    pub operation_journal_active: u64,
    pub subagent_lease_expired: SubagentLeaseMetrics,
    pub history_size: usize,
    pub history_limit: usize,
}

#[derive(Clone)]
pub struct ToolTelemetryRegistry {
    inner: Arc<Mutex<RegistryState>>,
    enabled: bool,
    history_limit: usize,
    active_snapshot_limit: usize,
}

struct RegistryState {
    active: HashMap<String, ActiveState>,
    metrics: BTreeMap<(String, ToolClass, ToolStatus), ToolMetric>,
    history: VecDeque<TerminalRecord>,
    completed_ids: HashSet<String>,
    completed_order: VecDeque<String>,
    progress: ProgressMetrics,
    artifact_bytes: u64,
    blob_bytes: u64,
    operation_journal_active: u64,
    subagent_lease_expired: SubagentLeaseMetrics,
}

struct ActiveState {
    operation_id: String,
    tool: &'static str,
    class: ToolClass,
    phase: ToolPhase,
    started: Instant,
    request_correlation: String,
    task_correlation: Option<String>,
    turn_correlation: Option<String>,
    session_correlation: Option<String>,
    usage: ToolUsage,
    artifact_created: bool,
}

struct TerminalRecord;

impl Default for ToolTelemetryRegistry {
    fn default() -> Self {
        Self::new(true)
    }
}

impl ToolTelemetryRegistry {
    #[must_use]
    pub fn new(enabled: bool) -> Self {
        Self::with_limits(
            enabled,
            DEFAULT_HISTORY_LIMIT,
            DEFAULT_ACTIVE_SNAPSHOT_LIMIT,
        )
    }

    #[must_use]
    pub fn with_limits(enabled: bool, history_limit: usize, active_snapshot_limit: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(RegistryState {
                active: HashMap::new(),
                metrics: BTreeMap::new(),
                history: VecDeque::new(),
                completed_ids: HashSet::new(),
                completed_order: VecDeque::new(),
                progress: ProgressMetrics::default(),
                artifact_bytes: 0,
                blob_bytes: 0,
                operation_journal_active: 0,
                subagent_lease_expired: SubagentLeaseMetrics::default(),
            })),
            enabled,
            history_limit,
            active_snapshot_limit: active_snapshot_limit.max(1),
        }
    }

    #[must_use]
    pub fn start(&self, context: &OperationContext, tool: &str) -> ToolCallTelemetry {
        if !self.enabled {
            return ToolCallTelemetry {
                registry: self.clone(),
                operation_id: String::new(),
                span: Span::none(),
                finished: false,
            };
        }
        let safe_tool = tool_label(tool);
        let class = tool_class(tool);
        let operation_id = context.request_id.clone();
        let request_correlation = correlation_hash(&context.request_id);
        let task_correlation = context.task_id.as_deref().map(correlation_hash);
        let turn_correlation = context.turn_id.as_deref().map(correlation_hash);
        let session_correlation = context.mcp_session_id.as_deref().map(correlation_hash);
        let span = info_span!(
            "chatcmd.tool_call",
            tool.name = safe_tool,
            tool.class = ?class,
            request.id = %request_correlation,
            task.id = task_correlation.as_deref().unwrap_or("none"),
            turn.id = turn_correlation.as_deref().unwrap_or("none"),
            session.id = session_correlation.as_deref().unwrap_or("none"),
            phase = ToolPhase::Queued.label(),
            result.status = tracing::field::Empty,
            error.code = tracing::field::Empty,
            usage.elapsed_ms = tracing::field::Empty,
        );
        if let Ok(mut state) = self.inner.lock() {
            state.active.insert(
                operation_id.clone(),
                ActiveState {
                    operation_id: request_correlation.clone(),
                    tool: safe_tool,
                    class,
                    phase: ToolPhase::Queued,
                    started: Instant::now(),
                    request_correlation,
                    task_correlation,
                    turn_correlation,
                    session_correlation,
                    usage: ToolUsage::default(),
                    artifact_created: false,
                },
            );
        }
        ToolCallTelemetry {
            registry: self.clone(),
            operation_id,
            span,
            finished: false,
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> TelemetrySnapshot {
        let Ok(state) = self.inner.lock() else {
            return TelemetrySnapshot {
                enabled: self.enabled,
                active_operations: Vec::new(),
                active_operations_total: 0,
                active_operations_omitted: 0,
                metrics: Vec::new(),
                progress_events: ProgressMetrics::default(),
                artifact_bytes: 0,
                blob_bytes: 0,
                operation_journal_active: 0,
                subagent_lease_expired: SubagentLeaseMetrics::default(),
                history_size: 0,
                history_limit: self.history_limit,
            };
        };
        let active_total = state.active.len();
        let mut active = state
            .active
            .values()
            .take(self.active_snapshot_limit)
            .map(ActiveState::view)
            .collect::<Vec<_>>();
        active.sort_by(|left, right| left.operation_id.cmp(&right.operation_id));
        let metrics = state
            .metrics
            .iter()
            .map(|((tool, class, status), metric)| ToolMetricView {
                tool: tool.clone(),
                class: *class,
                status: *status,
                metric: metric.clone(),
            })
            .collect();
        TelemetrySnapshot {
            enabled: self.enabled,
            active_operations: active,
            active_operations_total: active_total,
            active_operations_omitted: active_total.saturating_sub(self.active_snapshot_limit),
            metrics,
            progress_events: state.progress.clone(),
            artifact_bytes: state.artifact_bytes,
            blob_bytes: state.blob_bytes,
            operation_journal_active: state.operation_journal_active,
            subagent_lease_expired: state.subagent_lease_expired.clone(),
            history_size: state.history.len(),
            history_limit: self.history_limit,
        }
    }

    pub fn record_progress(&self, emitted: u64, coalesced: u64, dropped: u64) {
        if let Ok(mut state) = self.inner.lock() {
            state.progress.emitted = state.progress.emitted.saturating_add(emitted);
            state.progress.coalesced = state.progress.coalesced.saturating_add(coalesced);
            state.progress.dropped = state.progress.dropped.saturating_add(dropped);
        }
    }

    pub fn set_resource_usage(&self, artifact_bytes: u64, blob_bytes: u64, journal_active: u64) {
        if let Ok(mut state) = self.inner.lock() {
            state.artifact_bytes = artifact_bytes;
            state.blob_bytes = blob_bytes;
            state.operation_journal_active = journal_active;
        }
    }

    pub fn set_blob_bytes(&self, blob_bytes: u64) {
        if let Ok(mut state) = self.inner.lock() {
            state.blob_bytes = blob_bytes;
        }
    }

    pub fn set_persisted_resource_usage(&self, artifact_bytes: u64, journal_active: u64) {
        if let Ok(mut state) = self.inner.lock() {
            state.artifact_bytes = artifact_bytes;
            state.operation_journal_active = journal_active;
        }
    }

    pub fn record_subagent_lease_expired(&self, reason: SubagentLeaseExpiryReason) {
        if let Ok(mut state) = self.inner.lock() {
            let counter = match reason {
                SubagentLeaseExpiryReason::Heartbeat => &mut state.subagent_lease_expired.heartbeat,
                SubagentLeaseExpiryReason::HardDeadline => {
                    &mut state.subagent_lease_expired.hard_deadline
                }
                SubagentLeaseExpiryReason::WorkerRestart => {
                    &mut state.subagent_lease_expired.worker_restart
                }
            };
            *counter = counter.saturating_add(1);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentLeaseExpiryReason {
    Heartbeat,
    HardDeadline,
    WorkerRestart,
}

pub struct ToolCallTelemetry {
    registry: ToolTelemetryRegistry,
    operation_id: String,
    span: Span,
    finished: bool,
}

impl ToolCallTelemetry {
    #[must_use]
    pub fn span(&self) -> Span {
        self.span.clone()
    }

    pub fn set_phase(&self, phase: ToolPhase) {
        if !self.registry.enabled {
            return;
        }
        if let Ok(mut state) = self.registry.inner.lock()
            && let Some(active) = state.active.get_mut(&self.operation_id)
        {
            if active.phase == ToolPhase::Queued {
                active.usage.queue_wait_ms = elapsed_ms(active.started);
            }
            active.phase = phase;
        }
        self.span.record("phase", phase.label());
    }

    pub fn update_context(&self, context: &OperationContext) {
        if let Ok(mut state) = self.registry.inner.lock()
            && let Some(active) = state.active.get_mut(&self.operation_id)
        {
            active.task_correlation = context.task_id.as_deref().map(correlation_hash);
            active.turn_correlation = context.turn_id.as_deref().map(correlation_hash);
            active.session_correlation = context.mcp_session_id.as_deref().map(correlation_hash);
            self.span.record(
                "task.id",
                active.task_correlation.as_deref().unwrap_or("none"),
            );
            self.span.record(
                "turn.id",
                active.turn_correlation.as_deref().unwrap_or("none"),
            );
            self.span.record(
                "session.id",
                active.session_correlation.as_deref().unwrap_or("none"),
            );
        }
    }

    pub fn update_usage(&self, usage: ToolUsage) {
        if let Ok(mut state) = self.registry.inner.lock()
            && let Some(active) = state.active.get_mut(&self.operation_id)
        {
            let queue_wait_ms = active.usage.queue_wait_ms;
            active.usage = usage;
            active.usage.queue_wait_ms = active.usage.queue_wait_ms.max(queue_wait_ms);
        }
    }

    pub fn mark_artifact_created(&self) {
        if let Ok(mut state) = self.registry.inner.lock()
            && let Some(active) = state.active.get_mut(&self.operation_id)
        {
            active.artifact_created = true;
        }
    }

    pub fn finish(
        mut self,
        status: ToolStatus,
        mut usage: ToolUsage,
        error_code: Option<&str>,
        truncated: bool,
    ) {
        self.finish_inner(status, &mut usage, error_code, truncated);
        self.finished = true;
    }

    fn finish_inner(
        &self,
        status: ToolStatus,
        usage: &mut ToolUsage,
        error_code: Option<&str>,
        truncated: bool,
    ) {
        if !self.registry.enabled {
            return;
        }
        let active_usage = self.registry.inner.lock().ok().and_then(|state| {
            state
                .active
                .get(&self.operation_id)
                .map(|active| active.usage.clone())
        });
        if let Some(active_usage) = active_usage {
            usage.queue_wait_ms = usage.queue_wait_ms.max(active_usage.queue_wait_ms);
        }
        let elapsed_ms = self
            .registry
            .inner
            .lock()
            .ok()
            .and_then(|state| {
                state
                    .active
                    .get(&self.operation_id)
                    .map(|active| elapsed_ms(active.started))
            })
            .unwrap_or(usage.elapsed_ms);
        usage.elapsed_ms = usage.elapsed_ms.max(elapsed_ms);
        self.span.record("result.status", status.label());
        self.span.record("usage.elapsed_ms", usage.elapsed_ms);
        if let Some(code) = error_code {
            self.span.record("error.code", safe_error_code(code));
        }
        tracing::event!(
            parent: &self.span,
            tracing::Level::INFO,
            result.status = status.label(),
            error.code = error_code.map(safe_error_code).unwrap_or("none"),
            usage.elapsed_ms = usage.elapsed_ms,
            usage.queue_wait_ms = usage.queue_wait_ms,
            usage.bytes_read = usage.bytes_read.unwrap_or(0),
            usage.bytes_written = usage.bytes_written.unwrap_or(0),
            usage.output_bytes = usage.output_bytes,
            "tool call finished"
        );
        let Ok(mut state) = self.registry.inner.lock() else {
            return;
        };
        let Some(active) = state.active.remove(&self.operation_id) else {
            return;
        };
        if !state.completed_ids.insert(self.operation_id.clone()) {
            return;
        }
        state.completed_order.push_back(self.operation_id.clone());
        while state.completed_order.len() > COMPLETED_ID_LIMIT {
            if let Some(expired) = state.completed_order.pop_front() {
                state.completed_ids.remove(&expired);
            }
        }
        let metric = state
            .metrics
            .entry((active.tool.to_owned(), active.class, status))
            .or_default();
        metric.calls = metric.calls.saturating_add(1);
        metric.duration_ms = metric.duration_ms.saturating_add(usage.elapsed_ms);
        metric.max_duration_ms = metric.max_duration_ms.max(usage.elapsed_ms);
        observe_histogram(&mut metric.duration_ms_histogram, usage.elapsed_ms);
        metric.queue_wait_ms = metric.queue_wait_ms.saturating_add(usage.queue_wait_ms);
        observe_histogram(&mut metric.queue_wait_ms_histogram, usage.queue_wait_ms);
        metric.files_scanned = metric
            .files_scanned
            .saturating_add(usage.files_scanned.unwrap_or(0));
        metric.entries_scanned = metric
            .entries_scanned
            .saturating_add(usage.entries_scanned.unwrap_or(0));
        metric.bytes_read = metric
            .bytes_read
            .saturating_add(usage.bytes_read.unwrap_or(0));
        metric.bytes_written = metric
            .bytes_written
            .saturating_add(usage.bytes_written.unwrap_or(0));
        metric.output_bytes = metric.output_bytes.saturating_add(usage.output_bytes);
        metric.progress_events = metric
            .progress_events
            .saturating_add(u64::from(usage.progress_events));
        metric.retries = metric.retries.saturating_add(u64::from(usage.retries));
        metric.truncations = metric.truncations.saturating_add(u64::from(truncated));
        metric.artifact_externalizations = metric
            .artifact_externalizations
            .saturating_add(u64::from(active.artifact_created));
        if self.registry.history_limit > 0 {
            state.history.push_back(TerminalRecord);
            while state.history.len() > self.registry.history_limit {
                state.history.pop_front();
            }
        }
    }
}

impl Drop for ToolCallTelemetry {
    fn drop(&mut self) {
        if !self.finished {
            let mut usage = ToolUsage::default();
            self.finish_inner(
                ToolStatus::Cancelled,
                &mut usage,
                Some("futureDropped"),
                false,
            );
        }
    }
}

impl ActiveState {
    fn view(&self) -> ActiveToolOperation {
        let mut usage = self.usage.clone();
        usage.elapsed_ms = usage.elapsed_ms.max(elapsed_ms(self.started));
        ActiveToolOperation {
            operation_id: self.operation_id.clone(),
            tool: self.tool.to_owned(),
            class: self.class,
            phase: self.phase,
            elapsed_ms: usage.elapsed_ms,
            request_correlation: self.request_correlation.clone(),
            task_correlation: self.task_correlation.clone(),
            turn_correlation: self.turn_correlation.clone(),
            session_correlation: self.session_correlation.clone(),
            usage,
        }
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn observe_histogram(buckets: &mut [u64; 8], value: u64) {
    if let Some((index, _)) = HISTOGRAM_BOUNDS_MS
        .iter()
        .enumerate()
        .find(|(_, upper)| value <= **upper)
    {
        buckets[index] = buckets[index].saturating_add(1);
    }
}

fn correlation_hash(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut encoded = String::with_capacity(16);
    for byte in &digest[..8] {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn safe_error_code(code: &str) -> &'static str {
    match code {
        "operationCancelled" | "cancelled" | "activity_stopped" => "cancelled",
        "timeBudgetExceeded" | "timeout" | "timed_out" => "timeout",
        "policy_denied" | "unauthorized" | "approval_required" => "policy",
        "not_found" | "device_not_found" | "tool_not_found" => "notFound",
        "conflict" | "version_conflict" => "conflict",
        "byteBudgetExceeded" | "fileBudgetExceeded" | "outputBudgetExceeded" => "budget",
        _ => "internal",
    }
}

fn tool_class(tool: &str) -> ToolClass {
    if tool.starts_with("git_") {
        ToolClass::Git
    } else if tool.starts_with("shell_") {
        ToolClass::Shell
    } else if tool.starts_with("agent_") || tool.starts_with("task_") {
        ToolClass::Agent
    } else if tool.starts_with("blob_") {
        ToolClass::Blob
    } else if tool.starts_with("process_") {
        ToolClass::Process
    } else if matches!(tool, "fs_search" | "fs_find" | "fs_list" | "fs_list_v2") {
        ToolClass::Search
    } else if matches!(tool, "fs_read_text" | "fs_read_text_v2" | "fs_stat") {
        ToolClass::Read
    } else if tool.starts_with("fs_") {
        ToolClass::Mutation
    } else {
        ToolClass::Other
    }
}

fn tool_label(tool: &str) -> &'static str {
    match tool {
        "device_list" => "device_list",
        "device_get" => "device_get",
        "shell_create" => "shell_create",
        "shell_write" => "shell_write",
        "shell_wait" => "shell_wait",
        "shell_read" => "shell_read",
        "shell_signal" => "shell_signal",
        "shell_resize" => "shell_resize",
        "shell_close" => "shell_close",
        "shell_list" => "shell_list",
        "shell_inspect" => "shell_inspect",
        "workspace_roots" => "workspace_roots",
        "blob_begin" => "blob_begin",
        "blob_write_chunk" => "blob_write_chunk",
        "blob_status" => "blob_status",
        "blob_seal" => "blob_seal",
        "blob_abort" => "blob_abort",
        "fs_list" => "fs_list",
        "fs_list_v2" => "fs_list_v2",
        "fs_search" => "fs_search",
        "fs_find" => "fs_find",
        "fs_read_text" => "fs_read_text",
        "fs_read_text_v2" => "fs_read_text_v2",
        "fs_write_text" => "fs_write_text",
        "fs_replace_text" => "fs_replace_text",
        "fs_apply_edits" => "fs_apply_edits",
        "fs_write_raw" => "fs_write_raw",
        "fs_stat" => "fs_stat",
        "fs_create_directory" => "fs_create_directory",
        "fs_copy" => "fs_copy",
        "fs_move" => "fs_move",
        "fs_delete" => "fs_delete",
        "git_status" => "git_status",
        "git_diff" => "git_diff",
        "git_log" => "git_log",
        "git_branch" => "git_branch",
        "git_show" => "git_show",
        "git_commit" => "git_commit",
        "process_list" => "process_list",
        "process_inspect" => "process_inspect",
        "process_kill" => "process_kill",
        "skills_list" => "skills_list",
        "skill_read" => "skill_read",
        "task_get" => "task_get",
        "task_list" => "task_list",
        "task_set_execution_mode" => "task_set_execution_mode",
        "task_artifact_list" => "task_artifact_list",
        "task_artifact_read" => "task_artifact_read",
        "agent_user_message" => "agent_user_message",
        "agent_progress" => "agent_progress",
        "agent_plan_question" => "agent_plan_question",
        "agent_subagent_start" => "agent_subagent_start",
        "agent_subagent_wait" => "agent_subagent_wait",
        "agent_turn_complete" => "agent_turn_complete",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Write};
    use tracing_subscriber::fmt::MakeWriter;

    #[derive(Clone)]
    struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

    struct CaptureGuard(Arc<Mutex<Vec<u8>>>);

    impl Write for CaptureGuard {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| io::Error::other("capture lock poisoned"))?
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for CaptureWriter {
        type Writer = CaptureGuard;

        fn make_writer(&'a self) -> Self::Writer {
            CaptureGuard(self.0.clone())
        }
    }

    fn context(request: &str) -> OperationContext {
        let mut context = OperationContext::new(request, "secret-agent", "fs_search");
        context.task_id = Some("private-task".to_owned());
        context.turn_id = Some("private-turn".to_owned());
        context.mcp_session_id = Some("private-session".to_owned());
        context.conversation_scope_id = Some("password=marker".to_owned());
        context
    }

    #[test]
    fn terminal_status_and_usage_are_counted_once() {
        let registry = ToolTelemetryRegistry::default();
        let call = registry.start(&context("request-1"), "fs_search");
        call.finish(
            ToolStatus::Success,
            ToolUsage {
                elapsed_ms: 7,
                queue_wait_ms: 2,
                files_scanned: Some(3),
                bytes_read: Some(11),
                output_bytes: 5,
                ..ToolUsage::default()
            },
            None,
            true,
        );
        let duplicate = registry.start(&context("request-1"), "fs_search");
        duplicate.finish(
            ToolStatus::Failure,
            ToolUsage::default(),
            Some("secret-error"),
            false,
        );

        let snapshot = registry.snapshot();
        assert_eq!(snapshot.metrics.len(), 1);
        assert_eq!(snapshot.metrics[0].status, ToolStatus::Success);
        assert_eq!(snapshot.metrics[0].metric.calls, 1);
        assert_eq!(snapshot.metrics[0].metric.files_scanned, 3);
        assert_eq!(snapshot.metrics[0].metric.bytes_read, 11);
        assert_eq!(snapshot.metrics[0].metric.queue_wait_ms, 2);
        assert!(snapshot.metrics[0].metric.duration_ms >= 7);
        assert_eq!(
            snapshot.metrics[0]
                .metric
                .duration_ms_histogram
                .iter()
                .sum::<u64>(),
            1
        );
        assert_eq!(snapshot.metrics[0].metric.truncations, 1);
    }

    #[test]
    fn diagnostics_redact_identifiers_content_and_unknown_labels() {
        let registry = ToolTelemetryRegistry::default();
        let call = registry.start(&context("authorization=secret"), "user_supplied_tool_name");
        call.set_phase(ToolPhase::Reading);
        let encoded = serde_json::to_string(&registry.snapshot()).expect("serialize snapshot");
        assert!(!encoded.contains("authorization=secret"));
        assert!(!encoded.contains("private-task"));
        assert!(!encoded.contains("password=marker"));
        assert!(!encoded.contains("user_supplied_tool_name"));
        assert!(encoded.contains("unknown"));
        assert!(encoded.contains("reading"));
    }

    #[test]
    fn captured_spans_and_terminal_logs_do_not_contain_sensitive_markers() {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(CaptureWriter(bytes.clone()))
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            let registry = ToolTelemetryRegistry::default();
            let call = registry.start(&context("token=top-secret"), "untrusted-password-tool");
            call.finish(
                ToolStatus::Failure,
                ToolUsage::default(),
                Some("authorization=private-path-C:/secret"),
                false,
            );
            tracing::info!("capture probe");
        });
        let captured =
            String::from_utf8(bytes.lock().expect("capture lock").clone()).expect("logs are utf-8");
        assert!(!captured.contains("top-secret"));
        assert!(!captured.contains("password"));
        assert!(!captured.contains("C:/secret"));
        assert!(!captured.contains("private-task"));
        assert!(captured.contains("capture probe"));
    }

    #[test]
    fn diagnostics_snapshot_is_bounded_for_many_active_operations() {
        let registry = ToolTelemetryRegistry::with_limits(true, 8, 16);
        let calls = (0..2_000)
            .map(|index| registry.start(&context(&format!("request-{index}")), "fs_search"))
            .collect::<Vec<_>>();
        let snapshot = registry.snapshot();
        assert_eq!(snapshot.active_operations_total, 2_000);
        assert_eq!(snapshot.active_operations.len(), 16);
        assert_eq!(snapshot.active_operations_omitted, 1_984);
        drop(calls);
        assert_eq!(registry.snapshot().history_size, 8);
    }

    #[test]
    fn counters_saturate_instead_of_overflowing() {
        let registry = ToolTelemetryRegistry::default();
        let call = registry.start(&context("overflow"), "fs_search");
        call.finish(
            ToolStatus::Success,
            ToolUsage {
                bytes_read: Some(u64::MAX),
                output_bytes: u64::MAX,
                ..ToolUsage::default()
            },
            None,
            false,
        );
        let call = registry.start(&context("overflow-2"), "fs_search");
        call.finish(
            ToolStatus::Success,
            ToolUsage {
                bytes_read: Some(1),
                output_bytes: 1,
                ..ToolUsage::default()
            },
            None,
            false,
        );
        let metric = &registry.snapshot().metrics[0].metric;
        assert_eq!(metric.bytes_read, u64::MAX);
        assert_eq!(metric.output_bytes, u64::MAX);
    }

    #[test]
    fn every_terminal_status_increments_exactly_one_counter() {
        let registry = ToolTelemetryRegistry::default();
        for (index, status) in [
            ToolStatus::Success,
            ToolStatus::Failure,
            ToolStatus::Cancelled,
            ToolStatus::Timeout,
        ]
        .into_iter()
        .enumerate()
        {
            registry
                .start(&context(&format!("terminal-{index}")), "fs_search")
                .finish(status, ToolUsage::default(), None, false);
        }
        let snapshot = registry.snapshot();
        assert_eq!(snapshot.metrics.len(), 4);
        assert!(
            snapshot
                .metrics
                .iter()
                .all(|metric| metric.metric.calls == 1)
        );
    }

    #[test]
    fn progress_metrics_distinguish_emitted_coalesced_and_dropped() {
        let registry = ToolTelemetryRegistry::default();
        registry.record_progress(3, 2, 1);
        registry.record_progress(1, 1, 1);
        assert_eq!(
            registry.snapshot().progress_events,
            ProgressMetrics {
                emitted: 4,
                coalesced: 3,
                dropped: 2,
            }
        );
    }

    #[test]
    fn resource_and_subagent_metrics_use_typed_bounded_counters() {
        let registry = ToolTelemetryRegistry::default();
        registry.set_blob_bytes(10);
        registry.set_persisted_resource_usage(20, 3);
        registry.record_subagent_lease_expired(SubagentLeaseExpiryReason::Heartbeat);
        registry.record_subagent_lease_expired(SubagentLeaseExpiryReason::HardDeadline);
        registry.record_subagent_lease_expired(SubagentLeaseExpiryReason::WorkerRestart);
        let snapshot = registry.snapshot();
        assert_eq!(snapshot.blob_bytes, 10);
        assert_eq!(snapshot.artifact_bytes, 20);
        assert_eq!(snapshot.operation_journal_active, 3);
        assert_eq!(snapshot.subagent_lease_expired.heartbeat, 1);
        assert_eq!(snapshot.subagent_lease_expired.hard_deadline, 1);
        assert_eq!(snapshot.subagent_lease_expired.worker_restart, 1);
    }

    #[test]
    fn poisoned_telemetry_backend_does_not_panic_or_change_call_result() {
        let registry = ToolTelemetryRegistry::default();
        let inner = registry.inner.clone();
        let _ = std::panic::catch_unwind(move || {
            let _guard = inner.lock().expect("lock registry");
            panic!("simulate telemetry backend failure");
        });
        let call = registry.start(&context("backend-failure"), "fs_search");
        call.set_phase(ToolPhase::Scanning);
        call.finish(ToolStatus::Success, ToolUsage::default(), None, false);
        assert!(registry.snapshot().metrics.is_empty());
    }
}
