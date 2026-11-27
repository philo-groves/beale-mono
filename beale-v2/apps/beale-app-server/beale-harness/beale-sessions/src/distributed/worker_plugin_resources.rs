use super::*;
use beale_contracts::swarm_plugins::{ResourceReply, ResourceRequest, ResourceResult};
use beale_storage::swarm_files::{hash, materialize_plugin_resource};

impl Worker<'_> {
    async fn plugin_resource(&self, request: ResourceRequest) -> Result<(String, ResourceResult)> {
        let id = format!(
            "plugin-resource:{}",
            hash(&serde_json::to_vec(&(&self.launch.identity, &request))?)
        );
        let reply: ResourceReply = serde_json::from_value(
            self.remote(self.request(
                id.clone(),
                WorkerOperation::PluginResources {
                    request: request.clone(),
                },
            ))
            .await?,
        )?;
        reply
            .validate_for(&self.launch.identity, &request)
            .map_err(anyhow::Error::msg)?;
        Ok((id, reply.result))
    }
    /// Startup is checkpointed only after resources have been installed. Replays
    /// reuse accepted manifests/chunks and verify any already-installed files.
    pub(super) async fn prepare_plugin_resources(&self, enabled: &[String]) -> Result<()> {
        if !enabled
            .iter()
            .any(|name| matches!(name.as_str(), "read" | "plugin_call"))
        {
            return Ok(());
        }
        let (catalog_id, catalog) = self.plugin_resource(ResourceRequest::Catalog {}).await?;
        let plugins = match catalog {
            ResourceResult::Catalog { plugins } => plugins,
            ResourceResult::Unavailable { .. } => {
                self.spool.acknowledge(&catalog_id)?;
                return Ok(());
            }
            _ => anyhow::bail!("invalid plugin catalog"),
        };
        for plugin in plugins {
            let (prepare_id, prepared) = self
                .plugin_resource(ResourceRequest::Prepare {
                    plugin: plugin.clone(),
                })
                .await?;
            let (bundle, manifest) = match prepared {
                ResourceResult::Prepared { bundle, manifest } => (bundle, manifest),
                ResourceResult::Unavailable { .. } => {
                    self.spool.acknowledge(&prepare_id)?;
                    continue;
                }
                _ => anyhow::bail!("invalid prepared plugin resources"),
            };
            ensure!(
                hash(&serde_json::to_vec(&manifest)?) == bundle,
                "plugin manifest integrity mismatch"
            );
            for file in &manifest.files {
                self.deadline()?;
                let mut bytes = Vec::new();
                let mut operations = Vec::new();
                while (bytes.len() as u64) < file.size {
                    let offset = bytes.len() as u64;
                    let (id, result) = self
                        .plugin_resource(ResourceRequest::Read {
                            plugin: plugin.clone(),
                            bundle: bundle.clone(),
                            path: file.path.clone(),
                            offset,
                        })
                        .await?;
                    let ResourceResult::Chunk {
                        file: captured,
                        bytes: chunk,
                        sha256,
                        ..
                    } = result
                    else {
                        anyhow::bail!("plugin resource download unavailable");
                    };
                    ensure!(
                        captured == *file && !chunk.is_empty() && hash(&chunk) == sha256,
                        "plugin resource chunk changed"
                    );
                    bytes.extend_from_slice(&chunk);
                    ensure!(bytes.len() as u64 <= file.size, "plugin resource overflow");
                    operations.push(id);
                }
                let directory = beale_storage::swarm_state::plugin_resource_directory(
                    &self.launch.identity,
                    &bundle,
                    &plugin,
                )?;
                let destination = format!("{directory}/{}", file.path);
                let workspace = self.launch.workspace.clone();
                let file = file.clone();
                tokio::task::spawn_blocking(move || {
                    materialize_plugin_resource(&workspace, &destination, &file, &bytes)
                })
                .await??;
                self.deadline()?;
                for id in operations {
                    self.spool.acknowledge(&id)?;
                }
            }
            self.spool.acknowledge(&prepare_id)?;
        }
        self.spool.acknowledge(&catalog_id)?;
        Ok(())
    }
}
