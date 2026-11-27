use beale_swarms::{BASIC_ID, restore_snapshot, swarm_definition};
use std::sync::Arc;

#[test]
fn saved_scans_reopen_after_more_than_1024_snapshot_revisions() {
    let installed = swarm_definition(BASIC_ID).unwrap();
    let raw = serde_json::to_string(&installed.snapshot).unwrap();
    let active = restore_snapshot(&raw).unwrap();
    let active_weak = Arc::downgrade(&active);
    let mut evicted = None;
    for index in 0..1100 {
        let mut snapshot = installed.snapshot.clone();
        snapshot.definition["name"] = serde_json::json!(format!("Example saved revision {index}"));
        snapshot.leader_rule.instruction = format!("Example saved coordinator rule {index}");
        let restored = restore_snapshot(&serde_json::to_string(&snapshot).unwrap()).unwrap();
        assert_eq!(restored.snapshot, snapshot);
        if index == 0 {
            evicted = Some(Arc::downgrade(&restored));
        }
    }
    assert!(evicted.unwrap().upgrade().is_none());
    assert_eq!(active.snapshot, installed.snapshot);
    assert_eq!(Arc::strong_count(&active), 1);
    let reopened = restore_snapshot(&raw).unwrap();
    assert!(!Arc::ptr_eq(&active, &reopened));
    assert_eq!(reopened.snapshot, active.snapshot);
    assert!(Arc::ptr_eq(&reopened, &restore_snapshot(&raw).unwrap()));
    drop(active);
    assert!(active_weak.upgrade().is_none());
    assert_eq!(
        swarm_definition(BASIC_ID).unwrap().snapshot,
        installed.snapshot
    );
}
