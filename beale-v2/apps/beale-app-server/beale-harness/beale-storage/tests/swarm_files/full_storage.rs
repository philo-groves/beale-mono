use super::*;

#[test]
fn patch_and_test_references_publish_validate_and_preserve_exact_identity() {
    let f = Fixture::new();
    f.capture();
    fs::write(f.source.join("patch.txt"), "Synthetic patch").unwrap();
    fs::write(f.source.join("check.txt"), "Synthetic check").unwrap();
    let original =
        json!({"payload":{"patch_refs":["file:patch.txt"],"test_refs":["file:check.txt"]}});
    let base = artifact("unused-example", b"");
    let mut accepted = original.clone();
    f.files
        .resolve_attachments(&f.source, "patch-example", &base, &mut accepted)
        .unwrap();
    let check = |record: &serde_json::Value| {
        f.db.with_connection(|db| {
            validate_record_artifacts(db, "scan-example", "workspace-example", record)
        })
    };
    check(&accepted).unwrap();
    let mut retry = original;
    f.reopen()
        .resolve_attachments(&f.source, "patch-example", &base, &mut retry)
        .unwrap();
    assert_eq!(retry, accepted);
    assert_ne!(
        accepted["payload"]["patch_refs"],
        accepted["payload"]["test_refs"]
    );
    for slot in ["patch_refs", "test_refs"] {
        let id = accepted["payload"][slot][0].as_str().unwrap();
        assert!(!id.starts_with("file:"));
        let metadata = f.files.artifact("workspace-example", id).unwrap();
        assert_eq!(metadata.producer, base.producer);
        for invalid in [
            json!(["artifact-missing"]),
            json!(["file:missing.txt"]),
            json!([7]),
            json!("not-an-array"),
        ] {
            let mut rejected = accepted.clone();
            rejected["payload"][slot] = invalid;
            let before = rejected.clone();
            assert!(check(&rejected).is_err());
            assert!(
                f.files
                    .resolve_attachments(&f.source, "rejected-example", &base, &mut rejected)
                    .is_err()
            );
            assert_eq!(rejected, before);
        }
    }
}

#[test]
fn sqlite_full_keeps_accepted_bytes_and_retries_canonical_record_identity() {
    let f = Fixture::new();
    fs::write(f.source.join("result.txt"), "Synthetic evidence").unwrap();
    let original = json!({"artifact_refs":["file:result.txt"],"summary":"x".repeat(64*1024)});
    let mut args = original.clone();
    f.files
        .resolve_attachments(
            &f.source,
            "record-example",
            &artifact("unused-example", b""),
            &mut args,
        )
        .unwrap();
    f.db.with_connection(|db| {
        db.execute_batch("CREATE TABLE example_records(id TEXT PRIMARY KEY, document TEXT)")?;
        let pages: i64 = db.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        db.pragma_update(None, "max_page_count", pages)?;
        Ok(())
    })
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
    let error = commit(&args).unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<beale_storage::rusqlite::Error>()
            .unwrap()
            .sqlite_error_code(),
        Some(beale_storage::rusqlite::ErrorCode::DiskFull)
    );
    f.db.with_connection(|db| {
        assert_eq!(
            db.query_row("SELECT count(*) FROM example_records", [], |r| r
                .get::<_, i64>(0))?,
            0
        );
        db.pragma_update(None, "max_page_count", 1_000_000)?;
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
    f.files
        .artifact(
            "workspace-example",
            args["artifact_refs"][0].as_str().unwrap(),
        )
        .unwrap();
}

// Run only inside a disposable Linux container with --tmpfs /pressure:rw,size=1m.
// Filling is strictly bounded even if the caller supplies an incorrect filesystem.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires disposable 1 MiB tmpfs at /pressure"]
fn enospc_preserves_artifact_and_worker_checkpoint_until_same_operation_retry() {
    use beale_contracts::swarm_worker::ExecutionIdentity;
    use beale_storage::worker_spool::WorkerSpool;
    use std::io::Write;
    let mounts = fs::read_to_string("/proc/mounts").unwrap();
    assert!(
        mounts.lines().any(|l| {
            let fields: Vec<_> = l.split_whitespace().collect();
            fields.get(1) == Some(&"/pressure") && fields.get(2) == Some(&"tmpfs")
        }),
        "a dedicated tmpfs is required"
    );
    let f = Fixture::new();
    let pressure = Path::new("/pressure").join(Uuid::new_v4().to_string());
    fs::create_dir(&pressure).unwrap();
    let identity = ExecutionIdentity {
        workspace_id: "workspace-example".into(),
        scan_id: "scan-example".into(),
        agent_id: "agent-example".into(),
        attempt_id: "attempt-example".into(),
        generation: 1,
    };
    let spool = WorkerSpool::open(&pressure.join("spool"), identity.clone()).unwrap();
    spool
        .checkpoint("checkpoint-one", 0, &json!({"saved":1}))
        .unwrap();
    // Canonical database stays outside the full artifact filesystem.
    let files = ResearchFiles::open(f.db.clone(), &pressure.join("artifacts")).unwrap();
    let bytes = vec![b'x'; 64 * 1024];
    let request = artifact("publication-example", &bytes);
    assert_eq!(files.begin_transfer(&request).unwrap(), 0);
    let filler_path = pressure.join("filler");
    let mut filler = fs::File::create(&filler_path).unwrap();
    let mut full = false;
    for _ in 0..512 {
        match filler.write_all(&[0; 4096]) {
            Ok(()) => (),
            Err(error) => {
                assert_eq!(error.raw_os_error(), Some(28));
                full = true;
                break;
            }
        }
    }
    assert!(
        full,
        "bounded fill did not reach ENOSPC; expected 1 MiB tmpfs"
    );
    drop(filler);
    let error = files.write_chunk(&request.id, 0, &bytes).unwrap_err();
    assert!(
        error.chain().any(|e| e
            .downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.raw_os_error() == Some(28))),
        "{error:#}"
    );
    assert!(files.artifact(&request.workspace, &request.id).is_err());
    let checkpoint = json!({"synthetic":"x".repeat(64*1024)});
    assert!(spool.checkpoint("checkpoint-two", 1, &checkpoint).is_err());
    assert_eq!(spool.latest_checkpoint().unwrap().unwrap().revision, 1);
    // An independent branch with separate storage can still publish.
    f.files
        .publish_file(&f.source, {
            fs::write(f.source.join("result.txt"), "unaffected").unwrap();
            artifact("unaffected-example", b"unaffected")
        })
        .unwrap();
    fs::remove_file(filler_path).unwrap();
    assert_eq!(files.begin_transfer(&request).unwrap(), 0);
    assert_eq!(
        files.write_chunk(&request.id, 0, &bytes).unwrap(),
        bytes.len() as u64
    );
    files.seal(&request.id).unwrap();
    assert_eq!(
        files.artifact(&request.workspace, &request.id).unwrap(),
        request
    );
    assert_eq!(
        spool
            .checkpoint("checkpoint-two", 1, &checkpoint)
            .unwrap()
            .revision,
        2
    );
    drop(spool);
    let reopened = WorkerSpool::open(&pressure.join("spool"), identity).unwrap();
    assert_eq!(
        reopened
            .checkpoint("checkpoint-two", 1, &checkpoint)
            .unwrap()
            .revision,
        2
    );
    drop(reopened);
    fs::remove_dir_all(pressure).unwrap();
}
