use anyhow::{Context, Result, ensure};
use beale_contracts::{
    FLEET_TOTP_PROVISION_PATH, PROTOBUF_CONTENT_TYPE, v1::FleetTotpProvisioningResponse,
};
use prost::Message;
use qrcodegen::{QrCode, QrCodeEcc};
use reqwest::{Client, StatusCode, header::CONTENT_TYPE};
use std::{cell::RefCell, net::SocketAddr, ptr, time::Duration};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::{
        BeginPaint, CreateSolidBrush, DT_CENTER, DT_WORDBREAK, DeleteObject, DrawTextW, EndPaint,
        FillRect, InvalidateRect, PAINTSTRUCT, SetBkMode, SetTextColor, TRANSPARENT, UpdateWindow,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::{
        CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect,
        RegisterClassW, SW_SHOWNORMAL, SetForegroundWindow, ShowWindow, WM_CLOSE, WM_DESTROY,
        WM_PAINT, WNDCLASSW, WS_CAPTION, WS_EX_APPWINDOW, WS_OVERLAPPED, WS_SYSMENU,
    },
};

const CLASS_NAME: &str = "BealeAppServerAuthenticatorQr";
const WINDOW_TITLE: &str = "Beale Authenticator QR Code";

struct PopupState {
    connection: Option<(SocketAddr, String)>,
    window: HWND,
    code: Option<QrCode>,
    error: bool,
}

impl Default for PopupState {
    fn default() -> Self {
        Self {
            connection: None,
            window: ptr::null_mut(),
            code: None,
            error: false,
        }
    }
}

thread_local! {
    static STATE: RefCell<PopupState> = RefCell::new(PopupState::default());
}

pub(super) fn configure(addr: SocketAddr, token: String) {
    STATE.with(|state| state.borrow_mut().connection = Some((addr, token)));
}

pub(super) fn show() {
    let (existing, connection) = STATE.with(|state| {
        let state = state.borrow();
        (state.window, state.connection.clone())
    });
    if !existing.is_null() {
        // The tray and QR windows share this thread, so the window is still live here.
        unsafe {
            ShowWindow(existing, SW_SHOWNORMAL);
            SetForegroundWindow(existing);
        }
        return;
    }
    let Some((addr, token)) = connection else {
        return;
    };
    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    if instance.is_null() {
        return;
    }
    let class_name = wide(CLASS_NAME);
    let class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        lpszClassName: class_name.as_ptr(),
        ..unsafe { std::mem::zeroed() }
    };
    // The class remains registered for this process; a later tray click can reuse it.
    unsafe { RegisterClassW(&class) };
    let title = wide(WINDOW_TITLE);
    let window = unsafe {
        CreateWindowExW(
            WS_EX_APPWINDOW,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            430,
            490,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            ptr::null(),
        )
    };
    if window.is_null() {
        beale_host_log::error!("Could not open the app-server authenticator QR window");
        return;
    }
    STATE.with(|state| state.borrow_mut().window = window);
    // Reveal during the menu click, before the local provisioning request begins.
    unsafe {
        ShowWindow(window, SW_SHOWNORMAL);
        SetForegroundWindow(window);
        UpdateWindow(window);
    }
    let result = (|| -> Result<QrCode> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("could not initialize local QR request")?;
        let uri = runtime.block_on(fetch_provisioning_uri(addr, &token))?;
        QrCode::encode_text(&uri, QrCodeEcc::Medium)
            .map_err(|_| anyhow::anyhow!("could not encode authenticator QR code"))
    })();
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        match result {
            Ok(code) => state.code = Some(code),
            Err(_) => state.error = true,
        }
    });
    unsafe { InvalidateRect(window, ptr::null(), 1) };
}

pub(super) fn shutdown() {
    let window = STATE.with(|state| state.borrow().window);
    if !window.is_null() {
        unsafe { DestroyWindow(window) };
    }
    STATE.with(|state| *state.borrow_mut() = PopupState::default());
}

async fn fetch_provisioning_uri(addr: SocketAddr, token: &str) -> Result<String> {
    let client = Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()?;
    let mut response = client
        .get(format!("http://{addr}{FLEET_TOTP_PROVISION_PATH}"))
        .bearer_auth(token)
        .send()
        .await?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    ensure!(status == StatusCode::OK, "provisioning unavailable");
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= 4_096,
            "provisioning response too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    decode_provisioning_response(status, content_type.as_deref(), &bytes)
}

fn decode_provisioning_response(
    status: StatusCode,
    content_type: Option<&str>,
    bytes: &[u8],
) -> Result<String> {
    ensure!(status == StatusCode::OK, "provisioning unavailable");
    ensure!(
        content_type == Some(PROTOBUF_CONTENT_TYPE),
        "unexpected provisioning content type"
    );
    ensure!(bytes.len() <= 4_096, "provisioning response too large");
    let reply = FleetTotpProvisioningResponse::decode(bytes)?;
    ensure!(
        reply.otpauth_uri.len() <= 300 && reply.otpauth_uri.starts_with("otpauth://totp/Beale:"),
        "invalid authenticator URI"
    );
    Ok(reply.otpauth_uri)
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_PAINT => {
            paint(window);
            0
        }
        WM_CLOSE => {
            unsafe { DestroyWindow(window) };
            0
        }
        WM_DESTROY => {
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.window = ptr::null_mut();
                state.code = None;
                state.error = false;
            });
            0
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

fn paint(window: HWND) {
    // WM_PAINT runs on the tray thread while this window is alive.
    unsafe {
        let mut paint: PAINTSTRUCT = std::mem::zeroed();
        let dc = BeginPaint(window, &mut paint);
        if dc.is_null() {
            return;
        }
        let mut client: RECT = std::mem::zeroed();
        GetClientRect(window, &mut client);
        fill(dc, &client, 0x151515);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, 0xeeeeee);
        draw_text(dc, "Authenticator QR Code", 14, 40, client.right);
        draw_text(
            dc,
            "Scan with an authenticator app. Use its six-digit code to connect another client.",
            42,
            78,
            client.right,
        );
        STATE.with(|state| {
            let state = state.borrow();
            if let Some(code) = &state.code {
                let modules = code.size() + 8;
                let pixel = (290 / modules).max(3);
                let side = modules * pixel;
                let left = (client.right - side) / 2;
                let top = 90;
                fill(
                    dc,
                    &RECT {
                        left,
                        top,
                        right: left + side,
                        bottom: top + side,
                    },
                    0xffffff,
                );
                let black = CreateSolidBrush(0x0d0d0d);
                if !black.is_null() {
                    for y in 0..code.size() {
                        for x in 0..code.size() {
                            if code.get_module(x, y) {
                                let left = left + (x + 4) * pixel;
                                let top = top + (y + 4) * pixel;
                                FillRect(
                                    dc,
                                    &RECT {
                                        left,
                                        top,
                                        right: left + pixel,
                                        bottom: top + pixel,
                                    },
                                    black,
                                );
                            }
                        }
                    }
                    DeleteObject(black);
                }
            } else if state.error {
                draw_text(
                    dc,
                    "The QR code is unavailable. Enable Fleet in Beale or try again.",
                    190,
                    240,
                    client.right,
                );
            } else {
                draw_text(dc, "Preparing QR code…", 190, 230, client.right);
            }
        });
        SetTextColor(dc, 0x999999);
        draw_text(
            dc,
            "Keep this QR private. Anyone who scans it can generate pairing codes.",
            client.bottom - 52,
            client.bottom - 12,
            client.right,
        );
        EndPaint(window, &paint);
    }
}

unsafe fn fill(dc: windows_sys::Win32::Graphics::Gdi::HDC, rect: &RECT, color: u32) {
    let brush = unsafe { CreateSolidBrush(color) };
    if !brush.is_null() {
        unsafe {
            FillRect(dc, rect, brush);
            DeleteObject(brush);
        }
    }
}

unsafe fn draw_text(
    dc: windows_sys::Win32::Graphics::Gdi::HDC,
    value: &str,
    top: i32,
    bottom: i32,
    width: i32,
) {
    let wide = wide(value);
    let mut rect = RECT {
        left: 20,
        top,
        right: width - 20,
        bottom,
    };
    unsafe {
        DrawTextW(
            dc,
            wide.as_ptr(),
            i32::try_from(wide.len() - 1).unwrap_or_default(),
            &mut rect,
            DT_CENTER | DT_WORDBREAK,
        );
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provisioning_response_is_bounded_and_encodes_as_a_qr_code() {
        let body = FleetTotpProvisioningResponse {
            otpauth_uri: "otpauth://totp/Beale:example-host?secret=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA&issuer=Beale&algorithm=SHA1&digits=6&period=30".to_owned(),
        }
        .encode_to_vec();
        let decode = |status, content_type, bytes: &[u8]| {
            decode_provisioning_response(status, content_type, bytes)
        };
        let uri = decode(StatusCode::OK, Some(PROTOBUF_CONTENT_TYPE), &body).unwrap();
        assert!(uri.starts_with("otpauth://totp/Beale:example-host?"));
        assert!(QrCode::encode_text(&uri, QrCodeEcc::Medium).is_ok());
        assert!(decode(StatusCode::UNAUTHORIZED, Some(PROTOBUF_CONTENT_TYPE), &body).is_err());
        assert!(decode(StatusCode::OK, Some("text/plain"), &body).is_err());
        assert!(decode(StatusCode::OK, Some(PROTOBUF_CONTENT_TYPE), &[0; 4_097]).is_err());
        let invalid = FleetTotpProvisioningResponse {
            otpauth_uri: "https://example.test/qr".to_owned(),
        }
        .encode_to_vec();
        assert!(decode(StatusCode::OK, Some(PROTOBUF_CONTENT_TYPE), &invalid).is_err());
    }
}
