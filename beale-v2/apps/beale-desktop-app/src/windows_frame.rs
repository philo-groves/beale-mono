use crate::appearance::{AppearancePreferences, AppearanceTheme, PaletteColor};
use raw_window_handle::RawWindowHandle;
use rune::WindowHandle;
use std::cell::RefCell;
use std::collections::HashMap;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateIconFromResourceEx, DestroyIcon, GetWindowRect, HICON, HTBOTTOM, HTBOTTOMLEFT,
    HTBOTTOMRIGHT, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT, ICON_BIG, ICON_SMALL, IsZoomed,
    SendMessageW, WM_NCDESTROY, WM_NCHITTEST, WM_SETICON,
};

const SUBCLASS_ID: usize = 0x4245_414c;
const APP_ICON: &[u8] = include_bytes!("../assets/app-icon.ico");

thread_local! {
    static WINDOW_ICONS: RefCell<HashMap<HWND, (HICON, HICON)>> = RefCell::new(HashMap::new());
}

fn icon_image(size: u8) -> Option<&'static [u8]> {
    let count = usize::from(u16::from_le_bytes(APP_ICON.get(4..6)?.try_into().ok()?));
    if APP_ICON.get(0..4)? != [0, 0, 1, 0] {
        return None;
    }
    for index in 0..count {
        let entry = APP_ICON.get(6 + 16 * index..6 + 16 * (index + 1))?;
        if entry[0] != size || entry[1] != size {
            continue;
        }
        let length = u32::from_le_bytes(entry[8..12].try_into().ok()?) as usize;
        let offset = u32::from_le_bytes(entry[12..16].try_into().ok()?) as usize;
        return APP_ICON.get(offset..offset.checked_add(length)?);
    }
    None
}

fn load_icon(size: u8) -> Option<HICON> {
    let image = icon_image(size)?;
    // SAFETY: the ICO entry is held in this binary for the lifetime of the call.
    let icon = unsafe {
        CreateIconFromResourceEx(image.as_ptr(), image.len() as u32, 1, 0x0003_0000, 0, 0, 0)
    };
    (!icon.is_null()).then_some(icon)
}

fn set_app_window_icon(hwnd: HWND) {
    let Some(small) = load_icon(32) else { return };
    let Some(big) = load_icon(64) else {
        // SAFETY: `small` was created by CreateIconFromResourceEx above.
        unsafe { DestroyIcon(small) };
        return;
    };
    // SAFETY: Rune calls on_window on this HWND's UI thread. These handles remain
    // alive until WM_NCDESTROY, where the subclass releases them.
    unsafe {
        SendMessageW(hwnd, WM_SETICON, ICON_SMALL as WPARAM, small as LPARAM);
        SendMessageW(hwnd, WM_SETICON, ICON_BIG as WPARAM, big as LPARAM);
    }
    WINDOW_ICONS.with(|icons| {
        icons.borrow_mut().insert(hwnd, (small, big));
    });
}

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
        let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
        WINDOW_ICONS.with(|icons| {
            if let Some((small, big)) = icons.borrow_mut().remove(&hwnd) {
                // SAFETY: both icons were created for this HWND and are no longer used.
                unsafe {
                    DestroyIcon(small);
                    DestroyIcon(big);
                }
            }
        });
        return result;
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

fn border_color(appearance: &AppearancePreferences) -> u32 {
    let theme = appearance.theme;
    let default = match theme {
        AppearanceTheme::Dark => "#323232",
        AppearanceTheme::Light => "#b9c2ca",
        AppearanceTheme::Cream => "#b8a387",
        AppearanceTheme::Midnight => "#334b6b",
    };
    let hex = if appearance.palette_color_is_default(theme, PaletteColor::Border) {
        default
    } else {
        appearance.palette_color(theme, PaletteColor::Border)
    };
    let rgb = u32::from_str_radix(&hex[1..], 16).unwrap_or(0x323232);
    ((rgb & 0xff) << 16) | (rgb & 0xff00) | ((rgb >> 16) & 0xff)
}

pub(super) fn set_border_color(hwnd: HWND, appearance: &AppearancePreferences) -> bool {
    let value = border_color(appearance);
    // SAFETY: the caller holds a live HWND on its UI thread.
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR as u32,
            (&raw const value).cast(),
            size_of_val(&value) as u32,
        ) == 0
    }
}

pub(super) fn configure(window: &WindowHandle, appearance: &AppearancePreferences) -> bool {
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
            } else {
                set_app_window_icon(hwnd);
            }
            let preference = DWMWCP_ROUND;
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                (&raw const preference).cast(),
                size_of_val(&preference) as u32,
            );
            // DWM traces the rounded frame. Older Windows versions use Rune's inset edge.
            native_border = set_border_color(hwnd, appearance);
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

    #[test]
    fn border_color_tracks_the_appearance_theme() {
        let mut appearance = AppearancePreferences::default();
        assert_eq!(border_color(&appearance), 0x0032_3232);
        appearance.theme = AppearanceTheme::Light;
        assert_eq!(border_color(&appearance), 0x00ca_c2b9);
        appearance.set_palette_color(
            AppearanceTheme::Light,
            PaletteColor::Border,
            Some("#123456".to_owned()),
        );
        assert_eq!(border_color(&appearance), 0x0056_3412);
    }

    #[test]
    fn v1_icon_has_window_sizes() {
        assert!(icon_image(32).is_some());
        assert!(icon_image(64).is_some());
        for size in [32, 64] {
            let icon = load_icon(size).expect("Windows decodes the Beale icon");
            // SAFETY: the test owns the icon returned by CreateIconFromResourceEx.
            unsafe { DestroyIcon(icon) };
        }
    }
}
