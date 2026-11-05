use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use beale_contracts::v1::{
    AutomationJob, AutomationRun, CancelAutomationRequest, ListAutomationJobsResponse,
};
use beale_contracts::v1::{SessionGraphConfig, SubmitSessionTurnRequest};
use beale_storage::{AutomationClaim, AutomationJobRow, AutomationRunRow};
use prost::Message;
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Semaphore;

use super::{
    SessionState,
    http::{authorized, checked_body, response},
    parse_id, run_turn,
};

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_millis().min(i64::MAX as u128) as i64)
}

pub(super) async fn run_worker(state: SessionState) {
    let permits = Arc::new(Semaphore::new(4));
    let mut timer = tokio::time::interval(Duration::from_secs(5));
    loop {
        timer.tick().await;
        let now = now_ms();
        if state.db.interrupt_automation_runs(now).is_err() {
            eprintln!("Beale automation recovery could not read host storage.");
            continue;
        }
        while let Ok(permit) = permits.clone().try_acquire_owned() {
            match state.db.claim_due_automation(now) {
                Ok(Some(claim)) => {
                    let state = state.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        execute_claim(state, claim).await;
                    });
                }
                Ok(None) => break,
                Err(_) => {
                    eprintln!("Beale automation scheduler could not claim a due job.");
                    break;
                }
            }
        }
    }
}

async fn execute_claim(state: SessionState, claim: AutomationClaim) {
    let graph_id = state
        .db
        .session_graph_id(&claim.job_id)
        .ok()
        .flatten()
        .unwrap_or_default();
    let graph_config = state
        .db
        .session_graph_config(&claim.job_id)
        .ok()
        .flatten()
        .and_then(|bytes| SessionGraphConfig::decode(bytes.as_slice()).ok())
        .unwrap_or_default();
    let mut main_seen = false;
    let graph_models = graph_config
        .models
        .into_iter()
        .filter(|item| {
            if item.node_id != "main" {
                true
            } else if main_seen {
                false
            } else {
                main_seen = true;
                true
            }
        })
        .collect();
    let request = SubmitSessionTurnRequest {
        workspace_id: claim.workspace_id,
        session_id: String::new(),
        provider: claim.provider,
        method: claim.method,
        model: claim.model,
        prompt: claim.prompt,
        reasoning_effort: claim.options.reasoning_effort,
        goal_mode: claim.options.goal_mode,
        goal_max_turns: claim.options.goal_max_turns,
        goal_max_duration_minutes: claim.options.goal_max_duration_minutes,
        repeat_preset: String::new(),
        graph_id,
        graph_models,
        graph_node_concurrency: graph_config.concurrency,
    };
    let heartbeat_state = state.clone();
    let heartbeat_id = claim.run_id.clone();
    let (lease_lost_tx, mut lease_lost_rx) = tokio::sync::oneshot::channel();
    let heartbeat = tokio::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_secs(10));
        timer.tick().await;
        loop {
            timer.tick().await;
            if !heartbeat_state
                .db
                .renew_automation_lease(&heartbeat_id, now_ms())
                .unwrap_or(false)
            {
                let _ = lease_lost_tx.send(());
                break;
            }
        }
    });
    let outcome = tokio::select! {
        result = run_turn(state.clone(), request, None) => Some(result),
        _ = &mut lease_lost_rx => None,
    };
    heartbeat.abort();
    let _ = heartbeat.await;
    let Some(outcome) = outcome else { return };
    let (session_id, status, error) = match outcome {
        Ok(detail) => match detail.summary {
            Some(summary) => (Some(summary.id), 2, ""),
            None => (None, 3, "The run completed without a saved session."),
        },
        Err(StatusCode::NOT_FOUND) => (None, 3, "The workspace is unavailable."),
        Err(StatusCode::CONFLICT) => (None, 3, "The saved model or feature is unavailable."),
        Err(StatusCode::BAD_REQUEST) => (None, 3, "The saved job configuration is invalid."),
        Err(StatusCode::BAD_GATEWAY) => (None, 3, "The provider could not complete this run."),
        Err(_) => (None, 3, "The run could not complete."),
    };
    if state
        .db
        .finish_automation_run(
            &claim.run_id,
            session_id.as_deref(),
            status,
            error,
            now_ms(),
        )
        .is_err()
    {
        eprintln!("Beale automation outcome could not be saved.");
    }
}

pub(super) async fn list(
    State(state): State<SessionState>,
    headers: HeaderMap,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let jobs = state
        .db
        .automation_jobs()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let runs = state
        .db
        .automation_runs()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(response(ListAutomationJobsResponse {
        jobs: jobs.into_iter().map(wire_job).collect(),
        runs: runs.into_iter().map(wire_run).collect(),
    }))
}

fn wire_job(job: AutomationJobRow) -> AutomationJob {
    AutomationJob {
        id: job.id,
        session_id: job.session_id,
        workspace_id: job.workspace_id,
        title: job.title,
        repeat_preset: job.repeat_preset,
        next_run_at_ms: job.next_run_at_ms.max(0) as u64,
        last_run_at_ms: job.last_run_at_ms.max(0) as u64,
        canceled: job.canceled,
    }
}

fn wire_run(run: AutomationRunRow) -> AutomationRun {
    AutomationRun {
        id: run.id,
        job_id: run.job_id,
        session_id: run.session_id.unwrap_or_default(),
        title: run.title,
        started_at_ms: run.started_at_ms.max(0) as u64,
        completed_at_ms: run.completed_at_ms.unwrap_or_default().max(0) as u64,
        status: run.status,
        error: run.error,
    }
}

pub(super) async fn cancel(
    State(state): State<SessionState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    if !authorized(&headers, &state.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let request: CancelAutomationRequest = checked_body(&headers, &body)?;
    parse_id(&request.id)?;
    let changed = state
        .db
        .cancel_automation(&request.id, request.series)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !changed {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(response(ListAutomationJobsResponse {
        jobs: state
            .db
            .automation_jobs()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .into_iter()
            .map(wire_job)
            .collect(),
        runs: state
            .db
            .automation_runs()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .into_iter()
            .map(wire_run)
            .collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use beale_contracts::{AUTOMATIONS_CANCEL_PATH, AUTOMATIONS_PATH};
    use beale_storage::{HostDatabase, SessionRow, TurnRow};
    use prost::Message;
    use tower::ServiceExt;
    use uuid::Uuid;

    #[tokio::test]
    async fn lists_and_cancels_a_saved_series() {
        let root =
            std::env::temp_dir().join(format!("beale-automation-example-{}", Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        let session = SessionRow {
            id: Uuid::new_v4().to_string(),
            workspace_id: Uuid::new_v4().to_string(),
            title: "Example scheduled session".to_owned(),
            created_at_ms: 1_800_000_000_000,
            last_message_at_ms: 1_800_000_000_000,
        };
        db.append_turn(
            &session,
            &TurnRow {
                number: 1,
                provider: 1,
                method: 1,
                model: "model-example".to_owned(),
                prompt: "Example prompt".to_owned(),
                answer: "Example answer".to_owned(),
                duration_ms: 1,
                internal_prompt: false,
            },
            true,
        )
        .unwrap();
        db.add_automation_job(&session, "day").unwrap();
        let app = crate::router(
            "example-token".to_owned(),
            beale_workspaces::WorkspaceStore::at(root.clone()).unwrap(),
        )
        .unwrap();
        let denied = Request::builder()
            .uri(AUTOMATIONS_PATH)
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(denied).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let list = Request::builder()
            .uri(AUTOMATIONS_PATH)
            .header("authorization", "Bearer example-token")
            .body(Body::empty())
            .unwrap();
        let reply = app.clone().oneshot(list).await.unwrap();
        let body = to_bytes(reply.into_body(), 4096).await.unwrap();
        let listing = ListAutomationJobsResponse::decode(body).unwrap();
        assert_eq!(listing.jobs.len(), 1);
        assert_eq!(listing.jobs[0].title, session.title);
        assert_eq!(listing.runs.len(), 1);
        assert_eq!(listing.runs[0].session_id, session.id);
        assert_eq!(listing.runs[0].status, 2);
        let cancel = Request::builder()
            .method("POST")
            .uri(AUTOMATIONS_CANCEL_PATH)
            .header("authorization", "Bearer example-token")
            .header("content-type", beale_contracts::PROTOBUF_CONTENT_TYPE)
            .body(Body::from(
                CancelAutomationRequest {
                    id: session.id.clone(),
                    series: true,
                }
                .encode_to_vec(),
            ))
            .unwrap();
        let reply = app.oneshot(cancel).await.unwrap();
        assert_eq!(reply.status(), StatusCode::OK);
        let body = to_bytes(reply.into_body(), 4096).await.unwrap();
        assert!(ListAutomationJobsResponse::decode(body).unwrap().jobs[0].canceled);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn claimed_run_records_an_unavailable_workspace_failure() {
        let root =
            std::env::temp_dir().join(format!("beale-automation-example-{}", Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        let session = SessionRow {
            id: Uuid::new_v4().to_string(),
            workspace_id: Uuid::new_v4().to_string(),
            title: "Example scheduled session".to_owned(),
            created_at_ms: 1_800_000_000_000,
            last_message_at_ms: 1_800_000_000_000,
        };
        db.append_turn(
            &session,
            &TurnRow {
                number: 1,
                provider: 1,
                method: 1,
                model: "model-example".to_owned(),
                prompt: "Example prompt".to_owned(),
                answer: "Example answer".to_owned(),
                duration_ms: 1,
                internal_prompt: false,
            },
            true,
        )
        .unwrap();
        db.add_automation_job(&session, "minute").unwrap();
        let claim = db.claim_due_automation(1_800_000_060_000).unwrap().unwrap();
        let state = super::super::build_state(
            String::new(),
            beale_workspaces::WorkspaceStore::at(root.clone()).unwrap(),
        )
        .unwrap();
        execute_claim(state, claim).await;
        let run = db.automation_runs().unwrap().remove(0);
        assert_eq!(run.status, 3);
        assert_eq!(run.error, "The workspace is unavailable.");
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
}
