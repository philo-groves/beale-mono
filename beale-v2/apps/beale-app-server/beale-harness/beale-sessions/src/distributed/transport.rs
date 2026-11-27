use beale_contracts::{
    swarm_worker::{WORKER_PATH, WorkerReply},
    swarms::{MAX_DOCUMENT_BYTES, WorkerRequest},
};
use reqwest::{
    Client, Url,
    header::{AUTHORIZATION, HeaderValue},
};
use std::{fmt, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportFailure {
    Retryable,
    Rejected,
    Protocol,
}

/// Bounded messages intentionally omit URLs, response bodies, headers and secrets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransportError {
    pub kind: TransportFailure,
    pub message: &'static str,
}
impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)
    }
}
impl std::error::Error for TransportError {}

fn error(kind: TransportFailure, message: &'static str) -> TransportError {
    TransportError { kind, message }
}

/// A worker knows only its scoped bearer and the designated leader. This client
/// never loads guest provider configuration, host pairing credentials or proxies.
/// Retries are owned by the supervisor and retain the journal's operation ID.
#[derive(Clone)]
pub struct WorkerTransport {
    client: Client,
    endpoint: Url,
    bearer: HeaderValue,
}
impl WorkerTransport {
    pub fn new(leader: &str, bearer: &str) -> Result<Self, TransportError> {
        let invalid = || {
            error(
                TransportFailure::Protocol,
                "invalid worker leader configuration",
            )
        };
        let mut endpoint = Url::parse(leader).map_err(|_| invalid())?;
        if !matches!(endpoint.scheme(), "http" | "https")
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || !matches!(endpoint.path(), "" | "/")
            || bearer.len() != 64
            || !bearer.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(invalid());
        }
        endpoint.set_path(WORKER_PATH);
        let mut bearer =
            HeaderValue::from_str(&format!("Bearer {bearer}")).map_err(|_| invalid())?;
        bearer.set_sensitive(true);
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| invalid())?;
        Ok(Self {
            client,
            endpoint,
            bearer,
        })
    }

    pub async fn send(
        &self,
        request: &WorkerRequest,
        timeout: Duration,
    ) -> Result<WorkerReply, TransportError> {
        if timeout.is_zero() || timeout > Duration::from_secs(300) {
            return Err(error(
                TransportFailure::Protocol,
                "invalid worker request deadline",
            ));
        }
        self.send_inner(request, Some(timeout)).await
    }
    /// The supervisor renews the lease independently and cancels this future on
    /// lease loss or stop. Pacing is not a transport or provider failure deadline.
    pub(super) async fn send_waiting(
        &self,
        request: &WorkerRequest,
    ) -> Result<WorkerReply, TransportError> {
        self.send_inner(request, None).await
    }
    async fn send_inner(
        &self,
        request: &WorkerRequest,
        timeout: Option<Duration>,
    ) -> Result<WorkerReply, TransportError> {
        request
            .validate()
            .map_err(|_| error(TransportFailure::Protocol, "invalid worker request"))?;
        let mut builder = self
            .client
            .post(self.endpoint.clone())
            .header(AUTHORIZATION, self.bearer.clone())
            .json(request);
        if let Some(timeout) = timeout {
            builder = builder.timeout(timeout);
        }
        let mut response = builder
            .send()
            .await
            .map_err(|_| error(TransportFailure::Retryable, "leader connection interrupted"))?;
        let status = response.status();
        if !status.is_success() {
            let kind = if status.is_server_error() || matches!(status.as_u16(), 408 | 429) {
                TransportFailure::Retryable
            } else {
                TransportFailure::Rejected
            };
            return Err(error(kind, "leader did not acknowledge worker operation"));
        }
        if response
            .content_length()
            .is_some_and(|size| size > MAX_DOCUMENT_BYTES as u64)
        {
            return Err(error(TransportFailure::Protocol, "worker reply too large"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| error(TransportFailure::Retryable, "leader reply interrupted"))?
        {
            if chunk.len() > MAX_DOCUMENT_BYTES.saturating_sub(bytes.len()) {
                return Err(error(TransportFailure::Protocol, "worker reply too large"));
            }
            bytes.extend_from_slice(&chunk);
        }
        let reply: WorkerReply = serde_json::from_slice(&bytes)
            .map_err(|_| error(TransportFailure::Protocol, "invalid worker reply"))?;
        reply.validate_for(request).map_err(|_| {
            error(
                TransportFailure::Protocol,
                "worker reply does not match request",
            )
        })?;
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::State,
        http::{HeaderMap, StatusCode},
        routing::post,
    };
    use beale_contracts::swarms::{MutationAcknowledgement, VERSION, WorkerOperation};
    use serde_json::json;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::net::TcpListener;

    fn request() -> WorkerRequest {
        WorkerRequest {
            version: VERSION,
            workspace_id: "workspace-example".into(),
            scan_id: "scan-example".into(),
            agent_id: "agent-example".into(),
            attempt_id: "attempt-example".into(),
            generation: 1,
            operation_id: "operation-example".into(),
            operation: WorkerOperation::Canonical {
                tool: "findings_create".into(),
                arguments: json!({"title":"Synthetic candidate"}),
            },
        }
    }
    fn reply(request: &WorkerRequest) -> WorkerReply {
        WorkerReply::Acknowledged {
            acknowledgement: MutationAcknowledgement {
                version: VERSION,
                operation_id: request.operation_id.clone(),
                result: json!({"accepted":true}),
            },
        }
    }
    async fn server(app: Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (url, task)
    }

    #[tokio::test]
    async fn pacing_wait_has_no_transport_deadline_and_heartbeats_stay_live() {
        async fn handler(Json(request): Json<WorkerRequest>) -> Json<WorkerReply> {
            if matches!(request.operation, WorkerOperation::Heartbeat {}) {
                return Json(WorkerReply::Lease {
                    grant: beale_contracts::swarm_worker::LeaseGrant {
                        identity: beale_contracts::swarm_worker::ExecutionIdentity {
                            workspace_id: request.workspace_id,
                            scan_id: request.scan_id,
                            agent_id: request.agent_id,
                            attempt_id: request.attempt_id,
                            generation: request.generation,
                        },
                        request_id: request.operation_id,
                        valid_for_ms: 30000,
                    },
                });
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
            Json(reply(&request))
        }
        let (url, server) = server(Router::new().route(WORKER_PATH, post(handler))).await;
        let client = WorkerTransport::new(&url, &"a".repeat(64)).unwrap();
        assert!(
            client
                .send(&request(), Duration::from_millis(20))
                .await
                .is_err()
        );
        let copy = client.clone();
        let waiting = tokio::spawn(async move { copy.send_waiting(&request()).await });
        tokio::time::sleep(Duration::from_millis(25)).await;
        let mut heartbeat = request();
        heartbeat.operation = WorkerOperation::Heartbeat {};
        heartbeat.operation_id = "heartbeat-example".into();
        assert!(matches!(
            client
                .send(&heartbeat, Duration::from_secs(1))
                .await
                .unwrap(),
            WorkerReply::Lease { .. }
        ));
        assert!(!waiting.is_finished());
        assert_eq!(waiting.await.unwrap().unwrap(), reply(&request()));
        let copy = client.clone();
        let canceled = tokio::spawn(async move { copy.send_waiting(&request()).await });
        tokio::time::sleep(Duration::from_millis(10)).await;
        canceled.abort();
        assert!(canceled.await.unwrap_err().is_cancelled());
        server.abort();
    }

    #[tokio::test]
    async fn transport_preserves_scoped_envelope_and_retries_only_when_caller_requests() {
        async fn handler(
            State(count): State<Arc<AtomicUsize>>,
            headers: HeaderMap,
            Json(value): Json<WorkerRequest>,
        ) -> Result<Json<WorkerReply>, StatusCode> {
            assert_eq!(headers[AUTHORIZATION], format!("Bearer {}", "a".repeat(64)));
            assert_eq!(value, request());
            if count.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(StatusCode::SERVICE_UNAVAILABLE);
            }
            Ok(Json(reply(&value)))
        }
        let count = Arc::new(AtomicUsize::new(0));
        let (url, task) = server(
            Router::new()
                .route(WORKER_PATH, post(handler))
                .with_state(count.clone()),
        )
        .await;
        let client = WorkerTransport::new(&url, &"a".repeat(64)).unwrap();
        assert_eq!(
            client
                .send(&request(), Duration::from_secs(2))
                .await
                .unwrap_err()
                .kind,
            TransportFailure::Retryable
        );
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(
            client
                .send(&request(), Duration::from_secs(2))
                .await
                .unwrap(),
            reply(&request())
        );
        assert_eq!(count.load(Ordering::SeqCst), 2);
        task.abort();
    }

    #[tokio::test]
    async fn transport_rejects_redirects_foreign_replies_and_oversized_chunked_bodies() {
        use axum::{
            body::Body,
            response::{IntoResponse, Response},
        };
        use futures_util::stream;
        async fn handler(State(mode): State<u8>) -> Response {
            match mode {
                0 => (
                    StatusCode::TEMPORARY_REDIRECT,
                    [("location", "http://example.test/collect")],
                )
                    .into_response(),
                1 => {
                    let mut request = request();
                    request.operation_id = "other-operation".into();
                    Json(reply(&request)).into_response()
                }
                2 => {
                    let chunks = (0..17).map(|_| Ok::<_, std::io::Error>(vec![b'x'; 65536]));
                    Response::new(Body::from_stream(stream::iter(chunks)))
                }
                _ => (StatusCode::UNAUTHORIZED, "synthetic private diagnostic").into_response(),
            }
        }
        for mode in 0..4 {
            let (url, task) = server(
                Router::new()
                    .route(WORKER_PATH, post(handler))
                    .with_state(mode),
            )
            .await;
            let client = WorkerTransport::new(&url, &"a".repeat(64)).unwrap();
            let error = client
                .send(&request(), Duration::from_secs(2))
                .await
                .unwrap_err();
            assert_eq!(
                error.kind,
                if matches!(mode, 0 | 3) {
                    TransportFailure::Rejected
                } else {
                    TransportFailure::Protocol
                }
            );
            assert!(!format!("{error:?}").contains("private diagnostic"));
            task.abort();
        }
    }

    #[test]
    fn worker_configuration_rejects_embedded_credentials_and_alternate_api_paths() {
        for endpoint in [
            "file:///example",
            "http://user:secret@example.test",
            "http://example.test/admin",
            "http://example.test?token=example",
            "http://example.test/#fragment",
        ] {
            assert!(WorkerTransport::new(endpoint, &"a".repeat(64)).is_err());
        }
        assert!(WorkerTransport::new("http://example.test", "host-control-token").is_err());
        assert!(WorkerTransport::new("http://example.test", &"a".repeat(64)).is_ok());
    }
}
