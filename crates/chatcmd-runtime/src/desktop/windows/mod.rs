mod capture;
mod input;
mod overlay;
mod uia;

use super::*;
use ::windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{GetClassNameW, IsIconic, IsWindow},
};
use std::sync::{Arc, atomic::AtomicBool};
use tokio_util::sync::CancellationToken;
use windows_capture::window::Window;

pub(super) use overlay::InputGuard;

pub(super) fn list_windows() -> RuntimeResult<NativeWindowList> {
    let mut allowed = Vec::new();
    let mut excluded_window_count = 0;
    let current_pid = std::process::id();
    let windows = Window::enumerate().map_err(backend_error)?;
    for window in windows {
        let title = window.title().unwrap_or_default();
        let process_id = match window.process_id() {
            Ok(id) => id,
            Err(_) => {
                excluded_window_count += 1;
                continue;
            }
        };
        let application = window.process_name().unwrap_or_default();
        let width = window.width().unwrap_or_default();
        let height = window.height().unwrap_or_default();
        let handle = window.as_raw_hwnd().addr() as isize;
        let class_name = window_class(hwnd(handle));
        if title.trim().is_empty()
            || width <= 0
            || height <= 0
            || is_denied_target(current_pid, process_id, &application, &title, &class_name)
        {
            excluded_window_count += 1;
            continue;
        }
        // SAFETY: `handle` comes from an enumerated, currently valid top-level window.
        let minimized = unsafe { IsIconic(HWND(window.as_raw_hwnd())) }.as_bool();
        allowed.push(NativeWindow {
            handle,
            process_id,
            title,
            application,
            width: u32::try_from(width).unwrap_or_default(),
            height: u32::try_from(height).unwrap_or_default(),
            minimized,
        });
    }
    allowed.sort_by(|left, right| {
        left.application
            .to_lowercase()
            .cmp(&right.application.to_lowercase())
            .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
    });
    Ok(NativeWindowList {
        windows: allowed,
        excluded_window_count,
    })
}

pub(super) fn observe(
    window: &NativeWindow,
    include_screenshot: bool,
    include_elements: bool,
) -> RuntimeResult<NativeObservation> {
    validate_target(window)?;
    // SAFETY: querying minimized state of a validated HWND has no side effects.
    if include_screenshot && unsafe { IsIconic(hwnd(window.handle)) }.as_bool() {
        return Err(RuntimeError::new(
            "desktop_window_minimized",
            "the target window is minimized; restore it before requesting a screenshot",
        ));
    }
    let screenshot_png = include_screenshot
        .then(|| capture::capture_png(window.handle))
        .transpose()?;
    let (elements, elements_truncated) = if include_elements {
        uia::snapshot(window.handle)?
    } else {
        (Vec::new(), false)
    };
    Ok(NativeObservation {
        elements,
        elements_truncated,
        screenshot_png,
    })
}

pub(super) fn element_act(
    window: &NativeWindow,
    element: &NativeElement,
    action: &DesktopElementAction,
) -> RuntimeResult<()> {
    validate_target(window)?;
    uia::act(window.handle, element, action)
}

pub(super) fn start_input(window: &NativeWindow) -> RuntimeResult<(InputGuard, Arc<AtomicBool>)> {
    validate_target(window)?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut guard = overlay::start(window.handle, cancelled.clone())?;
    if let Err(error) = input::activate(window) {
        let _ = guard.stop();
        return Err(error);
    }
    if let Err(error) = guard.arm() {
        let _ = guard.stop();
        return Err(error);
    }
    Ok((guard, cancelled))
}

pub(super) fn input_act(
    window: &NativeWindow,
    cancelled: &AtomicBool,
    cancellation: &CancellationToken,
    actions: &[DesktopInputAction],
) -> RuntimeResult<()> {
    validate_target(window)?;
    input::act(window, cancelled, cancellation, actions)
}

fn validate_target(window: &NativeWindow) -> RuntimeResult<()> {
    let hwnd = hwnd(window.handle);
    // SAFETY: querying validity of an opaque HWND is safe and does not dereference memory.
    if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
        return Err(RuntimeError::new(
            "desktop_window_closed",
            "the target window is no longer available; list windows again",
        ));
    }
    let current = Window::from_raw_hwnd(hwnd.0);
    let process_id = current.process_id().map_err(backend_error)?;
    if process_id != window.process_id {
        return Err(RuntimeError::new(
            "desktop_window_changed",
            "the target window identity changed; list windows again",
        ));
    }
    let title = current.title().map_err(backend_error)?;
    let application = current.process_name().map_err(backend_error)?;
    let class_name = window_class(hwnd);
    if is_denied_target(
        std::process::id(),
        process_id,
        &application,
        &title,
        &class_name,
    ) {
        return Err(RuntimeError::new(
            "desktop_target_denied",
            "the target window is no longer allowed for desktop automation",
        ));
    }
    Ok(())
}

fn is_denied_target(
    current_pid: u32,
    process_id: u32,
    application: &str,
    title: &str,
    class_name: &str,
) -> bool {
    if process_id == current_pid {
        return true;
    }
    let app = application.to_ascii_lowercase();
    let title = title.to_ascii_lowercase();
    let class_name = class_name.to_ascii_lowercase();
    const DENIED_APPS: &[&str] = &[
        "cmd.exe",
        "powershell.exe",
        "pwsh.exe",
        "windowsterminal.exe",
        "wt.exe",
        "conhost.exe",
        "wsl.exe",
        "bash.exe",
        "mintty.exe",
        "wezterm-gui.exe",
        "alacritty.exe",
        "tabby.exe",
        "hyper.exe",
        "putty.exe",
        "regedit.exe",
        "credentialuibroker.exe",
        "logonui.exe",
        "lockapp.exe",
        "securityhealthhost.exe",
        "securityhealthsystray.exe",
        "msmpeng.exe",
        "1password.exe",
        "bitwarden.exe",
        "keepass.exe",
        "keepassxc.exe",
        "enpass.exe",
        "dashlane.exe",
        "lastpass.exe",
        "nordpass.exe",
        "chatgpt.exe",
        "codex.exe",
    ];
    DENIED_APPS.iter().any(|denied| app == *denied)
        || app.contains("chatcmd")
        || matches!(
            class_name.as_str(),
            "consolewindowclass" | "cascadia_hosting_window_class" | "virtualconsoleclass"
        )
        || (app == "explorer.exe" && class_name == "#32770")
        || title.contains("windows security")
        || title.contains("windows powershell")
        || title.contains("command prompt")
        || title.contains("chatgpt")
        || title.contains("codex")
        || title.contains("sign in to windows")
        || title.contains("sign-in")
        || title.contains("sign in")
        || title.contains("log in")
        || title.contains("login")
        || title.contains("authentication")
        || title.contains("verify your identity")
        || title.contains("verification code")
        || title.contains("credential")
        || title.contains("password")
        || title.contains("password manager")
        || title.trim() == "run"
}

fn window_class(window: HWND) -> String {
    let mut buffer = [0u16; 256];
    // SAFETY: `buffer` is writable and `window` is only queried for its registered class name.
    let length = unsafe { GetClassNameW(window, &mut buffer) };
    let length = usize::try_from(length)
        .unwrap_or_default()
        .min(buffer.len());
    String::from_utf16_lossy(&buffer[..length])
}

pub(super) fn hwnd(handle: isize) -> HWND {
    HWND(std::ptr::with_exposed_provenance_mut(handle as usize))
}

pub(super) fn backend_error(error: impl std::fmt::Display) -> RuntimeError {
    let mut runtime = RuntimeError::new("desktop_backend_error", error.to_string());
    runtime.retryable = true;
    runtime
}

#[cfg(test)]
mod tests {
    use super::is_denied_target;

    #[test]
    fn sensitive_and_terminal_windows_are_denied() {
        assert!(is_denied_target(
            1,
            2,
            "powershell.exe",
            "work",
            "ConsoleWindowClass",
        ));
        assert!(is_denied_target(
            1,
            2,
            "explorer.exe",
            "Ausführen",
            "#32770",
        ));
        assert!(is_denied_target(
            1,
            2,
            "browser.exe",
            "Account login",
            "BrowserWindow",
        ));
    }

    #[test]
    fn ordinary_application_window_is_allowed() {
        assert!(!is_denied_target(
            1,
            2,
            "notepad.exe",
            "notes.txt - Notepad",
            "Notepad",
        ));
    }
}
