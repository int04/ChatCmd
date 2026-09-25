//! Window-scoped Windows desktop automation with an explicit input-takeover mode.

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

use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{
    collections::HashMap,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

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
}

struct ObservationRecord {
    owner: Owner,
    native: NativeWindow,
    created_at: Instant,
    elements: HashMap<String, NativeElement>,
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
    window_id: String,
    native: NativeWindow,
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
                },
            );
            windows.push(info);
        }
        Ok(DesktopWindowList {
            windows,
            excluded_window_count: result.excluded_window_count,
        })
    }

    pub async fn observe(
        &self,
        context: &OperationContext,
        request: DesktopObserveRequest,
    ) -> RuntimeResult<DesktopObservation> {
        let record = self.window(context, &request.window_id).await?;
        let native = record.native.clone();
        let include_screenshot = request.include_screenshot;
        let include_elements = request.include_elements;
        let observed = tokio::task::spawn_blocking(move || {
            platform::observe(&native, include_screenshot, include_elements)
        })
        .await
        .map_err(join_error)??;
        let observation_id = uuid::Uuid::new_v4().to_string();
        let mut public_elements = Vec::with_capacity(observed.elements.len());
        let mut native_elements = HashMap::with_capacity(observed.elements.len());
        for (element, native) in observed.elements {
            native_elements.insert(element.element_id.clone(), native);
            public_elements.push(element);
        }
        let screenshot_base64 = observed.screenshot_png.map(|bytes| STANDARD.encode(bytes));
        let mut state = self.state.lock().await;
        prune_observations(&mut state);
        state.observations.insert(
            observation_id.clone(),
            ObservationRecord {
                owner: owner(context),
                native: record.native,
                created_at: Instant::now(),
                elements: native_elements,
            },
        );
        Ok(DesktopObservation {
            observation_id,
            window: record.info,
            elements: public_elements,
            elements_truncated: observed.elements_truncated,
            screenshot_base64,
        })
    }

    pub async fn element_act(
        &self,
        context: &OperationContext,
        request: DesktopElementActRequest,
    ) -> RuntimeResult<DesktopActionResult> {
        validate_element_action(&request.action)?;
        let (native, element) = {
            let mut state = self.state.lock().await;
            let observation = state
                .observations
                .get(&request.observation_id)
                .ok_or_else(|| {
                    RuntimeError::new(
                        "desktop_observation_not_found",
                        "observation is stale or was not found",
                    )
                })?;
            ensure_owner(&observation.owner, context, "desktop_observation_not_found")?;
            if observation.created_at.elapsed() > OBSERVATION_TTL {
                return Err(RuntimeError::new(
                    "desktop_observation_stale",
                    "observation expired; observe the window again",
                ));
            }
            let element = observation
                .elements
                .get(&request.element_id)
                .cloned()
                .ok_or_else(|| {
                    RuntimeError::new(
                        "desktop_element_not_found",
                        "element was not found in that observation",
                    )
                })?;
            let native = observation.native.clone();
            state.observations.remove(&request.observation_id);
            (native, element)
        };
        let action = request.action;
        tokio::task::spawn_blocking(move || platform::element_act(&native, &element, &action))
            .await
            .map_err(join_error)??;
        Ok(DesktopActionResult {
            completed: true,
            observation_invalidated: true,
        })
    }

    pub async fn input_begin(
        &self,
        context: &OperationContext,
        request: DesktopInputBeginRequest,
    ) -> RuntimeResult<DesktopInputSessionInfo> {
        let record = self.window(context, &request.window_id).await?;
        if context.cancellation.is_cancelled() {
            return Err(RuntimeError::new(
                "operationCancelled",
                "desktop input start was cancelled",
            ));
        }
        let _start_guard = self.input_start_gate.lock().await;
        let id = uuid::Uuid::new_v4().to_string();
        let owner = owner(context);
        let stale_record = {
            let mut state = self.state.lock().await;
            match std::mem::take(&mut state.input) {
                InputSlot::Empty => None,
                InputSlot::Active(active)
                    if active.cancelled.load(std::sync::atomic::Ordering::Acquire) =>
                {
                    Some(active)
                }
                other => {
                    state.input = other;
                    return Err(RuntimeError::busy(
                        "another desktop input session is active",
                    ));
                }
            }
        };
        if let Some(stale) = stale_record {
            stale
                .cancelled
                .store(true, std::sync::atomic::Ordering::Release);
            let action_gate = stale.action_gate.clone();
            let _action_guard = action_gate.lock().await;
            let input_guard = stale.guard;
            tokio::task::spawn_blocking(move || input_guard.stop())
                .await
                .map_err(join_error)??;
        }
        if context.cancellation.is_cancelled() {
            return Err(RuntimeError::new(
                "operationCancelled",
                "desktop input start was cancelled",
            ));
        }
        let native = record.native.clone();
        let started = tokio::task::spawn_blocking(move || platform::start_input(&native))
            .await
            .map_err(join_error)?;
        let (guard, cancelled) = started?;
        if context.cancellation.is_cancelled()
            || cancelled.load(std::sync::atomic::Ordering::Acquire)
        {
            tokio::task::spawn_blocking(move || guard.stop())
                .await
                .map_err(join_error)??;
            return Err(RuntimeError::new(
                "desktop_input_stopped",
                "desktop input was stopped during startup",
            ));
        }
        self.state.lock().await.input = InputSlot::Active(Box::new(InputRecord {
            id: id.clone(),
            owner,
            window_id: request.window_id.clone(),
            native: record.native,
            cancelled,
            action_gate: Arc::new(Mutex::new(())),
            guard,
        }));
        Ok(DesktopInputSessionInfo {
            input_session_id: id,
            window_id: request.window_id,
            active: true,
            escape_to_stop: true,
            overlay_visible: true,
        })
    }

    pub async fn input_act(
        &self,
        context: &OperationContext,
        request: DesktopInputActRequest,
    ) -> RuntimeResult<DesktopInputSessionInfo> {
        validate_input_actions(&request.actions)?;
        let (native, cancelled, action_gate, window_id) = self
            .input_access(context, &request.input_session_id)
            .await?;
        let _action_guard = action_gate.lock().await;
        if cancelled.load(std::sync::atomic::Ordering::Acquire) {
            return Err(RuntimeError::new(
                "desktop_input_stopped",
                "desktop input was stopped by the user",
            ));
        }
        let cancellation = context.cancellation.clone();
        let actions = request.actions;
        tokio::task::spawn_blocking(move || {
            platform::input_act(&native, &cancelled, &cancellation, &actions)
        })
        .await
        .map_err(join_error)??;
        Ok(DesktopInputSessionInfo {
            input_session_id: request.input_session_id,
            window_id,
            active: true,
            escape_to_stop: true,
            overlay_visible: true,
        })
    }

    pub async fn input_end(
        &self,
        context: &OperationContext,
        request: DesktopInputEndRequest,
    ) -> RuntimeResult<()> {
        let _lifecycle_guard = self.input_start_gate.lock().await;
        let record = {
            let mut state = self.state.lock().await;
            let InputSlot::Active(active) = &state.input else {
                return Err(RuntimeError::new(
                    "desktop_input_session_not_found",
                    "input session was not found",
                ));
            };
            if active.id != request.input_session_id {
                return Err(RuntimeError::new(
                    "desktop_input_session_not_found",
                    "input session was not found",
                ));
            }
            ensure_owner(&active.owner, context, "desktop_input_session_not_found")?;
            let slot = std::mem::take(&mut state.input);
            match slot {
                InputSlot::Active(record) => {
                    record
                        .cancelled
                        .store(true, std::sync::atomic::Ordering::Release);
                    record
                }
                other => {
                    state.input = other;
                    return Err(RuntimeError::new(
                        "desktop_input_session_not_found",
                        "input session was not found",
                    ));
                }
            }
        };
        let action_gate = record.action_gate.clone();
        let _action_guard = action_gate.lock().await;
        let input_guard = record.guard;
        tokio::task::spawn_blocking(move || input_guard.stop())
            .await
            .map_err(join_error)??;
        Ok(())
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

    async fn input_access(
        &self,
        context: &OperationContext,
        id: &str,
    ) -> RuntimeResult<(NativeWindow, Arc<AtomicBool>, Arc<Mutex<()>>, String)> {
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
            record.native.clone(),
            record.cancelled.clone(),
            record.action_gate.clone(),
            record.window_id.clone(),
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
