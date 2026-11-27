use beale_contracts::{swarm_files::*, swarm_host::HostWorkerLaunch};

#[test]
fn snapshot_read_contract_rejects_paths_foreign_replies_and_invalid_bounds() {
    let read = SnapshotRead {
        snapshot: "a".repeat(64),
        blob: None,
        offset: 0,
    };
    read.validate().unwrap();
    let chunk = SnapshotChunk {
        request: read.clone(),
        total: 3,
        bytes: vec![1, 2, 3],
        sha256: "b".repeat(64),
    };
    chunk.validate_for(&read).unwrap();
    let mut changed = read.clone();
    changed.snapshot = "../outside".into();
    assert!(changed.validate().is_err());
    changed = read.clone();
    changed.blob = Some("C:/host-file".into());
    assert!(changed.validate().is_err());
    changed = read.clone();
    changed.offset = u64::MAX;
    assert!(changed.validate().is_err());
    changed = read.clone();
    changed.snapshot = "c".repeat(64);
    assert!(chunk.validate_for(&changed).is_err());
    for total in [2, 4, SNAPSHOT_MANIFEST_BYTES + 1, u64::MAX] {
        assert!(
            SnapshotChunk {
                total,
                ..chunk.clone()
            }
            .validate_for(&read)
            .is_err()
        );
    }
    let mut extra = serde_json::to_value(&read).unwrap();
    extra["path"] = "outside".into();
    assert!(serde_json::from_value::<SnapshotRead>(extra).is_err());
}

#[test]
fn peer_snapshot_selection_is_a_digest_and_chunk_lengths_cannot_skip_content() {
    let mut launch = HostWorkerLaunch {
        leader_origin: "https://leader.example.ts.net".into(),
        worker_bearer: "a".repeat(64),
        snapshot: Some("b".repeat(64)),
    };
    launch.validate().unwrap();
    launch.snapshot = Some("arbitrary-source".into());
    assert!(launch.validate().is_err());
    let request = SnapshotRead {
        snapshot: "a".repeat(64),
        blob: Some("b".repeat(64)),
        offset: 3,
    };
    let mut chunk = SnapshotChunk {
        request: request.clone(),
        total: 3,
        bytes: vec![],
        sha256: "c".repeat(64),
    };
    chunk.validate_for(&request).unwrap();
    chunk.total = 2;
    assert!(chunk.validate_for(&request).is_err());
    chunk.total = 4;
    assert!(chunk.validate_for(&request).is_err());
    chunk.bytes.push(1);
    chunk.validate_for(&request).unwrap();
}
