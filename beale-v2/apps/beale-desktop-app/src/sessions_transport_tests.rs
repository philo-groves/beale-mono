use super::*;
use axum::{Router, body::Body, response::IntoResponse, routing::post};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

// Exercise real HTTP submission, delayed terminal frames, Stop retries, and UI retry notification.
#[tokio::test]
async fn replayed_stop_preserves_completion_and_retries_without_dropping_stream() {
    for failures in [0, 1, 3] {
        let control = SubmissionControl::default();
        let ready = Arc::new(tokio::sync::Semaphore::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let warnings = Arc::new(AtomicUsize::new(0));
        let detail = SessionDetail {
            summary: Some(SessionSummary {
                id: uuid::Uuid::new_v4().to_string(),
                workspace_id: uuid::Uuid::new_v4().to_string(),
                title: "Synthetic research".into(),
                last_message_at_ms: 1,
            }),
            provider: 1,
            method: 1,
            model: "example-model".into(),
            first_turn_number: 1,
            committed_turn_number: 1,
            ..Default::default()
        };
        let expected = detail.clone();
        let request = SubmitSessionTurnRequest {
            workspace_id: detail.summary.as_ref().unwrap().workspace_id.clone(),
            prompt: "Synthetic prompt".into(),
            provider: 1,
            method: 1,
            model: "example-model".into(),
            ..Default::default()
        };
        let turn = uuid::Uuid::new_v4().to_string();
        let frame = beale_contracts::session_stream_frame(&SessionStreamEvent {
            kind: SessionStreamKind::Completed as i32,
            detail: Some(detail),
            ..Default::default()
        })
        .unwrap();
        let app = Router::new()
            .route(
                SESSION_TURN_STREAM_PATH,
                post({
                    let ready = ready.clone();
                    let control = control.clone();
                    move || {
                        let ready = ready.clone();
                        let frame = frame.clone();
                        let control = control.clone();
                        async move {
                            assert!(!control.cancel(), "cancel while POST is in flight");
                            let frames = futures_util::stream::once(async move {
                                ready.acquire().await.unwrap().forget();
                                Ok::<_, std::io::Error>(frame)
                            });
                            (
                                [(CONTENT_TYPE, PROTOBUF_STREAM_CONTENT_TYPE)],
                                Body::from_stream(frames),
                            )
                                .into_response()
                        }
                    }
                }),
            )
            .route(
                &SESSION_TURN_STOP_PATH.replace("{id}", &turn),
                post({
                    let ready = ready.clone();
                    let calls = calls.clone();
                    move || {
                        let ready = ready.clone();
                        let calls = calls.clone();
                        async move {
                            let attempt = calls.fetch_add(1, Ordering::SeqCst);
                            if attempt < failures {
                                StatusCode::SERVICE_UNAVAILABLE
                            } else {
                                ready.add_permits(1);
                                if failures == 0 {
                                    StatusCode::NOT_FOUND
                                } else {
                                    StatusCode::NO_CONTENT
                                }
                            }
                        }
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = stream_client().unwrap();
        let retry_client = client.clone();
        let retry_turn = turn.clone();
        let warnings_seen = warnings.clone();
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            submit_authenticated_stream(
                &client,
                addr,
                request,
                &turn,
                control,
                "synthetic-token".into(),
                |_| {},
                move |error| {
                    assert!(error.contains("Stop can be retried"));
                    warnings_seen.fetch_add(1, Ordering::SeqCst);
                    let client = retry_client.clone();
                    let turn = retry_turn.clone();
                    tokio::spawn(async move {
                        stop_authenticated(&client, addr, &turn, "synthetic-token")
                            .await
                            .unwrap();
                    });
                },
            ),
        )
        .await
        .unwrap();
        server.abort();
        let _ = server.await;
        assert_eq!(result.unwrap(), expected);
        assert_eq!(calls.load(Ordering::SeqCst), failures + 1);
        assert_eq!(warnings.load(Ordering::SeqCst), usize::from(failures == 3));
    }
}
