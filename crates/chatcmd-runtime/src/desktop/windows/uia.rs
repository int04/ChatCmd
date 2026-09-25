//! Recoverable Windows UI Automation sessions.
//!
//! Each generation owns its COM objects on one worker thread. A timed-out or disconnected
//! generation is abandoned and the next request creates a fresh worker, so one bad provider
//! cannot permanently poison desktop automation.

mod focus;
mod worker;

use super::*;
use std::{
    sync::{
        Arc, Mutex,
        mpsc::{self, SyncSender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

const REQUEST_CAPACITY: usize = 8;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);

type Snapshot = (Vec<(DesktopElement, NativeElement)>, bool);
type Reply<T> = SyncSender<RuntimeResult<T>>;

enum Request {
    Snapshot {
        handle: isize,
        reply: Reply<Snapshot>,
    },
    Act {
        handle: isize,
        target: NativeElement,
        action: DesktopElementAction,
        reply: Reply<()>,
    },
    FocusedElementSafety {
        handle: isize,
        reply: Reply<FocusedElementSafety>,
    },
    Shutdown,
}

/// Read-only classification of the focused element before injected keyboard input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FocusedElementSafety {
    pub(super) password: bool,
    pub(super) authentication_surface: bool,
}

impl FocusedElementSafety {
    pub(super) fn denies_keyboard_input(self) -> bool {
        self.password || self.authentication_surface
    }
}

struct Generation {
    id: u64,
    requests: SyncSender<Request>,
    _worker: JoinHandle<()>,
}

impl Generation {
    fn start(id: u64, startup_timeout: Duration) -> RuntimeResult<Self> {
        let (request_tx, request_rx) = mpsc::sync_channel(REQUEST_CAPACITY);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name(format!("chatcmd-uia-{id}"))
            .spawn(move || worker::run(request_rx, ready_tx))
            .map_err(|error| {
                backend_error(format!("failed to start UI Automation worker: {error}"))
            })?;
        let generation = Self {
            id,
            requests: request_tx,
            _worker: worker,
        };
        match ready_rx.recv_timeout(startup_timeout) {
            Ok(Ok(())) => Ok(generation),
            Ok(Err(error)) => Err(error),
            Err(_) => {
                let mut error = RuntimeError::new(
                    "desktop_uia_startup_timeout",
                    "UI Automation worker initialization timed out; retry the operation",
                );
                error.retryable = true;
                Err(error)
            }
        }
    }
}

impl Drop for Generation {
    fn drop(&mut self) {
        // Never join here: a provider may have permanently blocked the COM worker. Dropping its
        // JoinHandle detaches it, while a healthy worker receives Shutdown and releases COM.
        let _ = self.requests.try_send(Request::Shutdown);
    }
}

#[derive(Default)]
struct SessionState {
    current: Option<Arc<Generation>>,
    next_generation: u64,
}

struct SessionInner {
    state: Mutex<SessionState>,
    startup_timeout: Duration,
    request_timeout: Duration,
}

/// A cheap, cloneable UIA session that replaces unresponsive worker generations.
#[derive(Clone)]
pub(super) struct UiAutomationSession {
    inner: Arc<SessionInner>,
}

impl UiAutomationSession {
    #[must_use]
    pub(super) fn new() -> Self {
        Self::with_timeouts(STARTUP_TIMEOUT, REQUEST_TIMEOUT)
    }

    pub(super) fn snapshot(&self, handle: isize) -> RuntimeResult<Snapshot> {
        let generation = self.generation()?;
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.send(
            &generation,
            Request::Snapshot {
                handle,
                reply: reply_tx,
            },
        )?;
        self.receive(&generation, reply_rx)
    }

    pub(super) fn act(
        &self,
        handle: isize,
        target: &NativeElement,
        action: &DesktopElementAction,
    ) -> RuntimeResult<()> {
        let generation = self.generation()?;
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.send(
            &generation,
            Request::Act {
                handle,
                target: target.clone(),
                action: action.clone(),
                reply: reply_tx,
            },
        )?;
        self.receive_action(&generation, reply_rx)
    }

    pub(super) fn focused_element_safety(
        &self,
        handle: isize,
    ) -> RuntimeResult<FocusedElementSafety> {
        let generation = self.generation()?;
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.send(
            &generation,
            Request::FocusedElementSafety {
                handle,
                reply: reply_tx,
            },
        )?;
        self.receive(&generation, reply_rx)
    }

    fn with_timeouts(startup_timeout: Duration, request_timeout: Duration) -> Self {
        Self {
            inner: Arc::new(SessionInner {
                state: Mutex::new(SessionState::default()),
                startup_timeout,
                request_timeout,
            }),
        }
    }

    fn generation(&self) -> RuntimeResult<Arc<Generation>> {
        let mut state = lock_state(&self.inner.state);
        if let Some(generation) = &state.current {
            return Ok(generation.clone());
        }
        state.next_generation = state.next_generation.saturating_add(1);
        let generation = Arc::new(Generation::start(
            state.next_generation,
            self.inner.startup_timeout,
        )?);
        state.current = Some(generation.clone());
        Ok(generation)
    }

    fn send(&self, generation: &Arc<Generation>, request: Request) -> RuntimeResult<()> {
        generation
            .requests
            .try_send(request)
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => {
                    RuntimeError::busy("UI Automation request queue is full; retry shortly")
                }
                mpsc::TrySendError::Disconnected(_) => {
                    self.invalidate(generation.id);
                    backend_error("UI Automation worker stopped; the next request will restart it")
                }
            })
    }

    fn receive<T>(
        &self,
        generation: &Arc<Generation>,
        receiver: mpsc::Receiver<RuntimeResult<T>>,
    ) -> RuntimeResult<T> {
        match receiver.recv_timeout(self.inner.request_timeout) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.invalidate(generation.id);
                let mut error = RuntimeError::new(
                    "desktop_uia_timeout",
                    "UI Automation timed out; its worker was replaced for the next request",
                );
                error.retryable = true;
                Err(error)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.invalidate(generation.id);
                Err(backend_error(
                    "UI Automation worker stopped; the next request will restart it",
                ))
            }
        }
    }

    fn receive_action(
        &self,
        generation: &Arc<Generation>,
        receiver: mpsc::Receiver<RuntimeResult<()>>,
    ) -> RuntimeResult<()> {
        match receiver.recv_timeout(self.inner.request_timeout) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {
                self.invalidate(generation.id);
                Err(RuntimeError::new(
                    "desktop_action_outcome_unknown",
                    "the UI Automation action may have completed before its worker stopped responding; observe the window again and never repeat the action automatically",
                ))
            }
        }
    }

    fn invalidate(&self, generation_id: u64) {
        let mut state = lock_state(&self.inner.state);
        if state.current.as_ref().map(|value| value.id) == Some(generation_id) {
            state.current = None;
        }
    }
}

impl Default for UiAutomationSession {
    fn default() -> Self {
        Self::new()
    }
}

fn lock_state(mutex: &Mutex<SessionState>) -> std::sync::MutexGuard<'_, SessionState> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn session_with_generation(
        request_timeout: Duration,
        worker: impl FnOnce(mpsc::Receiver<Request>) + Send + 'static,
    ) -> (UiAutomationSession, Arc<Generation>) {
        let (requests, receiver) = mpsc::sync_channel(REQUEST_CAPACITY);
        let generation = Arc::new(Generation {
            id: 1,
            requests,
            _worker: thread::spawn(move || worker(receiver)),
        });
        let session =
            UiAutomationSession::with_timeouts(Duration::from_millis(10), request_timeout);
        lock_state(&session.inner.state).current = Some(generation.clone());
        (session, generation)
    }

    fn test_element() -> NativeElement {
        NativeElement {
            path: vec![0],
            signature: ElementSignature {
                runtime_id: vec![1, 2],
                name: "Save".to_owned(),
                automation_id: "save".to_owned(),
                control_type: "button".to_owned(),
                class_name: "Button".to_owned(),
            },
        }
    }

    #[test]
    fn disconnected_generation_is_invalidated() {
        let (session, generation) = session_with_generation(Duration::from_millis(20), drop);
        while !generation._worker.is_finished() {
            thread::yield_now();
        }

        let error = session.snapshot(1).expect_err("worker is disconnected");

        assert_eq!(error.code, "desktop_backend_error");
        assert!(lock_state(&session.inner.state).current.is_none());
    }

    #[test]
    fn timed_out_generation_is_detached_without_blocking_drop() {
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let (session, _) = session_with_generation(Duration::from_millis(20), move |requests| {
            let request = requests.recv().ok();
            let _ = release_rx.recv();
            drop(request);
        });
        let started = Instant::now();

        let error = session.snapshot(1).expect_err("worker intentionally hangs");

        assert_eq!(error.code, "desktop_uia_timeout");
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(lock_state(&session.inner.state).current.is_none());
        let _ = release_tx.send(());
    }

    #[test]
    fn action_timeout_reports_unknown_non_retryable_outcome() {
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let (session, _) = session_with_generation(Duration::from_millis(20), move |requests| {
            let request = requests.recv().ok();
            let _ = release_rx.recv();
            drop(request);
        });

        let error = session
            .act(1, &test_element(), &DesktopElementAction::Invoke)
            .expect_err("action worker intentionally hangs");

        assert_eq!(error.code, "desktop_action_outcome_unknown");
        assert!(!error.retryable);
        assert!(error.message.contains("never repeat"));
        assert!(lock_state(&session.inner.state).current.is_none());
        let _ = release_tx.send(());
    }

    #[test]
    fn action_disconnect_after_enqueue_reports_unknown_outcome() {
        let (session, _) = session_with_generation(Duration::from_millis(100), |requests| {
            let _ = requests.recv();
        });

        let error = session
            .act(1, &test_element(), &DesktopElementAction::Invoke)
            .expect_err("worker drops the enqueued action reply");

        assert_eq!(error.code, "desktop_action_outcome_unknown");
        assert!(!error.retryable);
    }

    #[test]
    fn action_send_failure_before_enqueue_remains_retryable_backend_error() {
        let (session, generation) = session_with_generation(Duration::from_millis(20), drop);
        while !generation._worker.is_finished() {
            thread::yield_now();
        }

        let error = session
            .act(1, &test_element(), &DesktopElementAction::Invoke)
            .expect_err("request was never enqueued");

        assert_eq!(error.code, "desktop_backend_error");
        assert!(error.retryable);
    }

    #[test]
    fn focused_safety_combines_password_and_authentication_flags() {
        assert!(
            FocusedElementSafety {
                password: true,
                authentication_surface: false,
            }
            .denies_keyboard_input()
        );
        assert!(
            !FocusedElementSafety {
                password: false,
                authentication_surface: false,
            }
            .denies_keyboard_input()
        );
    }
}
