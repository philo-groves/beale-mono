use super::*;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("beale-linux-supervisor-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn prepare(&self, script: &str) -> LinuxWorker {
        LinuxWorker::prepare(
            &self.0.join("owners"),
            Uuid::new_v4(),
            Path::new("/bin/sh"),
            &["-c".into(), script.into()],
            &self.0,
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn spawn(worker: &LinuxWorker) -> std::process::Child {
    Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "linux_worker::tests::helper_fixture",
        ])
        .env("BEALE_TEST_SUPERVISOR", worker.directory())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn release(child: &mut std::process::Child) {
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"synthetic startup")
        .unwrap();
}

fn wait_for(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(Instant::now() < deadline, "supervisor acceptance timed out");
        thread::sleep(TICK);
    }
}

fn gone(pid: &str) -> bool {
    // Read-only existence check. Never signal a saved PID from test/recovery.
    !Path::new("/proc").join(pid.trim()).exists()
}

#[test]
#[ignore = "subprocess fixture invoked by native supervisor acceptance"]
fn helper_fixture() {
    run(&PathBuf::from(
        std::env::var_os("BEALE_TEST_SUPERVISOR").unwrap(),
    ))
    .unwrap();
}

#[test]
fn normal_root_exit_reaps_background_and_new_session_descendants() {
    let f = Fixture::new();
    let worker = f.prepare("cat >/dev/null; echo $$ > root.pid; setsid /bin/sh -c 'echo $$ > detached.pid; sleep 60' & sleep 0.2");
    let mut helper = spawn(&worker);
    release(&mut helper);
    wait_for(|| f.0.join("detached.pid").exists());
    wait_for(|| helper.try_wait().unwrap().is_some());
    assert!(worker.recover_terminated().unwrap());
    assert!(gone(&fs::read_to_string(f.0.join("root.pid")).unwrap()));
    assert!(gone(&fs::read_to_string(f.0.join("detached.pid")).unwrap()));
    assert!(worker.recover_terminated().unwrap());
}

#[test]
fn cancellation_before_bootstrap_and_recovery_tombstone_prevent_late_launch() {
    let f = Fixture::new();
    let worker = f.prepare("cat >/dev/null; echo started > ran");
    let mut helper = spawn(&worker);
    worker.request_stop().unwrap();
    drop(helper.stdin.take());
    wait_for(|| helper.try_wait().unwrap().is_some());
    assert!(worker.recover_terminated().unwrap());
    assert!(!f.0.join("ran").exists());
    let unlaunched = f.prepare("cat >/dev/null; echo started > late");
    assert!(unlaunched.recover_terminated().unwrap());
    let mut late = spawn(&unlaunched);
    drop(late.stdin.take());
    wait_for(|| late.try_wait().unwrap().is_some());
    assert!(!f.0.join("late").exists());
    assert!(
        LinuxWorker::prepare(
            &f.0.join("owners"),
            unlaunched.key,
            Path::new("/bin/sh"),
            &[],
            &f.0
        )
        .is_err()
    );
}

#[test]
fn stop_request_reaps_live_descendants_but_corrupt_proof_is_rejected() {
    let f = Fixture::new();
    let worker =
        f.prepare("cat >/dev/null; echo $$ > root.pid; sleep 60 & echo $! > child.pid; wait");
    let mut helper = spawn(&worker);
    release(&mut helper);
    wait_for(|| f.0.join("child.pid").exists());
    assert!(!worker.recover_terminated().unwrap());
    wait_for(|| helper.try_wait().unwrap().is_some());
    assert!(worker.recover_terminated().unwrap());
    assert!(gone(&fs::read_to_string(f.0.join("child.pid")).unwrap()));
    fs::write(worker.directory().join("stopped"), b"wrong-owner").unwrap();
    assert!(worker.recover_terminated().is_err());
    let uncertain = f.prepare("exit 0");
    uncertain.mark("started").unwrap();
    assert!(!uncertain.recover_terminated().unwrap());
    assert!(!uncertain.directory().join("stopped").exists());
}

#[test]
#[ignore = "host-loss subprocess fixture invoked by native supervisor acceptance"]
fn host_fixture() {
    let root = PathBuf::from(std::env::var_os("BEALE_TEST_HOST_ROOT").unwrap());
    let key: Uuid = std::env::var("BEALE_TEST_HOST_KEY")
        .unwrap()
        .parse()
        .unwrap();
    let worker = LinuxWorker::prepare(&root.join("owners"), key, Path::new("/bin/sh"),
        &["-c".into(), "cat >/dev/null; echo $$ > root.pid; setsid /bin/sh -c 'echo $$ > detached.pid; sleep 60' & wait".into()], &root).unwrap();
    let mut helper = spawn(&worker);
    release(&mut helper);
    wait_for(|| root.join("detached.pid").exists());
    fs::write(root.join("ready"), b"ready").unwrap();
    loop {
        thread::sleep(Duration::from_secs(1));
    }
}

#[test]
fn abrupt_host_exit_leaves_helper_to_reap_and_publish_durable_proof() {
    let f = Fixture::new();
    let key = Uuid::new_v4();
    let mut host = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "linux_worker::tests::host_fixture"])
        .env("BEALE_TEST_HOST_ROOT", &f.0)
        .env("BEALE_TEST_HOST_KEY", key.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for(|| f.0.join("ready").exists());
    host.kill().unwrap();
    host.wait().unwrap();
    let worker = LinuxWorker::open(&f.0.join("owners"), key).unwrap();
    wait_for(|| worker.recover_terminated().unwrap());
    assert!(gone(&fs::read_to_string(f.0.join("root.pid")).unwrap()));
    assert!(gone(&fs::read_to_string(f.0.join("detached.pid")).unwrap()));
}

#[test]
fn helper_loss_never_confuses_root_absence_with_termination_proof() {
    let f = Fixture::new();
    let worker = f.prepare("cat >/dev/null; echo ready > ready; sleep 1");
    let mut helper = spawn(&worker);
    release(&mut helper);
    wait_for(|| f.0.join("ready").exists());
    helper.kill().unwrap();
    helper.wait().unwrap();
    assert!(!worker.recover_terminated().unwrap());
    // The bounded synthetic target finishes by itself. Recovery must still not
    // infer a complete managed tree from a missing root or an unlocked owner.
    thread::sleep(Duration::from_millis(1200));
    assert!(!worker.recover_terminated().unwrap());
    assert!(!worker.directory().join("stopped").exists());
}

#[test]
fn proof_storage_failure_retains_owner_until_publication_can_be_retried() {
    let f = Fixture::new();
    let worker = f.prepare("cat >/dev/null; echo $$ > root.pid; sleep 60");
    let mut helper = spawn(&worker);
    release(&mut helper);
    wait_for(|| f.0.join("root.pid").exists());
    // Renaming a file over a directory fails without replacing the directory.
    fs::create_dir(worker.directory().join("stopped")).unwrap();
    worker.request_stop().unwrap();
    wait_for(|| gone(&fs::read_to_string(f.0.join("root.pid")).unwrap()));
    assert!(helper.try_wait().unwrap().is_none());
    assert!(!worker.recover_terminated().unwrap());
    fs::remove_dir(worker.directory().join("stopped")).unwrap();
    wait_for(|| helper.try_wait().unwrap().is_some());
    assert!(worker.recover_terminated().unwrap());
}

#[test]
fn cancellation_interrupts_a_full_worker_startup_pipe() {
    let f = Fixture::new();
    let worker = f.prepare("echo $$ > root.pid; sleep 60");
    let mut helper = spawn(&worker);
    helper
        .stdin
        .take()
        .unwrap()
        .write_all(&vec![b'x'; 256 * 1024])
        .unwrap();
    wait_for(|| f.0.join("root.pid").exists());
    worker.request_stop().unwrap();
    wait_for(|| helper.try_wait().unwrap().is_some());
    assert!(worker.recover_terminated().unwrap());
    assert!(gone(&fs::read_to_string(f.0.join("root.pid")).unwrap()));
}

#[test]
fn inherited_sigchld_ignore_does_not_discard_owned_children() {
    let f = Fixture::new();
    let worker = f.prepare("cat >/dev/null; sleep 60 & echo $! > child.pid; sleep 0.2");
    let mut helper = Command::new("/bin/sh")
        .args(["-c", "trap '' CHLD; exec \"$@\"", "supervisor-example"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "linux_worker::tests::helper_fixture",
        ])
        .env("BEALE_TEST_SUPERVISOR", worker.directory())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    release(&mut helper);
    wait_for(|| f.0.join("child.pid").exists());
    wait_for(|| helper.try_wait().unwrap().is_some());
    assert!(worker.recover_terminated().unwrap());
    assert!(gone(&fs::read_to_string(f.0.join("child.pid")).unwrap()));
}
