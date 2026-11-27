use super::*;
use beale_contracts::swarms::*;
use std::sync::{
    Arc,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};

struct Temp(std::path::PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let path = self.0.canonicalize().unwrap();
        assert!(path.starts_with(std::env::temp_dir().canonicalize().unwrap()));
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("beale-pacing-test-")
        );
        let _ = std::fs::remove_dir_all(path);
    }
}
struct Fixture {
    db: HostDatabase,
    store: AccountingStore,
    binding: AccountBinding,
    root: Temp,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("beale-pacing-test-{}", uuid::Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        let store = AccountingStore::open(db.clone()).unwrap();
        let binding = AccountBinding {
            group_id: uuid::Uuid::new_v4().to_string(),
            provider_id: "provider-2".into(),
            owner_host_id: store.host_id().unwrap(),
            generation: 1,
            participants: vec![store.host_id().unwrap()],
            limited_identity: false,
        };
        store
            .bind(
                &route_key(Provider::Xai, AuthMode::ApiKey, "example"),
                binding.clone(),
            )
            .unwrap();
        Self {
            db,
            store,
            binding,
            root: Temp(root),
        }
    }
    fn scan(&self, id: &str, target: Option<PacingTarget>) {
        SwarmStore::open(self.db.clone())
            .unwrap()
            .create_scan(
                ScanConfig {
                    version: VERSION,
                    scan_id: id.into(),
                    workspace_id: "workspace-example".into(),
                    mode: ScanMode::Local {},
                    authorized_scope: "Synthetic accounting test.".into(),
                    enabled_tools: vec![],
                    provider_routes: vec![ProviderRoute {
                        id: "route-example".into(),
                        provider_id: "provider-2".into(),
                        model: "example".into(),
                        budget_group_id: self.binding.group_id.clone(),
                    }],
                    destinations: vec![],
                    pacing: target.into_iter().collect(),
                    retention: RetentionPolicy::default(),
                },
                true,
                true,
            )
            .unwrap();
    }
    fn target(&self) -> PacingTarget {
        PacingTarget {
            budget_group_id: self.binding.group_id.clone(),
            interval_seconds: 7200,
            amount: PacingAmount::Currency {
                currency: "USD".into(),
                micros: 6000,
            },
        }
    }
    fn request(&self, id: &str, scan: &str) -> Admission {
        Admission {
            id: id.into(),
            group_id: self.binding.group_id.clone(),
            generation: 1,
            participant_id: self.store.host_id().unwrap(),
            scan_id: scan.into(),
            target: Some(self.target()),
            class: CallClass::Research,
        }
    }
}
fn step() -> ProviderStep {
    ProviderStep {
        text: "synthetic result".into(),
        calls: vec![],
        continuation: vec![],
        response_id: None,
        reasoning_summaries: vec![],
        usage: Some(crate::TokenUsage {
            input_tokens: 100,
            output_tokens: 10,
            cache_read_tokens: Some(20),
        }),
    }
}

#[test]
fn pricing_labels_estimates_and_missing_usage_or_wrong_units_stays_unknown() {
    let price = Price {
        model: "example".into(),
        currency_or_credit: "USD".into(),
        input_micros: 2_000_000,
        output_micros: 4_000_000,
        cached_input_micros: 1_000_000,
    };
    let target = PacingTarget {
        budget_group_id: "example".into(),
        interval_seconds: 60,
        amount: PacingAmount::Currency {
            currency: "USD".into(),
            micros: 6000,
        },
    };
    let result = price_charge(
        "attempt",
        step().usage.as_ref(),
        Some(&price),
        Some(&target),
    );
    assert_eq!(result.amount, Some(220));
    assert_eq!(result.quality, CostQuality::Estimated);
    let unknown = price_charge("attempt", step().usage.as_ref(), Some(&price), None);
    assert_eq!(unknown.amount, None);
    assert_eq!(unknown.quality, CostQuality::Unknown);
    assert!(unknown.uncertain);
    assert_eq!(unknown.input_tokens, result.input_tokens);
    assert_eq!(
        price_charge("attempt", step().usage.as_ref(), None, None).amount,
        None
    );
    assert_eq!(
        price_charge("attempt", None, Some(&price), None).quality,
        CostQuality::Unknown
    );
    let target = PacingTarget {
        budget_group_id: "example".into(),
        interval_seconds: 60,
        amount: PacingAmount::Credits { micros: 100 },
    };
    assert_eq!(
        price_charge(
            "attempt",
            step().usage.as_ref(),
            Some(&price),
            Some(&target)
        )
        .amount,
        None
    );
}

#[tokio::test(start_paused = true)]
async fn long_admission_wait_is_outside_review_timeout_and_cancellation_is_free() {
    let f = Fixture::new();
    f.scan("scan-a", Some(f.target()));
    f.store.admit(&f.request("prior", "scan-a"), 0).unwrap();
    let clock = Arc::new(AtomicU64::new(1));
    let calls = Arc::new(AtomicUsize::new(0));
    let db = f.db.clone();
    let observed = calls.clone();
    let task = tokio::spawn(TEST_CLOCK.scope(
        clock.clone(),
        scope(db, "scan-a", CallClass::Supporting, async move {
            dispatch(Provider::Xai, AuthMode::ApiKey, "example", async move {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(step())
            })
            .await
        }),
    ));
    tokio::task::yield_now().await;
    clock.store(90000, Ordering::SeqCst);
    tokio::time::advance(Duration::from_secs(90)).await;
    tokio::task::yield_now().await;
    assert!(!task.is_finished());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    clock.store(120000, Ordering::SeqCst);
    tokio::time::advance(Duration::from_secs(30)).await;
    assert_eq!(task.await.unwrap().unwrap().text, "synthetic result");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let db = f.db.clone();
    let observed = calls.clone();
    let queued = tokio::spawn(TEST_CLOCK.scope(
        clock.clone(),
        scope(db, "scan-a", CallClass::Research, async move {
            dispatch(Provider::Xai, AuthMode::ApiKey, "example", async move {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(step())
            })
            .await
        }),
    ));
    tokio::task::yield_now().await;
    queued.abort();
    let _ = queued.await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    flush(&f.db).await.unwrap();
    let saved = f.store.snapshot(&f.binding.group_id).unwrap();
    assert_eq!(
        saved
            .reservations
            .iter()
            .filter(|r| r.charge.is_some())
            .count(),
        1
    );
    assert!(saved.reservations.iter().any(|r| r.canceled && !r.admitted));
}

#[tokio::test]
async fn admitted_call_finishes_while_other_admission_and_host_operations_continue() {
    let f = Fixture::new();
    f.scan("scan-a", None);
    let (db, release) = (f.db.clone(), Arc::new(tokio::sync::Notify::new()));
    let finish = release.clone();
    let task = tokio::spawn(scope(db, "scan-a", CallClass::Research, async move {
        dispatch(Provider::Xai, AuthMode::ApiKey, "example", async move {
            finish.notified().await;
            Ok(step())
        })
        .await
    }));
    for _ in 0..100 {
        if !f.store.outbox().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    // Admission owns no shared SQLite/orchestration lock during provider work.
    f.store
        .warn(
            "scan-a",
            "Synthetic concurrent heartbeat/publication marker.",
        )
        .unwrap();
    assert!(
        f.store
            .admit(
                &Admission {
                    target: None,
                    ..f.request("other", "scan-b")
                },
                now_ms()
            )
            .unwrap()
            .admitted
    );
    assert!(!task.is_finished());
    release.notify_one();
    assert!(task.await.unwrap().is_ok());
    flush(&f.db).await.unwrap();
    let state = f.store.snapshot(&f.binding.group_id).unwrap();
    assert!(state.reservations.iter().any(|r| r.charge.is_some()));
    assert!(
        f.store
            .warnings("scan-a")
            .unwrap()
            .iter()
            .any(|w| w.contains("unknown, not zero"))
    );
}

#[tokio::test]
async fn physical_retries_and_lost_research_results_have_durable_distinct_charges() {
    let f = Fixture::new();
    let mut target = f.target();
    target.interval_seconds = 1;
    f.scan("scan-a", Some(target));
    f.store
        .price(
            &f.binding.group_id,
            Price {
                model: "example".into(),
                currency_or_credit: "USD".into(),
                input_micros: 1_000_000,
                output_micros: 1_000_000,
                cached_input_micros: 1_000_000,
            },
        )
        .unwrap();
    scope(f.db.clone(), "scan-a", CallClass::Supporting, async {
        assert!(
            dispatch(Provider::Xai, AuthMode::ApiKey, "example", async {
                anyhow::bail!("synthetic lost response")
            })
            .await
            .is_err()
        );
        let _discarded = dispatch(Provider::Xai, AuthMode::ApiKey, "example", async {
            Ok(step())
        })
        .await
        .unwrap();
    })
    .await;
    flush(&f.db).await.unwrap();
    flush(&f.db).await.unwrap();
    let state = f.store.snapshot(&f.binding.group_id).unwrap();
    assert_eq!(state.reservations.len(), 2);
    assert_ne!(
        state.reservations[0].request.id,
        state.reservations[1].request.id
    );
    assert_eq!(
        state.reservations[0].charge.as_ref().unwrap().quality,
        CostQuality::Unknown
    );
    assert_eq!(
        state.reservations[1].charge.as_ref().unwrap().amount,
        Some(110)
    );
}

#[tokio::test]
async fn two_hosts_share_http_authority_and_reconnect_flushes_degraded_attempts() {
    shared_authority_roundtrip(false).await;
}
#[tokio::test]
async fn acknowledged_allocation_survives_authority_loss_and_background_reconnect() {
    shared_authority_roundtrip(true).await;
}

#[tokio::test]
async fn interrupted_authority_response_retains_best_effort_continuation() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let fixture = Fixture::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    FleetCoordinationStore::open(fixture.db.clone()).unwrap();
    fixture
        .db
        .with_connection(|db| {
            db.execute(
                "INSERT INTO fleet_coordination_peers VALUES(?1,?2,?3,1,?4)",
                beale_storage::rusqlite::params![
                    fixture.binding.owner_host_id,
                    endpoint,
                    "a".repeat(64),
                    beale_contracts::PROTOCOL_MAJOR
                ],
            )?;
            Ok(())
        })
        .unwrap();
    let server = tokio::spawn(async move {
        let (mut connection, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        assert!(connection.read(&mut request).await.unwrap() > 0);
        connection.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nContent-Type: application/json\r\n\r\n{").await.unwrap();
        // Deliver the headers and partial body before losing the connection.
        tokio::time::sleep(Duration::from_millis(100)).await;
    });
    let reply = remote(
        &fixture.db,
        &fixture.binding,
        &AccountingRequest::Status {
            group_id: fixture.binding.group_id.clone(),
        },
    )
    .await
    .unwrap();
    assert!(
        reply.is_none(),
        "a lost acknowledgement must remain retryable"
    );
    server.await.unwrap();
}

async fn shared_authority_roundtrip(paced: bool) {
    let authority = Fixture::new();
    let mut leader = Fixture::new();
    let mut binding = authority.binding.clone();
    binding.participants.push(leader.store.host_id().unwrap());
    // A distinct, explicitly configured shared group, registered before use.
    binding.group_id = "shared-example".into();
    authority.store.bind("shared", binding.clone()).unwrap();
    leader
        .store
        .bind(
            &route_key(Provider::Xai, AuthMode::ApiKey, "example"),
            binding.clone(),
        )
        .unwrap();
    leader.binding = binding.clone();
    let mut target = leader.target();
    target.interval_seconds = 1;
    leader.scan("scan-b", paced.then_some(target));
    let maintenance = start_maintenance(leader.db.clone()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let origin = format!("http://{address}");
    let token = "a".repeat(64);
    FleetCoordinationStore::open(leader.db.clone()).unwrap();
    leader
        .db
        .with_connection(|db| {
            db.execute(
                "INSERT INTO fleet_coordination_peers VALUES(?1,?2,?3,1,?4)",
                beale_storage::rusqlite::params![
                    binding.owner_host_id,
                    origin,
                    token,
                    beale_contracts::PROTOCOL_MAJOR
                ],
            )?;
            Ok(())
        })
        .unwrap();
    let app = http::router(token.clone(), authority.db.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    scope(
        leader.db.clone(),
        "scan-b",
        CallClass::Research,
        dispatch(Provider::Xai, AuthMode::ApiKey, "example", async {
            Ok(step())
        }),
    )
    .await
    .unwrap();
    flush(&leader.db).await.unwrap();
    let mut stale = leader.request("stale", "scan-b");
    stale.generation = 2;
    assert!(
        remote(
            &leader.db,
            &binding,
            &AccountingRequest::Admit { request: stale }
        )
        .await
        .is_err()
    );
    server.abort();
    let _ = server.await;
    scope(
        leader.db.clone(),
        "scan-b",
        CallClass::Supporting,
        dispatch(Provider::Xai, AuthMode::ApiKey, "example", async {
            Ok(step())
        }),
    )
    .await
    .unwrap();
    assert!(
        leader
            .store
            .warnings("scan-b")
            .unwrap()
            .iter()
            .any(|w| w.contains(if paced {
                "last acknowledged allocation"
            } else {
                "Shared accounting is unavailable"
            }))
    );
    assert_eq!(leader.store.outbox().unwrap().len(), 1);
    assert_eq!(
        leader
            .store
            .snapshot(&binding.group_id)
            .unwrap()
            .coordination_mode,
        Some(CoordinationMode::Degraded)
    );
    let listener = tokio::net::TcpListener::bind(address).await.unwrap();
    let app = http::router(token, authority.db.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    for _ in 0..70 {
        if leader.store.outbox().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(flush(&leader.db).await.unwrap());
    assert!(leader.store.outbox().unwrap().is_empty());
    scope(
        leader.db.clone(),
        "scan-b",
        CallClass::Research,
        dispatch(Provider::Xai, AuthMode::ApiKey, "example", async {
            Ok(step())
        }),
    )
    .await
    .unwrap();
    flush(&leader.db).await.unwrap();
    assert_eq!(
        leader
            .store
            .snapshot(&binding.group_id)
            .unwrap()
            .coordination_mode,
        Some(CoordinationMode::Coordinated)
    );
    assert_eq!(
        authority
            .store
            .snapshot(&binding.group_id)
            .unwrap()
            .reservations
            .len(),
        3
    );
    maintenance.abort();
    let _ = maintenance.await;
    server.abort();
    let _ = server.await;
}

#[test]
fn unknown_identity_is_local_and_explicit_grouping_overrides_without_tokens() {
    let f = Fixture::new();
    let first = resolve(&f.db, Provider::Anthropic, AuthMode::ApiKey, "example").unwrap();
    assert!(first.limited_identity);
    assert_eq!(
        first,
        resolve(&f.db, Provider::Anthropic, AuthMode::ApiKey, "other-model").unwrap()
    );
    let mut explicit = first.clone();
    explicit.group_id = "explicit-account".into();
    explicit.limited_identity = false;
    f.store
        .bind(
            &route_key(Provider::Anthropic, AuthMode::ApiKey, "example"),
            explicit.clone(),
        )
        .unwrap();
    assert_eq!(
        resolve(&f.db, Provider::Anthropic, AuthMode::ApiKey, "example").unwrap(),
        explicit
    );
    let serialized = serde_json::to_string(&f.store.snapshot(&explicit.group_id).unwrap()).unwrap();
    assert!(!serialized.contains("access_token"));
    assert!(!serialized.contains("refresh_token"));
    assert!(f.root.0.exists());
}

#[test]
fn stable_account_identity_survives_refresh_and_explicit_authority_generation() {
    let f = Fixture::new();
    let route = route_key(Provider::OpenAi, AuthMode::Subscription, "example");
    let first =
        resolve_identity(&f.db, Provider::OpenAi, &route, Some("account-example-a")).unwrap();
    assert_eq!(
        resolve_identity(&f.db, Provider::OpenAi, &route, Some("account-example-a")).unwrap(),
        first
    );
    let other =
        resolve_identity(&f.db, Provider::OpenAi, &route, Some("account-example-b")).unwrap();
    assert_ne!(first.group_id, other.group_id);
    let current =
        resolve_identity(&f.db, Provider::OpenAi, &route, Some("account-example-a")).unwrap();
    assert_eq!(current, first);
    f.store
        .drain(&first.group_id, 1, &f.store.host_id().unwrap())
        .unwrap();
    let frozen = f.store.freeze(&first.group_id, 1).unwrap();
    f.store.import(frozen, &first.owner_host_id).unwrap();
    assert_eq!(
        resolve_identity(&f.db, Provider::OpenAi, &route, Some("account-example-a"))
            .unwrap()
            .generation,
        2
    );
}

#[test]
fn background_session_attribution_does_not_capture_unrelated_sessions() {
    let f = Fixture::new();
    f.scan("scan-a", None);
    assert_eq!(
        f.store.scan_for_session("scan-a").unwrap(),
        Some("scan-a".into())
    );
    assert_eq!(f.store.scan_for_session("ordinary-session").unwrap(), None);
}

#[tokio::test]
async fn stream_usage_is_durable_when_the_response_or_process_is_lost() {
    let f = Fixture::new();
    let mut target = f.target();
    target.interval_seconds = 1;
    f.scan("scan-a", Some(target));
    f.store
        .price(
            &f.binding.group_id,
            Price {
                model: "example".into(),
                currency_or_credit: "USD".into(),
                input_micros: 1_000_000,
                output_micros: 1_000_000,
                cached_input_micros: 1_000_000,
            },
        )
        .unwrap();
    assert!(
        scope(
            f.db.clone(),
            "scan-a",
            CallClass::Research,
            dispatch(
                Provider::Xai,
                AuthMode::ApiKey,
                "example",
                crate::wire::test_interrupted_usage_stream()
            )
        )
        .await
        .is_err()
    );
    flush(&f.db).await.unwrap();
    let snapshot = f.store.snapshot(&f.binding.group_id).unwrap();
    let charge = snapshot.reservations[0].charge.as_ref().unwrap();
    assert_eq!(charge.amount, Some(110));
    assert!(charge.uncertain);
    assert_eq!(charge.input_tokens, Some(100));
    let db = f.db.clone();
    let ready = Arc::new(tokio::sync::Notify::new());
    let signal = ready.clone();
    let task = tokio::spawn(scope(
        db,
        "scan-a",
        CallClass::Research,
        dispatch(Provider::Xai, AuthMode::ApiKey, "example", async move {
            observe_usage(Some(200), None, None)?;
            signal.notify_one();
            std::future::pending::<Result<ProviderStep>>().await
        }),
    ));
    ready.notified().await;
    let observed = f
        .store
        .outbox()
        .unwrap()
        .into_iter()
        .find(|e| e.charge.is_none())
        .unwrap();
    assert_eq!(
        observed.observed_charge.as_ref().unwrap().input_tokens,
        Some(200)
    );
    task.abort();
    let _ = task.await;
    // Model a process exit that loses Drop: recover the last streamed observation.
    f.store.enqueue(&observed).unwrap();
    f.store.recover_abandoned().unwrap();
    flush(&f.db).await.unwrap();
    let saved = f.store.snapshot(&f.binding.group_id).unwrap();
    let partial = saved
        .reservations
        .iter()
        .find(|r| r.request.id == observed.request.id)
        .unwrap()
        .charge
        .as_ref()
        .unwrap();
    assert_eq!(partial.amount, None);
    assert_eq!(partial.input_tokens, Some(200));
    assert!(partial.uncertain);
}

fn degraded_fixture() -> Fixture {
    let mut f = Fixture::new();
    f.binding.group_id = "offline-account-example".into();
    f.binding.owner_host_id = "unavailable-authority".into();
    f.store
        .bind(
            &route_key(Provider::Xai, AuthMode::ApiKey, "example"),
            f.binding.clone(),
        )
        .unwrap();
    let mut target = f.target();
    target.interval_seconds = 60;
    f.scan("scan-a", Some(target));
    f.store
        .price(
            &f.binding.group_id,
            Price {
                model: "example".into(),
                currency_or_credit: "USD".into(),
                input_micros: 1_000_000,
                output_micros: 1_000_000,
                cached_input_micros: 1_000_000,
            },
        )
        .unwrap();
    f
}

async fn unpaced_shared_account_prices(degraded: bool) {
    for (amount, price_unit, expected) in [
        (
            PacingAmount::Currency {
                currency: "EUR".into(),
                micros: 6000,
            },
            "USD",
            None,
        ),
        (PacingAmount::Credits { micros: 6000 }, "USD", None),
        (
            PacingAmount::Subscription {
                window_id: "example-window".into(),
                basis_points: 6000,
            },
            "USD",
            None,
        ),
        (
            PacingAmount::Currency {
                currency: "EUR".into(),
                micros: 6000,
            },
            "EUR",
            Some(6000),
        ),
        (
            PacingAmount::Credits { micros: 6000 },
            "credits",
            Some(6000),
        ),
    ] {
        for canceled in [false, true] {
            let mut f = Fixture::new();
            if degraded {
                f.binding.group_id = "offline-account-example".into();
                f.binding.owner_host_id = "unavailable-authority".into();
                f.store
                    .bind(
                        &route_key(Provider::Xai, AuthMode::ApiKey, "example"),
                        f.binding.clone(),
                    )
                    .unwrap();
            }
            let target = PacingTarget {
                budget_group_id: f.binding.group_id.clone(),
                interval_seconds: 60,
                amount: amount.clone(),
            };
            if degraded {
                f.store
                    .save_allocation(&f.binding.group_id, &target)
                    .unwrap();
            } else {
                let mut paced = f.request("paced", "scan-paced");
                paced.target = Some(target.clone());
                assert!(f.store.admit(&paced, 0).unwrap().admitted);
                f.store
                    .settle(
                        &paced,
                        &Charge {
                            attempt_id: paced.id.clone(),
                            amount: None,
                            quality: CostQuality::Unknown,
                            uncertain: true,
                            input_tokens: None,
                            output_tokens: None,
                            cache_read_tokens: None,
                        },
                    )
                    .unwrap();
            }
            f.scan("scan-unpaced", None);
            f.store
                .price(
                    &f.binding.group_id,
                    Price {
                        model: "example".into(),
                        currency_or_credit: price_unit.into(),
                        input_micros: 1_000_000,
                        output_micros: 0,
                        cached_input_micros: 0,
                    },
                )
                .unwrap();
            let (ready, observed) = tokio::sync::oneshot::channel();
            let task = tokio::spawn(TEST_CLOCK.scope(
                Arc::new(AtomicU64::new(2000)),
                scope(
                    f.db.clone(),
                    "scan-unpaced",
                    CallClass::Research,
                    dispatch(Provider::Xai, AuthMode::ApiKey, "example", async move {
                        observe_usage(Some(6000), Some(0), None)?;
                        ready.send(()).unwrap();
                        if canceled {
                            std::future::pending::<()>().await;
                        }
                        let mut result = step();
                        result.usage = Some(crate::TokenUsage {
                            input_tokens: 6000,
                            output_tokens: 0,
                            cache_read_tokens: None,
                        });
                        Ok(result)
                    }),
                ),
            ));
            observed.await.unwrap();
            if canceled {
                task.abort();
                assert!(task.await.unwrap_err().is_cancelled());
            } else {
                task.await.unwrap().unwrap();
            }
            let entry = f.store.outbox().unwrap().remove(0);
            assert!(entry.request.target.is_none());
            assert!(entry.allocation.is_some());
            assert_eq!(entry.observed_charge.as_ref().unwrap().amount, expected);
            let charge = entry.charge.as_ref().unwrap();
            assert_eq!(charge.amount, expected);
            assert_eq!(
                charge.quality,
                if expected.is_some() {
                    CostQuality::Estimated
                } else {
                    CostQuality::Unknown
                }
            );
            assert_eq!(charge.uncertain, canceled || expected.is_none());
            assert_eq!(charge.input_tokens, Some(6000));
            if expected.is_none() {
                let warning = if canceled {
                    "uncertain usage"
                } else {
                    "cost is unknown"
                };
                assert!(
                    f.store
                        .warnings("scan-unpaced")
                        .unwrap()
                        .iter()
                        .any(|w| w.contains(warning))
                );
            }
            let deadline = if expected.is_some() { 62_000 } else { 3000 };
            if degraded {
                assert!(!flush(&f.db).await.unwrap());
                assert!(
                    f.store
                        .local_admit(&f.binding.group_id, Some(&target), deadline - 1)
                        .unwrap()
                        .is_none()
                );
                assert!(
                    f.store
                        .local_admit(&f.binding.group_id, Some(&target), deadline)
                        .unwrap()
                        .is_some()
                );
            } else {
                assert!(flush(&f.db).await.unwrap());
                assert_eq!(
                    f.store.snapshot(&f.binding.group_id).unwrap().next_at_ms,
                    deadline
                );
            }
        }
    }
}

#[tokio::test]
async fn unpaced_calls_validate_shared_units_for_final_and_interrupted_usage() {
    unpaced_shared_account_prices(false).await;
}

#[tokio::test]
async fn degraded_unpaced_calls_validate_acknowledged_units_for_final_and_interrupted_usage() {
    unpaced_shared_account_prices(true).await;
}

async fn first_use_outage_roundtrip(interrupted: bool) {
    for amount in [
        PacingAmount::Currency {
            currency: "EUR".into(),
            micros: 6000,
        },
        PacingAmount::Credits { micros: 6000 },
        PacingAmount::Subscription {
            window_id: "example-window".into(),
            basis_points: 6000,
        },
        PacingAmount::Currency {
            currency: "USD".into(),
            micros: 6000,
        },
    ] {
        let authority = Fixture::new();
        let mut leader = Fixture::new();
        let mut binding = authority.binding.clone();
        binding.group_id = "first-use-account-example".into();
        binding.participants.push(leader.store.host_id().unwrap());
        authority.store.bind("shared", binding.clone()).unwrap();
        leader
            .store
            .bind(
                &route_key(Provider::Xai, AuthMode::ApiKey, "example"),
                binding.clone(),
            )
            .unwrap();
        leader.binding = binding.clone();
        leader.scan("scan-unpaced", None);
        leader
            .store
            .price(
                &binding.group_id,
                Price {
                    model: "example".into(),
                    currency_or_credit: "USD".into(),
                    input_micros: 1_000_000,
                    output_micros: 1_000_000,
                    cached_input_micros: 1_000_000,
                },
            )
            .unwrap();
        let target = PacingTarget {
            budget_group_id: binding.group_id.clone(),
            interval_seconds: 60,
            amount: amount.clone(),
        };
        let mut paced = authority.request("paced", "scan-paced");
        paced.group_id = binding.group_id.clone();
        paced.target = Some(target.clone());
        assert!(authority.store.admit(&paced, now_ms()).unwrap().admitted);
        authority
            .store
            .settle(&paced, &price_charge(&paced.id, None, None, Some(&target)))
            .unwrap();
        let before = authority
            .store
            .snapshot(&binding.group_id)
            .unwrap()
            .next_at_ms;
        assert!(
            leader
                .store
                .allocation(&binding.group_id)
                .unwrap()
                .is_none()
        );

        let (ready, observed) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(scope(
            leader.db.clone(),
            "scan-unpaced",
            CallClass::Research,
            dispatch(Provider::Xai, AuthMode::ApiKey, "example", async move {
                observe_usage(Some(6000), Some(0), Some(0))?;
                ready.send(()).unwrap();
                if interrupted {
                    std::future::pending::<()>().await;
                }
                let mut result = step();
                result.usage = Some(crate::TokenUsage {
                    input_tokens: 6000,
                    output_tokens: 0,
                    cache_read_tokens: Some(0),
                });
                Ok(result)
            }),
        ));
        tokio::time::timeout(Duration::from_secs(5), observed)
            .await
            .unwrap()
            .unwrap();
        let in_flight = leader.store.outbox().unwrap().remove(0);
        if interrupted {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            task.await.unwrap().unwrap();
        }
        let entry = leader.store.outbox().unwrap().remove(0);
        assert!(entry.local_admitted);
        assert!(entry.request.target.is_none() && entry.allocation.is_none());
        let charge = entry.charge.as_ref().unwrap();
        assert_eq!(charge.amount, None);
        assert_eq!(charge.quality, CostQuality::Unknown);
        assert!(charge.uncertain);
        assert_eq!(charge.input_tokens, Some(6000));
        assert_eq!(charge.output_tokens, Some(0));
        assert_eq!(charge.cache_read_tokens, Some(0));
        assert_eq!(entry.observed_charge.as_ref().unwrap().amount, None);
        assert!(
            leader
                .store
                .warnings("scan-unpaced")
                .unwrap()
                .iter()
                .any(|w| w.contains("Shared accounting is unavailable"))
        );
        if interrupted {
            // Also replay the exact streamed record left by a killed process,
            // so startup recovery must preserve unknown cost and token counts.
            assert!(in_flight.charge.is_none());
            leader.store.enqueue(&in_flight).unwrap();
        }
        let reopened = HostDatabase::open(&leader.root.0).unwrap();
        let maintenance = start_maintenance(reopened).unwrap();
        assert_eq!(
            leader.store.outbox().unwrap()[0].charge.as_ref(),
            Some(charge)
        );
        // Confirm usage remains queued while the authority is unavailable.
        assert!(!flush(&leader.db).await.unwrap());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let token = "a".repeat(64);
        let app = http::router(token.clone(), authority.db.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        FleetCoordinationStore::open(leader.db.clone()).unwrap();
        leader
            .db
            .with_connection(|db| {
                db.execute(
                    "INSERT INTO fleet_coordination_peers VALUES(?1,?2,?3,1,?4)",
                    beale_storage::rusqlite::params![
                        binding.owner_host_id,
                        origin,
                        token,
                        beale_contracts::PROTOCOL_MAJOR
                    ],
                )?;
                Ok(())
            })
            .unwrap();
        // Only ordinary background maintenance drains the completed/interrupted event.
        tokio::time::timeout(Duration::from_secs(8), async {
            while !leader.store.outbox().unwrap().is_empty() {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();
        let state = authority.store.snapshot(&binding.group_id).unwrap();
        let saved = state
            .reservations
            .iter()
            .find(|r| r.request.id == entry.request.id)
            .unwrap();
        assert_eq!(saved.charge.as_ref(), Some(charge));
        assert_eq!(state.next_at_ms, before.max(saved.touched_at_ms) + 1000);
        for _ in 0..2 {
            assert!(matches!(
                remote(
                    &leader.db,
                    &binding,
                    &AccountingRequest::Reconcile {
                        request: entry.request.clone(),
                        charge: charge.clone()
                    }
                )
                .await
                .unwrap(),
                Some(AccountingReply::Saved)
            ));
        }
        assert_eq!(
            serde_json::to_value(&state).unwrap(),
            serde_json::to_value(authority.store.snapshot(&binding.group_id).unwrap()).unwrap()
        );
        // New calls use the now-acknowledged account units. Matching USD prices
        // become estimates; incompatible and subscription prices remain unknown.
        scope(
            leader.db.clone(),
            "scan-unpaced",
            CallClass::Research,
            dispatch(Provider::Xai, AuthMode::ApiKey, "example", async {
                Ok(step())
            }),
        )
        .await
        .unwrap();
        let next = leader.store.outbox().unwrap().remove(0);
        assert!(next.allocation.is_some());
        let expected = if matches!(amount, PacingAmount::Currency { ref currency, .. } if currency == "USD")
        {
            Some(110)
        } else {
            None
        };
        assert_eq!(next.charge.as_ref().unwrap().amount, expected);
        assert!(flush(&leader.db).await.unwrap());
        assert_eq!(
            authority
                .store
                .snapshot(&binding.group_id)
                .unwrap()
                .reservations
                .len(),
            3
        );
        maintenance.abort();
        let _ = maintenance.await;
        server.abort();
        let _ = server.await;
    }
}

#[tokio::test]
async fn first_use_outage_completed_usage_reconnects_without_reinterpreting_units() {
    first_use_outage_roundtrip(false).await;
}

#[tokio::test]
async fn first_use_outage_interrupted_usage_and_restart_reconnect_without_reinterpreting_units() {
    first_use_outage_roundtrip(true).await;
}

#[tokio::test(start_paused = true)]
async fn canceled_and_timed_out_degraded_calls_settle_streamed_usage_before_retry() {
    for timeout in [false, true] {
        let f = degraded_fixture();
        let clock = Arc::new(AtomicU64::new(0));
        let (ready, observed) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(TEST_CLOCK.scope(
            clock.clone(),
            scope(
                f.db.clone(),
                "scan-a",
                CallClass::Supporting,
                dispatch(Provider::Xai, AuthMode::ApiKey, "example", async move {
                    observe_usage(Some(6000), Some(0), None)?;
                    ready.send(()).unwrap();
                    std::future::pending::<Result<ProviderStep>>().await
                }),
            ),
        ));
        observed.await.unwrap();
        if timeout {
            clock.store(45000, Ordering::SeqCst);
            tokio::time::advance(Duration::from_secs(45)).await;
            assert!(
                task.await
                    .unwrap()
                    .unwrap_err()
                    .to_string()
                    .contains("timed out")
            );
        } else {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        }
        let entry = f.store.outbox().unwrap().remove(0);
        assert_eq!(entry.charge.as_ref().unwrap().amount, Some(6000));
        assert!(entry.charge.as_ref().unwrap().uncertain);
        // Normal maintenance cannot reach the authority, but local debt is already durable.
        let maintenance = start_maintenance(f.db.clone()).unwrap();
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(5)).await;
        tokio::task::yield_now().await;
        assert!(
            f.store
                .local_admit(
                    &entry.request.group_id,
                    entry.request.target.as_ref(),
                    59999
                )
                .unwrap()
                .is_none()
        );
        assert_eq!(
            f.store
                .local_admit(
                    &entry.request.group_id,
                    entry.request.target.as_ref(),
                    60000
                )
                .unwrap(),
            Some(6000)
        );
        assert_eq!(f.store.outbox().unwrap().len(), 1);
        maintenance.abort();
        let _ = maintenance.await;
    }
}

#[tokio::test]
async fn restart_maintenance_settles_known_degraded_usage_once_while_authority_is_down() {
    let f = degraded_fixture();
    let mut request = f.request("interrupted", "scan-a");
    request.target.as_mut().unwrap().interval_seconds = 60;
    let estimate = f
        .store
        .local_admit(&request.group_id, request.target.as_ref(), 0)
        .unwrap()
        .unwrap();
    let observed = price_charge(
        &request.id,
        Some(&crate::TokenUsage {
            input_tokens: 6000,
            output_tokens: 0,
            cache_read_tokens: None,
        }),
        f.store.snapshot(&request.group_id).unwrap().prices.first(),
        request.target.as_ref(),
    );
    let entry = OutboxEntry {
        request: request.clone(),
        allocation: request.target.clone(),
        charge: None,
        canceled: false,
        local_admitted: true,
        dispatched: true,
        local_estimate: estimate,
        observed_charge: Some(observed),
    };
    // Persist precisely the in-flight streamed state a killed host leaves behind.
    f.store.enqueue(&entry).unwrap();
    let reopened = HostDatabase::open(&f.root.0).unwrap();
    let maintenance = start_maintenance(reopened.clone()).unwrap();
    tokio::task::yield_now().await;
    assert!(
        f.store.outbox().unwrap()[0]
            .charge
            .as_ref()
            .unwrap()
            .uncertain
    );
    assert!(
        f.store
            .local_admit(&request.group_id, request.target.as_ref(), 59999)
            .unwrap()
            .is_none()
    );
    maintenance.abort();
    let _ = maintenance.await;
    let maintenance = start_maintenance(reopened).unwrap();
    tokio::task::yield_now().await;
    assert_eq!(
        f.store
            .local_admit(&request.group_id, request.target.as_ref(), 60000)
            .unwrap(),
        Some(6000)
    );
    assert_eq!(f.store.outbox().unwrap().len(), 1);
    maintenance.abort();
    let _ = maintenance.await;
}
