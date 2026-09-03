//! Shared resource budgets, cooperative cancellation, and admission control.

use crate::{RuntimeError, RuntimeResult};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

/// Limits for one tool operation. `None` means that the corresponding policy
/// layer does not impose a limit; it never removes a limit from another layer.
#[derive(Debug, Clone, Default)]
pub struct ToolBudget {
    pub deadline: Option<Instant>,
    pub max_files: Option<u64>,
    pub max_entries: Option<u64>,
    pub max_bytes_read: Option<u64>,
    pub max_bytes_written: Option<u64>,
    pub max_output_bytes: Option<u64>,
    pub max_open_files: Option<u32>,
    pub max_progress_events: Option<u32>,
    pub memory_reservation_bytes: Option<u64>,
}

impl ToolBudget {
    /// Intersects policy layers. A caller can lower, but never raise, a limit.
    #[must_use]
    pub fn intersect<'a>(layers: impl IntoIterator<Item = &'a Self>) -> Self {
        let mut effective = Self::default();
        for layer in layers {
            effective.deadline = earlier(effective.deadline, layer.deadline);
            effective.max_files = lower(effective.max_files, layer.max_files);
            effective.max_entries = lower(effective.max_entries, layer.max_entries);
            effective.max_bytes_read = lower(effective.max_bytes_read, layer.max_bytes_read);
            effective.max_bytes_written =
                lower(effective.max_bytes_written, layer.max_bytes_written);
            effective.max_output_bytes = lower(effective.max_output_bytes, layer.max_output_bytes);
            effective.max_open_files = lower(effective.max_open_files, layer.max_open_files);
            effective.max_progress_events =
                lower(effective.max_progress_events, layer.max_progress_events);
            effective.memory_reservation_bytes = lower(
                effective.memory_reservation_bytes,
                layer.memory_reservation_bytes,
            );
        }
        effective
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.deadline = Some(Instant::now() + timeout.max(Duration::from_millis(1)));
        self
    }
}

fn lower<T: Ord + Copy>(left: Option<T>, right: Option<T>) -> Option<T> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (left @ Some(_), None) => left,
        (None, right) => right,
    }
}

fn earlier(left: Option<Instant>, right: Option<Instant>) -> Option<Instant> {
    lower(left, right)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BudgetUsage {
    pub files: u64,
    pub entries: u64,
    pub bytes_read: u64,
    pub bytes_written: u64,
    pub output_bytes: u64,
    pub progress_events: u64,
    pub elapsed_ms: u64,
}

struct BudgetCounters {
    files: AtomicU64,
    entries: AtomicU64,
    bytes_read: AtomicU64,
    bytes_written: AtomicU64,
    output_bytes: AtomicU64,
    progress_events: AtomicU64,
}

/// Thread-safe tracker suitable for async tasks and `spawn_blocking` workers.
#[derive(Clone)]
pub struct BudgetTracker {
    cancellation: CancellationToken,
    budget: ToolBudget,
    started: Instant,
    phase: Arc<Mutex<String>>,
    counters: Arc<BudgetCounters>,
}

impl BudgetTracker {
    #[must_use]
    pub fn new(cancellation: CancellationToken, budget: ToolBudget) -> Self {
        Self {
            cancellation,
            budget,
            started: Instant::now(),
            phase: Arc::new(Mutex::new("starting".to_owned())),
            counters: Arc::new(BudgetCounters {
                files: AtomicU64::new(0),
                entries: AtomicU64::new(0),
                bytes_read: AtomicU64::new(0),
                bytes_written: AtomicU64::new(0),
                output_bytes: AtomicU64::new(0),
                progress_events: AtomicU64::new(0),
            }),
        }
    }

    pub fn set_phase(&self, phase: impl Into<String>) {
        if let Ok(mut current) = self.phase.lock() {
            *current = phase.into();
        }
    }

    pub fn checkpoint(&self) -> RuntimeResult<()> {
        if self.cancellation.is_cancelled() {
            return Err(self.failure("operationCancelled", false));
        }
        if self
            .budget
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(self.failure("timeBudgetExceeded", true));
        }
        Ok(())
    }

    pub fn consume_files(&self, amount: u64) -> RuntimeResult<()> {
        self.consume(
            &self.counters.files,
            amount,
            self.budget.max_files,
            "fileBudgetExceeded",
        )
    }

    pub fn consume_entries(&self, amount: u64) -> RuntimeResult<()> {
        self.consume(
            &self.counters.entries,
            amount,
            self.budget.max_entries,
            "entryBudgetExceeded",
        )
    }

    pub fn consume_read_bytes(&self, amount: u64) -> RuntimeResult<()> {
        self.consume(
            &self.counters.bytes_read,
            amount,
            self.budget.max_bytes_read,
            "byteBudgetExceeded",
        )
    }

    pub fn consume_write_bytes(&self, amount: u64) -> RuntimeResult<()> {
        self.consume(
            &self.counters.bytes_written,
            amount,
            self.budget.max_bytes_written,
            "byteBudgetExceeded",
        )
    }

    pub fn reserve_output(&self, amount: u64) -> RuntimeResult<()> {
        self.consume(
            &self.counters.output_bytes,
            amount,
            self.budget.max_output_bytes,
            "outputBudgetExceeded",
        )
    }

    pub fn consume_progress_event(&self) -> RuntimeResult<()> {
        self.consume(
            &self.counters.progress_events,
            1,
            self.budget.max_progress_events.map(u64::from),
            "progressBudgetExceeded",
        )
    }

    /// Records counters for operations that expose budget exhaustion as a partial result.
    /// The caller must perform `checkpoint` and enforce its cursor-safe boundary separately.
    pub fn record_files(&self, amount: u64) {
        saturating_add(&self.counters.files, amount);
    }

    pub fn record_entries(&self, amount: u64) {
        saturating_add(&self.counters.entries, amount);
    }

    pub fn record_read_bytes(&self, amount: u64) {
        saturating_add(&self.counters.bytes_read, amount);
    }

    fn consume(
        &self,
        counter: &AtomicU64,
        amount: u64,
        limit: Option<u64>,
        code: &str,
    ) -> RuntimeResult<()> {
        self.checkpoint()?;
        let updated = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            let next = current.saturating_add(amount);
            limit.is_none_or(|limit| next <= limit).then_some(next)
        });
        if updated.is_err() {
            return Err(self.failure(code, false));
        }
        Ok(())
    }

    #[must_use]
    pub fn finish_usage(&self) -> BudgetUsage {
        BudgetUsage {
            files: self.counters.files.load(Ordering::Relaxed),
            entries: self.counters.entries.load(Ordering::Relaxed),
            bytes_read: self.counters.bytes_read.load(Ordering::Relaxed),
            bytes_written: self.counters.bytes_written.load(Ordering::Relaxed),
            output_bytes: self.counters.output_bytes.load(Ordering::Relaxed),
            progress_events: self.counters.progress_events.load(Ordering::Relaxed),
            elapsed_ms: u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
        }
    }

    fn failure(&self, code: &str, retryable: bool) -> RuntimeError {
        let phase = self
            .phase
            .lock()
            .map_or_else(|_| "unknown".to_owned(), |value| value.clone());
        let usage = serde_json::to_string(&self.finish_usage()).unwrap_or_else(|_| "{}".to_owned());
        let mut error =
            RuntimeError::new(code, format!("tool stopped during {phase}; usage={usage}"));
        error.retryable = retryable;
        error
    }
}

fn saturating_add(counter: &AtomicU64, amount: u64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        Some(current.saturating_add(amount))
    });
}

/// Rejects work immediately when host, actor, or memory capacity is exhausted.
#[derive(Clone)]
pub struct AdmissionController {
    global: Arc<Semaphore>,
    per_actor_limit: usize,
    actors: Arc<Mutex<HashMap<String, Arc<Semaphore>>>>,
    memory_limit: u64,
    memory_used: Arc<AtomicU64>,
}

impl AdmissionController {
    #[must_use]
    pub fn new(global_slots: usize, per_actor_limit: usize, memory_limit: u64) -> Self {
        Self {
            global: Arc::new(Semaphore::new(global_slots.max(1))),
            per_actor_limit: per_actor_limit.max(1),
            actors: Arc::new(Mutex::new(HashMap::new())),
            memory_limit,
            memory_used: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn try_admit(
        &self,
        actor: &str,
        weight: u32,
        memory: u64,
    ) -> RuntimeResult<AdmissionPermit> {
        let weight = weight.max(1);
        let actor_semaphore = {
            let mut actors = self
                .actors
                .lock()
                .map_err(|_| RuntimeError::busy("admission registry unavailable"))?;
            actors
                .entry(actor.to_owned())
                .or_insert_with(|| Arc::new(Semaphore::new(self.per_actor_limit)))
                .clone()
        };
        let global = self
            .global
            .clone()
            .try_acquire_many_owned(weight)
            .map_err(|_| admission_denied())?;
        let actor = actor_semaphore
            .try_acquire_owned()
            .map_err(|_| admission_denied())?;
        reserve_memory(&self.memory_used, self.memory_limit, memory)?;
        Ok(AdmissionPermit {
            _global: global,
            _actor: actor,
            memory,
            memory_used: self.memory_used.clone(),
        })
    }
}

fn reserve_memory(used: &AtomicU64, limit: u64, amount: u64) -> RuntimeResult<()> {
    used.fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
        current.checked_add(amount).filter(|next| *next <= limit)
    })
    .map(|_| ())
    .map_err(|_| admission_denied())
}

fn admission_denied() -> RuntimeError {
    let mut error = RuntimeError::new("admissionDenied", "tool capacity is busy; retry later");
    error.retryable = true;
    error
}

#[derive(Debug)]
pub struct AdmissionPermit {
    _global: OwnedSemaphorePermit,
    _actor: OwnedSemaphorePermit,
    memory: u64,
    memory_used: Arc<AtomicU64>,
}

impl Drop for AdmissionPermit {
    fn drop(&mut self) {
        self.memory_used.fetch_sub(self.memory, Ordering::AcqRel);
    }
}

/// Coalesces frequent progress and always permits a terminal update.
pub struct ProgressLimiter {
    interval: Duration,
    max_events: u32,
    state: Mutex<(u32, Option<Instant>)>,
}

impl ProgressLimiter {
    #[must_use]
    pub fn new(max_events_per_second: u32, max_events: u32) -> Self {
        let interval = if max_events_per_second == 0 {
            Duration::MAX
        } else {
            Duration::from_secs_f64(1.0 / f64::from(max_events_per_second))
        };
        Self {
            interval,
            max_events,
            state: Mutex::new((0, None)),
        }
    }

    #[must_use]
    pub fn should_emit(&self, terminal: bool) -> bool {
        if terminal {
            return true;
        }
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if state.0 >= self.max_events {
            return false;
        }
        let now = Instant::now();
        if state
            .1
            .is_some_and(|last| now.duration_since(last) < self.interval)
        {
            return false;
        }
        state.0 = state.0.saturating_add(1);
        state.1 = Some(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_intersection_never_allows_caller_to_raise_caps() {
        let hard = ToolBudget {
            max_files: Some(10),
            max_bytes_read: Some(100),
            ..ToolBudget::default()
        };
        let caller = ToolBudget {
            max_files: Some(99),
            max_bytes_read: Some(50),
            ..ToolBudget::default()
        };
        let effective = ToolBudget::intersect([&hard, &caller]);
        assert_eq!(effective.max_files, Some(10));
        assert_eq!(effective.max_bytes_read, Some(50));
    }

    #[test]
    fn counters_are_exact_at_boundary_and_do_not_overshoot() {
        let tracker = BudgetTracker::new(
            CancellationToken::new(),
            ToolBudget {
                max_bytes_read: Some(8),
                ..ToolBudget::default()
            },
        );
        tracker.consume_read_bytes(8).expect("boundary accepted");
        assert_eq!(
            tracker.consume_read_bytes(1).expect_err("over budget").code,
            "byteBudgetExceeded"
        );
        assert_eq!(tracker.finish_usage().bytes_read, 8);
    }

    #[test]
    fn cancellation_is_typed_and_includes_usage() {
        let token = CancellationToken::new();
        let tracker = BudgetTracker::new(token.clone(), ToolBudget::default());
        tracker.consume_entries(2).expect("within budget");
        token.cancel();
        let error = tracker.checkpoint().expect_err("cancelled");
        assert_eq!(error.code, "operationCancelled");
        assert!(error.message.contains("\"entries\":2"));
    }

    #[test]
    fn admission_rejects_and_releases_capacity() {
        let controller = AdmissionController::new(1, 1, 10);
        let permit = controller.try_admit("actor", 1, 10).expect("first permit");
        let error = controller
            .try_admit("actor", 1, 1)
            .expect_err("capacity exhausted");
        assert_eq!(error.code, "admissionDenied");
        assert!(error.retryable);
        drop(permit);
        controller.try_admit("actor", 1, 10).expect("released");
    }

    #[test]
    fn progress_is_bounded_but_terminal_is_always_allowed() {
        let limiter = ProgressLimiter::new(u32::MAX, 1);
        assert!(limiter.should_emit(false));
        assert!(!limiter.should_emit(false));
        assert!(limiter.should_emit(true));
    }
}
