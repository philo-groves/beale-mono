use super::{Docker, DockerStore, Instance, RuntimeBundle};
use anyhow::{Context, Result, ensure};
use beale_contracts::swarm_container::*;
use std::{
    fs::{File, OpenOptions, TryLockError},
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub struct InstanceBusy;
impl std::fmt::Display for InstanceBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("container instance is already supervised")
    }
}
impl std::error::Error for InstanceBusy {}

#[derive(Clone)]
pub struct DockerEnvironments {
    pub docker: Docker,
    pub store: DockerStore,
    root: PathBuf,
    bundle: Option<PathBuf>,
}
impl DockerEnvironments {
    pub fn new(
        docker: Docker,
        store: DockerStore,
        root: PathBuf,
        bundle: Option<PathBuf>,
    ) -> Result<Self> {
        std::fs::create_dir_all(root.join("docker-instances"))?;
        Ok(Self {
            docker,
            store,
            root,
            bundle,
        })
    }
    pub fn available(&self) -> bool {
        self.bundle
            .as_ref()
            .is_some_and(|p| RuntimeBundle::open(p).is_ok())
    }
    pub fn lock(&self, id: uuid::Uuid) -> Result<File> {
        let path = self
            .root
            .join("docker-instances")
            .join(format!("{id}.lock"));
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        f.try_lock().map_err(|e| match e {
            TryLockError::WouldBlock => anyhow::Error::new(InstanceBusy),
            TryLockError::Error(e) => e.into(),
        })?;
        Ok(f)
    }
    /// Stage 7 admission calls this with a newly reserved instance ID. This
    /// method does not select hosts, reserve capacity, or reclaim retained data.
    pub async fn prepare(&self, configuration: &str, instance: uuid::Uuid) -> Result<Instance> {
        let revision = self
            .store
            .configuration(configuration)?
            .configuration
            .revision;
        self.prepare_revision(configuration, revision, instance)
            .await
    }
    pub async fn prepare_revision(
        &self,
        configuration: &str,
        revision: u64,
        instance: uuid::Uuid,
    ) -> Result<Instance> {
        let _owner = self.lock(instance)?;
        let config = self.store.configuration(configuration)?.configuration;
        ensure!(
            config.revision == revision,
            "reserved configuration revision changed before preparation"
        );
        self.store.begin_preparation(&config, instance)?;
        let result = self.prepare_inner(&config, instance).await;
        match &result {
            Ok(row) => self.store.finish_preparation(
                instance,
                &config,
                Readiness::Ready,
                None,
                Some(&row.image.digest),
            )?,
            Err(error) => {
                // Only our bounded stage context crosses the client boundary.
                let reason = error
                    .downcast_ref::<super::cli::ProbeFailure>()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| error.to_string());
                self.store.finish_preparation(
                    instance,
                    &config,
                    Readiness::Unavailable,
                    Some(&reason),
                    None,
                )?;
            }
        }
        result
    }
    async fn prepare_inner(
        &self,
        config: &ImageConfiguration,
        instance: uuid::Uuid,
    ) -> Result<Instance> {
        let bundle=RuntimeBundle::open(self.bundle.as_deref().context("Install a portable worker runtime bundle on this Fleet host")?)
            .context("Portable worker runtime is invalid; install a matching versioned static runtime bundle")?;
        ensure!(
            bundle.manifest().architecture == config.architecture,
            "Selected architecture has no matching worker runtime on this host"
        );
        let image=self.docker.resolve_image(&config.image,config.architecture).await
            .context("Image pull or platform inspection failed; check this host's Docker daemon, registry login, image name, and Linux architecture")?;
        let mut row = Instance {
            id: instance,
            configuration: config.clone(),
            image,
            handle: None,
            target: None,
            stopped: false,
            prepared: false,
        };
        // Durable intent precedes every Docker mutation. Repeating an ID fails.
        self.store.insert(&row).context("Instance already exists or its creation intent could not be persisted; inspect retained state")?;
        let archive = self
            .root
            .join("docker-instances")
            .join(format!("{instance}.tar"));
        let result = self
            .docker
            .prepare_new(
                self.store.host(),
                instance,
                row.image.clone(),
                &bundle,
                &archive,
                &config.options,
            )
            .await;
        let _ = std::fs::remove_file(&archive);
        match result {
            Ok(handle) => {
                row.handle = Some(handle);
                self.store.save_instance(&row)?;
                ensure!(
                    self.docker
                        .stop(row.handle.as_ref().context("missing container handle")?)
                        .await?,
                    "Container termination could not be confirmed after readiness probe"
                );
                row.stopped = true;
                row.prepared = true;
                self.store.save_instance(&row)?;
                Ok(row)
            }
            Err(error) => {
                // Retain partial state for explicit recovery, never recreate it.
                if let Ok(handle) = self
                    .docker
                    .discover(self.store.host(), instance, &row.image)
                    .await
                {
                    row.stopped = self.docker.stop(&handle).await.unwrap_or(false);
                    row.handle = Some(handle);
                    self.store.save_instance(&row)?;
                }
                Err(error).context("Container startup or readiness failed; verify image entrypoint, configured user, writable storage, and static Linux runtime compatibility")
            }
        }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Caller owns the instance lock and has persisted the imported image intent.
    pub(crate) async fn prepare_restored(&self, row: &Instance) -> Result<super::ContainerHandle> {
        let bundle = RuntimeBundle::open(
            self.bundle
                .as_deref()
                .context("matching runtime bundle unavailable")?,
        )?;
        let path = self
            .root
            .join("docker-instances")
            .join(format!("restore-{}.tar", row.id));
        // Only the harness-generated runtime archive is replaced, never guest data.
        if path.try_exists()? {
            std::fs::remove_file(&path)?;
        }
        let result = self
            .docker
            .prepare_new(
                self.store.host(),
                row.id,
                row.image.clone(),
                &bundle,
                &path,
                &row.configuration.options,
            )
            .await;
        let _ = std::fs::remove_file(path);
        result
    }
    /// Reconcile abandoned preparation without recreating containers or
    /// releasing retained storage. Live preparations keep their instance lock.
    pub async fn recover_preparations(&self, after: &mut String) -> Result<()> {
        let rows = self.store.unfinished(after)?;
        if rows.is_empty() {
            *after = String::new();
            return Ok(());
        }
        for id in rows {
            *after = id.to_string();
            self.recover_preparation(id).await?;
        }
        Ok(())
    }
    pub(crate) async fn recover_preparation(&self, id: uuid::Uuid) -> Result<()> {
        let Ok(_owner) = self.lock(id) else {
            return Ok(());
        };
        // The page is only a list of candidates. Reload all mutable state
        // under the owner lock: preparation or execution may have finished.
        let pending = self.store.preparation(id)?;
        let row = self.store.instance_if_present(id)?;
        if let Some(mut row) = row {
            if row.prepared {
                if let Some(config) = pending {
                    self.store.finish_preparation(
                        id,
                        &config,
                        Readiness::Ready,
                        None,
                        Some(&row.image.digest),
                    )?;
                }
                return Ok(());
            }
            if row.stopped {
                if let Some(config) = pending {
                    self.store.finish_preparation(id, &config, Readiness::Unavailable, Some("Container preparation was interrupted; retained resources require inspection"), Some(&row.image.digest))?;
                }
                return Ok(());
            }
            let handle = match row.handle.clone() {
                Some(handle) => handle,
                None => match self
                    .docker
                    .discover(self.store.host(), row.id, &row.image)
                    .await
                {
                    Ok(handle) => handle,
                    Err(_) => {
                        if let Some(config) = pending {
                            self.store.finish_preparation(id, &config, Readiness::Unavailable, Some("Interrupted container preparation cannot be inspected; check Docker availability and retained resources"), Some(&row.image.digest))?;
                        }
                        return Ok(());
                    }
                },
            };
            row.stopped = self.docker.stop(&handle).await.unwrap_or(false);
            row.handle = Some(handle);
            self.store.save_instance(&row)?;
            if let Some(config) = pending {
                self.store.finish_preparation(id, &config, Readiness::Unavailable, Some("Container preparation was interrupted; retained resources require inspection before a new preparation"), Some(&row.image.digest))?;
            }
        } else if let Some(config) = pending {
            self.store.finish_preparation(id, &config, Readiness::Unavailable, Some("Container preparation was interrupted before instance creation; retry preparation"), None)?;
        }
        Ok(())
    }
    pub fn start_recovery(&self) -> tokio::task::JoinHandle<()> {
        let environments = self.clone();
        tokio::spawn(async move {
            let mut cursor = String::new();
            loop {
                if let Err(error) = environments.recover_preparations(&mut cursor).await {
                    beale_host_log::error!(
                        "Docker preparation recovery remains pending: {error:#}"
                    );
                }
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            }
        })
    }
}
