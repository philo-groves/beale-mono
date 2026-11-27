use beale_contracts::swarm_files::*;
use beale_storage::{HostDatabase, swarm_files::*};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use uuid::Uuid;

#[path = "swarm_files/full_storage.rs"]
mod full_storage;
#[path = "swarm_files/snapshot_transfer.rs"]
mod snapshot_transfer;

struct Fixture {
    root: PathBuf,
    source: PathBuf,
    db: HostDatabase,
    files: ResearchFiles,
    _cleanup: Cleanup,
}

#[test]
fn deleting_private_environment_preserves_shared_snapshot_and_published_evidence() {
    let f = Fixture::new();
    fs::write(f.source.join("proof.txt"), b"synthetic evidence").unwrap();
    f.capture();
    let first = f
        .files
        .prepare_agent("scan-example", "agent-first")
        .unwrap();
    let second = f
        .files
        .prepare_agent("scan-example", "agent-second")
        .unwrap();
    let mut publication = artifact("artifact-example", b"synthetic evidence");
    publication.source_path = "proof.txt".into();
    let accepted = f.files.publish_file(&first, publication).unwrap();
    f.files
        .delete_private_environment("scan-example", "agent-first")
        .unwrap();
    f.files
        .delete_private_environment("scan-example", "agent-first")
        .unwrap();
    assert!(!first.exists());
    assert!(
        f.files
            .prepare_agent("scan-example", "agent-first")
            .is_err()
    );
    assert_eq!(
        fs::read(second.join("proof.txt")).unwrap(),
        b"synthetic evidence"
    );
    assert_eq!(
        f.files.artifact(&accepted.workspace, &accepted.id).unwrap(),
        accepted
    );
    let (_, manifest) = f.files.snapshot("scan-example").unwrap();
    f.files.verify(&manifest).unwrap();
}

#[test]
fn worker_publication_spool_preserves_captured_bytes_and_materialization_never_overwrites() {
    let f = Fixture::new();
    let recovery = f.root.join("worker-recovery");
    fs::create_dir(&recovery).unwrap();
    fs::write(
        f.source.join("candidate.txt"),
        "Synthetic captured evidence",
    )
    .unwrap();
    let (first, path) =
        stage_worker_artifact(&f.source, &recovery, "publication-example", "candidate.txt")
            .unwrap();
    fs::write(f.source.join("candidate.txt"), "Later workspace edit").unwrap();
    let (retry, retry_path) =
        stage_worker_artifact(&f.source, &recovery, "publication-example", "candidate.txt")
            .unwrap();
    assert_eq!(first, retry);
    assert_eq!(path, retry_path);
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "Synthetic captured evidence"
    );
    let artifact = Artifact {
        id: first.id,
        operation: "publication-example".into(),
        workspace: "workspace-example".into(),
        scan: "scan-example".into(),
        producer: "worker-example".into(),
        assignment: Some("assignment-example".into()),
        attempt: "attempt-example".into(),
        generation: 1,
        revision: 1,
        source_path: first.source_path,
        sha256: first.sha256,
        size: first.size,
        executable: first.executable,
        previous: None,
    };
    materialize_worker_artifact(&f.source, "received/candidate.txt", &path, &artifact).unwrap();
    fs::write(f.source.join("received/candidate.txt"), "Operator edit").unwrap();
    assert!(
        materialize_worker_artifact(&f.source, "received/candidate.txt", &path, &artifact).is_err()
    );
    assert_eq!(
        fs::read_to_string(f.source.join("received/candidate.txt")).unwrap(),
        "Operator edit"
    );
    assert!(materialize_worker_artifact(&f.source, "../outside.txt", &path, &artifact).is_err());
    assert!(
        stage_worker_artifact(&f.source, &recovery, "outside-example", "../outside.txt").is_err()
    );
    assert!(
        stage_worker_artifact(&f.source, &recovery, "excluded-example", ".git/config").is_err()
    );
    fs::write(&path, "Damaged retained bytes").unwrap();
    assert!(
        stage_worker_artifact(&f.source, &recovery, "publication-example", "candidate.txt")
            .is_err()
    );
    assert!(materialize_worker_artifact(&f.source, "corrupted.txt", &path, &artifact).is_err());
    assert!(!f.source.join("corrupted.txt").exists());
    assert!(!recovery.join("state.sqlite").exists());
}
impl Fixture {
    fn new() -> Self {
        #[cfg(windows)]
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(4)
            .unwrap()
            .join("target/swarm-files-tests");
        // Unix permission/link acceptance needs a native filesystem, even when
        // the checkout is on a Windows mount without POSIX metadata support.
        #[cfg(not(windows))]
        let root = std::env::temp_dir().join("beale-swarm-files-tests");
        let root = root.join(Uuid::new_v4().to_string());
        let source = root.join("source");
        fs::create_dir_all(&source).unwrap();
        let db = HostDatabase::open(&root.join("host")).unwrap();
        let files = ResearchFiles::open(db.clone(), &root.join("host")).unwrap();
        Self {
            _cleanup: Cleanup(root.clone()),
            root,
            source,
            db,
            files,
        }
    }
    fn capture(&self) -> String {
        self.files
            .capture(
                "scan-example",
                &[SnapshotInput {
                    source: self.source.clone(),
                    destination: String::new(),
                }],
            )
            .unwrap()
    }
    fn reopen(&self) -> ResearchFiles {
        ResearchFiles::open(
            HostDatabase::open(&self.root.join("host")).unwrap(),
            &self.root.join("host"),
        )
        .unwrap()
    }
}
struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        fn writable(path: &Path) {
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    if let Ok(metadata) = fs::symlink_metadata(entry.path()) {
                        if metadata.file_type().is_symlink() {
                            continue;
                        }
                        if metadata.is_dir() {
                            writable(&entry.path());
                        }
                        #[cfg(windows)]
                        if metadata.is_file() && metadata.permissions().readonly() {
                            #[allow(clippy::permissions_set_readonly_false)]
                            let result = {
                                let mut permissions = metadata.permissions();
                                permissions.set_readonly(false);
                                fs::set_permissions(entry.path(), permissions)
                            };
                            let _ = result;
                        }
                    }
                }
            }
        }
        // Dropped last, after the host database handles close.
        writable(&self.0);
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn artifact(id: &str, bytes: &[u8]) -> Artifact {
    Artifact {
        id: id.into(),
        operation: id.into(),
        workspace: "workspace-example".into(),
        scan: "scan-example".into(),
        producer: "agent-example".into(),
        assignment: Some("assignment-example".into()),
        attempt: "attempt-example".into(),
        generation: 1,
        revision: 1,
        source_path: "result.txt".into(),
        sha256: hash(bytes),
        size: bytes.len() as u64,
        executable: None,
        previous: None,
    }
}

#[test]
fn checkpoint_replay_and_environment_reopen_require_verified_checkpoint_bytes() {
    for corrupt in [false, true] {
        let f = Fixture::new();
        f.capture();
        f.files
            .prepare_agent("scan-example", "agent-example")
            .unwrap();
        let checkpoint = f
            .files
            .checkpoint(
                "scan-example",
                "agent-example",
                "checkpoint-example",
                &json!({"cursor":1}),
            )
            .unwrap();
        let blob = f.root.join("host/swarm-files/blobs").join(checkpoint);
        if corrupt {
            fs::write(blob, "corrupt synthetic checkpoint").unwrap();
        } else {
            fs::remove_file(blob).unwrap();
        }
        assert!(
            f.reopen()
                .checkpoint(
                    "scan-example",
                    "agent-example",
                    "checkpoint-example",
                    &json!({"cursor":1})
                )
                .is_err(),
            "checkpoint replay must verify its committed bytes"
        );
        assert!(
            f.reopen()
                .prepare_agent("scan-example", "agent-example")
                .is_err(),
            "resume must reject missing/corrupt retained context"
        );
    }
}

#[test]
fn artifact_provenance_requires_an_attempt_and_positive_generation() {
    let f = Fixture::new();
    let mut request = artifact("artifact-example", b"example");
    request.attempt.clear();
    assert!(f.files.begin_transfer(&request).is_err());
    request.attempt = "attempt-example".into();
    request.generation = 0;
    assert!(f.files.begin_transfer(&request).is_err());
    request.generation = 1;
    f.files.begin_transfer(&request).unwrap();
    request.attempt = "different-attempt".into();
    assert!(f.files.begin_transfer(&request).is_err());
}

#[test]
fn central_artifact_integrity_survives_edits_from_non_swarm_sessions() {
    let f = Fixture::new();
    let request = artifact(
        &format!("artifact-{}", hash(b"operation-example")),
        b"candidate",
    );
    f.files.begin_transfer(&request).unwrap();
    f.files.write_chunk(&request.id, 0, b"candidate").unwrap();
    f.files.seal(&request.id).unwrap();
    let record =
        json!({"root_cause":{"evidence_refs":[{"kind":"artifact","reference":request.id}]}});
    let check = |workspace: &str| {
        f.db.with_connection(|db| {
            validate_record_artifacts(db, "operator-session-example", workspace, &record)
        })
    };
    check("workspace-example").unwrap();
    assert!(
        check("different-workspace").is_err(),
        "changing session type must not bypass workspace ownership"
    );
    fs::remove_file(f.root.join("host/swarm-files/blobs").join(&request.sha256)).unwrap();
    assert!(
        check("workspace-example").is_err(),
        "operator edits must not acknowledge missing central bytes"
    );
}

#[test]
fn artifact_input_rejections_do_not_mask_durable_storage_failures() {
    use beale_contracts::tool_error::{ToolError, ToolErrorCode};
    let f = Fixture::new();
    f.capture();
    let request = artifact("artifact-example", b"candidate");
    let record = json!({"artifact_refs":[request.id]});
    let check = |workspace: &str, record: &serde_json::Value| {
        f.db.with_connection(|db| validate_record_artifacts(db, "scan-example", workspace, record))
    };
    let rejection = |error: anyhow::Error, code| {
        assert_eq!(error.downcast_ref::<ToolError>().unwrap().code, code);
    };
    rejection(
        check("workspace-example", &json!({"artifact_refs":[7]})).unwrap_err(),
        ToolErrorCode::InvalidArguments,
    );
    rejection(
        check("workspace-example", &record).unwrap_err(),
        ToolErrorCode::Rejected,
    );
    f.files.begin_transfer(&request).unwrap();
    f.files.write_chunk(&request.id, 0, b"candidate").unwrap();
    rejection(
        check("workspace-example", &record).unwrap_err(),
        ToolErrorCode::Rejected,
    );
    f.files.seal(&request.id).unwrap();
    check("workspace-example", &record).unwrap();
    rejection(
        check("other-workspace-example", &record).unwrap_err(),
        ToolErrorCode::Rejected,
    );
    let blob = f.root.join("host/swarm-files/blobs").join(&request.sha256);
    // A previously accepted artifact becoming unreadable is a host failure;
    // the model must not receive a durable user-error receipt for it.
    let backup = blob.with_extension("backup");
    fs::rename(&blob, &backup).unwrap();
    assert!(
        check("workspace-example", &record)
            .unwrap_err()
            .downcast_ref::<ToolError>()
            .is_none()
    );
    fs::write(&blob, b"corrupt").unwrap();
    assert!(
        check("workspace-example", &record)
            .unwrap_err()
            .downcast_ref::<ToolError>()
            .is_none()
    );
    fs::remove_file(&blob).unwrap();
    fs::rename(&backup, &blob).unwrap();
    check("workspace-example", &record).unwrap();
    f.db.with_connection(|db| {
        db.execute(
            "UPDATE swarm_file_transfers SET document='invalid' WHERE id=?1",
            [&request.id],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(
        check("workspace-example", &record)
            .unwrap_err()
            .downcast_ref::<ToolError>()
            .is_none()
    );
    f.db.with_connection(|db| {
        db.execute_batch(
            "ALTER TABLE swarm_file_transfers RENAME COLUMN document TO unavailable_document;",
        )?;
        Ok(())
    })
    .unwrap();
    assert!(
        check("workspace-example", &record)
            .unwrap_err()
            .downcast_ref::<ToolError>()
            .is_none()
    );
}

#[test]
fn explicit_external_mounts_cannot_import_any_host_runtime_directory() {
    let f = Fixture::new();
    let private = f.root.join("host/private-cache");
    fs::create_dir(&private).unwrap();
    fs::write(private.join("provider-state"), "synthetic private state").unwrap();
    let result = f.files.capture(
        "scan-example",
        &[
            SnapshotInput {
                source: f.source.clone(),
                destination: String::new(),
            },
            SnapshotInput {
                source: private,
                destination: "external".into(),
            },
        ],
    );
    assert!(
        result.is_err(),
        "explicit mounts must still exclude the host data boundary"
    );
    assert!(f.files.snapshot("scan-example").is_err());
}

#[test]
fn transferred_manifest_order_does_not_change_materialization_semantics() {
    let f = Fixture::new();
    fs::create_dir_all(f.source.join("nested/deeper")).unwrap();
    fs::write(f.source.join("nested/deeper/result.txt"), "synthetic bytes").unwrap();
    f.capture();
    let (_, mut manifest) = f.files.snapshot("scan-example").unwrap();
    manifest.entries.reverse();
    let target = f.root.join("reordered");
    f.files.materialize_snapshot(&manifest, &target).unwrap();
    assert_eq!(
        fs::read_to_string(target.join("nested/deeper/result.txt")).unwrap(),
        "synthetic bytes"
    );
}

#[cfg(any(windows, unix))]
#[test]
fn retained_directory_links_never_alias_another_agents_workspace() {
    let f = Fixture::new();
    f.capture();
    let first = f
        .files
        .prepare_agent("scan-example", "agent-example")
        .unwrap();
    let second = f
        .files
        .prepare_agent("scan-example", "other-example")
        .unwrap();
    fs::rename(&first, first.with_extension("retained")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&second, &first).unwrap();
    #[cfg(windows)]
    {
        let path = |p: &Path| p.to_str().unwrap().trim_start_matches(r"\\?\").to_owned();
        let output = Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J", &path(&first), &path(&second)])
            .output()
            .unwrap();
        assert!(output.status.success(), "synthetic junction fixture failed");
    }
    let resumed = f.reopen().prepare_agent("scan-example", "agent-example");
    let request = artifact("artifact-example", b"candidate");
    f.files.begin_transfer(&request).unwrap();
    f.files.write_chunk(&request.id, 0, b"candidate").unwrap();
    f.files.seal(&request.id).unwrap();
    let materialized =
        f.files
            .materialize_artifact(&request.workspace, &request.id, &first, "wrong-agent.txt");
    let checkpoint = f.reopen().checkpoint(
        "scan-example",
        "agent-example",
        "checkpoint-example",
        &json!({"cursor":1}),
    );
    // Remove only the link before normal fixture cleanup.
    #[cfg(windows)]
    fs::remove_dir(&first).unwrap();
    #[cfg(unix)]
    fs::remove_file(&first).unwrap();
    assert!(resumed.is_err(), "a retained root link must not be resumed");
    assert!(
        checkpoint.is_err(),
        "a retained root link must not be checkpointed"
    );
    assert!(
        materialized.is_err(),
        "explicit materialization must reject a replaced workspace root"
    );
    assert!(!second.join("wrong-agent.txt").exists());
}

#[test]
fn merge_only_excluded_git_paths_never_enter_a_sealed_snapshot() {
    let f = Fixture::new();
    git(&f.source, &["init", "--quiet"]);
    fs::write(f.source.join("base.txt"), "synthetic base").unwrap();
    git(&f.source, &["add", "base.txt"]);
    git(&f.source, &["commit", "--quiet", "-m", "Base fixture"]);
    let branch = git(&f.source, &["symbolic-ref", "--short", "HEAD"]);
    git(&f.source, &["checkout", "--quiet", "-b", "example-topic"]);
    fs::write(f.source.join("topic.txt"), "synthetic topic").unwrap();
    git(&f.source, &["add", "topic.txt"]);
    git(&f.source, &["commit", "--quiet", "-m", "Topic fixture"]);
    git(&f.source, &["checkout", "--quiet", branch.trim()]);
    fs::write(f.source.join("main.txt"), "synthetic main").unwrap();
    git(&f.source, &["add", "main.txt"]);
    git(&f.source, &["commit", "--quiet", "-m", "Main fixture"]);
    git(
        &f.source,
        &[
            "merge",
            "--quiet",
            "--no-ff",
            "--no-commit",
            "example-topic",
        ],
    );
    fs::write(
        f.source.join("memory.sqlite"),
        "synthetic excluded merge bytes",
    )
    .unwrap();
    git(&f.source, &["add", "memory.sqlite"]);
    git(&f.source, &["commit", "--quiet", "-m", "Merge fixture"]);
    let result = f.files.capture(
        "scan-example",
        &[SnapshotInput {
            source: f.source.clone(),
            destination: String::new(),
        }],
    );
    assert!(
        result.is_err(),
        "merge-only excluded history must fail before sealing"
    );
    assert!(f.files.snapshot("scan-example").is_err());
}
struct FailOnce {
    point: FaultPoint,
    count: AtomicUsize,
}
impl StorageFaults for FailOnce {
    fn check(&self, point: FaultPoint) -> anyhow::Result<()> {
        if point == self.point && self.count.fetch_add(1, Ordering::SeqCst) == 0 {
            anyhow::bail!("synthetic out of space");
        }
        Ok(())
    }
}
fn fault(files: &ResearchFiles, point: FaultPoint) -> ResearchFiles {
    files.clone().with_faults(Arc::new(FailOnce {
        point,
        count: AtomicUsize::new(0),
    }))
}

#[test]
fn immutable_snapshot_private_copies_and_retained_edits() {
    let f = Fixture::new();
    fs::write(f.source.join("shared.txt"), "initial").unwrap();
    let snapshot = f.capture();
    fs::write(f.source.join("shared.txt"), "leader edit").unwrap();
    assert_eq!(f.capture(), snapshot);
    let a = f
        .files
        .prepare_agent("scan-example", "agent-example")
        .unwrap();
    let b = f
        .files
        .prepare_agent("scan-example", "second-example")
        .unwrap();
    assert_ne!(a, b);
    assert_eq!(fs::read_to_string(a.join("shared.txt")).unwrap(), "initial");
    fs::write(a.join("shared.txt"), "first edit").unwrap();
    fs::write(b.join("shared.txt"), "second edit").unwrap();
    let checkpoint = f
        .files
        .checkpoint(
            "scan-example",
            "agent-example",
            "checkpoint-example",
            &json!({"cursor":1}),
        )
        .unwrap();
    let reopened = f.reopen();
    assert_eq!(
        reopened
            .prepare_agent("scan-example", "agent-example")
            .unwrap(),
        a
    );
    assert_eq!(
        reopened
            .environment("scan-example", "agent-example")
            .unwrap()
            .checkpoint,
        Some(checkpoint)
    );
    assert_eq!(
        fs::read_to_string(a.join("shared.txt")).unwrap(),
        "first edit"
    );
    assert_eq!(
        fs::read_to_string(b.join("shared.txt")).unwrap(),
        "second edit"
    );
    assert_eq!(
        fs::read_to_string(f.source.join("shared.txt")).unwrap(),
        "leader edit"
    );
}

#[test]
fn interrupted_transfers_resume_and_reject_changed_or_corrupt_bytes() {
    let f = Fixture::new();
    let a = artifact("artifact-example", b"abcdef");
    assert_eq!(f.files.begin_transfer(&a).unwrap(), 0);
    assert_eq!(f.files.write_chunk(&a.id, 0, b"abc").unwrap(), 3);
    assert!(f.files.seal(&a.id).is_err());
    let files = f.reopen();
    assert_eq!(files.begin_transfer(&a).unwrap(), 3);
    assert_eq!(files.write_chunk(&a.id, 0, b"abc").unwrap(), 3);
    assert!(files.write_chunk(&a.id, 0, b"bad").is_err());
    assert!(files.write_chunk(&a.id, 4, b"ef").is_err());
    let mut changed = a.clone();
    changed.sha256 = hash(b"uvwxyz");
    assert!(files.begin_transfer(&changed).is_err());
    files.write_chunk(&a.id, 3, b"def").unwrap();
    assert_eq!(files.seal(&a.id).unwrap(), a);
    assert_eq!(files.seal(&a.id).unwrap(), a);
    let corrupt = artifact("artifact-corrupt", b"expected");
    files.begin_transfer(&corrupt).unwrap();
    files.write_chunk(&corrupt.id, 0, b"replaced").unwrap();
    assert!(files.seal(&corrupt.id).is_err());
    assert!(files.artifact(&corrupt.workspace, &corrupt.id).is_err());
    assert!(files.artifact("other-workspace", &a.id).is_err());
}

#[test]
fn publication_faults_keep_pending_bytes_and_retry_identity() {
    for point in [
        FaultPoint::ChunkWrite,
        FaultPoint::ChunkCommit,
        FaultPoint::ArtifactSeal,
        FaultPoint::ArtifactCommit,
    ] {
        let f = Fixture::new();
        f.capture();
        f.files
            .prepare_agent("scan-example", "agent-example")
            .unwrap();
        let files = fault(&f.files, point);
        let a = artifact("artifact-example", b"content");
        files.begin_transfer(&a).unwrap();
        let result = files
            .write_chunk(&a.id, 0, b"content")
            .and_then(|_| files.seal(&a.id));
        assert!(result.is_err(), "{point:?}");
        assert!(files.artifact(&a.workspace, &a.id).is_err());
        assert_eq!(
            files
                .environment(&a.scan, &a.producer)
                .unwrap()
                .pending_publications,
            vec![a.id.clone()]
        );
        let recovered = f.reopen();
        let offset = recovered.begin_transfer(&a).unwrap();
        if offset == 0 {
            recovered.write_chunk(&a.id, 0, b"content").unwrap();
        }
        assert_eq!(recovered.seal(&a.id).unwrap(), a);
        assert!(
            recovered
                .environment(&a.scan, &a.producer)
                .unwrap()
                .pending_publications
                .is_empty()
        );
    }
}

#[test]
fn checkpoints_keep_previous_ack_on_write_and_commit_exhaustion() {
    for point in [FaultPoint::CheckpointWrite, FaultPoint::CheckpointCommit] {
        let f = Fixture::new();
        f.capture();
        let directory = f
            .files
            .prepare_agent("scan-example", "agent-example")
            .unwrap();
        let previous = f
            .files
            .checkpoint(
                "scan-example",
                "agent-example",
                "first-example",
                &json!({"cursor":1}),
            )
            .unwrap();
        fs::write(directory.join("pending.txt"), "retain output").unwrap();
        let files = fault(&f.files, point);
        assert!(
            files
                .checkpoint(
                    "scan-example",
                    "agent-example",
                    "next-example",
                    &json!({"cursor":2})
                )
                .is_err()
        );
        assert_eq!(
            files
                .environment("scan-example", "agent-example")
                .unwrap()
                .checkpoint,
            Some(previous)
        );
        let next = files
            .checkpoint(
                "scan-example",
                "agent-example",
                "next-example",
                &json!({"cursor":2}),
            )
            .unwrap();
        assert_eq!(
            files
                .checkpoint(
                    "scan-example",
                    "agent-example",
                    "next-example",
                    &json!({"cursor":2})
                )
                .unwrap(),
            next
        );
        assert!(
            files
                .checkpoint(
                    "scan-example",
                    "agent-example",
                    "next-example",
                    &json!({"cursor":3})
                )
                .is_err()
        );
        assert_eq!(
            fs::read_to_string(directory.join("pending.txt")).unwrap(),
            "retain output"
        );
    }
}

#[test]
fn attachment_resolution_is_all_or_nothing_and_checks_central_bytes() {
    let f = Fixture::new();
    fs::write(f.source.join("proof.txt"), "candidate bytes").unwrap();
    let base = artifact("unused-example", b"");
    for arguments in [
        json!({"evidence_refs":[{"kind":"artifact","reference":"file:proof.txt"}]}),
        json!({"handoff":{"artifact_refs":["file:proof.txt"]}}),
    ] {
        let mut args = arguments.clone();
        let files = fault(&f.files, FaultPoint::ArtifactCommit);
        assert!(
            files
                .resolve_attachments(&f.source, "call-example", &base, &mut args)
                .is_err()
        );
        assert_eq!(args, arguments);
        files
            .resolve_attachments(&f.source, "call-example", &base, &mut args)
            .unwrap();
        assert!(!args.to_string().contains("file:proof.txt"));
        let accepted = args.clone();
        files
            .resolve_attachments(&f.source, "call-example", &base, &mut args)
            .unwrap();
        assert_eq!(args, accepted);
    }
    let mut missing = json!({"handoff":{"artifact_refs":["artifact-missing"]}});
    assert!(
        f.files
            .resolve_attachments(&f.source, "missing-example", &base, &mut missing)
            .is_err()
    );
    let mut missing = json!({"reported":{"poc_artifact_refs":["file:missing.txt"]}});
    assert!(
        f.files
            .resolve_attachments(&f.source, "missing-example", &base, &mut missing)
            .is_err()
    );
    let mut partial = json!({"artifact_refs":["file:proof.txt","file:missing.txt"]});
    let before = partial.clone();
    assert!(
        f.files
            .resolve_attachments(&f.source, "partial-example", &base, &mut partial)
            .is_err()
    );
    assert_eq!(before, partial);
}

#[test]
fn record_commit_failure_leaves_accepted_bytes_recoverable_without_dangling_refs() {
    let f = Fixture::new();
    fs::write(f.source.join("result.txt"), "candidate bytes").unwrap();
    f.db.with_connection(|db|{db.execute_batch("CREATE TABLE example_records(id TEXT PRIMARY KEY, document TEXT);CREATE TRIGGER reject_record BEFORE INSERT ON example_records BEGIN SELECT RAISE(FAIL,'synthetic full disk'); END;")?;Ok(())}).unwrap();
    let original = json!({"artifact_refs":["file:result.txt"]});
    let mut args = original.clone();
    f.files
        .resolve_attachments(
            &f.source,
            "record-example",
            &artifact("unused-example", b""),
            &mut args,
        )
        .unwrap();
    let commit = |args: &serde_json::Value| {
        f.db.with_connection(|db| {
            db.execute(
                "INSERT INTO example_records VALUES('record-example',?1)",
                [args.to_string()],
            )?;
            Ok(())
        })
    };
    assert!(commit(&args).is_err());
    f.db.with_connection(|db| {
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM example_records", [], |r| r
                .get::<_, i64>(0))?,
            0
        );
        db.execute_batch("DROP TRIGGER reject_record")?;
        Ok(())
    })
    .unwrap();
    let mut retry = original;
    f.reopen()
        .resolve_attachments(
            &f.source,
            "record-example",
            &artifact("unused-example", b""),
            &mut retry,
        )
        .unwrap();
    assert_eq!(retry, args);
    commit(&retry).unwrap();
    let id = args["artifact_refs"][0].as_str().unwrap();
    f.files.artifact("workspace-example", id).unwrap();
}

#[test]
fn explicit_materialization_never_overwrites_or_crosses_workspace_boundary() {
    let f = Fixture::new();
    fs::write(f.source.join("result.txt"), "first version").unwrap();
    let a = f
        .files
        .publish_file(&f.source, artifact("artifact-example", b""))
        .unwrap();
    let receiver = f.root.join("recipient");
    fs::create_dir(&receiver).unwrap();
    f.files
        .materialize_artifact(&a.workspace, &a.id, &receiver, "handoff/version-1.txt")
        .unwrap();
    assert_eq!(
        fs::read_to_string(receiver.join("handoff/version-1.txt")).unwrap(),
        "first version"
    );
    assert!(
        f.files
            .materialize_artifact(&a.workspace, &a.id, &receiver, "handoff/version-1.txt")
            .is_err()
    );
    for path in [
        "../outside.txt",
        "/absolute.txt",
        "C:/host.txt",
        "state.sqlite",
        ".beale/database",
    ] {
        assert!(
            f.files
                .materialize_artifact(&a.workspace, &a.id, &receiver, path)
                .is_err()
        );
    }
    fs::write(f.source.join("result.txt"), "second version").unwrap();
    let mut next = artifact("artifact-next", b"");
    next.previous = Some(a.id.clone());
    next.revision = 2;
    let next = f.files.publish_file(&f.source, next).unwrap();
    assert_eq!(next.previous, Some(a.id));
    assert_eq!(
        fs::read_to_string(receiver.join("handoff/version-1.txt")).unwrap(),
        "first version"
    );
}

#[test]
fn snapshots_exclude_canonical_databases_credentials_and_runtime_state() {
    let f = Fixture::new();
    for path in [
        "memory.sqlite",
        "state.sqlite-wal",
        "memories.sqlite-shm",
        ".env",
        ".gitconfig",
        ".beale/data.txt",
        ".aws/credentials",
        ".codex/auth.json",
    ] {
        let path = f.source.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "excluded synthetic bytes").unwrap();
    }
    fs::write(f.source.join("research.txt"), "included").unwrap();
    f.capture();
    let (_, manifest) = f.files.snapshot("scan-example").unwrap();
    assert_eq!(manifest.entries.len(), 1);
    assert_eq!(manifest.entries[0].path, "research.txt");
    for path in ["state.sqlite", "memory.sqlite-wal", ".aws/credentials"] {
        let mut a = artifact("artifact-excluded", b"bytes");
        a.source_path = path.into();
        assert!(f.files.begin_transfer(&a).is_err());
    }
    assert!(
        f.files
            .capture(
                "host-example",
                &[SnapshotInput {
                    source: f.root.join("host"),
                    destination: String::new()
                }]
            )
            .is_err()
    );
}

struct Mutate {
    path: PathBuf,
    count: AtomicUsize,
    once: bool,
}
impl StorageFaults for Mutate {
    fn check(&self, point: FaultPoint) -> anyhow::Result<()> {
        if point == FaultPoint::CaptureVerify {
            let n = self.count.fetch_add(1, Ordering::SeqCst);
            if !self.once || n == 0 {
                fs::write(&self.path, format!("change-{n}"))?;
            }
        }
        Ok(())
    }
}
#[test]
fn changing_sources_retry_boundedly_and_only_seal_verified_inputs() {
    for once in [true, false] {
        let f = Fixture::new();
        let path = f.source.join("changing.txt");
        fs::write(&path, "initial").unwrap();
        let mutate = Arc::new(Mutate {
            path,
            count: AtomicUsize::new(0),
            once,
        });
        let files = f.files.clone().with_faults(mutate.clone());
        let result = files.capture(
            "scan-example",
            &[SnapshotInput {
                source: f.source.clone(),
                destination: String::new(),
            }],
        );
        if once {
            assert!(result.is_ok());
            assert_eq!(mutate.count.load(Ordering::SeqCst), 2);
        } else {
            assert!(result.unwrap_err().to_string().contains("quiesce"));
            assert_eq!(mutate.count.load(Ordering::SeqCst), 3);
            assert!(
                files
                    .prepare_agent("scan-example", "agent-example")
                    .is_err()
            );
        }
    }
}

fn file(path: &str, exec: Option<bool>, mode: Option<u32>) -> Entry {
    Entry {
        path: path.into(),
        kind: EntryKind::File {
            size: 1,
            sha256: hash(b"x"),
            executable: exec,
            mode,
        },
    }
}
#[test]
fn simulated_destinations_reject_collisions_names_links_and_required_permissions() {
    let mut windows = Destination::native();
    windows.case_sensitive = false;
    windows.windows_names = true;
    windows.posix_permissions = false;
    windows.symlinks = false;
    for entries in [
        vec![file("A.txt", None, None), file("a.txt", None, None)],
        vec![file("CON.txt", None, None)],
        vec![file("bad?.txt", None, None)],
        vec![file("private.txt", Some(false), Some(0o600))],
        vec![Entry {
            path: "link".into(),
            kind: EntryKind::Symlink {
                target: "outside".into(),
                directory: false,
            },
        }],
    ] {
        assert!(
            windows
                .validate(&Manifest {
                    version: VERSION,
                    entries,
                    repositories: vec![]
                })
                .is_err()
        );
    }
    let mut linux = Destination::portable();
    linux.require_executable_metadata = true;
    assert!(
        linux
            .validate(&Manifest {
                version: VERSION,
                entries: vec![file("unknown.sh", None, None)],
                repositories: vec![]
            })
            .is_err()
    );
    for target in ["../outside", "/outside", "C:/outside", "missing"] {
        assert!(
            linux
                .validate(&Manifest {
                    version: VERSION,
                    entries: vec![
                        file("source", Some(true), None),
                        Entry {
                            path: "link".into(),
                            kind: EntryKind::Symlink {
                                target: target.into(),
                                directory: false
                            }
                        }
                    ],
                    repositories: vec![]
                })
                .is_err()
        );
    }
    let supported = Manifest {
        version: VERSION,
        entries: vec![
            file("source", Some(true), None),
            Entry {
                path: "link".into(),
                kind: EntryKind::Symlink {
                    target: "source".into(),
                    directory: false,
                },
            },
        ],
        repositories: vec![],
    };
    linux.validate(&supported).unwrap();
    let wire = serde_json::to_vec(&supported).unwrap();
    assert_eq!(
        serde_json::from_slice::<Manifest>(&wire).unwrap(),
        supported
    );
}

#[test]
fn declared_external_roots_and_simulated_transfer_preserve_bytes_and_metadata() {
    let f = Fixture::new();
    fs::write(f.source.join("source.txt"), "primary").unwrap();
    let external = f.root.join("external");
    fs::create_dir(&external).unwrap();
    fs::write(external.join("dependency.txt"), "dependency").unwrap();
    f.files
        .capture(
            "scan-example",
            &[
                SnapshotInput {
                    source: f.source.clone(),
                    destination: "workspace".into(),
                },
                SnapshotInput {
                    source: external,
                    destination: "inputs/dependency".into(),
                },
            ],
        )
        .unwrap();
    let (_, manifest) = f.files.snapshot("scan-example").unwrap();
    let remote = Fixture::new();
    assert!(
        remote
            .files
            .import_snapshot("scan-example", &manifest)
            .is_err()
    );
    for (i, entry) in manifest.entries.iter().enumerate() {
        if let EntryKind::File { sha256, size, .. } = &entry.kind {
            let bytes = f.files.read_blob(sha256, *size).unwrap();
            let a = artifact(&format!("transfer-{i}"), &bytes);
            remote.files.begin_transfer(&a).unwrap();
            for (offset, byte) in bytes.iter().enumerate() {
                remote
                    .files
                    .write_chunk(&a.id, offset as u64, &[*byte])
                    .unwrap();
            }
            remote.files.seal(&a.id).unwrap();
        }
    }
    remote
        .files
        .import_snapshot("scan-example", &manifest)
        .unwrap();
    assert_eq!(remote.files.snapshot("scan-example").unwrap().1, manifest);
    let directory = remote
        .files
        .prepare_agent("scan-example", "agent-example")
        .unwrap();
    assert_eq!(
        fs::read_to_string(directory.join("inputs/dependency/dependency.txt")).unwrap(),
        "dependency"
    );
    assert_eq!(
        fs::read_to_string(directory.join("workspace/source.txt")).unwrap(),
        "primary"
    );
    let mut changed = manifest;
    changed.entries.pop();
    assert!(
        remote
            .files
            .import_snapshot("scan-example", &changed)
            .is_err()
    );
}

fn git(path: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(path)
        .args([
            "-c",
            "user.name=Example Research",
            "-c",
            "user.email=research@example.test",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
#[test]
fn git_snapshots_are_self_contained_and_preserve_authoritative_executable_metadata() {
    let f = Fixture::new();
    git(&f.source, &["init", "--quiet"]);
    fs::write(f.source.join("runner.txt"), "synthetic runner").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            f.source.join("runner.txt"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    git(&f.source, &["add", "runner.txt"]);
    git(&f.source, &["update-index", "--chmod=+x", "runner.txt"]);
    git(&f.source, &["commit", "--quiet", "-m", "Synthetic fixture"]);
    git(
        &f.source,
        &["config", "credential.helper", "synthetic-secret-helper"],
    );
    git(
        &f.source,
        &[
            "config",
            "remote.origin.url",
            "https://synthetic-user:synthetic-password@example.test/repository",
        ],
    );
    f.capture();
    let (_, manifest) = f.files.snapshot("scan-example").unwrap();
    assert!(manifest.entries.iter().any(|e| matches!(
        &e.kind,
        EntryKind::File {
            executable: Some(true),
            ..
        }
    ) && e.path == "runner.txt"));
    assert_eq!(manifest.repositories.len(), 1);
    assert!(
        !serde_json::to_string(&manifest)
            .unwrap()
            .contains("synthetic-password")
    );
    let directory = f
        .files
        .prepare_agent("scan-example", "agent-example")
        .unwrap();
    f.files
        .checkpoint(
            "scan-example",
            "agent-example",
            "git-checkpoint-example",
            &json!({"saved":true}),
        )
        .unwrap();
    let config = fs::read_to_string(directory.join(".git/config")).unwrap();
    assert!(
        !config.contains("credential")
            && !config.contains("origin")
            && !config.contains("synthetic-secret")
    );
    assert!(!directory.join(".git/objects/info/alternates").exists());
    fs::rename(&f.source, f.root.join("source-moved")).unwrap();
    assert_eq!(
        git(&directory, &["rev-parse", "HEAD"]).trim(),
        manifest.repositories[0].head
    );
    assert!(git(&directory, &["ls-files", "--stage"]).starts_with("100755 "));
    assert!(git(&directory, &["log", "--oneline"]).contains("Synthetic fixture"));
}

#[test]
fn environment_metadata_commit_failure_never_acknowledges_partial_directory() {
    let f = Fixture::new();
    fs::write(f.source.join("source.txt"), "initial").unwrap();
    f.capture();
    let files = fault(&f.files, FaultPoint::EnvironmentCommit);
    assert!(
        files
            .prepare_agent("scan-example", "agent-example")
            .is_err()
    );
    assert!(files.environment("scan-example", "agent-example").is_err());
    let directory = f
        .reopen()
        .prepare_agent("scan-example", "agent-example")
        .unwrap();
    assert_eq!(
        fs::read_to_string(directory.join("source.txt")).unwrap(),
        "initial"
    );
    fs::rename(&directory, directory.with_extension("missing")).unwrap();
    assert!(
        f.files
            .prepare_agent("scan-example", "agent-example")
            .is_err()
    );
}

#[test]
fn git_history_exclusions_and_staged_metadata_fail_explicitly() {
    let f = Fixture::new();
    git(&f.source, &["init", "--quiet"]);
    fs::write(f.source.join("source.txt"), "synthetic source").unwrap();
    git(&f.source, &["add", "source.txt"]);
    git(&f.source, &["commit", "--quiet", "-m", "Initial fixture"]);
    fs::write(f.source.join("source.txt"), "staged source").unwrap();
    git(&f.source, &["add", "source.txt"]);
    assert!(
        f.files
            .capture(
                "staged-example",
                &[SnapshotInput {
                    source: f.source.clone(),
                    destination: String::new()
                }]
            )
            .is_err()
    );
    git(&f.source, &["commit", "--quiet", "-m", "Staged fixture"]);
    fs::write(f.source.join("memory.sqlite"), "synthetic excluded bytes").unwrap();
    git(&f.source, &["add", "memory.sqlite"]);
    git(&f.source, &["commit", "--quiet", "-m", "Excluded fixture"]);
    git(&f.source, &["rm", "--quiet", "memory.sqlite"]);
    git(
        &f.source,
        &["commit", "--quiet", "-m", "Remove excluded fixture"],
    );
    let error = f
        .files
        .capture(
            "excluded-example",
            &[SnapshotInput {
                source: f.source.clone(),
                destination: String::new(),
            }],
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("repository history contains excluded path: memory.sqlite")
    );
}

#[test]
fn linked_git_worktrees_become_self_contained_without_host_pointers() {
    let f = Fixture::new();
    git(&f.source, &["init", "--quiet"]);
    fs::write(f.source.join("source.txt"), "synthetic source").unwrap();
    git(&f.source, &["add", "source.txt"]);
    git(&f.source, &["commit", "--quiet", "-m", "Initial fixture"]);
    let worktree = f.root.join("linked");
    git(
        &f.source,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "example-branch",
            worktree.to_str().unwrap(),
        ],
    );
    assert!(worktree.join(".git").is_file());
    f.files
        .capture(
            "scan-example",
            &[SnapshotInput {
                source: worktree,
                destination: String::new(),
            }],
        )
        .unwrap();
    let copy = f
        .files
        .prepare_agent("scan-example", "agent-example")
        .unwrap();
    assert!(copy.join(".git").is_dir());
    assert!(!copy.join(".git/commondir").exists());
    assert!(!copy.join(".git/gitdir").exists());
    assert_eq!(
        git(&copy, &["symbolic-ref", "HEAD"]).trim(),
        "refs/heads/example-branch"
    );
    fs::rename(&f.source, f.root.join("original-moved")).unwrap();
    assert!(git(&copy, &["log", "--oneline"]).contains("Initial fixture"));
}

#[cfg(windows)]
#[test]
fn windows_git_link_and_executable_metadata_survive_simulated_linux_transfer() {
    let f = Fixture::new();
    git(&f.source, &["init", "--quiet"]);
    git(&f.source, &["config", "core.symlinks", "false"]);
    fs::write(f.source.join("runner.txt"), "synthetic runner").unwrap();
    fs::write(f.source.join("link"), "runner.txt").unwrap();
    git(&f.source, &["add", "runner.txt", "link"]);
    git(&f.source, &["update-index", "--chmod=+x", "runner.txt"]);
    let object = git(&f.source, &["hash-object", "-w", "link"]);
    git(
        &f.source,
        &[
            "update-index",
            "--cacheinfo",
            &format!("120000,{},link", object.trim()),
        ],
    );
    git(&f.source, &["commit", "--quiet", "-m", "Portable fixture"]);
    f.capture();
    let (_, manifest) = f.files.snapshot("scan-example").unwrap();
    let mut linux = Destination::portable();
    linux.require_executable_metadata = true;
    linux.validate(&manifest).unwrap();
    assert!(manifest.entries.iter().any(|e| e.path == "link"
        && e.kind
            == EntryKind::Symlink {
                target: "runner.txt".into(),
                directory: false
            }));
    assert!(manifest.entries.iter().any(|e| matches!(
        e.kind,
        EntryKind::File {
            executable: Some(true),
            ..
        }
    )));
    let transferred: Manifest =
        serde_json::from_slice(&serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert_eq!(manifest, transferred);
    for repo in &transferred.repositories {
        assert_eq!(
            hash(&f.files.read_blob(&repo.bundle, repo.size).unwrap()),
            repo.bundle
        );
    }
    assert!(
        f.files
            .prepare_agent("scan-example", "unsupported-windows")
            .unwrap_err()
            .to_string()
            .contains("symlink")
    );
}

#[cfg(unix)]
#[test]
fn native_symlinks_are_preserved_without_importing_ambient_host_files() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    fs::write(f.source.join("source.txt"), "source").unwrap();
    symlink("source.txt", f.source.join("link")).unwrap();
    f.capture();
    let copy = f
        .files
        .prepare_agent("scan-example", "agent-example")
        .unwrap();
    assert_eq!(
        fs::read_link(copy.join("link")).unwrap(),
        Path::new("source.txt")
    );
    fs::write(f.root.join("outside.txt"), "outside").unwrap();
    symlink("../outside.txt", f.source.join("escape")).unwrap();
    assert!(
        f.files
            .capture(
                "scan-escape",
                &[SnapshotInput {
                    source: f.source.clone(),
                    destination: String::new()
                }]
            )
            .unwrap_err()
            .to_string()
            .contains("escape")
    );
    assert!(
        f.files
            .publish_file(
                &f.source,
                Artifact {
                    source_path: "link".into(),
                    ..artifact("linked-artifact", b"")
                }
            )
            .is_err()
    );
}

#[test]
fn plugin_resources_materialize_idempotently_without_replacing_private_edits() {
    let f = Fixture::new();
    let bytes = b"Synthetic plugin resource";
    let file = beale_contracts::swarm_plugins::ResourceFile {
        path: "PLUGIN.md".into(),
        sha256: hash(bytes),
        size: bytes.len() as u64,
        executable: false,
    };
    let destination = "plugins/bundle-example/example-plugin/PLUGIN.md";
    materialize_plugin_resource(&f.source, destination, &file, bytes).unwrap();
    materialize_plugin_resource(&f.source, destination, &file, bytes).unwrap();
    assert!(materialize_plugin_resource(&f.source, "../escape", &file, bytes).is_err());
    assert!(materialize_plugin_resource(&f.source, "corrupt", &file, b"invalid").is_err());
    fs::write(f.source.join(destination), b"Private edit").unwrap();
    assert!(materialize_plugin_resource(&f.source, destination, &file, bytes).is_err());
    assert_eq!(
        fs::read(f.source.join(destination)).unwrap(),
        b"Private edit"
    );
    #[cfg(windows)]
    {
        let mut executable = file.clone();
        executable.executable = true;
        assert!(
            materialize_plugin_resource(&f.source, "exec-resource", &executable, bytes).is_err()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let mut executable = file.clone();
        executable.executable = true;
        materialize_plugin_resource(&f.source, "exec-resource", &executable, bytes).unwrap();
        assert_ne!(
            fs::metadata(f.source.join("exec-resource"))
                .unwrap()
                .permissions()
                .mode()
                & 0o111,
            0
        );
        symlink(f.root.join("host"), f.source.join("linked-plugin")).unwrap();
        assert!(
            materialize_plugin_resource(&f.source, "linked-plugin/file", &file, bytes).is_err()
        );
    }
}

#[cfg(unix)]
#[test]
fn plugin_publication_verifies_permissions_on_the_checkout_filesystem() {
    use std::os::unix::fs::PermissionsExt;

    // Exercise the actual checkout mount as well as Fixture's native temp
    // filesystem. WSL/DrvFS without metadata accepts chmod without applying it.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .unwrap()
        .join("target/swarm-mount-tests")
        .join(Uuid::new_v4().to_string());
    let _cleanup = Cleanup(root.clone());
    fs::create_dir_all(&root).unwrap();
    let probe = root.join("permission-probe");
    fs::write(&probe, b"Synthetic permission probe").unwrap();
    fs::set_permissions(&probe, fs::Permissions::from_mode(0o644)).unwrap();
    let supports_mode = fs::metadata(&probe).unwrap().permissions().mode() & 0o7777 == 0o644;
    let bytes = b"Synthetic plugin guidance";
    let resource = beale_contracts::swarm_plugins::ResourceFile {
        path: "PLUGIN.md".into(),
        sha256: hash(bytes),
        size: bytes.len() as u64,
        executable: false,
    };
    for _ in 0..2 {
        let result = materialize_plugin_resource(&root, "PLUGIN.md", &resource, bytes);
        if supports_mode {
            result.unwrap();
            assert_eq!(
                fs::metadata(root.join("PLUGIN.md"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o7777,
                0o644
            );
        } else {
            assert!(result.unwrap_err().to_string().contains("permission bits"));
            assert!(!root.join("PLUGIN.md").exists());
        }
    }
}

#[test]
fn resumed_plugin_resources_keep_previous_private_edits_and_references() {
    use beale_contracts::swarm_worker::ExecutionIdentity;
    use beale_storage::swarm_state::plugin_resource_directory;
    let f = Fixture::new();
    let bytes = b"Synthetic initial guidance";
    let file = beale_contracts::swarm_plugins::ResourceFile {
        path: "PLUGIN.md".into(),
        sha256: hash(bytes),
        size: bytes.len() as u64,
        executable: false,
    };
    let mut execution = ExecutionIdentity {
        workspace_id: "workspace-example".into(),
        scan_id: "scan-example".into(),
        agent_id: "agent-example".into(),
        attempt_id: "attempt-example".into(),
        generation: 1,
    };
    let first = format!(
        "{}/PLUGIN.md",
        plugin_resource_directory(&execution, &"a".repeat(64), "example-plugin").unwrap()
    );
    materialize_plugin_resource(&f.source, &first, &file, bytes).unwrap();
    fs::write(f.source.join(&first), b"Private edit").unwrap();
    execution.attempt_id = "resumed-example".into();
    execution.generation = 2;
    let resumed = format!(
        "{}/PLUGIN.md",
        plugin_resource_directory(&execution, &"a".repeat(64), "example-plugin").unwrap()
    );
    assert_ne!(first, resumed);
    materialize_plugin_resource(&f.source, &resumed, &file, bytes).unwrap();
    assert_eq!(fs::read(f.source.join(first)).unwrap(), b"Private edit");
    assert_eq!(fs::read(f.source.join(resumed)).unwrap(), bytes);
}

#[test]
fn recovery_capture_reader_ignores_partial_staging_and_rejects_changed_metadata() {
    let f = Fixture::new();
    let recovery = f.root.join("capture-recovery");
    assert!(
        captured_worker_publications(&recovery)
            .unwrap()
            .next()
            .is_none()
    );
    fs::write(f.source.join("candidate.txt"), "Synthetic candidate").unwrap();
    let (upload, content) =
        stage_worker_artifact(&f.source, &recovery, "capture-example", "candidate.txt").unwrap();
    let staging = recovery
        .join("publications")
        .join(format!("staging-{}", Uuid::new_v4()));
    fs::create_dir(&staging).unwrap();
    fs::write(staging.join("content"), "Incomplete synthetic capture").unwrap();
    let captures = captured_worker_publications(&recovery)
        .unwrap()
        .collect::<anyhow::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(captures.len(), 1);
    assert_eq!(captures[0].upload(), &upload);
    let metadata = content.parent().unwrap().join("metadata.json");
    let mut forged = serde_json::to_value(&upload).unwrap();
    forged["assignment"] = json!("assignment-other");
    fs::write(&metadata, serde_json::to_vec(&forged).unwrap()).unwrap();
    assert!(
        captured_worker_publications(&recovery)
            .unwrap()
            .collect::<anyhow::Result<Vec<_>>>()
            .is_err()
    );
    fs::write(metadata, serde_json::to_vec(&upload).unwrap()).unwrap();
    fs::write(content, "Changed synthetic bytes").unwrap();
    assert!(
        captured_worker_publications(&recovery)
            .unwrap()
            .collect::<anyhow::Result<Vec<_>>>()
            .is_err()
    );
}
