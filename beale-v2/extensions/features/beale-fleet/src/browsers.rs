use beale_contracts::v1::FleetBrowser;
use beale_storage::ManualBrowserRow;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

struct BrowserDefinition {
    id: &'static str,
    name: &'static str,
    paths: &'static [&'static str],
}

#[cfg(target_os = "windows")]
const DEFINITIONS: &[BrowserDefinition] = &[
    BrowserDefinition {
        id: "chrome",
        name: "Google Chrome",
        paths: &[
            "ProgramFiles/Google/Chrome/Application/chrome.exe",
            "ProgramFilesX86/Google/Chrome/Application/chrome.exe",
            "LocalAppData/Google/Chrome/Application/chrome.exe",
        ],
    },
    BrowserDefinition {
        id: "safari",
        name: "Apple Safari",
        paths: &[
            "ProgramFiles/Safari/Safari.exe",
            "ProgramFilesX86/Safari/Safari.exe",
        ],
    },
    BrowserDefinition {
        id: "edge",
        name: "Microsoft Edge",
        paths: &[
            "ProgramFilesX86/Microsoft/Edge/Application/msedge.exe",
            "ProgramFiles/Microsoft/Edge/Application/msedge.exe",
        ],
    },
    BrowserDefinition {
        id: "firefox",
        name: "Mozilla Firefox",
        paths: &[
            "ProgramFiles/Mozilla Firefox/firefox.exe",
            "ProgramFilesX86/Mozilla Firefox/firefox.exe",
        ],
    },
    BrowserDefinition {
        id: "opera",
        name: "Opera",
        paths: &[
            "LocalAppData/Programs/Opera/launcher.exe",
            "ProgramFiles/Opera/launcher.exe",
            "ProgramFilesX86/Opera/launcher.exe",
        ],
    },
    BrowserDefinition {
        id: "brave",
        name: "Brave",
        paths: &[
            "ProgramFiles/BraveSoftware/Brave-Browser/Application/brave.exe",
            "ProgramFilesX86/BraveSoftware/Brave-Browser/Application/brave.exe",
            "LocalAppData/BraveSoftware/Brave-Browser/Application/brave.exe",
        ],
    },
    BrowserDefinition {
        id: "duckduckgo",
        name: "DuckDuckGo",
        paths: &["LocalAppData/Microsoft/WindowsApps/DuckDuckGo.exe"],
    },
];

#[cfg(target_os = "macos")]
const DEFINITIONS: &[BrowserDefinition] = &[
    BrowserDefinition {
        id: "chrome",
        name: "Google Chrome",
        paths: &["Applications/Google Chrome.app/Contents/MacOS/Google Chrome"],
    },
    BrowserDefinition {
        id: "safari",
        name: "Apple Safari",
        paths: &["Applications/Safari.app/Contents/MacOS/Safari"],
    },
    BrowserDefinition {
        id: "edge",
        name: "Microsoft Edge",
        paths: &["Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"],
    },
    BrowserDefinition {
        id: "firefox",
        name: "Mozilla Firefox",
        paths: &["Applications/Firefox.app/Contents/MacOS/firefox"],
    },
    BrowserDefinition {
        id: "opera",
        name: "Opera",
        paths: &["Applications/Opera.app/Contents/MacOS/Opera"],
    },
    BrowserDefinition {
        id: "brave",
        name: "Brave",
        paths: &["Applications/Brave Browser.app/Contents/MacOS/Brave Browser"],
    },
    BrowserDefinition {
        id: "duckduckgo",
        name: "DuckDuckGo",
        paths: &["Applications/DuckDuckGo.app/Contents/MacOS/DuckDuckGo"],
    },
];

#[cfg(target_os = "linux")]
const DEFINITIONS: &[BrowserDefinition] = &[
    BrowserDefinition {
        id: "chrome",
        name: "Google Chrome",
        paths: &[
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
            "/opt/google/chrome/chrome",
        ],
    },
    BrowserDefinition {
        id: "safari",
        name: "Apple Safari",
        paths: &[],
    },
    BrowserDefinition {
        id: "edge",
        name: "Microsoft Edge",
        paths: &[
            "/usr/bin/microsoft-edge",
            "/usr/bin/microsoft-edge-stable",
            "/opt/microsoft/msedge/msedge",
        ],
    },
    BrowserDefinition {
        id: "firefox",
        name: "Mozilla Firefox",
        paths: &[
            "/usr/bin/firefox",
            "/snap/bin/firefox",
            "/opt/firefox/firefox",
        ],
    },
    BrowserDefinition {
        id: "opera",
        name: "Opera",
        paths: &["/usr/bin/opera", "/snap/bin/opera", "/opt/opera/opera"],
    },
    BrowserDefinition {
        id: "brave",
        name: "Brave",
        paths: &[
            "/usr/bin/brave-browser",
            "/snap/bin/brave",
            "/opt/brave.com/brave/brave",
        ],
    },
    BrowserDefinition {
        id: "duckduckgo",
        name: "DuckDuckGo",
        paths: &[],
    },
];

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
const DEFINITIONS: &[BrowserDefinition] = &[];

fn browser_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return false;
        }
    }
    true
}

#[cfg(target_os = "windows")]
fn candidates(relative: &str) -> Vec<PathBuf> {
    let Some((root, rest)) = relative.split_once('/') else {
        return Vec::new();
    };
    let variable = match root {
        "ProgramFiles" => "ProgramFiles",
        "ProgramFilesX86" => "ProgramFiles(x86)",
        "LocalAppData" => "LOCALAPPDATA",
        _ => return Vec::new(),
    };
    std::env::var_os(variable)
        .map(|base| vec![PathBuf::from(base).join(rest)])
        .unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn candidates(relative: &str) -> Vec<PathBuf> {
    let Some(rest) = relative.strip_prefix("Applications/") else {
        return Vec::new();
    };
    let mut paths = vec![PathBuf::from("/Applications").join(rest)];
    if let Some(home) = std::env::var_os("HOME") {
        paths.push(PathBuf::from(home).join("Applications").join(rest));
    }
    paths
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn candidates(relative: &str) -> Vec<PathBuf> {
    vec![PathBuf::from(relative)]
}

pub(crate) fn detected_path(path: &Path) -> bool {
    DEFINITIONS
        .iter()
        .flat_map(|definition| definition.paths)
        .flat_map(|candidate| candidates(candidate))
        .filter(|candidate| browser_file(candidate))
        .any(|candidate| candidate.canonicalize().ok().as_deref() == Some(path))
}

pub(crate) fn inventory(manual: &[ManualBrowserRow]) -> Vec<FleetBrowser> {
    let mut browsers = Vec::new();
    let mut found = HashSet::new();
    for definition in DEFINITIONS {
        if let Some(path) = definition
            .paths
            .iter()
            .flat_map(|path| candidates(path))
            .find(|path| browser_file(path))
        {
            found.insert(path.canonicalize().unwrap_or(path));
            browsers.push(FleetBrowser {
                id: format!("detected:{}", definition.id),
                name: definition.name.to_owned(),
                manual: false,
                available: true,
                enabled: true,
                all_workspaces: true,
                workspace_ids: Vec::new(),
                preferred: false,
            });
        }
    }
    for entry in manual {
        let path = PathBuf::from(&entry.executable);
        if found.insert(path.canonicalize().unwrap_or(path.clone())) {
            browsers.push(FleetBrowser {
                id: entry.id.clone(),
                name: entry.name.clone(),
                manual: true,
                available: browser_file(&path),
                enabled: true,
                all_workspaces: true,
                workspace_ids: Vec::new(),
                preferred: false,
            });
        }
    }
    browsers.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
    browsers
}

#[cfg(windows)]
fn os_primary_hint() -> Option<String> {
    use windows_sys::Win32::UI::Shell::{
        ASSOCF_NONE, ASSOCSTR_EXECUTABLE, ASSOCSTR_PROGID, AssocQueryStringW,
    };

    let association = "https\0".encode_utf16().collect::<Vec<_>>();
    for kind in [ASSOCSTR_EXECUTABLE, ASSOCSTR_PROGID] {
        let mut length = 0_u32;
        // Querying a fixed URL association only reads the current user's OS choice.
        unsafe {
            AssocQueryStringW(
                ASSOCF_NONE,
                kind,
                association.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                &mut length,
            );
        }
        if length == 0 || length > 32_768 {
            continue;
        }
        let mut buffer = vec![0_u16; length as usize];
        let result = unsafe {
            AssocQueryStringW(
                ASSOCF_NONE,
                kind,
                association.as_ptr(),
                std::ptr::null(),
                buffer.as_mut_ptr(),
                &mut length,
            )
        };
        if result >= 0 {
            let end = buffer
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(buffer.len());
            if let Ok(value) = String::from_utf16(&buffer[..end]) {
                return Some(value);
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn os_primary_hint() -> Option<String> {
    use std::ffi::{CStr, c_char, c_void};

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(
            allocator: *const c_void,
            text: *const c_char,
            encoding: u32,
        ) -> *const c_void;
        fn CFStringGetCString(
            value: *const c_void,
            buffer: *mut c_char,
            size: isize,
            encoding: u32,
        ) -> u8;
        fn CFRelease(value: *const c_void);
    }
    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn LSCopyDefaultHandlerForURLScheme(scheme: *const c_void) -> *const c_void;
    }

    const UTF8: u32 = 0x0800_0100;
    let scheme = unsafe { CFStringCreateWithCString(std::ptr::null(), c"https".as_ptr(), UTF8) };
    if scheme.is_null() {
        return None;
    }
    let handler = unsafe { LSCopyDefaultHandlerForURLScheme(scheme) };
    unsafe { CFRelease(scheme) };
    if handler.is_null() {
        return None;
    }
    let mut buffer = [0_i8; 512];
    let copied =
        unsafe { CFStringGetCString(handler, buffer.as_mut_ptr(), buffer.len() as isize, UTF8) };
    unsafe { CFRelease(handler) };
    if copied == 0 {
        return None;
    }
    Some(
        unsafe { CStr::from_ptr(buffer.as_ptr()) }
            .to_string_lossy()
            .into_owned(),
    )
}

#[cfg(target_os = "linux")]
fn os_primary_hint() -> Option<String> {
    let output = std::process::Command::new("xdg-settings")
        .args(["get", "default-web-browser"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn os_primary_hint() -> Option<String> {
    None
}

fn browser_family(hint: &str) -> Option<&'static str> {
    let hint = hint.to_ascii_lowercase();
    if hint.contains("duckduckgo") {
        Some("duckduckgo")
    } else if hint.contains("brave") {
        Some("brave")
    } else if hint.contains("firefox") || hint.contains("mozilla") {
        Some("firefox")
    } else if hint.contains("msedge")
        || hint.contains("microsoft.edge")
        || hint.contains("microsoft-edge")
    {
        Some("edge")
    } else if hint.contains("opera") {
        Some("opera")
    } else if hint.contains("chrome") || hint.contains("google.chrome") {
        Some("chrome")
    } else if hint.contains("safari") {
        Some("safari")
    } else {
        None
    }
}

pub(crate) fn preferred_id(
    browsers: &[FleetBrowser],
    manual: &[ManualBrowserRow],
    saved_id: Option<&str>,
) -> Option<String> {
    preferred_id_with_hint(browsers, manual, saved_id, os_primary_hint().as_deref())
}

fn preferred_id_with_hint(
    browsers: &[FleetBrowser],
    manual: &[ManualBrowserRow],
    saved_id: Option<&str>,
    os_hint: Option<&str>,
) -> Option<String> {
    let eligible = |browser: &&FleetBrowser| browser.enabled && browser.available;
    if let Some(saved) = browsers
        .iter()
        .filter(eligible)
        .find(|browser| Some(browser.id.as_str()) == saved_id)
    {
        return Some(saved.id.clone());
    }
    if let Some(hint) = os_hint {
        let path = Path::new(hint);
        if path.is_absolute() {
            let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
            if let Some(entry) = manual.iter().find(|entry| {
                Path::new(&entry.executable).canonicalize().ok().as_deref() == Some(path.as_path())
            }) && let Some(browser) = browsers
                .iter()
                .filter(eligible)
                .find(|browser| browser.id == entry.id)
            {
                return Some(browser.id.clone());
            }
            for definition in DEFINITIONS {
                if definition
                    .paths
                    .iter()
                    .flat_map(|relative| candidates(relative))
                    .any(|candidate| {
                        candidate.canonicalize().ok().as_deref() == Some(path.as_path())
                    })
                {
                    let id = format!("detected:{}", definition.id);
                    if let Some(browser) = browsers
                        .iter()
                        .filter(eligible)
                        .find(|browser| browser.id == id)
                    {
                        return Some(browser.id.clone());
                    }
                }
            }
        }
        if let Some(family) = browser_family(hint) {
            let id = format!("detected:{family}");
            if let Some(browser) = browsers
                .iter()
                .filter(eligible)
                .find(|browser| browser.id == id)
            {
                return Some(browser.id.clone());
            }
        }
    }
    browsers
        .iter()
        .find(eligible)
        .map(|browser| browser.id.clone())
}

pub(crate) fn validate_manual(name: &str, executable: &str) -> Option<PathBuf> {
    if name.trim().is_empty()
        || name.len() > 80
        || name.chars().any(char::is_control)
        || executable.len() > 2_048
        || executable.chars().any(char::is_control)
    {
        return None;
    }
    let path = Path::new(executable);
    if !path.is_absolute() || !browser_file(path) {
        return None;
    }
    #[cfg(windows)]
    if !path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        return None;
    }
    path.canonicalize().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_includes_all_requested_browser_families() {
        assert_eq!(
            DEFINITIONS
                .iter()
                .map(|definition| definition.name)
                .collect::<Vec<_>>(),
            [
                "Google Chrome",
                "Apple Safari",
                "Microsoft Edge",
                "Mozilla Firefox",
                "Opera",
                "Brave",
                "DuckDuckGo"
            ]
        );
    }

    #[test]
    fn manual_entries_do_not_expose_paths() {
        let entry = ManualBrowserRow {
            id: "example-browser".to_owned(),
            name: "Example Browser".to_owned(),
            executable: "/example.test/browser".to_owned(),
        };
        let browsers = inventory(&[entry]);
        let browser = browsers
            .iter()
            .find(|browser| browser.id == "example-browser")
            .unwrap();
        assert!(!browser.available);
        assert_eq!(browser.name, "Example Browser");
    }

    #[test]
    fn preferred_browser_uses_os_choice_then_saved_override_then_fallback() {
        let browsers = ["brave", "chrome", "edge"]
            .into_iter()
            .map(|family| FleetBrowser {
                id: format!("detected:{family}"),
                name: family.to_owned(),
                manual: false,
                available: true,
                enabled: true,
                all_workspaces: true,
                workspace_ids: Vec::new(),
                preferred: false,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            preferred_id_with_hint(&browsers, &[], None, Some("MSEdgeHTM")),
            Some("detected:edge".to_owned())
        );
        assert_eq!(
            preferred_id_with_hint(&browsers, &[], Some("detected:brave"), Some("MSEdgeHTM")),
            Some("detected:brave".to_owned())
        );
        let mut disabled = browsers.clone();
        disabled[2].enabled = false;
        assert_eq!(
            preferred_id_with_hint(&disabled, &[], Some("detected:edge"), Some("MSEdgeHTM")),
            Some("detected:brave".to_owned())
        );
    }
}
