//! Dedicated Linux child subreaper for one assigned execution. This helper must
//! run in its own process, never inside the app-server. Durable proof comes from
//! reaping all children, not from testing a saved PID after host restart.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions, TryLockError},
    io::{Read, Write},
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

const LIMIT: usize = 1_048_576;
const TICK: Duration = Duration::from_millis(20);

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Launch {
    version: u32,
    key: Uuid,
    parent: u32,
    program: PathBuf,
    arguments: Vec<OsString>,
    directory: PathBuf,
}

/// Host-only paths and launch metadata; never sent through the worker router.
pub struct LinuxWorker {
    directory: PathBuf,
    key: Uuid,
}

impl LinuxWorker {
    /// Called before worker preparation, while holding the durable attempt lock.
    /// A key is single-use, including when startup has not been released.
    pub fn prepare(
        root: &Path,
        key: Uuid,
        program: &Path,
        arguments: &[OsString],
        directory: &Path,
    ) -> Result<Self> {
        fs::create_dir_all(root)?;
        ensure!(
            fs::symlink_metadata(root)?.file_type().is_dir(),
            "invalid supervisor root"
        );
        let result = Self {
            directory: root.join(key.to_string()),
            key,
        };
        fs::create_dir(&result.directory).context("supervisor identity already allocated")?;
        let config = Launch {
            version: 1,
            key,
            parent: std::process::id(),
            program: program.to_path_buf(),
            arguments: arguments.to_vec(),
            directory: directory.to_path_buf(),
        };
        ensure!(
            program.is_absolute() && directory.is_absolute(),
            "absolute launch paths required"
        );
        let bytes = serde_json::to_vec(&config)?;
        ensure!(bytes.len() <= LIMIT, "supervisor launch metadata too large");
        result.write_atomic("launch.json", &bytes)?;
        File::open(root)?.sync_all()?;
        Ok(result)
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn open(root: &Path, key: Uuid) -> Result<Self> {
        let result = Self {
            directory: root.join(key.to_string()),
            key,
        };
        result.validate_directory()?;
        Ok(result)
    }

    fn validate_directory(&self) -> Result<()> {
        ensure!(
            fs::symlink_metadata(&self.directory)?.file_type().is_dir(),
            "invalid supervisor directory"
        );
        Ok(())
    }

    fn lock(&self) -> Result<Option<File>> {
        self.validate_directory()?;
        let path = self.directory.join("owner.lock");
        if path.try_exists()? {
            ensure!(
                fs::symlink_metadata(&path)?.file_type().is_file(),
                "invalid supervisor lock"
            );
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(file)),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(error.into()),
        }
    }

    fn read(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let path = self.directory.join(name);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        ensure!(
            metadata.file_type().is_file() && metadata.len() <= LIMIT as u64,
            "invalid supervisor record"
        );
        let mut bytes = Vec::new();
        File::open(path)?
            .take(LIMIT as u64 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= LIMIT, "supervisor record too large");
        Ok(Some(bytes))
    }

    fn marked(&self, name: &str) -> Result<bool> {
        let Some(bytes) = self.read(name)? else {
            return Ok(false);
        };
        ensure!(
            bytes == self.key.to_string().as_bytes(),
            "supervisor proof identity mismatch"
        );
        Ok(true)
    }

    fn write_atomic(&self, name: &str, bytes: &[u8]) -> Result<()> {
        self.validate_directory()?;
        let temporary = self.directory.join(format!(".{}.tmp", Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, self.directory.join(name))?;
            File::open(&self.directory)?.sync_all()?;
            Ok(())
        })();
        let _ = fs::remove_file(temporary);
        result
    }

    fn mark(&self, name: &str) -> Result<()> {
        self.write_atomic(name, self.key.to_string().as_bytes())
    }

    pub fn request_stop(&self) -> Result<()> {
        self.mark("stop")
    }

    /// Must be called under the attempt's host lock. A delayed helper cannot
    /// launch after recovery publishes the stopped tombstone under its own lock.
    /// Missing records and dead helpers with started work never prove termination.
    pub fn recover_terminated(&self) -> Result<bool> {
        self.request_stop()?;
        let Some(_owner) = self.lock()? else {
            return Ok(false);
        };
        if self.marked("stopped")? {
            return Ok(true);
        }
        if self.marked("started")? {
            return Ok(false);
        }
        // Preparation alone does not start the target. The helper takes this
        // same lock and checks the tombstone before creating any process.
        self.mark("stopped")?;
        Ok(true)
    }
}

/// Entry point for a dedicated helper process. Only stdin carries the scoped
/// bootstrap; launch.json contains host-only paths and no provider credentials.
pub fn run(directory: &Path) -> Result<()> {
    let key = directory
        .file_name()
        .and_then(|s| s.to_str())
        .context("missing supervisor identity")?
        .parse()?;
    let worker = LinuxWorker {
        directory: directory.to_path_buf(),
        key,
    };
    let _owner = worker.lock()?.context("supervisor already owned")?;
    if worker.marked("stopped")? {
        return Ok(());
    }
    ensure!(
        !worker.marked("started")?,
        "supervisor cannot restart an execution"
    );
    let launch: Launch = serde_json::from_slice(
        &worker
            .read("launch.json")?
            .context("missing supervisor launch")?,
    )?;
    ensure!(
        launch.version == 1
            && launch.key == key
            && launch.parent > 0
            && launch.program.is_absolute()
            && launch.directory.is_absolute(),
        "invalid supervisor launch"
    );
    // Inherited SIG_IGN/SA_NOCLDWAIT would auto-reap the root and invalidate our
    // PID lifetime invariant. This process has no unrelated child consumers.
    // SAFETY: sigaction receives valid initialized storage and an empty mask.
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = libc::SIG_DFL;
    ensure!(
        unsafe { libc::sigemptyset(&mut action.sa_mask) } == 0,
        "cannot initialize child signal mask"
    );
    ensure!(
        unsafe { libc::sigaction(libc::SIGCHLD, &action, std::ptr::null_mut()) } == 0,
        "cannot retain waitable children"
    );
    // SAFETY: only this dedicated helper adopts children. No app-server or
    // provider processes are created here; all children belong to this attempt.
    ensure!(
        unsafe {
            libc::prctl(
                libc::PR_SET_CHILD_SUBREAPER,
                1 as libc::c_ulong,
                0 as libc::c_ulong,
                0 as libc::c_ulong,
                0 as libc::c_ulong,
            )
        } == 0,
        "cannot own orphan descendants"
    );
    let stop = || -> Result<bool> {
        // SAFETY: getppid only reads this helper's current parent identity.
        Ok(unsafe { libc::getppid() } as u32 != launch.parent || worker.marked("stop")?)
    };
    let startup = read_startup(&stop);
    let mut outcome = Ok(());
    if let Ok(Some(bytes)) = startup {
        if !stop()? {
            // Persist before spawn. A crash after this point is conservative:
            // only the surviving helper can supply proof by reaping children.
            worker.mark("started")?;
            let mut command = Command::new(&launch.program);
            command
                .args(&launch.arguments)
                .current_dir(&launch.directory)
                .env_clear()
                .envs(crate::process::tool_environment())
                // This helper reaps every descendant, so the worker may give
                // each command its own process group.
                .env(crate::process::SUPERVISED_WORKER_ENV, "1")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0);
            match command.spawn() {
                Ok(mut child) => {
                    // std::process::Child never reaps on Drop. This helper is
                    // the sole waiter; root PID stays reserved until cleanup.
                    let pid = i32::try_from(child.id())?;
                    outcome = (|| -> Result<()> {
                        let mut stdin = child.stdin.take().context("missing worker stdin")?;
                        write_startup(&mut stdin, &bytes, &stop)?;
                        drop(stdin);
                        while !stop()? && !exited_without_reaping(pid)? {
                            thread::sleep(TICK);
                        }
                        Ok(())
                    })();
                    // Always clean up even when startup, stop observation, or
                    // waitid failed. No termination proof is written on error.
                    cleanup(pid)?;
                }
                Err(error) => outcome = Err(error.into()),
            }
        }
    } else if let Err(error) = startup {
        outcome = Err(error);
    }
    // Keep ownership and retry a failed proof write after storage recovers.
    // No target processes remain; loss of this helper still retains uncertainty.
    while worker.mark("stopped").is_err() {
        thread::sleep(Duration::from_millis(100));
    }
    outcome
}

fn nonblocking(fd: i32) -> Result<()> {
    // SAFETY: fd belongs to a live stdin/pipe; only this helper uses it.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    ensure!(
        flags >= 0 && unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0,
        "cannot configure startup pipe"
    );
    Ok(())
}

fn read_startup(stop: &impl Fn() -> Result<bool>) -> Result<Option<Vec<u8>>> {
    let mut stdin = std::io::stdin();
    nonblocking(stdin.as_raw_fd())?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut bytes = Vec::new();
    let mut chunk = [0; 8192];
    loop {
        if stop()? {
            return Ok(None);
        }
        ensure!(Instant::now() < deadline, "worker startup expired");
        match stdin.read(&mut chunk) {
            Ok(0) => {
                ensure!(!bytes.is_empty(), "empty worker startup");
                return Ok(Some(bytes));
            }
            Ok(n) => {
                bytes.extend_from_slice(&chunk[..n]);
                ensure!(bytes.len() <= LIMIT, "worker startup too large");
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(TICK),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => (),
            Err(error) => return Err(error.into()),
        }
    }
}

fn write_startup(
    pipe: &mut std::process::ChildStdin,
    bytes: &[u8],
    stop: &impl Fn() -> Result<bool>,
) -> Result<()> {
    nonblocking(pipe.as_raw_fd())?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut offset = 0;
    while offset < bytes.len() {
        ensure!(
            !stop()? && Instant::now() < deadline,
            "worker startup interrupted"
        );
        match pipe.write(&bytes[offset..]) {
            Ok(0) => anyhow::bail!("worker startup pipe closed"),
            Ok(n) => offset += n,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(TICK),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => (),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn exited_without_reaping(pid: i32) -> Result<bool> {
    // SAFETY: zero-initialized siginfo is valid output storage. WNOWAIT leaves
    // the root waitable, preventing reuse of its PID/process-group identity.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as u32,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: waitid initialized the SIGCHLD fields (or left zero on WNOHANG).
    Ok(unsafe { info.si_pid() } != 0)
}

fn cleanup(root: i32) -> Result<()> {
    // SAFETY: the root has never been reaped, so its group ID cannot be reused.
    let result = unsafe { libc::killpg(root, libc::SIGKILL) };
    if result < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
        return Err(std::io::Error::last_os_error().into());
    }
    // The worker may have launched a new session/group. Kill adopted direct
    // children too; each remains our child until this single waiter reaps it.
    // Repeat after reparenting until ECHILD proves the whole owned tree ended.
    loop {
        // Adoption may select the thread-group leader even when the worker was
        // spawned from a test/runtime thread. waitpid below owns all threads'
        // children; enumerate the same scope before reaping any of them.
        let mut children = String::new();
        for task in fs::read_dir("/proc/self/task")? {
            match fs::read_to_string(task?.path().join("children")) {
                Ok(list) => {
                    children.push_str(&list);
                    children.push(' ');
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
        }
        for child in children.split_whitespace() {
            let pid: i32 = child.parse()?;
            ensure!(pid > 0, "invalid owned child identity");
            // SAFETY: the kernel enumerated an unreaped child of this thread;
            // no other thread in this dedicated helper waits on children.
            if unsafe { libc::kill(pid, libc::SIGKILL) } < 0
                && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
            {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        loop {
            // SAFETY: reaps only this helper's owned/adopted children.
            let result = unsafe { libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) };
            if result == 0 {
                break;
            }
            if result < 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ECHILD) {
                    return Ok(());
                }
                if error.kind() != std::io::ErrorKind::Interrupted {
                    return Err(error.into());
                }
            }
        }
        thread::sleep(TICK);
    }
}

#[cfg(test)]
mod tests;
