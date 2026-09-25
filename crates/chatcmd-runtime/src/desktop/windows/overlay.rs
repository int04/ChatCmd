use super::{backend_error, hwnd, overlay_paint};
use crate::{RuntimeError, RuntimeResult};
use ::windows::{
    Win32::{
        Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{
            GetMonitorInfoW, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO,
            MonitorFromWindow,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Input::KeyboardAndMouse::{MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey, VK_ESCAPE},
            WindowsAndMessaging::{
                CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
                GWLP_USERDATA, GetForegroundWindow, GetMessageW, GetWindowLongPtrW, GetWindowRect,
                HTTRANSPARENT, HWND_TOPMOST, IsIconic, IsWindow, KillTimer, LWA_COLORKEY, MSG,
                PostQuitMessage, RegisterClassW, SW_HIDE, SWP_NOACTIVATE, SWP_SHOWWINDOW,
                SetLayeredWindowAttributes, SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow,
                TranslateMessage, UnregisterClassW, WINDOW_EX_STYLE, WM_CLOSE, WM_DESTROY,
                WM_ERASEBKGND, WM_HOTKEY, WM_NCCREATE, WM_NCHITTEST, WM_PAINT, WM_TIMER, WNDCLASSW,
                WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
                WS_EX_TRANSPARENT, WS_POPUP,
            },
        },
    },
    core::{PCWSTR, w},
};
use std::{
    ffi::c_void,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

const FOLLOW_TIMER: usize = 1;
const BLINK_TIMER: usize = 2;
const HOTKEY_ID: i32 = 1;
const STAGED: u8 = 0;
const ARM_REQUESTED: u8 = 1;
const ARMED: u8 = 2;
const STOPPED: u8 = 3;

pub(in crate::desktop) struct InputGuard {
    cancelled: Arc<AtomicBool>,
    lifecycle: Arc<AtomicU8>,
    thread: Option<JoinHandle<RuntimeResult<()>>>,
}

impl InputGuard {
    pub(in crate::desktop) fn arm(&mut self) -> RuntimeResult<()> {
        if self.lifecycle.load(Ordering::Acquire) != STAGED {
            return Err(arm_error("overlay is not staged"));
        }
        self.lifecycle.store(ARM_REQUESTED, Ordering::Release);
        for _ in 0..200 {
            match self.lifecycle.load(Ordering::Acquire) {
                ARMED => return Ok(()),
                STOPPED => return Err(arm_error("target focus was lost before arming")),
                _ => thread::sleep(Duration::from_millis(5)),
            }
        }
        self.cancelled.store(true, Ordering::Release);
        Err(arm_error("overlay arm timed out"))
    }

    pub(in crate::desktop) fn stop(mut self) -> RuntimeResult<()> {
        self.stop_inner()
    }

    fn stop_inner(&mut self) -> RuntimeResult<()> {
        self.lifecycle.store(STOPPED, Ordering::Release);
        self.cancelled.store(true, Ordering::Release);
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        thread.join().map_err(|_| {
            RuntimeError::new("desktop_overlay_thread_panicked", "overlay thread panicked")
        })?
    }
}

impl Drop for InputGuard {
    fn drop(&mut self) {
        let _ = self.stop_inner();
    }
}

struct OverlayState {
    target: HWND,
    target_rect: RECT,
    monitor_rect: RECT,
    cancelled: Arc<AtomicBool>,
    lifecycle: Arc<AtomicU8>,
    bright: bool,
}

pub(super) fn start(handle: isize, cancelled: Arc<AtomicBool>) -> RuntimeResult<InputGuard> {
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let overlay_cancelled = cancelled.clone();
    let lifecycle = Arc::new(AtomicU8::new(STAGED));
    let overlay_lifecycle = lifecycle.clone();
    let thread = thread::Builder::new()
        .name("chatcmd-computer-overlay".into())
        .spawn(move || overlay_thread(handle, overlay_cancelled, overlay_lifecycle, ready_tx))
        .map_err(backend_error)?;

    ready_rx.recv().map_err(|_| {
        RuntimeError::new(
            "desktop_overlay_start_failed",
            "overlay startup channel closed",
        )
    })??;
    Ok(InputGuard {
        cancelled,
        lifecycle,
        thread: Some(thread),
    })
}

fn overlay_thread(
    handle: isize,
    cancelled: Arc<AtomicBool>,
    lifecycle: Arc<AtomicU8>,
    ready: mpsc::SyncSender<RuntimeResult<()>>,
) -> RuntimeResult<()> {
    let target = hwnd(handle);
    let mut target_rect = RECT::default();
    // SAFETY: the caller supplied a validated top-level window handle and `target_rect` is writable.
    if let Err(error) = unsafe { GetWindowRect(target, &mut target_rect) } {
        let error = backend_error(error);
        let _ = ready.send(Err(error.clone()));
        return Err(error);
    }
    let monitor_rect = monitor_rect(target)?;
    // SAFETY: a null module name requests the current process module without borrowing memory.
    let module = unsafe { GetModuleHandleW(None) }.map_err(backend_error)?;
    let instance = HINSTANCE(module.0);
    let class_name = w!("ChatCmdComputerOverlay");
    let class = WNDCLASSW {
        hInstance: instance,
        lpszClassName: class_name,
        lpfnWndProc: Some(window_proc),
        ..Default::default()
    };
    // SAFETY: `class` and its UTF-16 class name remain alive until after class unregistration.
    if unsafe { RegisterClassW(&class) } == 0 {
        let error = backend_error(::windows::core::Error::from_thread());
        let _ = ready.send(Err(error.clone()));
        return Err(error);
    }

    let mut state = OverlayState {
        target,
        target_rect,
        monitor_rect,
        cancelled,
        lifecycle,
        bright: true,
    };
    let result = create_and_run(instance, class_name, monitor_rect, &mut state, &ready);
    // SAFETY: the overlay window has been destroyed before its unique class is unregistered.
    let unregister = unsafe { UnregisterClassW(class_name, Some(instance)) };
    result.and_then(|()| unregister.map_err(backend_error))
}

fn create_and_run(
    instance: HINSTANCE,
    class_name: PCWSTR,
    rect: RECT,
    state: &mut OverlayState,
    ready: &mpsc::SyncSender<RuntimeResult<()>>,
) -> RuntimeResult<()> {
    let width = (rect.right - rect.left).max(1);
    let height = (rect.bottom - rect.top).max(1);
    let ex_style = WINDOW_EX_STYLE(
        WS_EX_TOPMOST.0
            | WS_EX_TOOLWINDOW.0
            | WS_EX_NOACTIVATE.0
            | WS_EX_LAYERED.0
            | WS_EX_TRANSPARENT.0,
    );
    // SAFETY: the registered class, coordinates, instance, and state pointer are valid. The state
    // remains pinned on this thread's stack for the entire window/message-loop lifetime.
    let overlay = unsafe {
        CreateWindowExW(
            ex_style,
            class_name,
            PCWSTR::null(),
            WS_POPUP,
            rect.left,
            rect.top,
            width,
            height,
            None,
            None,
            Some(instance),
            Some((state as *mut OverlayState).cast::<c_void>()),
        )
    };
    let overlay = match overlay {
        Ok(window) => window,
        Err(error) => {
            let error = backend_error(error);
            let _ = ready.send(Err(error.clone()));
            return Err(error);
        }
    };
    if let Err(error) = initialize_window(overlay) {
        // SAFETY: `overlay` was successfully created on this thread.
        let _ = unsafe { DestroyWindow(overlay) };
        let _ = ready.send(Err(error.clone()));
        return Err(error);
    }
    if ready.send(Ok(())).is_err() {
        // SAFETY: the receiver disappeared, so the unexposed overlay can be closed immediately.
        let _ = unsafe { DestroyWindow(overlay) };
        return Ok(());
    }
    message_loop()
}

fn initialize_window(overlay: HWND) -> RuntimeResult<()> {
    // SAFETY: the newly-created layered overlay is valid; black becomes fully transparent.
    unsafe { SetLayeredWindowAttributes(overlay, COLORREF(0), 255, LWA_COLORKEY) }
        .map_err(backend_error)?;
    // SAFETY: registering an unmodified Escape hotkey for this overlay is valid.
    unsafe {
        RegisterHotKey(
            Some(overlay),
            HOTKEY_ID,
            MOD_NOREPEAT,
            u32::from(VK_ESCAPE.0),
        )
    }
    .map_err(backend_error)?;
    // SAFETY: both timers belong to this valid window and use message-based callbacks.
    let follow = unsafe { SetTimer(Some(overlay), FOLLOW_TIMER, 25, None) };
    // SAFETY: this independent timer controls only the overlay blink cadence.
    let blink = unsafe { SetTimer(Some(overlay), BLINK_TIMER, 450, None) };
    if follow == 0 || blink == 0 {
        return Err(backend_error(::windows::core::Error::from_thread()));
    }
    Ok(())
}

fn message_loop() -> RuntimeResult<()> {
    let mut message = MSG::default();
    loop {
        // SAFETY: `message` is writable and this thread owns the overlay message queue.
        let status = unsafe { GetMessageW(&mut message, None, 0, 0) }.0;
        if status == 0 {
            return Ok(());
        }
        if status == -1 {
            return Err(backend_error(::windows::core::Error::from_thread()));
        }
        // SAFETY: `message` was initialized by GetMessageW.
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

/// Handles messages only for windows created with a live `OverlayState` pointer.
///
/// # Safety
/// Windows must invoke this callback with the HWND and message parameters belonging to the
/// registered overlay class. `WM_NCCREATE` must carry a pointer valid through `WM_DESTROY`.
unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = lparam.0 as *const CREATESTRUCTW;
        if !create.is_null() {
            // SAFETY: Windows provides a valid CREATESTRUCTW during WM_NCCREATE.
            let state = unsafe { (*create).lpCreateParams as *mut OverlayState };
            // SAFETY: the pointer was supplied to CreateWindowExW and fits in window user data.
            unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, state as isize) };
        }
    }
    match message {
        WM_ERASEBKGND => LRESULT(1),
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_PAINT => {
            if let Some(state) = state_mut(window) {
                overlay_paint::paint(window, state.bright, state.target_rect, state.monitor_rect);
            }
            LRESULT(0)
        }
        WM_TIMER => {
            on_timer(window, wparam.0);
            LRESULT(0)
        }
        WM_HOTKEY if wparam.0 == HOTKEY_ID as usize => {
            if let Some(state) = state_mut(window) {
                cancel_and_destroy(window, state);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            if let Some(state) = state_mut(window) {
                cancel_and_destroy(window, state);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            if let Some(state) = state_mut(window) {
                state.cancelled.store(true, Ordering::Release);
                state.lifecycle.store(STOPPED, Ordering::Release);
            }
            // SAFETY: these resources were registered for this window during initialization.
            unsafe {
                let _ = KillTimer(Some(window), FOLLOW_TIMER);
                let _ = KillTimer(Some(window), BLINK_TIMER);
                let _ = UnregisterHotKey(Some(window), HOTKEY_ID);
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        _ => {
            // SAFETY: unhandled messages are delegated to the system default procedure.
            unsafe { DefWindowProcW(window, message, wparam, lparam) }
        }
    }
}

fn on_timer(window: HWND, timer: usize) {
    let Some(state) = state_mut(window) else {
        return;
    };
    if state.cancelled.load(Ordering::Acquire) {
        // SAFETY: external cancellation ends this live overlay's message loop.
        let _ = unsafe { DestroyWindow(window) };
        return;
    }
    let lifecycle = state.lifecycle.load(Ordering::Acquire);
    if lifecycle == STAGED {
        return;
    }
    // SAFETY: retrieving the foreground HWND does not dereference application memory.
    let foreground = unsafe { GetForegroundWindow() };
    if foreground != state.target {
        cancel_and_destroy(window, state);
        return;
    }
    if timer == BLINK_TIMER {
        if lifecycle == ARMED {
            state.bright = !state.bright;
            // SAFETY: invalidating a live window schedules a repaint without activating it.
            let _ = unsafe { InvalidateRect(Some(window), None, false) };
        }
        return;
    }
    if timer != FOLLOW_TIMER {
        return;
    }
    // SAFETY: querying validity/minimized state of an opaque target HWND is safe.
    if !unsafe { IsWindow(Some(state.target)) }.as_bool() {
        cancel_and_destroy(window, state);
        return;
    }
    // SAFETY: querying minimized state does not mutate or dereference the target.
    if unsafe { IsIconic(state.target) }.as_bool() {
        // SAFETY: hiding the overlay does not activate either window.
        let _ = unsafe { ShowWindow(window, SW_HIDE) };
        return;
    }
    let mut target_rect = RECT::default();
    // SAFETY: the target is live and `rect` is writable.
    if unsafe { GetWindowRect(state.target, &mut target_rect) }.is_err() {
        cancel_and_destroy(window, state);
        return;
    }
    let Ok(monitor_rect) = monitor_rect(state.target) else {
        cancel_and_destroy(window, state);
        return;
    };
    let changed =
        !same_rect(state.target_rect, target_rect) || !same_rect(state.monitor_rect, monitor_rect);
    state.target_rect = target_rect;
    state.monitor_rect = monitor_rect;
    // SAFETY: repositioning the live overlay topmost without activation follows the target.
    let positioned = unsafe {
        SetWindowPos(
            window,
            Some(HWND_TOPMOST),
            monitor_rect.left,
            monitor_rect.top,
            (monitor_rect.right - monitor_rect.left).max(1),
            (monitor_rect.bottom - monitor_rect.top).max(1),
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )
    };
    if positioned.is_err() {
        cancel_and_destroy(window, state);
    } else {
        if changed {
            // SAFETY: repainting the transparent overlay reflects the latest window/monitor bounds.
            let _ = unsafe { InvalidateRect(Some(window), None, false) };
        }
        if lifecycle == ARM_REQUESTED {
            state.lifecycle.store(ARMED, Ordering::Release);
        }
    }
}

fn monitor_rect(target: HWND) -> RuntimeResult<RECT> {
    // SAFETY: querying the nearest monitor for a validated HWND has no side effects.
    let monitor = unsafe { MonitorFromWindow(target, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: u32::try_from(std::mem::size_of::<MONITORINFO>()).unwrap_or_default(),
        ..Default::default()
    };
    // SAFETY: `info` is writable and has the required structure size initialized.
    if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        Ok(info.rcMonitor)
    } else {
        Err(backend_error(::windows::core::Error::from_thread()))
    }
}

fn same_rect(left: RECT, right: RECT) -> bool {
    left.left == right.left
        && left.top == right.top
        && left.right == right.right
        && left.bottom == right.bottom
}

fn cancel_and_destroy(window: HWND, state: &OverlayState) {
    state.lifecycle.store(STOPPED, Ordering::Release);
    state.cancelled.store(true, Ordering::Release);
    // SAFETY: cancellation always runs for the live overlay on its owning thread.
    let _ = unsafe { DestroyWindow(window) };
}

fn arm_error(message: &str) -> RuntimeError {
    RuntimeError::new("desktop_overlay_arm_failed", message)
}

fn state_mut(window: HWND) -> Option<&'static mut OverlayState> {
    // SAFETY: user data is null before WM_NCCREATE or the live pointer installed there; the
    // OverlayState remains on the window thread's stack until the message loop exits.
    unsafe { (GetWindowLongPtrW(window, GWLP_USERDATA) as *mut OverlayState).as_mut() }
}
