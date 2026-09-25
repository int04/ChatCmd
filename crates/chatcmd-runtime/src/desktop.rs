//! Window-scoped Windows desktop automation with an explicit input-takeover mode.

mod observe;
mod takeover;
mod types;
#[cfg(not(target_os = "windows"))]
mod unsupported;
mod validation;
#[cfg(target_os = "windows")]
mod windows;

pub use types::*;
use validation::*;

#[cfg(not(target_os = "windows"))]
use unsupported as platform;
#[cfg(target_os = "windows")]
use windows as platform;

use std::{
    collections::HashMap,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, OnceCell};

use crate::{OperationContext, RuntimeError, RuntimeResult};

const OBSERVATION_TTL: Duration = Duration::from_secs(120);
const MAX_OBSERVATIONS: usize = 16;
const MAX_INPUT_ACTIONS: usize = 32;

#[derive(Clone, Default)]
pub struct DesktopControlService {
    state: Arc<Mutex<DesktopState>>,
    input_start_gate: Arc<Mutex<()>>,
}

#[derive(Default)]
struct DesktopState {
    windows: HashMap<String, WindowRecord>,
    observations: HashMap<String, ObservationRecord>,
    input: InputSlot,
}

#[derive(Clone)]
struct WindowRecord {
    owner: Owner,
    native: NativeWindow,
    info: DesktopWindowInfo,
    session: Arc<OnceCell<Arc<platform::WindowSession>>>,
}

struct ObservationRecord {
    owner: Owner,
    target: SessionTarget,
    created_at: Instant,
    elements: HashMap<String, NativeElement>,
}

#[derive(Clone)]
struct SessionTarget {
    window_id: String,
    native: NativeWindow,
    info: DesktopWindowInfo,
    session: Arc<platform::WindowSession>,
}

#[derive(Default)]
enum InputSlot {
    #[default]
    Empty,
    Active(Box<InputRecord>),
}

struct InputRecord {
    id: String,
    owner: Owner,
    target: SessionTarget,
    cancelled: Arc<AtomicBool>,
    action_gate: Arc<Mutex<()>>,
    guard: platform::InputGuard,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Owner {
    agent_id: String,
    task_id: Option<String>,
}

#[derive(Debug, Clone)]
struct NativeWindow {
    handle: isize,
    process_id: u32,
    title: String,
    application: String,
    width: u32,
    height: u32,
    minimized: bool,
}

#[derive(Debug, Clone)]
struct NativeElement {
    path: Vec<usize>,
    signature: ElementSignature,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ElementSignature {
    runtime_id: Vec<i32>,
    name: String,
    automation_id: String,
    control_type: String,
    class_name: String,
}

struct NativeObservation {
    elements: Vec<(DesktopElement, NativeElement)>,
    elements_truncated: bool,
    screenshot_png: Option<Vec<u8>>,
}

struct NativeInputOutcome {
    completed_action_count: usize,
    interrupted: bool,
}

impl DesktopControlService {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn list_windows(
        &self,
        context: &OperationContext,
    ) -> RuntimeResult<DesktopWindowList> {
        let result = tokio::task::spawn_blocking(platform::list_windows)
            .await
            .map_err(join_error)??;
        let owner = owner(context);
        let mut state = self.state.lock().await;
        state.windows.retain(|_, record| record.owner != owner);
        let mut windows = Vec::with_capacity(result.windows.len());
        for native in result.windows {
            let window_id = uuid::Uuid::new_v4().to_string();
            let info = window_info(&window_id, &native);
            state.windows.insert(
                window_id,
                WindowRecord {
                    owner: owner.clone(),
                    native,
                    info: info.clone(),
                    session: Arc::new(OnceCell::new()),
                },
            );
            windows.push(info);
        }
        Ok(DesktopWindowList {
            windows,
            excluded_window_count: result.excluded_window_count,
        })
    }

    async fn window(&self, context: &OperationContext, id: &str) -> RuntimeResult<WindowRecord> {
        let state = self.state.lock().await;
        let record = state.windows.get(id).cloned().ok_or_else(|| {
            RuntimeError::new(
                "desktop_window_not_found",
                "window was not found; list windows again",
            )
        })?;
        ensure_owner(&record.owner, context, "desktop_window_not_found")?;
        Ok(record)
    }

    async fn session_target(&self, record: &WindowRecord) -> RuntimeResult<SessionTarget> {
        let native = record.native.clone();
        let session = record
            .session
            .get_or_try_init(|| async move {
                tokio::task::spawn_blocking(move || platform::WindowSession::new(&native))
                    .await
                    .map_err(join_error)?
                    .map(Arc::new)
            })
            .await?
            .clone();
        Ok(SessionTarget {
            window_id: record.info.window_id.clone(),
            native: record.native.clone(),
            info: record.info.clone(),
            session,
        })
    }

    async fn input_access(
        &self,
        context: &OperationContext,
        id: &str,
    ) -> RuntimeResult<(SessionTarget, Arc<AtomicBool>, Arc<Mutex<()>>)> {
        let state = self.state.lock().await;
        let InputSlot::Active(record) = &state.input else {
            return Err(RuntimeError::new(
                "desktop_input_session_not_found",
                "input session was not found",
            ));
        };
        if record.id != id {
            return Err(RuntimeError::new(
                "desktop_input_session_not_found",
                "input session was not found",
            ));
        }
        ensure_owner(&record.owner, context, "desktop_input_session_not_found")?;
        if record.cancelled.load(std::sync::atomic::Ordering::Acquire) {
            return Err(RuntimeError::new(
                "desktop_input_stopped",
                "desktop input was stopped by the user",
            ));
        }
        Ok((
            record.target.clone(),
            record.cancelled.clone(),
            record.action_gate.clone(),
        ))
    }
}

fn owner(context: &OperationContext) -> Owner {
    Owner {
        agent_id: context.agent_id.clone(),
        task_id: context.task_id.clone(),
    }
}

fn ensure_owner(value: &Owner, context: &OperationContext, code: &str) -> RuntimeResult<()> {
    if value == &owner(context) {
        Ok(())
    } else {
        Err(RuntimeError::new(code, "resource was not found"))
    }
}

fn window_info(id: &str, native: &NativeWindow) -> DesktopWindowInfo {
    DesktopWindowInfo {
        window_id: id.to_owned(),
        title: native.title.clone(),
        application: native.application.clone(),
        process_id: native.process_id,
        width: native.width,
        height: native.height,
        minimized: native.minimized,
    }
}

struct NativeWindowList {
    windows: Vec<NativeWindow>,
    excluded_window_count: usize,
}
