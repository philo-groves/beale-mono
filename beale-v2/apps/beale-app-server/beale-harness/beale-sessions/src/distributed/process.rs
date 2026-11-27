//! Trusted worker startup and managed process-tree termination. The worker
//! entry point must wait for its stdin bootstrap before starting execution.
use anyhow::{Context, Result, ensure};
use beale_contracts::swarms::MAX_DOCUMENT_BYTES;
use beale_tools::process::{ProcessTree, contained_command};
use std::{ffi::OsString, path::Path, process::Stdio, time::Duration};
use tokio::{io::AsyncWriteExt, process::Child, time::Instant};

pub type ProcessFuture<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T>> + Send + 'a>>;

/// Backend-owned execution. A guest launcher exiting is not proof that guest
/// descendants stopped: each backend must implement its own termination proof.
/// Dropping an owner must request termination without asserting confirmation.
pub trait ManagedWorker: Send {
    fn release_startup<'a>(&'a mut self, bootstrap: &'a [u8]) -> ProcessFuture<'a, ()>;
    fn root_exited(&mut self) -> Result<bool>;
    fn request_stop(&mut self);
    fn stop(&mut self, timeout: Duration) -> ProcessFuture<'_, bool>;
}

impl ManagedWorker for WorkerProcess {
    fn release_startup<'a>(&'a mut self, bootstrap: &'a [u8]) -> ProcessFuture<'a, ()> {
        Box::pin(WorkerProcess::release_startup(self, bootstrap))
    }
    fn root_exited(&mut self) -> Result<bool> {
        WorkerProcess::root_exited(self)
    }
    fn request_stop(&mut self) {
        WorkerProcess::request_stop(self);
    }
    fn stop(&mut self, timeout: Duration) -> ProcessFuture<'_, bool> {
        Box::pin(WorkerProcess::stop(self, timeout))
    }
}

/// Owns a worker until its entire managed tree is confirmed stopped. Root exit,
/// bootstrap failure, and a kill request alone cannot release this ownership.
pub struct WorkerProcess {
    child: Child,
    tree: Option<ProcessTree>,
    #[cfg(target_os = "linux")]
    guardian: Option<beale_tools::linux_worker::LinuxWorker>,
    released: bool,
    stopping: bool,
    stopped: bool,
}

impl WorkerProcess {
    /// Program, arguments and working directory are host-owned launch data,
    /// never model tool arguments. Secrets belong in the scoped stdin bootstrap.
    pub async fn spawn(program: &Path, arguments: &[OsString], directory: &Path) -> Result<Self> {
        Self::spawn_inner(program, arguments, directory, None).await
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) async fn spawn_recoverable(
        program: &Path,
        arguments: &[OsString],
        directory: &Path,
        key: uuid::Uuid,
    ) -> Result<Self> {
        Self::spawn_inner(program, arguments, directory, Some(key)).await
    }

    #[cfg(target_os = "linux")]
    pub(super) fn spawn_guarded(guardian: beale_tools::linux_worker::LinuxWorker) -> Result<Self> {
        let mut command = contained_command(std::env::current_exe()?);
        // The helper survives host loss to reap descendants and persist proof.
        // Stop requests go through its durable record, never SIGKILL the helper.
        command.kill_on_drop(false);
        #[cfg(not(test))]
        command.arg("worker-supervisor").arg(guardian.directory());
        #[cfg(test)]
        command
            .args([
                "--ignored",
                "--exact",
                "distributed::process::tests::linux_supervisor_fixture",
            ])
            .env("BEALE_TEST_SUPERVISOR", guardian.directory());
        let child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Self {
            child,
            tree: None,
            guardian: Some(guardian),
            released: false,
            stopping: false,
            stopped: false,
        })
    }

    async fn spawn_inner(
        program: &Path,
        arguments: &[OsString],
        directory: &Path,
        key: Option<uuid::Uuid>,
    ) -> Result<Self> {
        let mut child = contained_command(program)
            .args(arguments)
            .current_dir(directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("start assigned worker")?;
        let tree = match key.map_or_else(
            || ProcessTree::attach_required(&child),
            |key| ProcessTree::attach_recoverable(&child, key),
        ) {
            Ok(tree) => tree,
            Err(error) => {
                // The trusted startup handshake has not been released. It
                // cannot have dispatched work or started target descendants.
                let _ = child.start_kill();
                let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
                return Err(error).context("own assigned worker process tree");
            }
        };
        Ok(Self {
            child,
            tree: Some(tree),
            #[cfg(target_os = "linux")]
            guardian: None,
            released: false,
            stopping: false,
            stopped: false,
        })
    }

    /// Releasing stdin happens only after process-tree ownership was acquired.
    /// A partial write is ambiguous and fences the worker; never resend it.
    pub async fn release_startup(&mut self, bootstrap: &[u8]) -> Result<()> {
        ensure!(
            !self.released && !self.stopping,
            "worker startup already released or stopped"
        );
        ensure!(
            !bootstrap.is_empty() && bootstrap.len() <= MAX_DOCUMENT_BYTES,
            "invalid worker bootstrap size"
        );
        self.released = true;
        let mut stdin = self
            .child
            .stdin
            .take()
            .context("worker startup input unavailable")?;
        let mut guard = StartupGuard {
            process: self,
            committed: false,
        };
        let result = tokio::time::timeout(Duration::from_secs(5), async {
            stdin.write_all(bootstrap).await?;
            stdin.flush().await?;
            stdin.shutdown().await
        })
        .await;
        // Closing stdin terminates the bounded bootstrap document.
        drop(stdin);
        match result {
            Ok(Ok(())) => {
                guard.committed = true;
                Ok(())
            }
            _ => {
                anyhow::bail!("worker startup delivery was not confirmed")
            }
        }
    }

    /// An exit is informational, never proof that all managed execution ended.
    pub fn root_exited(&mut self) -> Result<bool> {
        Ok(self.child.try_wait()?.is_some())
    }

    pub fn request_stop(&mut self) {
        self.stopping = true;
        self.child.stdin.take();
        if !self.stopped {
            #[cfg(target_os = "linux")]
            if let Some(guardian) = &self.guardian {
                let _ = guardian.request_stop();
                return;
            }
            // Failures remain termination unconfirmed. A later observation may
            // establish completion, otherwise replacement must remain blocked.
            if let Some(tree) = &self.tree {
                let _ = tree.request_termination();
            }
            let _ = self.child.start_kill();
        }
    }

    pub fn confirm_stopped(&mut self) -> Result<bool> {
        ensure!(
            self.stopping,
            "worker must be fenced before termination confirmation"
        );
        if !self.stopped && self.root_exited()? {
            #[cfg(target_os = "linux")]
            if let Some(guardian) = &self.guardian {
                self.stopped = guardian.recover_terminated()?;
                return Ok(self.stopped);
            }
            self.stopped = self
                .tree
                .as_mut()
                .context("missing process ownership")?
                .confirm_terminated()?;
        }
        Ok(self.stopped)
    }

    /// A bounded wait returning false keeps ownership and does not authorize a
    /// replacement. Callers can retry confirmation after backend reconciliation.
    pub async fn stop(&mut self, timeout: Duration) -> Result<bool> {
        ensure!(
            timeout <= Duration::from_secs(30),
            "termination wait exceeds limit"
        );
        self.request_stop();
        let deadline = Instant::now() + timeout;
        loop {
            if self.confirm_stopped()? {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep_until((Instant::now() + Duration::from_millis(10)).min(deadline))
                .await;
        }
    }
}

// Also fences on cancellation of the release_startup future after a partial
// write. The worker may see EOF when the future is dropped.
struct StartupGuard<'a> {
    process: &'a mut WorkerProcess,
    committed: bool,
}
impl Drop for StartupGuard<'_> {
    fn drop(&mut self) {
        if !self.committed {
            self.process.request_stop();
        }
    }
}

impl Drop for WorkerProcess {
    fn drop(&mut self) {
        self.request_stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Read;

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "dedicated Linux supervisor fixture invoked by parent tests"]
    fn linux_supervisor_fixture() {
        beale_tools::linux_worker::run(&std::path::PathBuf::from(
            std::env::var_os("BEALE_TEST_SUPERVISOR").unwrap(),
        ))
        .unwrap();
    }

    fn arguments() -> Vec<OsString> {
        [
            "--ignored",
            "--exact",
            "distributed::process::tests::worker_process_fixture",
            "--nocapture",
        ]
        .into_iter()
        .map(OsString::from)
        .collect()
    }

    // Executed only in a child test process. It never initializes host storage
    // or provider configuration and waits for stdin before spawning descendants.
    #[test]
    #[ignore = "subprocess fixture invoked by worker process tests"]
    fn worker_process_fixture() {
        if std::env::var_os("BEALE_EXAMPLE_CHILD").is_some() {
            loop {
                std::thread::park_timeout(Duration::from_secs(60));
            }
        }
        let mut input = String::new();
        std::io::stdin()
            .take(MAX_DOCUMENT_BYTES as u64 + 1)
            .read_to_string(&mut input)
            .unwrap();
        let config: serde_json::Value = serde_json::from_str(&input).unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(arguments())
            .env("BEALE_EXAMPLE_CHILD", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        std::fs::write(config["ready"].as_str().unwrap(), b"ready").unwrap();
        if config["root_exit"].as_bool().unwrap() {
            // Deliberately leave a managed descendant after normal root exit.
            drop(child);
        } else {
            let _ = child.wait();
        }
    }

    async fn fixture(root_exit: bool) {
        let root = std::env::temp_dir().join(format!(
            "beale-worker-process-example-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        let marker = root.join("ready");
        let mut process =
            WorkerProcess::spawn(&std::env::current_exe().unwrap(), &arguments(), &root)
                .await
                .unwrap();
        assert!(!process.root_exited().unwrap());
        assert!(
            !marker.exists(),
            "no execution before process-tree ownership and bootstrap"
        );
        assert!(process.confirm_stopped().is_err());
        let bootstrap = serde_json::to_vec(&json!({"ready":marker,"root_exit":root_exit})).unwrap();
        process.release_startup(&bootstrap).await.unwrap();
        assert!(process.release_startup(&bootstrap).await.is_err());
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if marker.exists() && (!root_exit || process.root_exited().unwrap()) {
                break;
            }
            assert!(Instant::now() < deadline, "worker did not start");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            !process.tree.as_mut().unwrap().confirm_terminated().unwrap(),
            "live descendant must retain ownership even after root exit"
        );
        assert!(process.stop(Duration::from_secs(5)).await.unwrap());
        assert!(process.confirm_stopped().unwrap());
        assert!(process.release_startup(&bootstrap).await.is_err());
        drop(process);
        // Windows may finish releasing a terminated process's current-directory
        // handle just after the job reports zero active processes.
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match std::fs::remove_dir_all(&root) {
                Ok(()) => break,
                Err(error)
                    if cfg!(windows)
                        && error.raw_os_error() == Some(32)
                        && Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(error) => panic!("remove terminated worker fixture: {error}"),
            }
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "subprocess fixture for abrupt supervisor exit"]
    async fn recoverable_supervisor_fixture() {
        let root =
            std::path::PathBuf::from(std::env::var_os("BEALE_EXAMPLE_RECOVERY_ROOT").unwrap());
        let key = uuid::Uuid::parse_str(&std::fs::read_to_string(root.join("identity")).unwrap())
            .unwrap();
        let mut process = WorkerProcess::spawn_recoverable(
            &std::env::current_exe().unwrap(),
            &arguments(),
            &root,
            key,
        )
        .await
        .unwrap();
        process
            .release_startup(
                &serde_json::to_vec(&json!({"ready":root.join("ready"),"root_exit":false}))
                    .unwrap(),
            )
            .await
            .unwrap();
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn named_job_recovers_managed_tree_after_abrupt_supervisor_exit() {
        let root =
            std::env::temp_dir().join(format!("beale-restart-example-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let key = uuid::Uuid::new_v4();
        std::fs::write(root.join("identity"), key.to_string()).unwrap();
        let mut host = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "distributed::process::tests::recoverable_supervisor_fixture",
            ])
            .env("BEALE_EXAMPLE_RECOVERY_ROOT", &root)
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !root.join("ready").exists() {
            assert!(
                host.try_wait().unwrap().is_none(),
                "supervisor exited before worker startup"
            );
            assert!(Instant::now() < deadline, "worker did not start");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // A duplicate identity must fail without attaching to or terminating the
        // original job. The host-side launch lock prevents this in production.
        assert!(
            WorkerProcess::spawn_recoverable(
                &std::env::current_exe().unwrap(),
                &arguments(),
                &root,
                key
            )
            .await
            .is_err()
        );
        assert!(host.try_wait().unwrap().is_none());
        host.kill().await.unwrap();
        host.wait().await.unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ProcessTree::recover_terminated(key).unwrap() {
            assert!(Instant::now() < deadline, "managed tree did not terminate");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(ProcessTree::recover_terminated(key).unwrap());
        // OS process teardown may release current-directory handles just after
        // the job becomes empty; wait only for this synthetic fixture directory.
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match std::fs::remove_dir_all(&root) {
                Ok(()) => break,
                Err(error) if error.raw_os_error() == Some(32) && Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(10)).await
                }
                Err(error) => panic!("remove restart fixture: {error}"),
            }
        }
    }

    #[tokio::test]
    async fn worker_stop_confirms_the_managed_process_tree() {
        fixture(false).await;
    }

    #[tokio::test]
    async fn worker_root_exit_does_not_confirm_background_descendant_termination() {
        fixture(true).await;
    }

    #[tokio::test]
    async fn worker_stopped_before_bootstrap_cannot_start_execution() {
        let root = std::env::temp_dir();
        let mut process =
            WorkerProcess::spawn(&std::env::current_exe().unwrap(), &arguments(), &root)
                .await
                .unwrap();
        assert!(process.stop(Duration::from_secs(5)).await.unwrap());
        assert!(process.release_startup(b"{}").await.is_err());
    }

    #[test]
    #[ignore = "subprocess fixture for cancellation during startup delivery"]
    fn worker_process_blocked_startup_fixture() {
        loop {
            std::thread::park_timeout(Duration::from_secs(60));
        }
    }

    #[tokio::test]
    async fn canceling_partial_startup_delivery_fences_and_terminates_the_worker() {
        let args: Vec<OsString> = [
            "--ignored",
            "--exact",
            "distributed::process::tests::worker_process_blocked_startup_fixture",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        let mut process = WorkerProcess::spawn(
            &std::env::current_exe().unwrap(),
            &args,
            &std::env::temp_dir(),
        )
        .await
        .unwrap();
        let bootstrap = vec![b'x'; MAX_DOCUMENT_BYTES];
        assert!(
            tokio::time::timeout(
                Duration::from_millis(50),
                process.release_startup(&bootstrap)
            )
            .await
            .is_err()
        );
        assert!(process.stopping);
        assert!(process.stop(Duration::from_secs(5)).await.unwrap());
        assert!(process.release_startup(b"{}").await.is_err());
    }
}
