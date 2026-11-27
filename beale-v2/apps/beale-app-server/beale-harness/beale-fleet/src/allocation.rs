//! Owning-host admission, independent of models and Desktop lifetime.
use crate::docker::{ContainerStatus, DockerEnvironments};
use anyhow::{Context, Result, ensure};
use beale_contracts::swarm_allocation::*;
use beale_storage::{HostDatabase, fleet_allocation::AllocationStore};
use std::{path::PathBuf, sync::Arc};

pub mod archives;
pub mod capacity;
pub mod http;
#[cfg(test)]
mod tests;

pub type CapacityFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<Capacity>> + Send + 'a>>;
pub trait CapacitySource: Send + Sync {
    fn measure<'a>(
        &'a self,
        environments: &'a DockerEnvironments,
        allocations: &'a [Allocation],
    ) -> CapacityFuture<'a>;
}

#[derive(Clone)]
pub struct Allocator {
    pub store: AllocationStore,
    pub environments: DockerEnvironments,
    pub source: Arc<dyn CapacitySource>,
}
impl Allocator {
    pub fn new(
        db: HostDatabase,
        environments: DockerEnvironments,
        source: Arc<dyn CapacitySource>,
    ) -> Result<Self> {
        Ok(Self {
            store: AllocationStore::open(db)?,
            environments,
            source,
        })
    }
    pub async fn capacity(&self) -> Result<Capacity> {
        self.source
            .measure(&self.environments, &self.store.allocations()?)
            .await
    }
    pub fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_millis()).ok())
            .unwrap_or(0)
    }
    /// Admission and preparation are separate durable steps. Crash recovery
    /// inspects named resources; it never treats a deadline as proof of stop.
    pub async fn prepare_next(&self) -> Result<Option<Allocation>> {
        let capacity = self.capacity().await?;
        let Some(row) = self.store.admit_next(&capacity, Self::now())? else {
            return Ok(None);
        };
        if self.store.restore_bundle(&row.request.id)?.is_some() {
            return Ok(Some(row));
        }
        let result = async {
            if let Some(instance) = self
                .environments
                .store
                .instance_if_present(row.request.instance.parse()?)?
            {
                ensure!(
                    instance.prepared
                        && instance.stopped
                        && instance.configuration.id == row.request.configuration
                        && instance.configuration.revision == row.request.configuration_revision,
                    "retained instance requires recovery"
                );
                let handle = instance.handle.context("retained container missing")?;
                ensure!(
                    self.environments.docker.inspect(&handle).await? == ContainerStatus::Stopped,
                    "retained execution is not stopped"
                );
                Ok(())
            } else {
                self.environments
                    .prepare_revision(
                        &row.request.configuration,
                        row.request.configuration_revision,
                        row.request.instance.parse()?,
                    )
                    .await
                    .map(|_| ())
            }
        }
        .await;
        if let Err(error) = result {
            self.reconcile(&row).await?;
            return Err(error);
        }
        self.store.prepared(&row.request.id)?;
        Ok(Some(self.store.allocation(&row.request.id)?))
    }
    pub async fn reconcile(&self, row: &Allocation) -> Result<()> {
        let _owner = self.environments.lock(row.request.instance.parse()?)?;
        // Preparation/launch may have advanced since maintenance took its
        // snapshot. Only the state read under the instance lock proves stop.
        let row = self.store.allocation(&row.request.id)?;
        if !matches!(
            row.phase,
            AllocationPhase::Reserved | AllocationPhase::Started
        ) {
            return Ok(());
        }
        match self
            .environments
            .store
            .instance_if_present(row.request.instance.parse()?)?
        {
            Some(mut instance) => {
                let handle = instance
                    .handle
                    .clone()
                    .context("partial allocation requires Docker preparation recovery")?;
                if self.environments.docker.stop(&handle).await? {
                    // A stop alone cannot release unpublished files or turn a
                    // failed checkpoint into deletion eligibility.
                    let never_started = row.execution.is_none() || instance.target != row.execution;
                    instance.stopped = true;
                    self.environments.store.save_instance(&instance)?;
                    self.store.confirm_stopped(&row.request.id, never_started)?;
                }
            }
            None => {
                // Only absence of creation intent after preparation maintenance
                // proves that no instance was started.
                self.store.confirm_stopped(&row.request.id, true)?;
            }
        }
        Ok(())
    }
    pub async fn checkpoint(
        &self,
        target: &beale_contracts::swarm_host::ExecutionTarget,
    ) -> Result<()> {
        let _owner = self.environments.lock(target.instance_id.parse()?)?;
        let instance = self.environments.store.validate_target(target)?;
        ensure!(
            instance.stopped && instance.target.as_ref() == Some(target),
            "checkpoint requires confirmed termination"
        );
        ensure!(
            self.environments
                .docker
                .inspect(
                    instance
                        .handle
                        .as_ref()
                        .context("retained instance missing")?
                )
                .await?
                == ContainerStatus::Stopped,
            "checkpoint execution still active"
        );
        // The original container and named volume are the durable same-host
        // checkpoint. Cross-host reconstruction additionally exports both.
        let rows = self.store.allocations()?;
        let row = rows
            .iter()
            .rev()
            .find(|r| {
                r.request.instance == target.instance_id
                    && r.execution.as_ref() == Some(target)
                    && r.request.agent == target.identity.agent_id
                    && r.request.scan == target.identity.scan_id
                    && r.request.leader == target.leader_id
                    && matches!(
                        r.phase,
                        AllocationPhase::Reserved
                            | AllocationPhase::Started
                            | AllocationPhase::Retained
                    )
            })
            .context("checkpoint allocation missing")?;
        self.store.confirm_stopped(&row.request.id, true)
    }
    pub fn start_maintenance(&self) -> tokio::task::JoinHandle<()> {
        let service = self.clone();
        tokio::spawn(async move {
            loop {
                let result = async {
                    for row in service.store.allocations()? {
                        if row.request.domain != "docker" {
                            continue;
                        }
                        if matches!(
                            row.phase,
                            AllocationPhase::Reserved | AllocationPhase::Started
                        ) && row.reservation_deadline_ms < Self::now()
                        {
                            let _ = service.reconcile(&row).await;
                        }
                    }
                    service.cleanup().await?;
                    service.prepare_next().await?;
                    anyhow::Ok(())
                }
                .await;
                if let Err(error) = result {
                    beale_host_log::error!("Fleet allocation remains pending: {error:#}");
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        })
    }
    pub async fn cleanup(&self) -> Result<()> {
        let allocations = self.store.allocations()?;
        for mut environment in self.store.environments()? {
            if !allocations
                .iter()
                .any(|a| a.request.instance == environment.instance && a.request.domain == "docker")
            {
                continue;
            }
            if environment.deletion == DeletionStep::Deleted {
                continue;
            }
            if environment.deletion == DeletionStep::Retained {
                match self.store.retention(
                    &environment.instance,
                    environment.generation,
                    RetentionAction::ClaimDelete { now: false },
                    Self::now(),
                ) {
                    Ok(claim) => environment = claim,
                    Err(_) => continue,
                }
            }
            let _owner = match self.environments.lock(environment.instance.parse()?) {
                Ok(owner) => owner,
                Err(_) => continue,
            };
            let result = async {
                let row = self
                    .environments
                    .store
                    .instance_if_present(environment.instance.parse()?)?;
                if let Some(row) = row {
                    let handle = row
                        .handle
                        .context("claimed deletion needs retained Docker identity")?;
                    if environment.deletion == DeletionStep::Claimed {
                        self.environments
                            .docker
                            .delete_container_step(&handle)
                            .await?;
                        self.store.deletion_step(
                            &environment.instance,
                            environment.generation,
                            DeletionStep::ContainerRemoved,
                        )?;
                        environment.deletion = DeletionStep::ContainerRemoved;
                    }
                    if environment.deletion == DeletionStep::ContainerRemoved {
                        self.environments.docker.delete_volume_step(&handle).await?;
                        self.store.deletion_step(
                            &environment.instance,
                            environment.generation,
                            DeletionStep::VolumeRemoved,
                        )?;
                    }
                } else {
                    // A reservation reconciled before creation has no physical
                    // resources, but advances through the same durable journal.
                    if environment.deletion == DeletionStep::Claimed {
                        self.store.deletion_step(
                            &environment.instance,
                            environment.generation,
                            DeletionStep::ContainerRemoved,
                        )?;
                        environment.deletion = DeletionStep::ContainerRemoved;
                    }
                    if environment.deletion == DeletionStep::ContainerRemoved {
                        self.store.deletion_step(
                            &environment.instance,
                            environment.generation,
                            DeletionStep::VolumeRemoved,
                        )?;
                    }
                }
                self.delete_archives(&environment.instance)?;
                self.store.deletion_step(
                    &environment.instance,
                    environment.generation,
                    DeletionStep::Deleted,
                )?;
                anyhow::Ok(())
            }
            .await;
            if let Err(error) = result {
                beale_host_log::error!(
                    "Claimed private environment cleanup remains pending: {error:#}"
                );
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct SystemCapacity {
    pub root: PathBuf,
}
impl CapacitySource for SystemCapacity {
    fn measure<'a>(
        &'a self,
        environments: &'a DockerEnvironments,
        allocations: &'a [Allocation],
    ) -> CapacityFuture<'a> {
        Box::pin(async move { capacity::measure(&self.root, environments, allocations).await })
    }
}
