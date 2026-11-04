use anyhow::{Context, Result, anyhow, bail, ensure};
use std::{net::SocketAddr, ptr, thread};
use tokio::sync::oneshot;
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::{
        Shell::{
            NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETVERSION,
            NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
        },
        WindowsAndMessaging::{
            AppendMenuW, CreateIconFromResourceEx, CreatePopupMenu, CreateWindowExW,
            DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW,
            GetCursorPos, GetMessageW, MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG, PostMessageW,
            PostQuitMessage, PostThreadMessageW, RegisterClassW, RegisterWindowMessageW,
            SetForegroundWindow, TPM_BOTTOMALIGN, TPM_RIGHTBUTTON, TrackPopupMenu,
            TranslateMessage, UnregisterClassW, WM_APP, WM_COMMAND, WM_CONTEXTMENU, WM_NULL,
            WM_QUIT, WM_RBUTTONUP, WNDCLASSW,
        },
    },
};

const TRAY_MESSAGE: u32 = WM_APP + 1;
const EXIT_COMMAND: usize = 1;
const CLASS_NAME: &str = "BealeAppServerTray";
const ICON_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../beale-desktop-app/assets/app-icon.ico"
));

pub(super) struct TrayHandle {
    thread_id: u32,
    thread: Option<thread::JoinHandle<()>>,
}

impl TrayHandle {
    pub(super) fn start(addr: SocketAddr, exit: oneshot::Sender<()>) -> Result<Self> {
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("beale-app-server-tray".to_owned())
            .spawn(move || match TrayResources::new(addr) {
                Ok(mut tray) => {
                    // The hidden window has created this thread's message queue.
                    let thread_id = unsafe { GetCurrentThreadId() };
                    if ready_tx.send(Ok(thread_id)).is_ok() {
                        match tray.run() {
                            Ok(true) => {
                                let _ = exit.send(());
                            }
                            Ok(false) => {}
                            Err(error) => eprintln!("Beale app-server tray failed: {error:#}"),
                        }
                    }
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(format!("{error:#}")));
                }
            })
            .context("failed to start Beale app-server tray thread")?;
        let thread_id = ready_rx
            .recv()
            .context("Beale app-server tray stopped during startup")?
            .map_err(|error| anyhow!(error))?;
        Ok(Self {
            thread_id,
            thread: Some(thread),
        })
    }
}

impl Drop for TrayHandle {
    fn drop(&mut self) {
        // WM_QUIT makes the tray thread remove its icon and destroy its own window.
        unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct TrayResources {
    window: HWND,
    icon: windows_sys::Win32::UI::WindowsAndMessaging::HICON,
    notice: NOTIFYICONDATAW,
    added: bool,
    taskbar_created: u32,
}

impl TrayResources {
    fn new(addr: SocketAddr) -> Result<Self> {
        // All handles are owned by this thread and released by Drop, including on setup errors.
        let mut tray = Self {
            window: ptr::null_mut(),
            icon: ptr::null_mut(),
            notice: unsafe { std::mem::zeroed() },
            added: false,
            taskbar_created: 0,
        };
        let image = icon_image(ICON_BYTES, 32)?;
        tray.icon = unsafe {
            CreateIconFromResourceEx(image.as_ptr(), image.len() as u32, 1, 0x30000, 32, 32, 0)
        };
        ensure!(
            !tray.icon.is_null(),
            "failed to create the Beale tray icon: {}",
            std::io::Error::last_os_error()
        );

        let instance = unsafe { GetModuleHandleW(ptr::null()) };
        ensure!(
            !instance.is_null(),
            "failed to get the app-server module: {}",
            std::io::Error::last_os_error()
        );
        let class_name = wide(CLASS_NAME);
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class_name.as_ptr(),
            ..unsafe { std::mem::zeroed() }
        };
        ensure!(
            unsafe { RegisterClassW(&class) } != 0,
            "failed to register the tray window: {}",
            std::io::Error::last_os_error()
        );
        tray.window = unsafe {
            CreateWindowExW(
                0,
                class_name.as_ptr(),
                ptr::null(),
                0,
                0,
                0,
                0,
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                instance,
                ptr::null(),
            )
        };
        ensure!(
            !tray.window.is_null(),
            "failed to create the tray window: {}",
            std::io::Error::last_os_error()
        );
        tray.taskbar_created = unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) };
        ensure!(
            tray.taskbar_created != 0,
            "failed to register the taskbar restart message"
        );

        tray.notice.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        tray.notice.hWnd = tray.window;
        tray.notice.uID = 1;
        tray.notice.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP;
        tray.notice.uCallbackMessage = TRAY_MESSAGE;
        tray.notice.hIcon = tray.icon;
        let tip = wide(&format!("Beale app-server: Running on {addr}"));
        tray.notice.szTip[..tip.len().min(128)].copy_from_slice(&tip[..tip.len().min(128)]);
        tray.add_icon()?;
        Ok(tray)
    }

    fn add_icon(&mut self) -> Result<()> {
        ensure!(
            unsafe { Shell_NotifyIconW(NIM_ADD, &self.notice) } != 0,
            "failed to show the Beale tray icon: {}",
            std::io::Error::last_os_error()
        );
        self.added = true;
        self.notice.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        ensure!(
            unsafe { Shell_NotifyIconW(NIM_SETVERSION, &self.notice) } != 0,
            "failed to set the Beale tray icon version: {}",
            std::io::Error::last_os_error()
        );
        Ok(())
    }

    fn run(&mut self) -> Result<bool> {
        loop {
            // MSG is a plain Windows message record with a valid zero state.
            let mut message: MSG = unsafe { std::mem::zeroed() };
            let result = unsafe { GetMessageW(&mut message, ptr::null_mut(), 0, 0) };
            if result == -1 {
                bail!(
                    "failed to read a tray message: {}",
                    std::io::Error::last_os_error()
                );
            }
            if result == 0 {
                return Ok(message.wParam == EXIT_COMMAND);
            }
            if message.message == self.taskbar_created {
                self.added = false;
                self.add_icon()
                    .context("failed to restore the tray icon after taskbar restart")?;
            }
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
}

impl Drop for TrayResources {
    fn drop(&mut self) {
        // Shell, USER32, and the window class all use handles from this thread.
        unsafe {
            if self.added {
                Shell_NotifyIconW(NIM_DELETE, &self.notice);
            }
            if !self.window.is_null() {
                DestroyWindow(self.window);
            }
            if !self.icon.is_null() {
                DestroyIcon(self.icon);
            }
            if !self.window.is_null() {
                UnregisterClassW(wide(CLASS_NAME).as_ptr(), GetModuleHandleW(ptr::null()));
            }
        }
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        TRAY_MESSAGE if matches!((lparam as u32) & 0xffff, WM_CONTEXTMENU | WM_RBUTTONUP) => {
            show_menu(window);
            0
        }
        WM_COMMAND if wparam & 0xffff == EXIT_COMMAND => {
            unsafe { PostQuitMessage(EXIT_COMMAND as i32) };
            0
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

fn show_menu(window: HWND) {
    // The popup belongs to the tray thread; no menu handles cross the async host boundary.
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        let status = wide("Beale app-server is running");
        let exit = wide("Exit");
        let mut point: POINT = std::mem::zeroed();
        if AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, status.as_ptr()) != 0
            && AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null()) != 0
            && AppendMenuW(menu, MF_STRING, EXIT_COMMAND, exit.as_ptr()) != 0
            && GetCursorPos(&mut point) != 0
        {
            SetForegroundWindow(window);
            TrackPopupMenu(
                menu,
                TPM_BOTTOMALIGN | TPM_RIGHTBUTTON,
                point.x,
                point.y,
                0,
                window,
                ptr::null(),
            );
            PostMessageW(window, WM_NULL, 0, 0);
        }
        DestroyMenu(menu);
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn icon_image(bytes: &[u8], width: u8) -> Result<&[u8]> {
    ensure!(
        bytes.len() >= 6 && bytes[..4] == [0, 0, 1, 0],
        "invalid Beale icon file"
    );
    let count = usize::from(u16::from_le_bytes([bytes[4], bytes[5]]));
    for index in 0..count {
        let offset = 6 + index * 16;
        let entry = bytes
            .get(offset..offset + 16)
            .context("truncated Beale icon entry")?;
        if entry[0] == width && entry[1] == width {
            let length = u32::from_le_bytes(entry[8..12].try_into()?) as usize;
            let start = u32::from_le_bytes(entry[12..16].try_into()?) as usize;
            let end = start
                .checked_add(length)
                .context("invalid Beale icon length")?;
            return bytes.get(start..end).context("truncated Beale icon image");
        }
    }
    bail!("Beale icon is missing a {width}px image")
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW;

    #[test]
    fn bundled_icon_contains_a_small_tray_image() {
        assert!(icon_image(ICON_BYTES, 32).is_ok());
        assert!(icon_image(&[0, 0, 1, 0, 1, 0], 32).is_err());
    }

    #[test]
    fn exit_menu_command_requests_shutdown() {
        let (exit_tx, exit_rx) = oneshot::channel();
        let tray = TrayHandle::start("127.0.0.1:43831".parse().unwrap(), exit_tx).unwrap();
        let window = unsafe { FindWindowW(wide(CLASS_NAME).as_ptr(), ptr::null()) };
        assert!(!window.is_null());
        assert_ne!(
            unsafe { PostMessageW(window, WM_COMMAND, EXIT_COMMAND, 0) },
            0
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        runtime.block_on(async {
            assert!(
                tokio::time::timeout(std::time::Duration::from_secs(3), exit_rx)
                    .await
                    .unwrap()
                    .is_ok()
            );
        });
        drop(tray);
    }
}
