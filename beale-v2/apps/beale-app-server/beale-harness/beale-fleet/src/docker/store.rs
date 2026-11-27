//! Host-owned Docker preparation records. No worker credentials are persisted.
use super::{ContainerHandle, ResolvedImage};
use anyhow::{Context, Result, ensure};
use beale_contracts::{swarm_container::*, swarm_host::ExecutionTarget};
use beale_storage::{
    HostDatabase,
    rusqlite::{OptionalExtension, params},
};
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct DockerStore {
    db: HostDatabase,
    host: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Instance {
    pub id: uuid::Uuid,
    pub configuration: ImageConfiguration,
    pub image: ResolvedImage,
    pub handle: Option<ContainerHandle>,
    pub target: Option<ExecutionTarget>,
    pub stopped: bool,
    pub prepared: bool,
}
/// Host-local observation token. It cannot authorize a different instance,
/// execution generation, or a result older than a subsequent readiness change.
pub struct PreflightObservation {
    target: ExecutionTarget,
    configuration: ImageConfiguration,
    digest: String,
    revision: i64,
}
impl DockerStore {
    pub fn open(db: HostDatabase) -> Result<Self> {
        let host =
            beale_storage::fleet_executions::FleetExecutionStore::open(db.clone())?.host_id()?;
        db.migrate_schema("fleet-docker", &[
            "CREATE TABLE fleet_docker_configurations(id TEXT PRIMARY KEY, document TEXT NOT NULL); CREATE TABLE fleet_docker_instances(id TEXT PRIMARY KEY, document TEXT NOT NULL);",
            "ALTER TABLE fleet_docker_configurations ADD COLUMN preparation TEXT; CREATE TABLE fleet_docker_preparations(id TEXT PRIMARY KEY, document TEXT NOT NULL);",
            "ALTER TABLE fleet_docker_configurations ADD COLUMN readiness_revision INTEGER NOT NULL DEFAULT 0;",
        ])?;
        Ok(Self { db, host })
    }
    pub fn host(&self) -> &str {
        &self.host
    }
    pub fn configurations(&self) -> Result<Vec<ConfigurationReadiness>> {
        self.db.with_connection(|db| {
            let mut query =
                db.prepare("SELECT document FROM fleet_docker_configurations ORDER BY id")?;
            let rows = query
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<beale_storage::rusqlite::Result<Vec<_>>>()?;
            rows.iter().map(|s| Ok(serde_json::from_str(s)?)).collect()
        })
    }
    pub fn configuration(&self, id: &str) -> Result<ConfigurationReadiness> {
        self.configurations()?
            .into_iter()
            .find(|c| c.configuration.id == id)
            .context("unknown container configuration")
    }
    pub fn save_configuration(&self, config: ImageConfiguration) -> Result<ConfigurationReadiness> {
        config.validate().map_err(anyhow::Error::msg)?;
        self.db.with_connection(|db| {
            let tx=db.transaction_with_behavior(beale_storage::rusqlite::TransactionBehavior::Immediate)?;
            let old: Option<String> = tx.query_row("SELECT document FROM fleet_docker_configurations WHERE id=?1", [&config.id], |r|r.get(0)).optional()?;
            if let Some(old) = old {
                let old: ConfigurationReadiness = serde_json::from_str(&old)?;
                if old.configuration == config { return Ok(old); }
                ensure!(config.revision == old.configuration.revision.checked_add(1).context("configuration revision exhausted")?, "configuration revision conflict");
            } else { ensure!(config.revision == 1, "new configuration requires revision one"); }
            let row = ConfigurationReadiness {configuration:config,host_id:self.host.clone(),state:Readiness::Unprepared,reason:None,image_digest:None};
            tx.execute("INSERT INTO fleet_docker_configurations(id,document) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET document=excluded.document,preparation=NULL,readiness_revision=readiness_revision+1",params![row.configuration.id,serde_json::to_string(&row)?])?;
            tx.commit()?;
            Ok(row)
        })
    }
    pub fn readiness(
        &self,
        config: &ImageConfiguration,
        state: Readiness,
        reason: Option<&str>,
        digest: Option<&str>,
    ) -> Result<()> {
        self.db.with_connection(|db| {
            let tx = db.transaction_with_behavior(
                beale_storage::rusqlite::TransactionBehavior::Immediate,
            )?;
            let old: String = tx.query_row(
                "SELECT document FROM fleet_docker_configurations WHERE id=?1",
                [&config.id],
                |r| r.get(0),
            )?;
            let mut old: ConfigurationReadiness = serde_json::from_str(&old)?;
            ensure!(
                old.configuration == *config,
                "configuration changed during preparation"
            );
            old.state = state;
            old.reason = reason.map(str::to_owned);
            old.image_digest = digest.map(str::to_owned);
            tx.execute(
                "UPDATE fleet_docker_configurations SET document=?2,preparation=NULL,readiness_revision=readiness_revision+1 WHERE id=?1",
                params![config.id, serde_json::to_string(&old)?],
            )?;
            tx.commit()?;
            Ok(())
        })
    }
    /// Called under the instance lock after durably binding the execution.
    /// An old configuration may still run its retained instance, but cannot
    /// publish readiness for a replacement configuration revision.
    pub fn begin_preflight(
        &self,
        target: &ExecutionTarget,
    ) -> Result<Option<PreflightObservation>> {
        let instance = self.validate_target(target)?;
        ensure!(
            instance.target.as_ref() == Some(target) && !instance.stopped,
            "preflight execution is not bound"
        );
        self.db.with_connection(|db| {
            let tx = db.transaction_with_behavior(
                beale_storage::rusqlite::TransactionBehavior::Immediate,
            )?;
            let (document, revision): (String, i64) = tx.query_row(
                "SELECT document,readiness_revision FROM fleet_docker_configurations WHERE id=?1",
                [&instance.configuration.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let readiness: ConfigurationReadiness = serde_json::from_str(&document)?;
            if readiness.configuration != instance.configuration {
                return Ok(None);
            }
            let revision = revision
                .checked_add(1)
                .context("readiness revision exhausted")?;
            tx.execute(
                "UPDATE fleet_docker_configurations SET readiness_revision=?2 WHERE id=?1",
                params![instance.configuration.id, revision],
            )?;
            tx.commit()?;
            Ok(Some(PreflightObservation {
                target: target.clone(),
                configuration: instance.configuration,
                digest: instance.image.digest,
                revision,
            }))
        })
    }
    /// Returns false for superseded observations. Persistence failure remains
    /// an error, so research cannot begin before a current result is durable.
    pub fn complete_preflight(
        &self,
        observation: &PreflightObservation,
        failure: Option<&str>,
    ) -> Result<bool> {
        self.db.with_connection(|db| {
            let tx = db.transaction_with_behavior(beale_storage::rusqlite::TransactionBehavior::Immediate)?;
            let document: String = tx.query_row("SELECT document FROM fleet_docker_instances WHERE id=?1", [&observation.target.instance_id], |r| r.get(0))?;
            let instance: Instance = serde_json::from_str(&document)?;
            if instance.target.as_ref() != Some(&observation.target) || instance.stopped || !instance.prepared || instance.configuration != observation.configuration || instance.image.digest != observation.digest { return Ok(false); }
            let (document, revision, preparing): (String, i64, Option<String>) = tx.query_row("SELECT document,readiness_revision,preparation FROM fleet_docker_configurations WHERE id=?1", [&observation.configuration.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            let mut readiness: ConfigurationReadiness = serde_json::from_str(&document)?;
            if revision != observation.revision || preparing.is_some() || readiness.configuration != observation.configuration { return Ok(false); }
            readiness.state = if failure.is_some() { Readiness::Unavailable } else { Readiness::Ready };
            readiness.reason = failure.map(str::to_owned);
            readiness.image_digest = Some(observation.digest.clone());
            let next = revision.checked_add(1).context("readiness revision exhausted")?;
            tx.execute("UPDATE fleet_docker_configurations SET document=?2,readiness_revision=?3 WHERE id=?1", params![observation.configuration.id, serde_json::to_string(&readiness)?, next])?;
            tx.commit()?;
            Ok(true)
        })
    }
    pub fn insert(&self, row: &Instance) -> Result<()> {
        self.db.with_connection(|db| {
            db.execute(
                "INSERT INTO fleet_docker_instances VALUES(?1,?2)",
                params![row.id.to_string(), serde_json::to_string(row)?],
            )?;
            Ok(())
        })
    }
    /// Journal and readiness change together, before even the image pull.
    pub(super) fn begin_preparation(
        &self,
        config: &ImageConfiguration,
        id: uuid::Uuid,
    ) -> Result<()> {
        self.db.with_connection(|db| {
            let tx = db.transaction_with_behavior(
                beale_storage::rusqlite::TransactionBehavior::Immediate,
            )?;
            let document: String = tx.query_row(
                "SELECT document FROM fleet_docker_configurations WHERE id=?1",
                [&config.id],
                |r| r.get(0),
            )?;
            let mut readiness: ConfigurationReadiness = serde_json::from_str(&document)?;
            ensure!(
                readiness.configuration == *config,
                "configuration changed during preparation"
            );
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM fleet_docker_instances WHERE id=?1)",
                [id.to_string()],
                |r| r.get(0),
            )?;
            ensure!(!exists, "container instance already exists");
            tx.execute(
                "INSERT INTO fleet_docker_preparations VALUES(?1,?2)",
                params![id.to_string(), serde_json::to_string(config)?],
            )?;
            readiness.state = Readiness::Preparing;
            readiness.reason = None;
            readiness.image_digest = None;
            tx.execute(
                "UPDATE fleet_docker_configurations SET document=?2,preparation=?3,readiness_revision=readiness_revision+1 WHERE id=?1",
                params![
                    config.id,
                    serde_json::to_string(&readiness)?,
                    id.to_string()
                ],
            )?;
            tx.commit()?;
            Ok(())
        })
    }
    pub(super) fn preparation(&self, id: uuid::Uuid) -> Result<Option<ImageConfiguration>> {
        self.db.with_connection(|db| {
            let document: Option<String> = db
                .query_row(
                    "SELECT document FROM fleet_docker_preparations WHERE id=?1",
                    [id.to_string()],
                    |r| r.get(0),
                )
                .optional()?;
            document.map(|s| Ok(serde_json::from_str(&s)?)).transpose()
        })
    }
    /// A stale preparation cannot replace a newer revision or preparation's
    /// readiness. Failed writes keep the journal available for maintenance.
    pub(super) fn finish_preparation(
        &self,
        id: uuid::Uuid,
        config: &ImageConfiguration,
        state: Readiness,
        reason: Option<&str>,
        digest: Option<&str>,
    ) -> Result<()> {
        self.db.with_connection(|db| {
            let tx = db.transaction_with_behavior(beale_storage::rusqlite::TransactionBehavior::Immediate)?;
            let (document, owner): (String, Option<String>) = tx.query_row("SELECT document,preparation FROM fleet_docker_configurations WHERE id=?1", [&config.id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            let mut readiness: ConfigurationReadiness = serde_json::from_str(&document)?;
            if readiness.configuration == *config && owner.as_deref() == Some(&id.to_string()) {
                readiness.state = state;
                readiness.reason = reason.map(str::to_owned);
                readiness.image_digest = digest.map(str::to_owned);
                tx.execute("UPDATE fleet_docker_configurations SET document=?2,preparation=NULL,readiness_revision=readiness_revision+1 WHERE id=?1", params![config.id, serde_json::to_string(&readiness)?])?;
            }
            tx.execute("DELETE FROM fleet_docker_preparations WHERE id=?1", [id.to_string()])?;
            tx.commit()?;
            Ok(())
        })
    }
    pub fn instance(&self, id: &str) -> Result<Instance> {
        self.db.with_connection(|db| {
            let doc: String = db
                .query_row(
                    "SELECT document FROM fleet_docker_instances WHERE id=?1",
                    [id],
                    |r| r.get(0),
                )
                .context("retained container instance unavailable")?;
            let row: Instance = serde_json::from_str(&doc)?;
            ensure!(
                row.id.to_string() == id,
                "container instance identity changed"
            );
            Ok(row)
        })
    }
    pub(super) fn unfinished(&self, after: &str) -> Result<Vec<uuid::Uuid>> {
        self.db.with_connection(|db| {
            let mut query=db.prepare("SELECT id FROM fleet_docker_instances WHERE id>?1 AND json_extract(document,'$.prepared')=0 AND json_extract(document,'$.stopped')=0 UNION SELECT id FROM fleet_docker_preparations WHERE id>?1 ORDER BY id LIMIT 32")?;
            let rows=query.query_map([after],|r|r.get::<_,String>(0))?.collect::<beale_storage::rusqlite::Result<Vec<_>>>()?;
            rows.iter().map(|s|Ok(s.parse()?)).collect()
        })
    }
    pub(crate) fn instance_if_present(&self, id: uuid::Uuid) -> Result<Option<Instance>> {
        self.db.with_connection(|db| {
            let document: Option<String> = db
                .query_row(
                    "SELECT document FROM fleet_docker_instances WHERE id=?1",
                    [id.to_string()],
                    |r| r.get(0),
                )
                .optional()?;
            let row: Option<Instance> = document.map(|s| serde_json::from_str(&s)).transpose()?;
            ensure!(
                row.as_ref().is_none_or(|row| row.id == id),
                "container instance identity changed"
            );
            Ok(row)
        })
    }
    pub(crate) fn creation_recorded(&self, id: &str) -> Result<bool> {
        self.db.with_connection(|db| Ok(db.query_row(
            "SELECT EXISTS(SELECT 1 FROM fleet_docker_instances WHERE id=?1 UNION ALL SELECT 1 FROM fleet_docker_preparations WHERE id=?1)",
            [id], |r| r.get(0))?))
    }
    /// Caller holds the cross-process instance lock for the whole operation.
    pub fn save_instance(&self, row: &Instance) -> Result<()> {
        self.db.with_connection(|db| {
            ensure!(
                db.execute(
                    "UPDATE fleet_docker_instances SET document=?2 WHERE id=?1",
                    params![row.id.to_string(), serde_json::to_string(row)?]
                )? == 1,
                "container instance disappeared"
            );
            Ok(())
        })
    }
    pub fn validate_target(&self, target: &ExecutionTarget) -> Result<Instance> {
        target.validate().map_err(anyhow::Error::msg)?;
        let row = self.instance(&target.instance_id)?;
        if let Some(handle) = &row.handle {
            ensure!(
                handle.host == self.host
                    && handle.instance == row.id
                    && handle.image.digest == row.image.digest,
                "retained Docker handle identity changed"
            );
        }
        ensure!(
            target.host_id == self.host
                && target.configuration_id == row.configuration.id
                && target.configuration_revision == row.configuration.revision
                && row.prepared
                && row.handle.is_some(),
            "container configuration is not prepared for this target"
        );
        if let Some(old) = &row.target {
            ensure!(
                old == target
                    || row.stopped
                        && old.leader_id == target.leader_id
                        && old.identity.workspace_id == target.identity.workspace_id
                        && old.identity.scan_id == target.identity.scan_id
                        && old.identity.agent_id == target.identity.agent_id
                        && target.identity.generation > old.identity.generation,
                "container still owned by another execution"
            );
        }
        Ok(row)
    }
}
