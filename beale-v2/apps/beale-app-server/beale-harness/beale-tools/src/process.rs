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

/// Explicit Docker worker entry only. The container is the outer managed
/// execution boundary; a dedicated group also retains normal command cleanup.
#[cfg(target_os = "linux")]
static CONTAINER_WORKER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(not(windows))]
pub(crate) fn is_container_worker() -> bool {
    #[cfg(target_os = "linux")]
    {
        CONTAINER_WORKER.load(std::sync::atomic::Ordering::Acquire)
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

#[cfg(target_os = "linux")]
pub fn establish_container_worker_group() -> std::io::Result<()> {
    // SAFETY: setpgid(0,0) changes only this process's group to its own PID.
    if unsafe { libc::setpgid(0, 0) } != 0 {
        // A container runtime may already have made this process a session
        // leader; accept that case only after checking the exact group.
        require_worker_group()?;
    }
    require_worker_group()?;
    CONTAINER_WORKER.store(true, std::sync::atomic::Ordering::Release);
    Ok(())
}

/// A dedicated worker must own its process group on Unix. The supervisor ends
/// this group with the worker; see [`worker_commands_are_isolated`] for where
/// target commands run.
pub fn require_worker_group() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        // SAFETY: these calls only read the current process and group IDs.
        if unsafe { libc::getpgrp() != libc::getpid() } {
            return Err(std::io::Error::other(
                "worker must be launched in its own process group",
            ));
        }
    }
    Ok(())
}

/// Set to `1` by the Linux subreaper supervisor in [`crate::linux_worker`] for
/// the worker it launches. Tool commands never inherit it: their environment
/// omits `BEALE_*` host settings.
pub const SUPERVISED_WORKER_ENV: &str = "BEALE_WORKER_SUPERVISED";

#[cfg(target_os = "linux")]
static SUPERVISED_WORKER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Records, once at worker startup, whether this worker runs under the Linux
/// subreaper supervisor. Has no effect on other platforms.
pub fn adopt_worker_supervision() {
    #[cfg(target_os = "linux")]
    if std::env::var_os(SUPERVISED_WORKER_ENV).is_some_and(|value| value == "1") {
        SUPERVISED_WORKER.store(true, std::sync::atomic::Ordering::Release);
    }
}

/// Whether a dedicated worker can end one command's process tree without ending
/// the worker.
///
/// Isolated commands run in their own process group or job, so their
/// descendants are no longer in the worker's group. They still end with the
/// worker only where the worker's supervisor proves every descendant ended:
/// Docker workers through container-stop proof, Linux workers under the
/// subreaper supervisor in [`crate::linux_worker`], which adopts and reaps
/// every orphan before writing termination proof, and Windows workers through
/// the supervisor's job, which contains each command's nested job. Any other
/// worker is supervised by its process group, so its commands stay in that
/// group and a limit ends only the direct child.
pub fn worker_commands_are_isolated() -> bool {
    #[cfg(target_os = "linux")]
    {
        is_container_worker() || SUPERVISED_WORKER.load(std::sync::atomic::Ordering::Acquire)
    }
    #[cfg(not(target_os = "linux"))]
    {
        cfg!(windows)
    }
}

/// Applies the worker command group policy described by
/// [`worker_commands_are_isolated`]. Only the worker entry point uses this.
pub fn retain_worker_group(command: &mut Command) -> std::io::Result<()> {
    require_worker_group()?;
    #[cfg(unix)]
    if !worker_commands_are_isolated() {
        command.process_group(std::process::id() as i32);
    }
    #[cfg(not(unix))]
    let _ = command;
    Ok(())
}

/// A command or interpreter cell stopped at its time or output limit.
///
/// Returned only after the limited execution was ended: its whole process tree
/// where [`worker_commands_are_isolated`] holds, otherwise its direct child.
/// Callers may record this as an ordinary failed result. Any other error after
/// dispatch leaves the outcome uncertain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LimitExceeded {
    pub subject: &'static str,
    pub limit: Limit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    Time(std::time::Duration),
    Output,
}

impl LimitExceeded {
    pub fn timed_out(subject: &'static str, budget: std::time::Duration) -> Self {
        Self {
            subject,
            limit: Limit::Time(budget),
        }
    }

    pub fn output(subject: &'static str) -> Self {
        Self {
            subject,
            limit: Limit::Output,
        }
    }

    /// Model-facing text for a worker result. It names the limit and whether
    /// descendants may still be running; it contains no host paths.
    pub fn worker_message(&self) -> String {
        let remainder = if worker_commands_are_isolated() {
            "Its process tree was terminated."
        } else {
            "Its direct process was terminated; background processes it started may continue until this subagent ends."
        };
        format!("{self}. {remainder}")
    }
}

impl std::fmt::Display for LimitExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.limit {
            Limit::Time(budget) => {
                let seconds = budget.as_millis().div_ceil(1_000);
                let unit = if seconds == 1 { "second" } else { "seconds" };
                write!(f, "{} timed out after {seconds} {unit}", self.subject)
            }
            Limit::Output => write!(f, "{} output is too large", self.subject),
        }
    }
}

impl std::error::Error for LimitExceeded {}

/// Ends execution that reached a [`LimitExceeded`] boundary.
///
/// Host commands keep their previous best-effort cleanup: dropping an armed
/// tree kills it. A worker command must have a successful termination request
/// for its isolated tree, otherwise the caller keeps the outcome uncertain.
pub fn end_limited(tree: Option<ProcessTree>, worker: bool) -> std::io::Result<()> {
    match tree {
        Some(tree) if worker => tree.request_termination(),
        Some(tree) => {
            drop(tree);
            Ok(())
        }
        // Shared-group workers: dropping the kill-on-drop child ends the direct
        // process. Descendants stay in the worker group until the worker ends.
        None if worker && !worker_commands_are_isolated() => Ok(()),
        None if worker => Err(std::io::Error::other(
            "worker command process-tree ownership unavailable",
        )),
        None => Ok(()),
    }
}

/// Finish the dedicated Unix worker's entire managed group, including any
/// background commands. The supervisor observes termination independently.
/// Windows workers rely on the owning supervisor's job-object termination.
pub fn terminate_worker_group() -> std::io::Result<()> {
    require_worker_group()?;
    #[cfg(unix)]
    {
        // SAFETY: require_worker_group proved this process owns the group. This
        // intentionally terminates the worker itself along with its descendants.
        if unsafe { libc::killpg(libc::getpgrp(), libc::SIGKILL) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
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
    /// Requires a process-tree handle before a trusted worker is released from
    /// its startup handshake. The caller must kill the child if this fails.
    /// This tracks managed descendants; it is not an OS isolation boundary.
    pub fn attach_required(child: &Child) -> std::io::Result<Self> {
        let tree = Self::attach(child);
        #[cfg(unix)]
        if tree.group.is_some() {
            return Ok(tree);
        }
        #[cfg(windows)]
        if let Some(job) = &tree.job {
            job.kill_on_close()?;
            return Ok(tree);
        }
        Err(std::io::Error::other(
            "worker process-tree ownership unavailable",
        ))
    }

    /// A fresh host-generated name permits recovery after the supervisor exits.
    /// The caller persists this identity and holds exclusive launch ownership
    /// before starting a process. Never attach to an already existing named job.
    pub fn attach_recoverable(child: &Child, key: uuid::Uuid) -> std::io::Result<Self> {
        #[cfg(windows)]
        {
            let handle = child
                .raw_handle()
                .ok_or_else(|| std::io::Error::other("worker exited before attachment"))?;
            let job = job::Job::assign_named(handle, key)?;
            job.kill_on_close()?;
            Ok(Self {
                job: Some(job),
                armed: true,
            })
        }
        #[cfg(not(windows))]
        {
            let _ = key;
            Self::attach_required(child)
        }
    }

    /// Only call while holding the saved launch's exclusive host lock. An absent
    /// Windows job proves that no associated managed processes remain. Other
    /// backends require their own durable termination evidence.
    pub fn recover_terminated(key: uuid::Uuid) -> std::io::Result<bool> {
        #[cfg(windows)]
        {
            let Some(job) = job::Job::open_named(key)? else {
                return Ok(true);
            };
            job.terminate()?;
            job.is_empty()
        }
        #[cfg(not(windows))]
        {
            let _ = key;
            Ok(false)
        }
    }

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

    /// Releases ordinary attachments after normal root exit. Strict worker
    /// attachments retain Windows kill-on-close cleanup and must instead use
    /// termination confirmation before releasing execution ownership.
    pub fn release(mut self) {
        self.armed = false;
    }

    /// Requests termination without discarding the handle needed to confirm it.
    /// A successful request is not confirmation that descendants have exited.
    pub fn request_termination(&self) -> std::io::Result<()> {
        #[cfg(unix)]
        if let Some(group) = self.group {
            // SAFETY: the group belongs to the child created by contained_command.
            if unsafe { libc::killpg(group, libc::SIGKILL) } == 0 {
                return Ok(());
            }
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(());
            }
            return Err(error);
        }
        #[cfg(windows)]
        if let Some(job) = &self.job {
            return job.terminate();
        }
        Err(std::io::Error::other(
            "worker process-tree ownership unavailable",
        ))
    }

    /// Confirms an empty managed tree, then disarms its cleanup handle. Root
    /// exit alone does not imply that background descendants have ended.
    pub fn confirm_terminated(&mut self) -> std::io::Result<bool> {
        if !self.armed {
            return Ok(true);
        }
        #[cfg(unix)]
        if let Some(group) = self.group {
            // SAFETY: signal zero tests existence without signaling processes.
            if unsafe { libc::killpg(group, 0) } == 0 {
                return Ok(false);
            }
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
            self.group = None;
            self.armed = false;
            return Ok(true);
        }
        #[cfg(windows)]
        if let Some(job) = &self.job {
            if !job.is_empty()? {
                return Ok(false);
            }
            self.job = None;
            self.armed = false;
            return Ok(true);
        }
        Err(std::io::Error::other(
            "worker process-tree ownership unavailable",
        ))
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
            let _ = job.terminate();
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
        Foundation::{
            CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, GetLastError, HANDLE,
        },
        System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation, OpenJobObjectW,
            QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
        },
    };

    use windows_sys::Win32::System::SystemServices::{JOB_OBJECT_QUERY, JOB_OBJECT_TERMINATE};

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

        fn name(key: uuid::Uuid) -> Vec<u16> {
            format!("Global\\BealeWorker-{}", key.simple())
                .encode_utf16()
                .chain(Some(0))
                .collect()
        }

        pub(super) fn assign_named(process: RawHandle, key: uuid::Uuid) -> std::io::Result<Self> {
            let name = Self::name(key);
            // SAFETY: a terminated UTF-16 name and default non-inheritable security.
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), name.as_ptr()) };
            if handle.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: capture the thread-local result before another Win32 call.
            let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
            let job = Self(handle);
            if existed {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "worker job identity already exists",
                ));
            }
            // SAFETY: the process handle is borrowed from a live tokio child.
            if unsafe { AssignProcessToJobObject(job.0, process as HANDLE) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(job)
        }

        pub(super) fn open_named(key: uuid::Uuid) -> std::io::Result<Option<Self>> {
            let name = Self::name(key);
            // SAFETY: valid terminated name; the returned handle is owned here.
            let handle = unsafe {
                OpenJobObjectW(JOB_OBJECT_QUERY | JOB_OBJECT_TERMINATE, 0, name.as_ptr())
            };
            if !handle.is_null() {
                return Ok(Some(Self(handle)));
            }
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) {
                return Ok(None);
            }
            Err(error)
        }

        pub(super) fn terminate(&self) -> std::io::Result<()> {
            // SAFETY: the handle is a valid job object owned by this value.
            if unsafe { TerminateJobObject(self.0, 1) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        }

        pub(super) fn kill_on_close(&self) -> std::io::Result<()> {
            // SAFETY: these plain Windows API structs accept all-zero initialization.
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: the owned handle and sized struct pointer are valid for the call.
            if unsafe {
                SetInformationJobObject(
                    self.0,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of_val(&limits) as u32,
                )
            } == 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        }

        pub(super) fn is_empty(&self) -> std::io::Result<bool> {
            // SAFETY: the API writes this plain output struct after validating its size.
            let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION =
                unsafe { std::mem::zeroed() };
            // SAFETY: output points to writable storage of the supplied size; the handle is owned.
            if unsafe {
                QueryInformationJobObject(
                    self.0,
                    JobObjectBasicAccountingInformation,
                    (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                    std::mem::size_of_val(&accounting) as u32,
                    std::ptr::null_mut(),
                )
            } == 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(accounting.ActiveProcesses == 0)
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

    #[test]
    fn missing_process_tree_handle_cannot_confirm_termination() {
        let mut tree = ProcessTree {
            #[cfg(unix)]
            group: None,
            #[cfg(windows)]
            job: None,
            armed: true,
        };
        assert!(tree.request_termination().is_err());
        assert!(tree.confirm_terminated().is_err());
    }

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
