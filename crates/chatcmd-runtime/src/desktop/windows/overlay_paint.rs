use ::windows::Win32::{
    Foundation::{COLORREF, HWND, RECT},
    Graphics::Gdi::{
        BACKGROUND_MODE, BeginPaint, CreateSolidBrush, DT_CENTER, DT_END_ELLIPSIS, DT_SINGLELINE,
        DT_VCENTER, DeleteObject, DrawTextW, EndPaint, FillRect, HBRUSH, HDC, HGDIOBJ, PAINTSTRUCT,
        SetBkMode, SetTextColor, TRANSPARENT,
    },
    UI::WindowsAndMessaging::GetClientRect,
};

const BANNER_TEXT: &str = "Computer control active — Press ESC to stop";
const BORDER: i32 = 8;

pub(super) fn paint(window: HWND, bright: bool, target: RECT, monitor: RECT) {
    let mut paint = PAINTSTRUCT::default();
    // SAFETY: called during WM_PAINT with writable paint storage.
    let dc = unsafe { BeginPaint(window, &mut paint) };
    let mut client = RECT::default();
    // SAFETY: the overlay is live and `client` is writable.
    let _ = unsafe { GetClientRect(window, &mut client) };
    let black = create_brush(rgb(0, 0, 0));
    let accent = create_brush(if bright {
        rgb(239, 68, 68)
    } else {
        rgb(249, 115, 22)
    });
    fill(dc, &client, black);
    draw_border(dc, accent, client);
    let target = target_in_monitor(target, monitor, client);
    draw_border(dc, accent, target);
    draw_target_banner(dc, accent, target);
    delete_brush(black);
    delete_brush(accent);
    // SAFETY: matches the BeginPaint call above using the same live window and PAINTSTRUCT.
    let _ = unsafe { EndPaint(window, &paint) };
}

fn target_in_monitor(target: RECT, monitor: RECT, client: RECT) -> RECT {
    RECT {
        left: (target.left - monitor.left).clamp(client.left, client.right),
        top: (target.top - monitor.top).clamp(client.top, client.bottom),
        right: (target.right - monitor.left).clamp(client.left, client.right),
        bottom: (target.bottom - monitor.top).clamp(client.top, client.bottom),
    }
}

fn draw_border(dc: HDC, brush: HBRUSH, rect: RECT) {
    let width = (rect.right - rect.left).max(0);
    let height = (rect.bottom - rect.top).max(0);
    if width == 0 || height == 0 {
        return;
    }
    let border = BORDER.min(width).min(height);
    fill_box(
        dc,
        brush,
        rect.left,
        rect.top,
        rect.right,
        rect.top + border,
    );
    fill_box(
        dc,
        brush,
        rect.left,
        rect.bottom - border,
        rect.right,
        rect.bottom,
    );
    fill_box(
        dc,
        brush,
        rect.left,
        rect.top,
        rect.left + border,
        rect.bottom,
    );
    fill_box(
        dc,
        brush,
        rect.right - border,
        rect.top,
        rect.right,
        rect.bottom,
    );
}

fn draw_target_banner(dc: HDC, brush: HBRUSH, target: RECT) {
    let mut banner = RECT {
        left: target.left + BORDER,
        top: target.top + BORDER,
        right: target.right - BORDER,
        bottom: (target.top + 42).min(target.bottom - BORDER),
    };
    if banner.right <= banner.left || banner.bottom <= banner.top {
        return;
    }
    fill(dc, &banner, brush);
    let mut text: Vec<u16> = BANNER_TEXT.encode_utf16().collect();
    // SAFETY: the HDC is active; the text buffer and rectangle remain valid for these calls.
    unsafe {
        SetBkMode(dc, BACKGROUND_MODE(TRANSPARENT.0));
        SetTextColor(dc, rgb(255, 255, 255));
        DrawTextW(
            dc,
            &mut text,
            &mut banner,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
        );
    }
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

const fn rgb(red: u8, green: u8, blue: u8) -> COLORREF {
    COLORREF((red as u32) | ((green as u32) << 8) | ((blue as u32) << 16))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_rect_is_translated_and_clipped_to_monitor() {
        let monitor = RECT {
            left: 1920,
            top: -100,
            right: 3840,
            bottom: 980,
        };
        let client = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let target = RECT {
            left: 1900,
            top: -50,
            right: 4000,
            bottom: 900,
        };

        let translated = target_in_monitor(target, monitor, client);

        assert_eq!(translated.left, 0);
        assert_eq!(translated.top, 50);
        assert_eq!(translated.right, 1920);
        assert_eq!(translated.bottom, 1000);
    }
}
