use super::{backend_error, hwnd};
use crate::{RuntimeError, RuntimeResult};
use ::windows::{
    Win32::{
        Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{
            BACKGROUND_MODE, BeginPaint, CreateSolidBrush, DT_CENTER, DT_END_ELLIPSIS,
            DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW, EndPaint, FillRect, HBRUSH, HDC,
            HGDIOBJ, InvalidateRect, PAINTSTRUCT, SetBkMode, SetTextColor, TRANSPARENT,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Input::KeyboardAndMouse::{MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey, VK_ESCAPE},
            WindowsAndMessaging::{
                CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
                GWLP_USERDATA, GetClientRect, GetForegroundWindow, GetMessageW, GetWindowLongPtrW,
                GetWindowRect, HTTRANSPARENT, HWND_TOPMOST, IsIconic, IsWindow, KillTimer,
                LWA_COLORKEY, MSG, PostQuitMessage, RegisterClassW, SW_HIDE, SWP_NOACTIVATE,
                SWP_SHOWWINDOW, SetLayeredWindowAttributes, SetTimer, SetWindowLongPtrW,
                SetWindowPos, ShowWindow, TranslateMessage, UnregisterClassW, WINDOW_EX_STYLE,
                WM_CLOSE, WM_DESTROY, WM_ERASEBKGND, WM_HOTKEY, WM_NCCREATE, WM_NCHITTEST,
                WM_PAINT, WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
                WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
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
const BANNER_TEXT: &str = "Computer control active — Press ESC to stop";
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
        cancelled,
        lifecycle,
        bright: true,
    };
    let result = create_and_run(instance, class_name, target_rect, &mut state, &ready);
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
            paint(window, state_mut(window).as_deref());
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
    let mut rect = RECT::default();
    // SAFETY: the target is live and `rect` is writable.
    if unsafe { GetWindowRect(state.target, &mut rect) }.is_err() {
        cancel_and_destroy(window, state);
        return;
    }
    // SAFETY: repositioning the live overlay topmost without activation follows the target.
    let positioned = unsafe {
        SetWindowPos(
            window,
            Some(HWND_TOPMOST),
            rect.left,
            rect.top,
            (rect.right - rect.left).max(1),
            (rect.bottom - rect.top).max(1),
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )
    };
    if positioned.is_err() {
        cancel_and_destroy(window, state);
    } else if lifecycle == ARM_REQUESTED {
        state.lifecycle.store(ARMED, Ordering::Release);
    }
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

fn paint(window: HWND, state: Option<&OverlayState>) {
    let Some(state) = state else {
        return;
    };
    let mut paint = PAINTSTRUCT::default();
    // SAFETY: called during WM_PAINT with writable paint storage.
    let dc = unsafe { BeginPaint(window, &mut paint) };
    let mut client = RECT::default();
    // SAFETY: the overlay is live and `client` is writable.
    let _ = unsafe { GetClientRect(window, &mut client) };
    let black = create_brush(rgb(0, 0, 0));
    let accent = create_brush(if state.bright {
        rgb(239, 68, 68)
    } else {
        rgb(249, 115, 22)
    });
    fill(dc, &client, black);
    let width = (client.right - client.left).max(0);
    let height = (client.bottom - client.top).max(0);
    if width > 0 && height > 0 {
        let border = 8.min(width).min(height);
        fill_box(dc, accent, 0, 0, width, border);
        fill_box(dc, accent, 0, height - border, width, height);
        fill_box(dc, accent, 0, 0, border, height);
        fill_box(dc, accent, width - border, 0, width, height);
        let mut banner = RECT {
            left: border,
            top: border,
            right: width - border,
            bottom: 42.min(height - border),
        };
        if banner.right > banner.left && banner.bottom > banner.top {
            fill(dc, &banner, accent);
            draw_banner(dc, &mut banner);
        }
    }
    delete_brush(black);
    delete_brush(accent);
    // SAFETY: matches the BeginPaint call above using the same live window and PAINTSTRUCT.
    let _ = unsafe { EndPaint(window, &paint) };
}

fn create_brush(color: COLORREF) -> HBRUSH {
    // SAFETY: creating a solid brush from a concrete COLORREF has no borrowed lifetime.
    unsafe { CreateSolidBrush(color) }
}

fn fill(dc: HDC, rect: &RECT, brush: HBRUSH) {
    // SAFETY: the HDC, rectangle, and brush are valid for the active paint operation.
    unsafe { FillRect(dc, rect, brush) };
}

fn fill_box(dc: HDC, brush: HBRUSH, left: i32, top: i32, right: i32, bottom: i32) {
    fill(
        dc,
        &RECT {
            left,
            top,
            right,
            bottom,
        },
        brush,
    );
}

fn delete_brush(brush: HBRUSH) {
    // SAFETY: each locally created brush is deleted once after the paint operation.
    let _ = unsafe { DeleteObject(HGDIOBJ(brush.0)) };
}

fn draw_banner(dc: HDC, rect: &mut RECT) {
    let mut text: Vec<u16> = BANNER_TEXT.encode_utf16().collect();
    // SAFETY: the HDC is active; the text buffer and rectangle remain valid for these calls.
    unsafe {
        SetBkMode(dc, BACKGROUND_MODE(TRANSPARENT.0));
        SetTextColor(dc, rgb(255, 255, 255));
        DrawTextW(
            dc,
            &mut text,
            rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
        );
    }
}

const fn rgb(red: u8, green: u8, blue: u8) -> COLORREF {
    COLORREF((red as u32) | ((green as u32) << 8) | ((blue as u32) << 16))
}
