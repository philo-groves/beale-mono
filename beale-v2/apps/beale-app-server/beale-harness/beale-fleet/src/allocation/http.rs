use super::*;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use beale_contracts::swarms::valid_id;
use std::time::Duration;

#[derive(Clone)]
struct Host {
    token: String,
    allocator: Allocator,
}
pub fn router(token: String, allocator: Allocator) -> Router {
    Router::new()
        .route(PATH, post(dispatch))
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .with_state(Host { token, allocator })
}
async fn dispatch(
    State(host): State<Host>,
    headers: HeaderMap,
    Json(request): Json<HostAllocationRequest>,
) -> Result<Json<HostAllocationReply>, StatusCode> {
    if headers.get("authorization").and_then(|h| h.to_str().ok())
        != Some(format!("Bearer {}", host.token).as_str())
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    // Preserve concrete bounded host diagnoses; never forward guest output,
    // paths, SQL errors or transport details to client/model callers.
    match handle(&host.allocator, request).await {
        Ok(reply) => Ok(Json(reply)),
        Err(error) if error.is::<crate::docker::InstanceBusy>() => {
            Ok(Json(HostAllocationReply::Busy {}))
        }
        Err(error) => {
            beale_host_log::error!("Fleet allocation request failed: {error:#}");
            Ok(Json(HostAllocationReply::Rejected{reason:"Destination allocation or retained storage unavailable; inspect host readiness and resource measurements. No local fallback was attempted.".into()}))
        }
    }
}
async fn specification(
    allocator: &Allocator,
    spec: &AllocationSpec,
) -> Result<(AllocationRequest, Capacity)> {
    ensure!(
        [
            &spec.id,
            &spec.leader,
            &spec.scan,
            &spec.agent,
            &spec.parent,
            &spec.configuration
        ]
        .iter()
        .all(|s| valid_id(s)),
        "invalid allocation identity"
    );
    let mut config = allocator
        .environments
        .store
        .configuration(&spec.configuration)?;
    let allocations = allocator.store.allocations()?;
    let candidates = allocator
        .store
        .environments()?
        .into_iter()
        .filter(|r| {
            spec.restore.is_none()
                && r.leader == spec.leader
                && r.scan == spec.scan
                && r.agent == spec.agent
                && allocations
                    .iter()
                    .any(|a| a.request.domain == "docker" && a.request.instance == r.instance)
        })
        .collect::<Vec<_>>();
    let mut existing = None;
    for environment in candidates {
        let rows = allocations
            .iter()
            .filter(|a| a.request.instance == environment.instance)
            .collect::<Vec<_>>();
        // A revision mismatch before preparation leaves no private filesystem
        // to resume. Only confirmed, unbound, never-created reservations qualify.
        if environment.termination_confirmed
            && environment.checkpoint_confirmed
            && environment.references.is_empty()
            && environment.pending_outputs.is_empty()
            && rows.iter().all(|a| {
                !a.prepared
                    && a.execution.is_none()
                    && matches!(
                        a.phase,
                        AllocationPhase::Retained | AllocationPhase::Canceled
                    )
            })
            && !allocator
                .environments
                .store
                .creation_recorded(&environment.instance)?
        {
            continue;
        }
        let sequence = rows.iter().map(|a| a.sequence).max().unwrap_or(0);
        if existing.as_ref().is_none_or(|(old, _)| sequence > *old) {
            existing = Some((sequence, environment));
        }
    }
    let existing = existing.map(|(_, environment)| environment);
    if let Some(bundle) = &spec.restore {
        bundle.validate().map_err(anyhow::Error::msg)?;
        ensure!(
            bundle.image_digest.is_some()
                && config.configuration.architecture == bundle.configuration.architecture
                && config.configuration.options == bundle.configuration.options,
            "retained environment is incompatible with destination"
        );
        ensure!(
            bundle.target.leader_id == spec.leader
                && bundle.target.identity.scan_id == spec.scan
                && bundle.target.identity.agent_id == spec.agent,
            "restore identity mismatch"
        );
    }
    if let Some(e) = &existing {
        ensure!(
            e.deletion == DeletionStep::Retained,
            "retained instance deletion in progress"
        );
        let retained = allocator.environments.store.instance(&e.instance)?;
        ensure!(
            retained.configuration.id == spec.configuration,
            "retained configuration differs; explicit compatible restore is required"
        );
        config.configuration = retained.configuration;
    }
    let instance = existing
        .map(|e| e.instance)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let parent = allocations.iter().rev().find(|a| {
        a.request.domain == "docker"
            && a.request.leader == spec.leader
            && a.request.scan == spec.scan
            && a.request.agent == spec.parent
            && matches!(
                a.phase,
                AllocationPhase::Reserved | AllocationPhase::Started
            )
    });
    let capacity = allocator.capacity().await?;
    // A shared filesystem appears once even when Docker and app-server storage
    // use different paths. Separate VM/backing volumes each impose a bound.
    let bytes = (DEFAULT_STORAGE / 2)
        .checked_add(spec.input_bytes)
        .context("input size overflow")?;
    let bytes = spec.restore.as_ref().map_or(Ok(bytes), |bundle| {
        bundle.archives.iter().try_fold(bytes, |n, a| {
            n.checked_add(a.bytes.checked_mul(2).context("restore size overflow")?)
                .context("restore size overflow")
        })
    })?;
    let memory = allocations
        .iter()
        .filter(|a| {
            a.request.configuration == config.configuration.id
                && a.request.configuration_revision == config.configuration.revision
        })
        .filter_map(|a| capacity.measured_memory.get(&a.request.id))
        .copied()
        .max()
        .unwrap_or(DEFAULT_MEMORY)
        .max(DEFAULT_MEMORY);
    let image_key = format!(
        "image-{}",
        beale_storage::swarm_files::hash(&serde_json::to_vec(&(
            &config.configuration.image,
            config.configuration.architecture
        ))?)
    );
    let mut demands = Vec::new();
    for pool in capacity.storage.keys() {
        demands.push(StorageDemand {
            pool: pool.clone(),
            key: format!("private-{instance}"),
            bytes,
        });
        demands.push(StorageDemand {
            pool: pool.clone(),
            key: image_key.clone(),
            bytes: DEFAULT_STORAGE / 2,
        });
    }
    let request = AllocationRequest {
        version: 1,
        id: spec.id.clone(),
        leader: spec.leader.clone(),
        scan: spec.scan.clone(),
        agent: spec.agent.clone(),
        branch: spec.parent.clone(),
        domain: capacity.domain.clone(),
        configuration: config.configuration.id,
        configuration_revision: config.configuration.revision,
        instance,
        memory,
        storage: demands,
        storage_reserve: spec.storage_reserve,
        release_parent: parent.map(|p| p.request.id.clone()),
        processing_result: spec.processing_result,
    };
    request.validate().map_err(anyhow::Error::msg)?;
    Ok((request, capacity))
}
pub async fn handle(
    allocator: &Allocator,
    request: HostAllocationRequest,
) -> Result<HostAllocationReply> {
    match request {
        HostAllocationRequest::PreviewDiscard {
            instance,
            generation,
        } => Ok(HostAllocationReply::DiscardPreview {
            preview: allocator
                .store
                .preview_discard(&instance, generation, Allocator::now())?,
        }),
        HostAllocationRequest::ConfirmDiscard { preview } => Ok(HostAllocationReply::Environment {
            environment: allocator
                .store
                .confirm_discard(&preview, Allocator::now())?,
        }),
        HostAllocationRequest::ScanLifecycle { lifecycle } => {
            allocator
                .store
                .scan_lifecycle(&lifecycle, Allocator::now())?;
            Ok(HostAllocationReply::Complete)
        }
        HostAllocationRequest::Feasibility { spec } => {
            let (r, c) = specification(allocator, &spec).await?;
            Ok(HostAllocationReply::Feasibility {
                decision: allocator.store.feasibility(&r, &c, Allocator::now())?,
            })
        }
        HostAllocationRequest::Enqueue { spec } => {
            let (r, c) = if let Some(bound) = allocator.store.bound_spec(&spec)? {
                if let Some(old) = allocator.store.allocation_if_present(&spec.id)? {
                    return Ok(HostAllocationReply::Allocation { allocation: old });
                }
                (bound, allocator.capacity().await?)
            } else {
                let (r, c) = specification(allocator, &spec).await?;
                (allocator.store.bind_spec(&spec, &r)?, c)
            };
            Ok(HostAllocationReply::Allocation {
                allocation: allocator.store.enqueue(r, &c, Allocator::now())?,
            })
        }
        HostAllocationRequest::Inspect { id } => Ok(HostAllocationReply::Allocation {
            allocation: allocator.store.allocation(&id)?,
        }),
        HostAllocationRequest::Cancel { id } => {
            if let Some(row) = allocator.store.allocation_if_present(&id)?
                && row.phase == AllocationPhase::Reserved
                && row.execution.is_none()
            {
                allocator.reconcile(&row).await?;
            }
            allocator.store.cancel_queued(&id)?;
            Ok(HostAllocationReply::Complete)
        }
        HostAllocationRequest::Checkpoint { target } => Ok(HostAllocationReply::Bundle {
            bundle: allocator.capture(&target, false).await?,
        }),
        HostAllocationRequest::CheckpointAccepted { target, digest } => {
            allocator.accept_checkpoint(&target, &digest).await?;
            Ok(HostAllocationReply::Complete)
        }
        HostAllocationRequest::Export { target, transfer } => Ok(HostAllocationReply::Bundle {
            bundle: allocator.capture_export(&target, &transfer).await?,
        }),
        HostAllocationRequest::CancelExport { target, transfer } => {
            allocator.cancel_export(&target, &transfer)?;
            Ok(HostAllocationReply::Complete)
        }
        HostAllocationRequest::ReadArchive {
            checkpoint,
            kind,
            offset,
        } => Ok(HostAllocationReply::Archive {
            offset,
            bytes: allocator.read_archive(&checkpoint, kind, offset)?,
        }),
        HostAllocationRequest::BeginRestore { allocation, bundle } => {
            allocator.begin_restore(&allocation, &bundle)?;
            Ok(HostAllocationReply::Complete)
        }
        HostAllocationRequest::WriteArchive {
            allocation,
            kind,
            offset,
            bytes,
        } => Ok(HostAllocationReply::Archive {
            offset: allocator.write_archive(&allocation, kind, offset, &bytes)?,
            bytes: vec![],
        }),
        HostAllocationRequest::FinishRestore { allocation } => {
            allocator.finish_restore(&allocation).await?;
            Ok(HostAllocationReply::Complete)
        }
        HostAllocationRequest::Retention {
            instance,
            generation,
            action,
        } => Ok(HostAllocationReply::Environment {
            environment: allocator.store.retention(
                &instance,
                generation,
                action,
                Allocator::now(),
            )?,
        }),
        HostAllocationRequest::Inventory => Ok(HostAllocationReply::Inventory {
            allocations: allocator.store.allocations()?,
            environments: allocator.store.environments()?,
        }),
        HostAllocationRequest::Environments => Ok(HostAllocationReply::Environments {
            environments: allocator.store.environments()?,
        }),
    }
}

#[derive(Clone)]
pub struct AllocationClient {
    client: reqwest::Client,
    endpoint: reqwest::Url,
    token: reqwest::header::HeaderValue,
}
impl AllocationClient {
    pub async fn download(
        &self,
        bundle: &EnvironmentBundle,
        kind: ArchiveKind,
        path: &std::path::Path,
    ) -> Result<()> {
        use std::io::{Seek, Write};
        bundle.validate().map_err(anyhow::Error::msg)?;
        let entry = bundle
            .archives
            .iter()
            .find(|a| a.kind == kind)
            .context("archive component missing")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .read(true)
            .open(path)?;
        let mut offset = file.metadata()?.len();
        ensure!(
            offset <= entry.bytes,
            "partial checkpoint exceeds expected length"
        );
        file.seek(std::io::SeekFrom::Start(offset))?;
        while offset < entry.bytes {
            let reply = self
                .send(&HostAllocationRequest::ReadArchive {
                    checkpoint: archives::bundle_id(bundle)?,
                    kind,
                    offset,
                })
                .await?;
            let HostAllocationReply::Archive {
                offset: received,
                bytes,
            } = reply
            else {
                anyhow::bail!("checkpoint transfer unavailable")
            };
            ensure!(
                received == offset
                    && !bytes.is_empty()
                    && bytes.len() <= archives::CHUNK
                    && offset + bytes.len() as u64 <= entry.bytes,
                "invalid archive chunk"
            );
            file.write_all(&bytes)?;
            file.sync_all()?;
            offset += bytes.len() as u64;
        }
        ensure!(
            archives::file_digest(path)? == (entry.sha256.clone(), entry.bytes),
            "checkpoint digest mismatch"
        );
        Ok(())
    }
    pub fn new(peer: &crate::FleetPeer) -> Result<Self> {
        ensure!(
            peer.enabled && peer.protocol_major == beale_contracts::PROTOCOL_MAJOR,
            "allocation peer disabled or incompatible"
        );
        beale_contracts::swarm_host::validate_origin(&peer.endpoint).map_err(anyhow::Error::msg)?;
        let mut endpoint = reqwest::Url::parse(&peer.endpoint)?;
        endpoint.set_path(PATH);
        let mut token =
            reqwest::header::HeaderValue::from_str(&format!("Bearer {}", peer.control_token))?;
        token.set_sensitive(true);
        Ok(Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .build()?,
            endpoint,
            token,
        })
    }
    pub async fn send(&self, request: &HostAllocationRequest) -> Result<HostAllocationReply> {
        let mut response = self
            .client
            .post(self.endpoint.clone())
            .header("authorization", self.token.clone())
            .json(request)
            .timeout(Duration::from_secs(
                if matches!(
                    request,
                    HostAllocationRequest::Checkpoint { .. }
                        | HostAllocationRequest::Export { .. }
                        | HostAllocationRequest::FinishRestore { .. }
                ) {
                    600
                } else {
                    45
                },
            ))
            .send()
            .await?;
        ensure!(
            response.status().is_success(),
            "Fleet allocation endpoint rejected the request"
        );
        let mut bytes = vec![];
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= 4 * 1024 * 1024,
                "allocation reply too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok(serde_json::from_slice(&bytes)?)
    }
}
