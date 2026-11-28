use super::*;
use beale_contracts::swarm_worker::ExecutionIdentity;

fn actor() -> ExecutionIdentity {
    ExecutionIdentity {
        workspace_id: "workspace-example".into(),
        scan_id: "scan-example".into(),
        agent_id: "agent-example".into(),
        attempt_id: "attempt-example".into(),
        generation: 1,
    }
}
fn download(f: &Fixture, receiver: &SnapshotReceiver, snapshot: &str, blob: Option<&str>) {
    let digest = blob.unwrap_or(snapshot);
    while !receiver.complete(digest, None).unwrap() {
        let request = SnapshotRead {
            snapshot: snapshot.into(),
            blob: blob.map(str::to_owned),
            offset: receiver.offset(digest).unwrap(),
        };
        let chunk = f.files.snapshot_chunk("scan-example", &request).unwrap();
        receiver.append(&request, &chunk).unwrap();
    }
}
fn capture(f: &Fixture) -> (String, String, Vec<u8>) {
    let bytes = vec![b'x'; SNAPSHOT_CHUNK_BYTES * 2 + 7];
    fs::write(f.source.join("seed.txt"), &bytes).unwrap();
    let digest = f
        .files
        .capture(
            "scan-example",
            &[SnapshotInput {
                source: f.source.clone(),
                destination: String::new(),
            }],
        )
        .unwrap();
    (digest, hash(&bytes), bytes)
}

#[test]
fn snapshot_transfer_resumes_verified_chunks_and_preserves_published_edits() {
    let f = Fixture::new();
    let (digest, blob, bytes) = capture(&f);
    let cache = f.root.join("receiver");
    let receiver = SnapshotReceiver::open(&cache).unwrap();
    assert!(SnapshotReceiver::open(&cache).is_err());
    download(&f, &receiver, &digest, None);
    let first = SnapshotRead {
        snapshot: digest.clone(),
        blob: Some(blob.clone()),
        offset: 0,
    };
    receiver
        .append(
            &first,
            &f.files.snapshot_chunk("scan-example", &first).unwrap(),
        )
        .unwrap();
    drop(receiver);
    let receiver = SnapshotReceiver::open(&cache).unwrap();
    assert_eq!(receiver.offset(&blob).unwrap(), SNAPSHOT_CHUNK_BYTES as u64);
    let next = SnapshotRead {
        offset: receiver.offset(&blob).unwrap(),
        ..first.clone()
    };
    let mut corrupted = f.files.snapshot_chunk("scan-example", &next).unwrap();
    corrupted.bytes[0] ^= 1;
    assert!(receiver.append(&next, &corrupted).is_err());
    assert_eq!(receiver.offset(&blob).unwrap(), next.offset);
    download(&f, &receiver, &digest, Some(&blob));
    let destination = f.root.join("worker");
    receiver.install(&digest, &actor(), &destination).unwrap();
    assert_eq!(fs::read(destination.join("seed.txt")).unwrap(), bytes);
    fs::write(destination.join("seed.txt"), "Retained synthetic edit").unwrap();
    receiver.install(&digest, &actor(), &destination).unwrap();
    assert_eq!(
        fs::read_to_string(destination.join("seed.txt")).unwrap(),
        "Retained synthetic edit"
    );
    let mut other = actor();
    other.generation = 2;
    assert!(receiver.install(&digest, &other, &destination).is_err());
    assert!(!cache.join("state.sqlite").exists());
    assert!(!cache.join("memory.sqlite").exists());
    fs::remove_dir_all(&destination).unwrap();
    assert!(receiver.install(&digest, &actor(), &destination).is_err());
    assert!(
        !destination.exists(),
        "missing retained work must not be silently reseeded"
    );
}

#[test]
fn retained_snapshot_receipt_allows_later_attempts_but_not_foreign_or_missing_storage() {
    let f = Fixture::new();
    let (digest, blob, _) = capture(&f);
    let receiver = SnapshotReceiver::open(&f.root.join("receiver")).unwrap();
    download(&f, &receiver, &digest, None);
    download(&f, &receiver, &digest, Some(&blob));
    let destination = f.root.join("worker");
    receiver.install(&digest, &actor(), &destination).unwrap();
    fs::write(destination.join("seed.txt"), "Retained synthetic edit").unwrap();
    let mut next = actor();
    next.attempt_id = "next-example".into();
    next.generation = 2;
    receiver.verify_retained(&next, &destination).unwrap();
    assert_eq!(
        fs::read_to_string(destination.join("seed.txt")).unwrap(),
        "Retained synthetic edit"
    );
    let mut foreign = next.clone();
    foreign.agent_id = "other-example".into();
    assert!(receiver.verify_retained(&foreign, &destination).is_err());
    fs::remove_file(destination.join(".beale/initial-snapshot")).unwrap();
    assert!(receiver.verify_retained(&next, &destination).is_err());
    assert!(destination.join("seed.txt").is_file());
}

#[test]
fn snapshot_transfer_scopes_objects_and_rejects_corrupt_content_and_existing_destinations() {
    let f = Fixture::new();
    let (digest, blob, _) = capture(&f);
    let receiver = SnapshotReceiver::open(&f.root.join("receiver")).unwrap();
    let mut request = SnapshotRead {
        snapshot: digest.clone(),
        blob: Some(hash(b"foreign-example")),
        offset: 0,
    };
    assert!(f.files.snapshot_chunk("scan-example", &request).is_err());
    request.blob = Some(blob.clone());
    request.snapshot = hash(b"different-snapshot");
    assert!(f.files.snapshot_chunk("scan-example", &request).is_err());
    request.snapshot = digest.clone();
    request.offset = u64::MAX;
    assert!(f.files.snapshot_chunk("scan-example", &request).is_err());
    download(&f, &receiver, &digest, None);
    download(&f, &receiver, &digest, Some(&blob));
    let destination = f.root.join("existing");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("operator.txt"), "Retained operator data").unwrap();
    assert!(receiver.install(&digest, &actor(), &destination).is_err());
    assert_eq!(
        fs::read_to_string(destination.join("operator.txt")).unwrap(),
        "Retained operator data"
    );
    fs::write(f.root.join("receiver").join(&blob), "Damaged cache bytes").unwrap();
    assert!(
        receiver
            .install(&digest, &actor(), &f.root.join("corrupt"))
            .is_err()
    );
    assert!(!f.root.join("corrupt").exists());
    assert!(!f.root.join("receiver/installation.json").exists());
}

#[test]
fn snapshot_publication_recovers_rename_and_acknowledgement_gaps_without_rewriting() {
    let f = Fixture::new();
    let (digest, blob, _) = capture(&f);
    let cache = f.root.join("receiver");
    let receiver = SnapshotReceiver::open(&cache).unwrap();
    download(&f, &receiver, &digest, None);
    download(&f, &receiver, &digest, Some(&blob));
    let destination = f.root.join("worker");
    receiver.install(&digest, &actor(), &destination).unwrap();
    let saved: serde_json::Value =
        serde_json::from_slice(&fs::read(cache.join("installation.json")).unwrap()).unwrap();
    // Simulate interruption after intent commit but before directory rename.
    fs::remove_file(cache.join("installed.json")).unwrap();
    fs::rename(
        &destination,
        f.root.join(saved["staging"].as_str().unwrap()),
    )
    .unwrap();
    let marker = f
        .root
        .join(saved["staging"].as_str().unwrap())
        .join(".beale/initial-snapshot");
    fs::write(&marker, "corrupted-marker").unwrap();
    assert!(receiver.install(&digest, &actor(), &destination).is_err());
    assert!(!destination.exists());
    fs::write(&marker, saved["marker"].as_str().unwrap()).unwrap();
    drop(receiver);
    let receiver = SnapshotReceiver::open(&cache).unwrap();
    receiver.install(&digest, &actor(), &destination).unwrap();
    // Simulate loss after publication but before the final acknowledgement.
    fs::remove_file(cache.join("installed.json")).unwrap();
    fs::write(destination.join("seed.txt"), "Retained candidate edit").unwrap();
    receiver.install(&digest, &actor(), &destination).unwrap();
    assert_eq!(
        fs::read_to_string(destination.join("seed.txt")).unwrap(),
        "Retained candidate edit"
    );
    assert!(cache.join("installed.json").exists());
}

#[cfg(unix)]
#[test]
fn snapshot_transfer_preserves_native_permissions_and_internal_links() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let f = Fixture::new();
    fs::write(f.source.join("seed.sh"), "synthetic content").unwrap();
    fs::set_permissions(f.source.join("seed.sh"), fs::Permissions::from_mode(0o751)).unwrap();
    symlink("seed.sh", f.source.join("alias.sh")).unwrap();
    let digest = f
        .files
        .capture(
            "scan-example",
            &[SnapshotInput {
                source: f.source.clone(),
                destination: String::new(),
            }],
        )
        .unwrap();
    let receiver = SnapshotReceiver::open(&f.root.join("receiver")).unwrap();
    download(&f, &receiver, &digest, None);
    download(&f, &receiver, &digest, Some(&hash(b"synthetic content")));
    let destination = f.root.join("worker");
    receiver.install(&digest, &actor(), &destination).unwrap();
    assert_eq!(
        fs::metadata(destination.join("seed.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o751
    );
    assert_eq!(
        fs::read_link(destination.join("alias.sh")).unwrap(),
        Path::new("seed.sh")
    );
}

#[test]
fn chunks_verify_only_the_served_blob_and_capture_releases_the_database() {
    let f = Fixture::new();
    // Capture runs a fault hook between its two passes. The hook uses the same
    // host database, which would deadlock if capture still held its lock.
    struct UsesDatabase(HostDatabase, std::sync::atomic::AtomicUsize);
    impl StorageFaults for UsesDatabase {
        fn check(&self, point: FaultPoint) -> anyhow::Result<()> {
            if point == FaultPoint::CaptureVerify {
                self.0.with_connection(|db| {
                    Ok(db.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))?)
                })?;
                self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            Ok(())
        }
    }
    let hook = Arc::new(UsesDatabase(
        f.db.clone(),
        std::sync::atomic::AtomicUsize::new(0),
    ));
    let files = f.files.clone().with_faults(hook.clone());
    let large = vec![b'x'; SNAPSHOT_CHUNK_BYTES * 2 + 7];
    fs::write(f.source.join("large.txt"), &large).unwrap();
    fs::write(f.source.join("small.txt"), b"synthetic small file").unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let source = f.source.clone();
    let worker = std::thread::spawn(move || {
        let digest = files.capture(
            "scan-example",
            &[SnapshotInput {
                source,
                destination: String::new(),
            }],
        );
        let _ = sender.send(digest.map_err(|e| e.to_string()));
    });
    let digest = receiver
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("capture must not hold the database lock while hashing")
        .unwrap();
    worker.join().unwrap();
    assert_eq!(hook.1.load(std::sync::atomic::Ordering::SeqCst), 1);
    let large_blob = hash(&large);
    let small_blob = hash(b"synthetic small file");
    // Corrupting an unrelated blob does not fail reads of another blob: a
    // chunk no longer re-hashes the whole snapshot.
    let small_path = f.root.join("host/swarm-files/blobs").join(&small_blob);
    let original_small = fs::read(&small_path).unwrap();
    fs::write(&small_path, b"Corrupted synthetic small").unwrap();
    for offset in [0, SNAPSHOT_CHUNK_BYTES as u64] {
        let request = SnapshotRead {
            snapshot: digest.clone(),
            blob: Some(large_blob.clone()),
            offset,
        };
        assert_eq!(
            f.files
                .snapshot_chunk("scan-example", &request)
                .unwrap()
                .request
                .offset,
            offset
        );
    }
    // The served blob itself is verified when its first chunk is read.
    let corrupt_first = SnapshotRead {
        snapshot: digest.clone(),
        blob: Some(small_blob.clone()),
        offset: 0,
    };
    assert!(
        f.files
            .snapshot_chunk("scan-example", &corrupt_first)
            .is_err()
    );
    // Materialization still rejects corrupt snapshot content.
    assert!(
        f.files
            .prepare_agent("scan-example", "agent-corrupt-example")
            .is_err()
    );
    fs::write(&small_path, original_small).unwrap();
    let workspace = f
        .files
        .prepare_agent("scan-example", "agent-corrupt-example")
        .unwrap();
    assert_eq!(fs::read(workspace.join("large.txt")).unwrap(), large);
    assert_eq!(
        f.files
            .prepare_agent("scan-example", "agent-corrupt-example")
            .unwrap(),
        workspace
    );
}
