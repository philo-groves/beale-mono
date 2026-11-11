use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::SessionState;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}

pub(super) async fn run(state: SessionState) {
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    loop {
        interval.tick().await;
        let job = match state.db.next_memory_job(now_ms()) {
            Ok(Some(job)) => job,
            Ok(None) => continue,
            Err(error) => {
                eprintln!("memory queue read failed: {error:#}");
                continue;
            }
        };
        match super::memories_feature_enabled(&state.db, &job.workspace_id) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(_) => continue,
        }
        let result = match beale_memories::MemoryStore::open(state.workspaces.root()) {
            Ok(store) => {
                beale_memories::process_job(
                    &store,
                    &state.db,
                    &job,
                    &state.provider,
                    &state.credentials,
                )
                .await
            }
            Err(error) => Err(error),
        };
        match result {
            Ok(()) => {
                if let Err(error) = state.db.finish_memory_job(job.id) {
                    eprintln!("memory queue acknowledgment failed: {error:#}");
                }
            }
            Err(error) => {
                eprintln!("memory job {} failed: {error:#}", job.id);
                if let Err(error) = state.db.fail_memory_job(job.id, now_ms()) {
                    eprintln!("memory retry update failed: {error:#}");
                }
            }
        }
    }
}
