use super::*;
use std::sync::{Arc, atomic::AtomicBool};
use tokio_util::sync::CancellationToken;

pub(super) struct InputGuard;

pub(super) struct WindowSession;

impl WindowSession {
    pub(super) fn new(_window: &NativeWindow) -> RuntimeResult<Self> {
        unsupported()
    }

    pub(super) fn prewarm_capture(&self) -> RuntimeResult<()> {
        unsupported()
    }

    pub(super) fn observe(
        &self,
        _include_screenshot: bool,
        _include_elements: bool,
    ) -> RuntimeResult<NativeObservation> {
        unsupported()
    }

    pub(super) fn observe_after(
        &self,
        _include_screenshot: bool,
        _include_elements: bool,
        _after_sequence: Option<u64>,
    ) -> RuntimeResult<NativeObservation> {
        unsupported()
    }

    pub(super) fn current_capture_sequence(&self) -> RuntimeResult<u64> {
        unsupported()
    }

    pub(super) fn element_act(
        &self,
        _element: &NativeElement,
        _action: &DesktopElementAction,
    ) -> RuntimeResult<()> {
        unsupported()
    }

    pub(super) fn input_act(
        &self,
        _cancelled: &AtomicBool,
        _cancellation: &CancellationToken,
        _actions: &[DesktopInputAction],
    ) -> RuntimeResult<NativeInputOutcome> {
        unsupported()
    }
}

impl InputGuard {
    pub(super) fn stop(self) -> RuntimeResult<()> {
        unsupported()
    }
}

pub(super) fn list_windows() -> RuntimeResult<NativeWindowList> {
    unsupported()
}

pub(super) fn start_input(_window: &NativeWindow) -> RuntimeResult<(InputGuard, Arc<AtomicBool>)> {
    unsupported()
}

fn unsupported<T>() -> RuntimeResult<T> {
    Err(RuntimeError::new(
        "desktop_unsupported",
        "desktop computer use is currently available on Windows only",
    ))
}
