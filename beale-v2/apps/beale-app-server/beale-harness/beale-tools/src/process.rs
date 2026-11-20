//! Host processes started for model-requested execution.
//!
//! Tool subprocesses start from an allowlisted environment, so provider
//! credentials and Beale host variables in the app-server environment are not
//! inherited. Each process runs in its own process group (Unix) or job object
//! (Windows) so a timeout, oversized output, or user stop ends its descendants
//! too. This is credential hygiene and cleanup, not OS isolation: the process
//! still runs with the current user's host privileges and file access.

use std::ffi::{OsStr, OsString};
use tokio::process::{Child, Command};

/// Operator-supplied, comma-separated environment variable names that tool
/// subprocesses may also inherit. Names that look like credentials are ignored.
pub const TOOL_ENV_PASSTHROUGH: &str = "BEALE_V2_TOOL_ENV";

/// Variables a shell, interpreter, toolchain, or proxy-aware client commonly
/// needs. Compared case-insensitively on Windows.
const ALLOWED: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TERM",
    "LANG",
    "LANGUAGE",
    "TZ",
    "TMPDIR",
    "TEMP",
    "TMP",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "all_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "CURL_CA_BUNDLE",
    "REQUESTS_CA_BUNDLE",
    "NODE_EXTRA_CA_CERTS",
    "JAVA_HOME",
    "GOPATH",
    "GOROOT",
    "GOCACHE",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "ANDROID_HOME",
    "ANDROID_SDK_ROOT",
    "ANDROID_NDK_HOME",
    "DOTNET_ROOT",
    "NVM_DIR",
    "PYENV_ROOT",
    "VIRTUAL_ENV",
    "CONDA_PREFIX",
    "PKG_CONFIG_PATH",
    "LD_LIBRARY_PATH",
    "SYSTEMROOT",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "SYSTEMDRIVE",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "PROGRAMW6432",
    "COMMONPROGRAMFILES",
    "COMMONPROGRAMFILES(X86)",
    "COMMONPROGRAMW6432",
    "PROGRAMDATA",
    "ALLUSERSPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "USERPROFILE",
    "USERNAME",
    "USERDOMAIN",
    "HOMEDRIVE",
    "HOMEPATH",
    "PUBLIC",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "PROCESSOR_IDENTIFIER",
    "OS",
    "PSMODULEPATH",
    "MSYSTEM",
];

const ALLOWED_PREFIXES: &[&str] = &["LC_", "XDG_", "GIT_CONFIG_"];

const SECRET_MARKERS: &[&str] = &[
    "API_KEY",
    "APIKEY",
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "CREDENTIAL",
    "PRIVATE_KEY",
];

fn normalized(name: &str) -> String {
    if cfg!(windows) {
        name.to_ascii_uppercase()
    } else {
        name.to_owned()
    }
}

fn allowed(name: &str) -> bool {
    let name = normalized(name);
    ALLOWED.iter().any(|item| normalized(item) == name)
        || ALLOWED_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(&normalized(prefix)))
}

/// Whether a variable name looks like it holds a credential value.
pub fn credential_env_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    SECRET_MARKERS.iter().any(|marker| upper.contains(marker))
}

/// Whether a variable name looks like credential material or Beale host state.
pub fn secret_like_env_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.starts_with("BEALE_")
        || upper == "CODEX_HOME"
        || upper.ends_with("AUTH_FILE")
        || credential_env_name(name)
}

/// Filters an environment for a tool subprocess.
pub fn tool_environment_from(
    vars: impl IntoIterator<Item = (OsString, OsString)>,
    passthrough: Option<&str>,
) -> Vec<(OsString, OsString)> {
    let extra: Vec<String> = passthrough
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty() && !secret_like_env_name(name))
        .map(normalized)
        .collect();
    vars.into_iter()
        .filter(|(name, _)| {
            let Some(name) = name.to_str() else {
                return false;
            };
            !secret_like_env_name(name)
                && (allowed(name) || extra.iter().any(|item| *item == normalized(name)))
        })
        .collect()
}

/// The app-server environment filtered for a tool subprocess.
pub fn tool_environment() -> Vec<(OsString, OsString)> {
    let passthrough = std::env::var(TOOL_ENV_PASSTHROUGH).ok();
    tool_environment_from(std::env::vars_os(), passthrough.as_deref())
}

/// A command with the tool environment and its own process group.
pub fn contained_command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    command
        .env_clear()
        .envs(tool_environment())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    command
}

/// Removes inherited credential-like variables while keeping the rest of the
/// environment. Used for operator-installed plugin servers, which may rely on
/// toolchain variables outside the tool allowlist.
pub fn remove_secret_env(command: &mut Command) {
    for (name, _) in std::env::vars_os() {
        if name.to_str().is_some_and(secret_like_env_name) {
            command.env_remove(name);
        }
    }
}

/// Ends a started process and its descendants unless released after a normal
/// exit. Dropping an unreleased tree kills it.
pub struct ProcessTree {
    #[cfg(unix)]
    group: Option<i32>,
    #[cfg(windows)]
    job: Option<job::Job>,
    armed: bool,
}

impl ProcessTree {
    /// Tracks the process tree rooted at a child started by [`contained_command`].
    pub fn attach(child: &Child) -> Self {
        Self {
            #[cfg(unix)]
            group: child.id().and_then(|id| i32::try_from(id).ok()),
            #[cfg(windows)]
            job: child.raw_handle().and_then(job::Job::assign),
            armed: true,
        }
    }

    /// Leaves background descendants running after the root exited normally.
    pub fn release(mut self) {
        self.armed = false;
    }

    fn kill(&mut self) {
        #[cfg(unix)]
        if let Some(group) = self.group.take() {
            // SAFETY: killpg only sends a signal; the group ID came from a child
            // this process started with its own process group.
            unsafe {
                libc::killpg(group, libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        if let Some(job) = self.job.take() {
            job.terminate();
        }
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        if self.armed {
            self.kill();
        }
    }
}

#[cfg(windows)]
mod job {
    use std::os::windows::io::RawHandle;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW, TerminateJobObject},
    };

    /// An owned job object handle. Closing it does not end its processes.
    pub(super) struct Job(HANDLE);

    // SAFETY: a job object handle is a kernel handle usable from any thread.
    unsafe impl Send for Job {}
    // SAFETY: the wrapped handle is only passed to thread-safe kernel calls.
    unsafe impl Sync for Job {}

    impl Job {
        pub(super) fn assign(process: RawHandle) -> Option<Self> {
            // SAFETY: null security attributes and name create an unnamed job
            // with default security; the returned handle is owned here.
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return None;
            }
            let job = Self(handle);
            // SAFETY: both handles are valid for the duration of the call; the
            // process handle is borrowed from a live tokio child.
            if unsafe { AssignProcessToJobObject(job.0, process as HANDLE) } == 0 {
                return None;
            }
            Some(job)
        }

        pub(super) fn terminate(&self) {
            // SAFETY: the handle is a valid job object owned by this value.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle is owned by this value and closed once.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(items: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        items
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect()
    }

    fn names(env: &[(OsString, OsString)]) -> Vec<String> {
        env.iter()
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn tool_environment_drops_credentials_and_host_state() {
        let env = tool_environment_from(
            vars(&[
                ("PATH", "/usr/bin"),
                ("HOME", "/home/example"),
                ("LC_ALL", "C.UTF-8"),
                ("GIT_CONFIG_COUNT", "1"),
                ("OPENAI_API_KEY", "sk-example"),
                ("ANTHROPIC_API_KEY", "example"),
                ("GITHUB_TOKEN", "example"),
                ("BEALE_V2_HOME", "/home/example/.beale/v2"),
                ("APP_SERVER_CODEX_AUTH_FILE", "/home/example/auth.json"),
                ("CODEX_HOME", "/home/example/.codex"),
                ("UNRELATED_SETTING", "value"),
            ]),
            None,
        );
        assert_eq!(names(&env), ["PATH", "HOME", "LC_ALL", "GIT_CONFIG_COUNT"]);
    }

    #[test]
    fn operator_passthrough_cannot_reintroduce_secrets() {
        let env = tool_environment_from(
            vars(&[
                ("EXAMPLE_SDK_ROOT", "/opt/example"),
                ("EXAMPLE_API_KEY", "example"),
                ("BEALE_V2_APP_SERVER_ADDR", "127.0.0.1:43831"),
            ]),
            Some("EXAMPLE_SDK_ROOT, EXAMPLE_API_KEY,BEALE_V2_APP_SERVER_ADDR"),
        );
        assert_eq!(names(&env), ["EXAMPLE_SDK_ROOT"]);
    }

    #[cfg(windows)]
    #[test]
    fn windows_names_match_case_insensitively() {
        let env = tool_environment_from(
            vars(&[("Path", "C:\\Windows"), ("SystemRoot", "C:\\Windows")]),
            None,
        );
        assert_eq!(names(&env), ["Path", "SystemRoot"]);
    }
}
