use super::{hwnd, *};
use ::windows::Win32::{
    Foundation::RECT,
    UI::{
        Input::KeyboardAndMouse::{
            INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
            KEYEVENTF_UNICODE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
            MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTDOWN,
            MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput, VIRTUAL_KEY, VK_BACK,
            VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_F1, VK_HOME, VK_LEFT, VK_MENU, VK_NEXT,
            VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
        },
        WindowsAndMessaging::{
            GetForegroundWindow, GetWindowRect, SetCursorPos, SetForegroundWindow,
        },
    },
};
use std::{
    mem::size_of,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

pub(super) fn activate(window: &NativeWindow) -> RuntimeResult<()> {
    let target = hwnd(window.handle);
    // SAFETY: `target` has been validated as a live top-level window immediately before this call.
    if !unsafe { SetForegroundWindow(target) }.as_bool() {
        return Err(RuntimeError::new(
            "desktop_input_activation_failed",
            "Windows refused to activate the target window",
        ));
    }
    let deadline = Instant::now() + Duration::from_millis(750);
    while Instant::now() < deadline {
        // SAFETY: this only reads the current foreground window handle.
        if unsafe { GetForegroundWindow() } == target {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(15));
    }
    Err(RuntimeError::new(
        "desktop_input_activation_failed",
        "the target window did not become active",
    ))
}

pub(super) fn act(
    window: &NativeWindow,
    cancelled: &AtomicBool,
    cancellation: &CancellationToken,
    actions: &[DesktopInputAction],
) -> RuntimeResult<()> {
    for action in actions {
        ensure_active(window, cancelled, cancellation)?;
        match action {
            DesktopInputAction::Click { x, y, button } => {
                move_to(window, cancelled, cancellation, *x, *y)?;
                mouse_click(window, cancelled, cancellation, *button, 1)?;
            }
            DesktopInputAction::DoubleClick { x, y, button } => {
                move_to(window, cancelled, cancellation, *x, *y)?;
                mouse_click(window, cancelled, cancellation, *button, 2)?;
            }
            DesktopInputAction::Move { x, y } => {
                move_to(window, cancelled, cancellation, *x, *y)?;
            }
            DesktopInputAction::Drag {
                start_x,
                start_y,
                end_x,
                end_y,
                duration_ms,
            } => drag(
                window,
                cancelled,
                cancellation,
                (*start_x, *start_y),
                (*end_x, *end_y),
                *duration_ms,
            )?,
            DesktopInputAction::Scroll {
                x,
                y,
                delta_x,
                delta_y,
            } => {
                move_to(window, cancelled, cancellation, *x, *y)?;
                scroll(window, cancelled, cancellation, *delta_x, *delta_y)?;
            }
            DesktopInputAction::Keypress { keys } => {
                keypress(window, cancelled, cancellation, keys)?;
            }
            DesktopInputAction::Type { text } => {
                type_text(window, cancelled, cancellation, text)?;
            }
            DesktopInputAction::Wait { duration_ms } => {
                wait(window, cancelled, cancellation, *duration_ms)?;
            }
        }
    }
    Ok(())
}

fn ensure_active(
    window: &NativeWindow,
    cancelled: &AtomicBool,
    cancellation: &CancellationToken,
) -> RuntimeResult<()> {
    if cancellation.is_cancelled() {
        cancelled.store(true, Ordering::Release);
        return Err(RuntimeError::new(
            "desktop_input_stopped",
            "desktop input was stopped by the caller",
        ));
    }
    if cancelled.load(Ordering::Acquire) {
        return Err(RuntimeError::new(
            "desktop_input_stopped",
            "desktop input was stopped by the user",
        ));
    }
    // SAFETY: this only reads the current foreground window handle.
    if unsafe { GetForegroundWindow() } != hwnd(window.handle) {
        cancelled.store(true, Ordering::Release);
        return Err(RuntimeError::new(
            "desktop_input_focus_lost",
            "the user switched away from the target; desktop input was stopped",
        ));
    }
    Ok(())
}

fn window_rect(window: &NativeWindow) -> RuntimeResult<RECT> {
    let mut rect = RECT::default();
    // SAFETY: `window.handle` is validated before each action batch; GetWindowRect initializes `rect`.
    unsafe { GetWindowRect(hwnd(window.handle), &mut rect) }.map_err(super::backend_error)?;
    Ok(rect)
}

fn move_to(
    window: &NativeWindow,
    cancelled: &AtomicBool,
    cancellation: &CancellationToken,
    x: i32,
    y: i32,
) -> RuntimeResult<()> {
    ensure_active(window, cancelled, cancellation)?;
    let rect = window_rect(window)?;
    let width = rect.right.saturating_sub(rect.left);
    let height = rect.bottom.saturating_sub(rect.top);
    if x < 0 || y < 0 || x >= width || y >= height {
        return Err(RuntimeError::new(
            "invalid_arguments",
            "input coordinates are outside the target window",
        ));
    }
    // SAFETY: the validated coordinates are converted to absolute screen coordinates.
    unsafe { SetCursorPos(rect.left.saturating_add(x), rect.top.saturating_add(y)) }
        .map_err(super::backend_error)
}

fn mouse_click(
    window: &NativeWindow,
    cancelled: &AtomicBool,
    cancellation: &CancellationToken,
    button: DesktopMouseButton,
    count: usize,
) -> RuntimeResult<()> {
    let (down, up) = mouse_flags(button);
    for _ in 0..count {
        ensure_active(window, cancelled, cancellation)?;
        send(&[mouse_input(down), mouse_input(up)])?;
        if count > 1 {
            thread::sleep(Duration::from_millis(60));
        }
    }
    Ok(())
}

fn mouse_flags(
    button: DesktopMouseButton,
) -> (
    ::windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS,
    ::windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS,
) {
    match button {
        DesktopMouseButton::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        DesktopMouseButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        DesktopMouseButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
    }
}

fn drag(
    window: &NativeWindow,
    cancelled: &AtomicBool,
    cancellation: &CancellationToken,
    start: (i32, i32),
    end: (i32, i32),
    duration_ms: u64,
) -> RuntimeResult<()> {
    move_to(window, cancelled, cancellation, start.0, start.1)?;
    ensure_active(window, cancelled, cancellation)?;
    send(&[mouse_input(MOUSEEVENTF_LEFTDOWN)])?;
    let steps = (duration_ms / 16).clamp(1, 240);
    let result = (1..=steps).try_for_each(|step| {
        ensure_active(window, cancelled, cancellation)?;
        let x = interpolate(start.0, end.0, step, steps);
        let y = interpolate(start.1, end.1, step, steps);
        move_to(window, cancelled, cancellation, x, y)?;
        thread::sleep(Duration::from_millis(duration_ms / steps));
        Ok(())
    });
    let release = send(&[mouse_input(MOUSEEVENTF_LEFTUP)]);
    result.and(release)
}

fn interpolate(start: i32, end: i32, step: u64, steps: u64) -> i32 {
    let delta = i64::from(end) - i64::from(start);
    let value = i64::from(start) + delta.saturating_mul(step as i64) / steps as i64;
    i32::try_from(value).unwrap_or(if value < 0 { i32::MIN } else { i32::MAX })
}

fn scroll(
    window: &NativeWindow,
    cancelled: &AtomicBool,
    cancellation: &CancellationToken,
    delta_x: i32,
    delta_y: i32,
) -> RuntimeResult<()> {
    ensure_active(window, cancelled, cancellation)?;
    let mut inputs = Vec::with_capacity(2);
    if delta_y != 0 {
        inputs.push(mouse_data_input(
            MOUSEEVENTF_WHEEL,
            delta_y.saturating_neg(),
        ));
    }
    if delta_x != 0 {
        inputs.push(mouse_data_input(MOUSEEVENTF_HWHEEL, delta_x));
    }
    send(&inputs)
}

fn keypress(
    window: &NativeWindow,
    cancelled: &AtomicBool,
    cancellation: &CancellationToken,
    keys: &[String],
) -> RuntimeResult<()> {
    if keys.iter().any(|key| is_forbidden_key(key)) {
        return Err(RuntimeError::new(
            "desktop_key_denied",
            "Windows/Meta/Command shortcuts are not allowed",
        ));
    }
    let virtual_keys = keys
        .iter()
        .map(|key| virtual_key(key))
        .collect::<RuntimeResult<Vec<_>>>()?;
    let mut inputs = Vec::with_capacity(virtual_keys.len() * 2);
    inputs.extend(virtual_keys.iter().map(|key| key_input(*key, false)));
    inputs.extend(virtual_keys.iter().rev().map(|key| key_input(*key, true)));
    ensure_active(window, cancelled, cancellation)?;
    send(&inputs)
}

fn is_forbidden_key(key: &str) -> bool {
    matches!(
        key.trim().to_ascii_lowercase().as_str(),
        "win" | "windows" | "meta" | "cmd" | "command" | "super" | "os"
    )
}

fn virtual_key(key: &str) -> RuntimeResult<VIRTUAL_KEY> {
    let normalized = key.trim().to_ascii_lowercase();
    let value = match normalized.as_str() {
        "ctrl" | "control" | "control_l" => VK_CONTROL,
        "shift" | "shift_l" => VK_SHIFT,
        "alt" | "menu" | "alt_l" => VK_MENU,
        "enter" | "return" => VK_RETURN,
        "tab" => VK_TAB,
        "backspace" => VK_BACK,
        "delete" => VK_DELETE,
        "space" => VK_SPACE,
        "left" | "arrowleft" => VK_LEFT,
        "right" | "arrowright" => VK_RIGHT,
        "up" | "arrowup" => VK_UP,
        "down" | "arrowdown" => VK_DOWN,
        "home" => VK_HOME,
        "end" => VK_END,
        "pageup" => VK_PRIOR,
        "pagedown" => VK_NEXT,
        value if value.len() == 1 && value.as_bytes()[0].is_ascii_alphanumeric() => {
            VIRTUAL_KEY(value.as_bytes()[0].to_ascii_uppercase().into())
        }
        value if value.starts_with('f') => {
            let number = value[1..].parse::<u16>().ok();
            let Some(number @ 1..=12) = number else {
                return Err(unknown_key(key));
            };
            VIRTUAL_KEY(VK_F1.0 + number - 1)
        }
        _ => return Err(unknown_key(key)),
    };
    Ok(value)
}

fn unknown_key(key: &str) -> RuntimeError {
    RuntimeError::new("invalid_arguments", format!("unsupported key name: {key}"))
}

fn type_text(
    window: &NativeWindow,
    cancelled: &AtomicBool,
    cancellation: &CancellationToken,
    text: &str,
) -> RuntimeResult<()> {
    for unit in text.encode_utf16() {
        ensure_active(window, cancelled, cancellation)?;
        send(&[unicode_input(unit, false), unicode_input(unit, true)])?;
    }
    Ok(())
}

fn wait(
    window: &NativeWindow,
    cancelled: &AtomicBool,
    cancellation: &CancellationToken,
    duration_ms: u64,
) -> RuntimeResult<()> {
    let deadline = Instant::now() + Duration::from_millis(duration_ms);
    while Instant::now() < deadline {
        ensure_active(window, cancelled, cancellation)?;
        thread::sleep(Duration::from_millis(25));
    }
    Ok(())
}

fn mouse_input(flags: ::windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS) -> INPUT {
    mouse_data_input(flags, 0)
}

fn mouse_data_input(
    flags: ::windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS,
    data: i32,
) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                mouseData: data as u32,
                dwFlags: flags,
                ..Default::default()
            },
        },
    }
}

fn key_input(key: VIRTUAL_KEY, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                dwFlags: if key_up {
                    KEYEVENTF_KEYUP
                } else {
                    Default::default()
                },
                ..Default::default()
            },
        },
    }
}

fn unicode_input(unit: u16, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wScan: unit,
                dwFlags: KEYEVENTF_UNICODE
                    | if key_up {
                        KEYEVENTF_KEYUP
                    } else {
                        Default::default()
                    },
                ..Default::default()
            },
        },
    }
}

fn send(inputs: &[INPUT]) -> RuntimeResult<()> {
    if inputs.is_empty() {
        return Ok(());
    }
    // SAFETY: `inputs` is a valid initialized contiguous INPUT array and the size matches INPUT.
    let sent = unsafe {
        SendInput(
            inputs,
            i32::try_from(size_of::<INPUT>()).unwrap_or_default(),
        )
    };
    let sent_count = usize::try_from(sent).unwrap_or_default().min(inputs.len());
    if sent_count == inputs.len() {
        Ok(())
    } else {
        best_effort_release(&inputs[..sent_count]);
        Err(super::backend_error("Windows rejected injected input"))
    }
}

fn best_effort_release(inserted: &[INPUT]) {
    let mut releases = Vec::with_capacity(inserted.len());
    for input in inserted.iter().rev() {
        if input.r#type == INPUT_KEYBOARD {
            // SAFETY: the union contains `ki` whenever the discriminating input type is keyboard.
            let keyboard = unsafe { input.Anonymous.ki };
            if !keyboard.dwFlags.contains(KEYEVENTF_KEYUP) {
                releases.push(INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            dwFlags: keyboard.dwFlags | KEYEVENTF_KEYUP,
                            ..keyboard
                        },
                    },
                });
            }
        } else if input.r#type == INPUT_MOUSE {
            // SAFETY: the union contains `mi` whenever the discriminating input type is mouse.
            let mouse = unsafe { input.Anonymous.mi };
            let release = if mouse.dwFlags.contains(MOUSEEVENTF_LEFTDOWN) {
                Some(MOUSEEVENTF_LEFTUP)
            } else if mouse.dwFlags.contains(MOUSEEVENTF_RIGHTDOWN) {
                Some(MOUSEEVENTF_RIGHTUP)
            } else if mouse.dwFlags.contains(MOUSEEVENTF_MIDDLEDOWN) {
                Some(MOUSEEVENTF_MIDDLEUP)
            } else {
                None
            };
            if let Some(flags) = release {
                releases.push(mouse_input(flags));
            }
        }
    }
    if !releases.is_empty() {
        // SAFETY: `releases` is a valid initialized INPUT array; this is best-effort cleanup.
        let _ = unsafe {
            SendInput(
                &releases,
                i32::try_from(size_of::<INPUT>()).unwrap_or_default(),
            )
        };
    }
}
