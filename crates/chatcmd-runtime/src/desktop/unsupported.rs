use super::*;
use std::sync::{Arc, atomic::AtomicBool};
use tokio_util::sync::CancellationToken;

pub(super) struct InputGuard;

impl InputGuard {
    pub(super) fn stop(self) -> RuntimeResult<()> {
        unsupported()
    }
}

pub(super) fn list_windows() -> RuntimeResult<NativeWindowList> {
    unsupported()
}

pub(super) fn observe(
    _window: &NativeWindow,
    _include_screenshot: bool,
    _include_elements: bool,
) -> RuntimeResult<NativeObservation> {
    unsupported()
}

pub(super) fn element_act(
    _window: &NativeWindow,
    _element: &NativeElement,
    _action: &DesktopElementAction,
) -> RuntimeResult<()> {
    unsupported()
}

pub(super) fn start_input(_window: &NativeWindow) -> RuntimeResult<(InputGuard, Arc<AtomicBool>)> {
    unsupported()
}

pub(super) fn input_act(
    _window: &NativeWindow,
    _cancelled: &AtomicBool,
    _cancellation: &CancellationToken,
    _actions: &[DesktopInputAction],
) -> RuntimeResult<()> {
    unsupported()
}

fn unsupported<T>() -> RuntimeResult<T> {
    Err(RuntimeError::new(
        "desktop_unsupported",
        "desktop computer use is currently available on Windows only",
    ))
}
