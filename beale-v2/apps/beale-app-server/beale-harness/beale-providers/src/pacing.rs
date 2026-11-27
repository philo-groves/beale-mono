//! Leader-owned accounting. Task scopes carry attribution, never credentials.
use crate::{AuthMode, Provider, ProviderStep};
use anyhow::{Context, Result, ensure};
use beale_contracts::{pacing::*, swarms::PacingAmount};
use beale_storage::{
    HostDatabase,
    fleet_coordination::FleetCoordinationStore,
    provider_accounting::{AccountingStore, OutboxEntry},
    swarm_state::SwarmStore,
};
use std::{
    future::Future,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub mod http;
pub fn now_ms() -> u64 {
    #[cfg(test)]
    if let Ok(now) = TEST_CLOCK.try_with(|clock| clock.load(std::sync::atomic::Ordering::SeqCst)) {
        return now;
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
#[cfg(test)]
tokio::task_local! {static TEST_CLOCK: std::sync::Arc<std::sync::atomic::AtomicU64>;}

#[derive(Clone)]
struct ContextState {
    db: HostDatabase,
    scan: String,
    class: CallClass,
}
tokio::task_local! { static CONTEXT: ContextState; }
pub async fn scope<T>(
    db: HostDatabase,
    scan: &str,
    class: CallClass,
    future: impl Future<Output = T>,
) -> T {
    CONTEXT
        .scope(
            ContextState {
                db,
                scan: scan.into(),
                class,
            },
            Box::pin(future),
        )
        .await
}
pub fn active() -> bool {
    CONTEXT.try_with(|_| ()).is_ok()
}
pub fn provider_id(provider: Provider) -> i32 {
    match provider {
        Provider::OpenAi => 1,
        Provider::Xai => 2,
        Provider::Anthropic => 3,
        Provider::Zai => 4,
        Provider::OpenRouter => 5,
        Provider::Custom => 6,
    }
}
pub fn route_key(provider: Provider, mode: AuthMode, model: &str) -> String {
    let custom = if provider == Provider::Custom {
        crate::split_qualified_model(model)
            .map(|(id, _)| id)
            .unwrap_or("")
    } else {
        ""
    };
    format!(
        "provider-{}:{}:{custom}",
        provider_id(provider),
        if mode == AuthMode::ApiKey {
            "api"
        } else {
            "subscription"
        }
    )
}
/// Explicit host binding takes precedence. Auto identities are never derived from tokens.
pub fn resolve(
    db: &HostDatabase,
    provider: Provider,
    mode: AuthMode,
    model: &str,
) -> Result<AccountBinding> {
    let accounting = AccountingStore::open(db.clone())?;
    let route = route_key(provider, mode, model);
    if let Some(binding) = accounting.explicit_binding(&route)? {
        return Ok(binding);
    }
    let identity = if provider == Provider::OpenAi && mode == AuthMode::Subscription {
        crate::codex::credential()
            .ok()
            .map(|credential| credential.account_id)
    } else {
        None
    };
    resolve_identity(db, provider, &route, identity.as_deref())
}
fn resolve_identity(
    db: &HostDatabase,
    provider: Provider,
    route: &str,
    identity: Option<&str>,
) -> Result<AccountBinding> {
    let accounting = AccountingStore::open(db.clone())?;
    let host = accounting.host_id()?;
    if identity.is_none()
        && let Some(previous) = accounting.binding(route)?
        && !previous.limited_identity
    {
        return Ok(previous);
    }
    let key = identity
        .as_ref()
        .map(|id| format!("account:{id}"))
        .unwrap_or_else(|| format!("local:{host}:{route}"));
    let provider_id = format!("provider-{}", provider_id(provider));
    let group =
        FleetCoordinationStore::open(db.clone())?.account_group(&provider_id, &key, &host)?;
    if let Ok(snapshot) = accounting.snapshot(&group.id)
        && let Some(binding) = snapshot.binding
    {
        accounting.bind_automatic(route, binding.clone())?;
        return Ok(binding);
    }
    let binding = AccountBinding {
        group_id: group.id,
        provider_id,
        owner_host_id: host.clone(),
        generation: 1,
        participants: vec![host],
        limited_identity: identity.is_none(),
    };
    accounting.bind_automatic(route, binding.clone())?;
    Ok(binding)
}

pub(crate) fn verify_codex_account(identity: &str) -> Result<()> {
    CONTEXT
        .try_with(|context| -> Result<()> {
            let route = route_key(Provider::OpenAi, AuthMode::Subscription, "");
            let actual = if let Some(binding) =
                AccountingStore::open(context.db.clone())?.explicit_binding(&route)?
            {
                binding
            } else {
                resolve_identity(&context.db, Provider::OpenAi, &route, Some(identity))?
            };
            CURRENT_USAGE
                .try_with(|record| -> Result<()> {
                    let record = record
                        .lock()
                        .map_err(|_| anyhow::anyhow!("usage recorder unavailable"))?;
                    ensure!(
                        record.entry.request.group_id == actual.group_id,
                        "provider account changed while awaiting admission"
                    );
                    Ok(())
                })
                .unwrap_or(Ok(()))
        })
        .unwrap_or(Ok(()))
}

async fn remote(
    db: &HostDatabase,
    binding: &AccountBinding,
    request: &AccountingRequest,
) -> Result<Option<AccountingReply>> {
    let peer = FleetCoordinationStore::open(db.clone())?.peer(&binding.owner_host_id)?;
    let Some(peer) = peer.filter(|p| p.enabled) else {
        return Ok(None);
    };
    beale_contracts::swarm_host::validate_origin(&peer.endpoint).map_err(anyhow::Error::msg)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(3))
        .build()?;
    let mut response = match client
        .post(format!("{}{PATH}", peer.endpoint.trim_end_matches('/')))
        .bearer_auth(&peer.control_token)
        .json(request)
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => return Ok(None),
    };
    if response.status().is_server_error() {
        return Ok(None);
    }
    ensure!(
        response.status().is_success(),
        "accounting authority rejected stale or invalid coordination"
    );
    ensure!(
        response
            .content_length()
            .is_none_or(|n| n <= 4 * 1024 * 1024),
        "accounting response too large"
    );
    let mut bytes = Vec::new();
    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            // A dropped body is the same temporary authority outage as a
            // failed connection. Keep the event queued for reconciliation.
            Err(_) => return Ok(None),
        };
        ensure!(
            bytes.len().saturating_add(chunk.len()) <= 4 * 1024 * 1024,
            "accounting response too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(Some(serde_json::from_slice(&bytes)?))
}
pub async fn flush(db: &HostDatabase) -> Result<bool> {
    flush_group(db, None).await
}
async fn flush_group(db: &HostDatabase, group: Option<&str>) -> Result<bool> {
    let store = AccountingStore::open(db.clone())?;
    let mut reconciled = true;
    for entry in store.outbox()? {
        if group.is_some_and(|id| id != entry.request.group_id)
            || entry.charge.is_none() && !entry.canceled
        {
            continue;
        }
        let outcome = async {
            // Repair any retained final charge before retrying the remote authority.
            store.local_settle(&entry, entry.charge.as_ref().and_then(|c| c.amount))?;
            let binding = store
                .snapshot(&entry.request.group_id)?
                .binding
                .context("missing accounting authority")?;
            let operation = if let Some(charge) = &entry.charge {
                AccountingRequest::Reconcile {
                    request: entry.request.clone(),
                    charge: charge.clone(),
                }
            } else {
                AccountingRequest::Cancel {
                    request: entry.request.clone(),
                }
            };
            if binding.owner_host_id == store.host_id()? {
                if let Some(charge) = &entry.charge {
                    store.reconcile(&entry.request, charge, now_ms())?;
                } else {
                    store.cancel(&entry.request)?;
                }
            } else {
                match remote(db, &binding, &operation).await? {
                    Some(AccountingReply::Saved) => (),
                    None => return Ok(false),
                    _ => anyhow::bail!("invalid accounting acknowledgement"),
                }
            }
            store.acknowledge(&entry.request.id)?;
            Ok::<_, anyhow::Error>(true)
        }
        .await;
        match outcome {
            Ok(true) => (),
            Ok(false) => reconciled = false,
            Err(error) if group.is_some() => return Err(error),
            Err(_) => {
                store.warn(
                    &entry.request.scan_id,
                    "Accounting reconciliation needs host attention; queued usage is retained.",
                )?;
                reconciled = false;
            }
        }
    }
    Ok(reconciled)
}

/// Called explicitly on each participating leader before exporting a handover.
pub async fn drain(db: &HostDatabase, group: &str, generation: u64) -> Result<()> {
    let store = AccountingStore::open(db.clone())?;
    ensure!(
        flush_group(db, Some(group)).await?,
        "accounting authority unavailable"
    );
    ensure!(
        store.outbox()?.iter().all(|e| e.request.group_id != group),
        "inference or usage reporting is still active"
    );
    let binding = store.snapshot(group)?.binding.context("unknown group")?;
    let participant = store.host_id()?;
    store.drain(group, generation, &participant)?;
    if binding.owner_host_id != participant {
        ensure!(
            matches!(
                remote(
                    db,
                    &binding,
                    &AccountingRequest::Drain {
                        group_id: group.into(),
                        generation,
                        participant_id: participant
                    }
                )
                .await?,
                Some(AccountingReply::Saved)
            ),
            "accounting authority unavailable"
        );
    }
    Ok(())
}

struct UsageRecorder {
    store: AccountingStore,
    entry: OutboxEntry,
    price: Option<Price>,
}
type SharedUsage = std::sync::Arc<std::sync::Mutex<UsageRecorder>>;
tokio::task_local! {static CURRENT_USAGE: SharedUsage;}
/// Persist usage as soon as the stream supplies it, before result validation or
/// publication. Partial token counts retain unknown cost rather than zero.
pub(crate) fn observe_usage(
    input: Option<u64>,
    output: Option<u64>,
    cached: Option<u64>,
) -> Result<()> {
    CURRENT_USAGE
        .try_with(|record| -> Result<()> {
            let mut record = record
                .lock()
                .map_err(|_| anyhow::anyhow!("usage recorder unavailable"))?;
            let usage = input
                .zip(output)
                .map(|(input_tokens, output_tokens)| crate::TokenUsage {
                    input_tokens,
                    output_tokens,
                    cache_read_tokens: cached,
                });
            let mut charge = price_charge(
                &record.entry.request.id,
                usage.as_ref(),
                record.price.as_ref(),
                record
                    .entry
                    .request
                    .target
                    .as_ref()
                    .or(record.entry.allocation.as_ref()),
            );
            charge.uncertain = true;
            charge.input_tokens = input;
            charge.output_tokens = output;
            charge.cache_read_tokens = cached;
            record.entry.observed_charge = Some(charge);
            record.store.enqueue(&record.entry)
        })
        .unwrap_or(Ok(()))
}

struct Attempt {
    store: AccountingStore,
    entry: OutboxEntry,
    dispatched: bool,
    usage: Option<SharedUsage>,
}
impl Drop for Attempt {
    fn drop(&mut self) {
        if let Some(record) = &self.usage
            && let Ok(record) = record.lock()
        {
            self.entry.observed_charge = record.entry.observed_charge.clone();
        }
        if self.entry.charge.is_none() {
            if self.dispatched {
                self.entry.charge = Some(self.entry.observed_charge.clone().unwrap_or(Charge {
                    attempt_id: self.entry.request.id.clone(),
                    amount: None,
                    quality: CostQuality::Unknown,
                    uncertain: true,
                    input_tokens: None,
                    output_tokens: None,
                    cache_read_tokens: None,
                }));
                if let Some(charge) = &mut self.entry.charge {
                    charge.uncertain = true;
                }
                let _ = self.store.warn(
                    &self.entry.request.scan_id,
                    "A provider attempt has uncertain usage; it is not counted as zero.",
                );
            } else {
                self.entry.canceled = true;
            }
        }
        let _ = self.store.enqueue(&self.entry);
    }
}

async fn admitted(
    context: &ContextState,
    provider: Provider,
    mode: AuthMode,
    model: &str,
) -> Result<Attempt> {
    let store = AccountingStore::open(context.db.clone())?;
    let binding = resolve(&context.db, provider, mode, model)?;
    if mode == AuthMode::Subscription && matches!(provider, Provider::Anthropic | Provider::Zai) {
        store.warn(&context.scan,"The official provider CLI does not expose individual upstream retries; accounting is limited to host dispatches and reported usage.")?;
    }
    ensure!(
        !store
            .snapshot(&binding.group_id)?
            .drained_participants
            .contains(&store.host_id()?),
        "accounting participant is drained for handover"
    );
    let scan = SwarmStore::open(context.db.clone())?.load(&context.scan, true)?;
    let target = scan
        .config
        .pacing
        .iter()
        .find(|t| t.budget_group_id == binding.group_id)
        .cloned();
    if !scan.config.pacing.is_empty() {
        ensure!(
            !scan
                .config
                .provider_routes
                .iter()
                .any(|r| r.provider_id == binding.provider_id
                    && r.model == model
                    && r.budget_group_id != binding.group_id),
            "provider account changed; pacing configuration requires a new scan"
        );
    }
    let request = Admission {
        id: uuid::Uuid::new_v4().to_string(),
        group_id: binding.group_id.clone(),
        generation: binding.generation,
        participant_id: store.host_id()?,
        scan_id: context.scan.clone(),
        target,
        class: context.class.clone(),
    };
    let allocation = beale_storage::provider_accounting::conservative_allocation(
        store.allocation(&binding.group_id)?,
        request.target.as_ref(),
    )?;
    let mut attempt = Attempt {
        store: store.clone(),
        entry: OutboxEntry {
            request,
            allocation,
            charge: None,
            canceled: false,
            local_admitted: false,
            dispatched: false,
            local_estimate: 0,
            observed_charge: None,
        },
        dispatched: false,
        usage: None,
    };
    store.enqueue(&attempt.entry)?;
    loop {
        if mode == AuthMode::Subscription
            && matches!(
                attempt.entry.request.target.as_ref().map(|t| &t.amount),
                Some(PacingAmount::Subscription { .. })
            )
            && store.claim_probe(&binding.group_id, now_ms())?
            && refresh_observation(&context.db, &binding, provider)
                .await
                .is_err()
        {
            store.warn(&context.scan,"Provider allowance probe unavailable; pacing continues with uncertain measurements.")?;
        }
        let reconciled = flush_group(&context.db, Some(&binding.group_id)).await?;
        let reply = if binding.owner_host_id == store.host_id()? {
            Some(AccountingReply::Admission(
                store.admit(&attempt.entry.request, now_ms())?,
            ))
        } else if reconciled {
            remote(
                &context.db,
                &binding,
                &AccountingRequest::Admit {
                    request: attempt.entry.request.clone(),
                },
            )
            .await?
        } else {
            None
        };
        ensure!(
            reply
                .as_ref()
                .is_none_or(|r| matches!(r, AccountingReply::Admission(_))),
            "invalid accounting authority response"
        );
        if let Some(AccountingReply::Admission(reply)) = reply {
            store.coordination_mode(&binding.group_id, CoordinationMode::Coordinated)?;
            for warning in reply.warnings {
                store.warn(&context.scan, &warning)?;
            }
            if let Some(allocation) = reply.allocation {
                store.save_allocation(&binding.group_id, &allocation)?;
                attempt.entry.allocation =
                    beale_storage::provider_accounting::conservative_allocation(
                        Some(allocation),
                        attempt.entry.request.target.as_ref(),
                    )?;
            }
            if reply.admitted {
                attempt.dispatched = true;
                attempt.entry.dispatched = true;
                store.enqueue(&attempt.entry)?;
                return Ok(attempt);
            }
        } else {
            store.coordination_mode(&binding.group_id, CoordinationMode::Degraded)?;
            store.warn(&context.scan,if attempt.entry.allocation.is_some(){"Accounting authority unavailable; using the last acknowledged allocation locally. Combined pacing is degraded."}else{"Accounting authority unavailable and no allocation was acknowledged; using the scan target locally. Shared accounting is unavailable."})?;
            let target = attempt.entry.allocation.as_ref().or(attempt
                .entry
                .request
                .target
                .as_ref());
            if let Some(estimate) = store.local_admit(&binding.group_id, target, now_ms())? {
                attempt.entry.local_estimate = estimate;
                attempt.entry.local_admitted = true;
                attempt.dispatched = true;
                attempt.entry.dispatched = true;
                store.enqueue(&attempt.entry)?;
                return Ok(attempt);
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn refresh_observation(
    db: &HostDatabase,
    binding: &AccountBinding,
    provider: Provider,
) -> Result<()> {
    let client = crate::ProviderClient::new()?;
    let windows = match provider {
        Provider::OpenAi => crate::usage::openai_codex_usage(client.http()).await?,
        Provider::Xai => crate::usage::xai_usage(&client, &crate::CredentialStore::new()?).await?,
        _ => anyhow::bail!("allowance probe unavailable"),
    };
    ensure!(!windows.is_empty(), "allowance measurement unavailable");
    let store = AccountingStore::open(db.clone())?;
    for window in windows {
        let observation = Observation {
            window_id: window.id.into(),
            used_basis_points: window.used_basis_points,
            resets_at_ms: window.resets_at_unix.saturating_mul(1000),
            observed_at_ms: now_ms(),
        };
        if binding.owner_host_id == store.host_id()? {
            store.observe(&binding.group_id, binding.generation, observation, now_ms())?;
        } else {
            ensure!(
                remote(
                    db,
                    binding,
                    &AccountingRequest::Observe {
                        group_id: binding.group_id.clone(),
                        generation: binding.generation,
                        observation
                    }
                )
                .await?
                .is_some(),
                "accounting authority unavailable"
            );
        }
    }
    Ok(())
}

pub fn price_charge(
    id: &str,
    usage: Option<&crate::TokenUsage>,
    price: Option<&Price>,
    target: Option<&beale_contracts::swarms::PacingTarget>,
) -> Charge {
    let price = price.filter(|p| match target.map(|t| &t.amount) {
        Some(PacingAmount::Currency { currency, .. }) => p.currency_or_credit == *currency,
        Some(PacingAmount::Credits { .. }) => p.currency_or_credit == "credits",
        Some(PacingAmount::Subscription { .. }) => false,
        // A local price does not establish the shared account's units. In
        // particular, first-use outages have no acknowledged allocation yet.
        // Preserve token counts with unknown cost until those units are known.
        None => false,
    });
    let amount = usage.zip(price).and_then(|(u, p)| {
        let cached = u.cache_read_tokens.unwrap_or(0).min(u.input_tokens);
        let micros = u128::from(u.input_tokens - cached) * u128::from(p.input_micros)
            + u128::from(cached) * u128::from(p.cached_input_micros)
            + u128::from(u.output_tokens) * u128::from(p.output_micros);
        u64::try_from(micros.div_ceil(1_000_000)).ok()
    });
    Charge {
        attempt_id: id.into(),
        amount,
        quality: if amount.is_some() {
            CostQuality::Estimated
        } else {
            CostQuality::Unknown
        },
        uncertain: amount.is_none(),
        input_tokens: usage.map(|u| u.input_tokens),
        output_tokens: usage.map(|u| u.output_tokens),
        cache_read_tokens: usage.and_then(|u| u.cache_read_tokens),
    }
}

/// The future is polled only after admission; response timeout starts here.
pub fn dispatch<'a>(
    provider: Provider,
    mode: AuthMode,
    model: &'a str,
    future: impl Future<Output = Result<ProviderStep>> + Send + 'a,
) -> std::pin::Pin<Box<dyn Future<Output = Result<ProviderStep>> + Send + 'a>> {
    Box::pin(async move {
        let context = CONTEXT.try_with(Clone::clone).ok();
        let mut attempt = if let Some(context) = &context {
            Some(Box::pin(admitted(context, provider, mode, model)).await?)
        } else {
            None
        };
        let seconds = if context
            .as_ref()
            .is_some_and(|c| c.class == CallClass::Supporting)
        {
            45
        } else if provider == Provider::OpenAi {
            120
        } else {
            180
        };
        let record = if let Some(attempt) = &mut attempt {
            let snapshot = attempt.store.snapshot(&attempt.entry.request.group_id)?;
            let price = snapshot.prices.iter().find(|p| p.model == model).cloned();
            let record = std::sync::Arc::new(std::sync::Mutex::new(UsageRecorder {
                store: attempt.store.clone(),
                entry: attempt.entry.clone(),
                price,
            }));
            attempt.usage = Some(record.clone());
            Some(record)
        } else {
            None
        };
        let call = tokio::time::timeout(Duration::from_secs(seconds), future);
        let result = if let Some(record) = record {
            CURRENT_USAGE.scope(record, call).await
        } else {
            call.await
        }
        .context("provider response timed out")?;
        if let Some(attempt) = &mut attempt {
            let snapshot = attempt.store.snapshot(&attempt.entry.request.group_id)?;
            let price = snapshot.prices.iter().find(|p| p.model == model);
            let mut charge = price_charge(
                &attempt.entry.request.id,
                result.as_ref().ok().and_then(|s| s.usage.as_ref()),
                price,
                attempt
                    .entry
                    .request
                    .target
                    .as_ref()
                    .or(attempt.entry.allocation.as_ref()),
            );
            if result
                .as_ref()
                .ok()
                .and_then(|s| s.usage.as_ref())
                .is_none()
                && let Some(record) = &attempt.usage
                && let Ok(record) = record.lock()
                && let Some(observed) = &record.entry.observed_charge
            {
                charge = observed.clone();
            }
            if result.is_err() {
                charge.uncertain = true;
            }
            if charge.amount.is_none() {
                attempt.store.warn(
                    &attempt.entry.request.scan_id,
                    "Pricing or usage is unavailable; provider cost is unknown, not zero.",
                )?;
            }
            attempt.entry.charge = Some(charge.clone());
            attempt.store.enqueue(&attempt.entry)?;
            if snapshot.binding.as_ref().is_some_and(|b| {
                attempt
                    .store
                    .host_id()
                    .is_ok_and(|host| host == b.owner_host_id)
            }) {
                attempt.store.settle(&attempt.entry.request, &charge)?;
            }
        }
        drop(attempt);
        // Usage is durable before the result leaves this boundary. Reconnect flushing
        // is separate so an authority outage cannot lose a completed research result.
        result
    })
}

pub fn start_maintenance(db: HostDatabase) -> Result<tokio::task::JoinHandle<()>> {
    AccountingStore::open(db.clone())?.recover_abandoned()?;
    Ok(tokio::spawn(async move {
        loop {
            let _ = flush(&db).await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }))
}

#[cfg(test)]
mod tests;
