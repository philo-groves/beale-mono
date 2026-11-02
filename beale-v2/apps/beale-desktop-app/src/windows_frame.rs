use raw_window_handle::RawWindowHandle;
use rune::WindowHandle;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT,
    HTTOPRIGHT, IsZoomed, WM_NCDESTROY, WM_NCHITTEST,
};

const SUBCLASS_ID: usize = 0x4245_414c;

fn resize_hit(rect: RECT, x: i32, y: i32, edge: i32) -> Option<LRESULT> {
    if x < rect.left || x >= rect.right || y < rect.top || y >= rect.bottom {
        return None;
    }
    let left = x - rect.left < edge;
    let right = rect.right - x <= edge;
    let top = y - rect.top < edge;
    let bottom = rect.bottom - y <= edge;
    let hit = match (left, right, top, bottom) {
        (true, _, true, _) => HTTOPLEFT,
        (_, true, true, _) => HTTOPRIGHT,
        (true, _, _, true) => HTBOTTOMLEFT,
        (_, true, _, true) => HTBOTTOMRIGHT,
        (true, _, _, _) => HTLEFT,
        (_, true, _, _) => HTRIGHT,
        (_, _, true, _) => HTTOP,
        (_, _, _, true) => HTBOTTOM,
        _ => return None,
    };
    Some(hit as LRESULT)
}

unsafe extern "system" fn frame_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    _: usize,
) -> LRESULT {
    if message == WM_NCHITTEST {
        // Maximized windows have no drag-resize edge. Win32 packs signed screen
        // coordinates into LPARAM, including on monitors left of the primary.
        if unsafe { IsZoomed(hwnd) } == 0 {
            let mut rect = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            if unsafe { GetWindowRect(hwnd, &mut rect) } != 0 {
                let x = (lparam as u16) as i16 as i32;
                let y = ((lparam >> 16) as u16) as i16 as i32;
                let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
                let edge = (8 * dpi as i32 / 96).max(8);
                if let Some(hit) = resize_hit(rect, x, y, edge) {
                    return hit;
                }
            }
        }
    }
    if message == WM_NCDESTROY {
        // The native window may be recreated; keep no callback on its old HWND.
        unsafe { RemoveWindowSubclass(hwnd, Some(frame_proc), SUBCLASS_ID) };
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

pub(super) fn configure(window: &WindowHandle) -> bool {
    let mut native_border = false;
    let _ = window.with_raw_handle(|handle| {
        let RawWindowHandle::Win32(raw) = handle.as_raw() else {
            return;
        };
        let hwnd = raw.hwnd.get() as HWND;
        // SAFETY: Rune calls on_window on the HWND's UI thread. The subclass holds
        // no borrowed data and removes itself when that HWND is destroyed.
        unsafe {
            if SetWindowSubclass(hwnd, Some(frame_proc), SUBCLASS_ID, 0) == 0 {
                eprintln!("Beale window edge resizing could not be enabled");
            }
            let preference = DWMWCP_ROUND;
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                (&raw const preference).cast(),
                size_of_val(&preference) as u32,
            );
            // DWM traces the rounded frame. Older Windows versions use Rune's inset edge.
            let border_color: u32 = 0x0032_3232;
            native_border = DwmSetWindowAttribute(
                hwnd,
                DWMWA_BORDER_COLOR as u32,
                (&raw const border_color).cast(),
                size_of_val(&border_color) as u32,
            ) == 0;
        }
    });
    native_border
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_edges_and_corners_resize_without_stealing_the_interior() {
        let rect = RECT {
            left: -200,
            top: 100,
            right: 800,
            bottom: 700,
        };
        let at = |x, y| resize_hit(rect, x, y, 8);
        assert_eq!(at(-200, 100), Some(HTTOPLEFT as LRESULT));
        assert_eq!(at(799, 100), Some(HTTOPRIGHT as LRESULT));
        assert_eq!(at(-200, 699), Some(HTBOTTOMLEFT as LRESULT));
        assert_eq!(at(799, 699), Some(HTBOTTOMRIGHT as LRESULT));
        assert_eq!(at(-200, 400), Some(HTLEFT as LRESULT));
        assert_eq!(at(799, 400), Some(HTRIGHT as LRESULT));
        assert_eq!(at(400, 100), Some(HTTOP as LRESULT));
        assert_eq!(at(400, 699), Some(HTBOTTOM as LRESULT));
        assert_eq!(at(400, 400), None);
        assert_eq!(at(800, 400), None);
    }
}
