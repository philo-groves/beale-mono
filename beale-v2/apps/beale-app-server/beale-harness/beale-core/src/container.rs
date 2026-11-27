//! Portable worker entry modes. Run before any host service or credential store.
use anyhow::{Result, ensure};
use beale_contracts::swarm_container::*;
use std::{io::Write, process::Stdio, time::Duration};

pub async fn idle() -> Result<()> {
    ensure!(
        std::env::args_os().count() == 2,
        "container idle takes no arguments"
    );
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut children = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::child())?;
    // Signals can coalesce. Periodic bounded drains also handle a large batch
    // without allowing constant child exits to starve shutdown signals.
    let mut sweep = tokio::time::interval(Duration::from_millis(250));
    loop {
        tokio::select! {
            biased;
            _ = term.recv() => return Ok(()),
            _ = interrupt.recv() => return Ok(()),
            _ = children.recv() => reap_children()?,
            _ = sweep.tick() => reap_children()?,
        }
    }
}
fn reap_children() -> std::io::Result<()> {
    for _ in 0..256 {
        // SAFETY: this dedicated init owns no Tokio Child handles. waitpid
        // observes only this process's children; WNOHANG never waits on or
        // terminates a live child, and no exit-status pointer is dereferenced.
        let result = unsafe { libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) };
        if result == 0 {
            break;
        }
        if result < 0 {
            let error = std::io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::ECHILD) => break,
                Some(libc::EINTR) => continue,
                _ => return Err(error),
            }
        }
    }
    Ok(())
}
pub async fn probe() -> Result<()> {
    ensure!(
        std::env::args_os().count() == 2,
        "container probe takes no arguments"
    );
    let architecture = match std::env::consts::ARCH {
        "x86_64" => LinuxArchitecture::Amd64,
        "aarch64" => LinuxArchitecture::Arm64,
        _ => anyhow::bail!("unsupported container worker architecture"),
    };
    let shell_available = tokio::time::timeout(
        Duration::from_secs(5),
        beale_tools::process::contained_command("bash")
            .args([
                "--noprofile",
                "--norc",
                "-c",
                "test \"$BASH_VERSINFO\" -ge 5",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status(),
    )
    .await
    .is_ok_and(|result| result.is_ok_and(|status| status.success()));
    let workspace_writable = probe_writes().is_ok();
    println!(
        "{}",
        serde_json::to_string(&RuntimeProbe {
            version: RUNTIME_VERSION,
            worker_protocol: beale_contracts::swarms::VERSION,
            architecture,
            shell_available,
            workspace_writable
        })?
    );
    Ok(())
}
fn probe_writes() -> Result<()> {
    let name = format!(".beale-write-probe-{}", uuid::Uuid::new_v4());
    let source = std::path::PathBuf::from(&name);
    let destination = source.with_extension("done");
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&source)?;
        file.write_all(b"beale-runtime-probe")?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&source, &destination)?;
        ensure!(
            std::fs::read(&destination)? == b"beale-runtime-probe",
            "workspace write changed"
        );
        std::fs::remove_file(&destination)?;
        std::fs::File::open(".")?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&source);
        let _ = std::fs::remove_file(&destination);
    }
    result
}
