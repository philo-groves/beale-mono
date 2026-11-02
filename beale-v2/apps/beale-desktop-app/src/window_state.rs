use raw_window_handle::RawWindowHandle;
use rune::WindowHandle;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONULL, MONITORINFO, MonitorFromRect,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowPlacement, IsIconic, IsZoomed, SW_SHOWMAXIMIZED, SW_SHOWNORMAL, SetWindowPlacement,
    WINDOWPLACEMENT, WM_CLOSE, WM_EXITSIZEMOVE, WM_MOVE, WM_NCDESTROY, WM_SIZE,
    WPF_RESTORETOMAXIMIZED,
};

const SUBCLASS_ID: usize = 0x4245_414d;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct SavedPlacement {
    version: u8,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    maximized: bool,
}

impl SavedPlacement {
    fn valid(self) -> bool {
        self.version == 1
            && (500..=16_000).contains(&self.width)
            && (350..=16_000).contains(&self.height)
            && self.x.checked_add(self.width).is_some()
            && self.y.checked_add(self.height).is_some()
    }

    fn rect(self) -> RECT {
        RECT {
            left: self.x,
            top: self.y,
            right: self.x + self.width,
            bottom: self.y + self.height,
        }
    }

    fn read(path: &Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        let saved: Self = serde_json::from_slice(&bytes).ok()?;
        saved.valid().then_some(saved)
    }

    fn write(self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(&self).map_err(std::io::Error::other)?;
        std::fs::write(path, bytes)
    }

    fn visible_on_current_display(self) -> bool {
        let rect = self.rect();
        // SAFETY: RECT and MONITORINFO are initialized and the monitor handle is
        // used only while checking this placement on the UI thread.
        unsafe {
            let monitor = MonitorFromRect(&rect, MONITOR_DEFAULTTONULL);
            if monitor.is_null() {
                return false;
            }
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(monitor, &mut info) == 0 {
                return false;
            }
            let work = info.rcWork;
            let overlap_width = rect.right.min(work.right) - rect.left.max(work.left);
            let overlap_height = rect.bottom.min(work.bottom) - rect.top.max(work.top);
            overlap_width >= 200 && overlap_height >= 80
        }
    }
}

struct Tracking {
    path: Option<PathBuf>,
    last: Option<SavedPlacement>,
}

impl Tracking {
    fn save(&self) {
        if let (Some(path), Some(saved)) = (&self.path, self.last)
            && let Err(error) = saved.write(path)
        {
            eprintln!("Beale window placement could not be saved: {error}");
        }
    }
}

fn snapshot(hwnd: HWND, previous: Option<SavedPlacement>) -> Option<SavedPlacement> {
    let mut placement = WINDOWPLACEMENT {
        length: size_of::<WINDOWPLACEMENT>() as u32,
        flags: 0,
        showCmd: 0,
        ptMinPosition: POINT { x: 0, y: 0 },
        ptMaxPosition: POINT { x: 0, y: 0 },
        rcNormalPosition: RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
    };
    // SAFETY: this is a live HWND in its window procedure or on_window hook.
    if unsafe { GetWindowPlacement(hwnd, &mut placement) } == 0 {
        return previous;
    }
    let rect = placement.rcNormalPosition;
    let minimized = unsafe { IsIconic(hwnd) } != 0;
    let maximized = if minimized {
        placement.flags & WPF_RESTORETOMAXIMIZED != 0
            || previous.is_some_and(|saved| saved.maximized)
    } else {
        (unsafe { IsZoomed(hwnd) }) != 0
    };
    let saved = SavedPlacement {
        version: 1,
        x: rect.left,
        y: rect.top,
        width: rect.right.checked_sub(rect.left)?,
        height: rect.bottom.checked_sub(rect.top)?,
        maximized,
    };
    saved.valid().then_some(saved).or(previous)
}

unsafe extern "system" fn track_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    reference: usize,
) -> LRESULT {
    // SAFETY: attach allocated this Tracking and the subclass is removed below
    // before its single Box is freed on WM_NCDESTROY.
    let tracking = unsafe { &mut *(reference as *mut Tracking) };
    if matches!(
        message,
        WM_MOVE | WM_SIZE | WM_EXITSIZEMOVE | WM_CLOSE | WM_NCDESTROY
    ) {
        tracking.last = snapshot(hwnd, tracking.last);
    }
    if matches!(message, WM_EXITSIZEMOVE | WM_CLOSE | WM_NCDESTROY) {
        tracking.save();
    }
    if message == WM_NCDESTROY {
        unsafe { RemoveWindowSubclass(hwnd, Some(track_proc), SUBCLASS_ID) };
        let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
        unsafe { drop(Box::from_raw(reference as *mut Tracking)) };
        return result;
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

pub(super) fn restore_and_track(window: &WindowHandle) {
    let path = crate::appearance::preference_path()
        .map(|appearance| appearance.with_file_name("desktop-window.json"));
    let saved = path.as_deref().and_then(SavedPlacement::read);
    let _ = window.with_raw_handle(|handle| {
        let RawWindowHandle::Win32(raw) = handle.as_raw() else {
            return;
        };
        let hwnd = raw.hwnd.get() as HWND;
        let tracking = Box::new(Tracking { path, last: saved });
        let reference = Box::into_raw(tracking) as usize;
        // SAFETY: Rune runs on_window on the native UI thread. The subclass
        // owns `reference` until WM_NCDESTROY and never retains a borrowed HWND.
        if unsafe { SetWindowSubclass(hwnd, Some(track_proc), SUBCLASS_ID, reference) } == 0 {
            unsafe { drop(Box::from_raw(reference as *mut Tracking)) };
            return;
        }
        if let Some(saved) = saved.filter(|state| state.visible_on_current_display()) {
            let placement = WINDOWPLACEMENT {
                length: size_of::<WINDOWPLACEMENT>() as u32,
                flags: 0,
                showCmd: if saved.maximized {
                    SW_SHOWMAXIMIZED as u32
                } else {
                    SW_SHOWNORMAL as u32
                },
                ptMinPosition: POINT { x: 0, y: 0 },
                ptMaxPosition: POINT { x: 0, y: 0 },
                rcNormalPosition: saved.rect(),
            };
            // SAFETY: both HWND and WINDOWPLACEMENT are valid on the UI thread.
            unsafe { SetWindowPlacement(hwnd, &placement) };
        }
        // A discarded off-screen position should not be written back on close.
        let tracking = unsafe { &mut *(reference as *mut Tracking) };
        tracking.last = snapshot(hwnd, None);
    });
}

pub(super) fn save_now(hwnd: HWND) {
    // Rune calls on_close on the HWND's UI thread before hiding the window.
    if let Some(saved) = snapshot(hwnd, None) {
        let path = crate::appearance::preference_path()
            .map(|appearance| appearance.with_file_name("desktop-window.json"));
        Tracking {
            path,
            last: Some(saved),
        }
        .save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_round_trip_and_validation() {
        let path = std::env::temp_dir().join(format!(
            "beale-window-example-{}.json",
            uuid::Uuid::new_v4()
        ));
        let saved = SavedPlacement {
            version: 1,
            x: -1200,
            y: 80,
            width: 1100,
            height: 760,
            maximized: true,
        };
        saved.write(&path).unwrap();
        assert_eq!(SavedPlacement::read(&path), Some(saved));
        assert!(
            !SavedPlacement {
                width: 100,
                ..saved
            }
            .valid()
        );
        assert!(
            !SavedPlacement {
                version: 2,
                ..saved
            }
            .valid()
        );
        std::fs::remove_file(path).unwrap();
    }
}
