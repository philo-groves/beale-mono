//! A Surface edit and its retry result share one atomic file replacement.
//! Acquire the workspace writer before opening a host receipt transaction.
use super::*;
use anyhow::{Context, ensure};
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SurfaceTarget {
    Manifest,
    ThreatModel(String),
}

impl SurfaceTarget {
    pub fn threat_model(id: &str) -> anyhow::Result<Self> {
        ensure!(
            threat_model::valid_id(id, 'S'),
            beale_contracts::tool_error::ToolError::invalid("invalid surface ID")
        );
        Ok(Self::ThreatModel(id.to_owned()))
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SurfaceReceipt {
    operation: String,
    request_hash: String,
    result: String,
}

pub(super) fn validate_receipts(receipts: &[SurfaceReceipt]) -> Result<(), WorkspaceError> {
    let mut ids = HashSet::new();
    if receipts.iter().any(|r| {
        !beale_contracts::swarms::valid_id(&r.operation)
            || !beale_contracts::swarm_files::digest(&r.request_hash)
            || r.result.len() > beale_contracts::swarms::MAX_DOCUMENT_BYTES / 2
            || !serde_json::from_str::<serde_json::Value>(&r.result)
                .is_ok_and(|value| value.is_object())
            || !ids.insert(&r.operation)
    }) {
        return Err(WorkspaceError::Internal(
            "invalid Surface operation receipt",
        ));
    }
    Ok(())
}

pub(super) enum PendingSurface {
    Manifest(StoredManifest),
    Threat(threat_model::StoredThreatModel),
}

pub(super) struct SurfaceStage {
    pub entry: RegistryEntry,
    pending: Mutex<Option<PendingSurface>>,
}
impl SurfaceStage {
    pub fn save(&self, pending: PendingSurface) -> Result<(), WorkspaceError> {
        let mut slot = self
            .pending
            .lock()
            .map_err(|_| WorkspaceError::Internal("Surface staging lock"))?;
        if slot.is_some() {
            return Err(WorkspaceError::Internal(
                "multiple Surface writes in one operation",
            ));
        }
        *slot = Some(pending);
        Ok(())
    }
}

pub struct SurfaceWriter<'a> {
    store: &'a WorkspaceStore,
    entry: RegistryEntry,
}

impl WorkspaceStore {
    /// The scoped writer holds no database lock. Its action may open the
    /// canonical receipt transaction before executing the prepared file edit.
    pub fn with_surface_writer<T>(
        &self,
        workspace: &str,
        action: impl FnOnce(&SurfaceWriter<'_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("workspace write lock"))?;
        let entry = self
            .entry(workspace)
            .map_err(|e| anyhow::anyhow!("workspace unavailable: {}", e.status()))?;
        action(&SurfaceWriter { store: self, entry })
    }
}

impl SurfaceWriter<'_> {
    fn load(&self, target: &SurfaceTarget) -> anyhow::Result<PendingSurface> {
        match target {
            SurfaceTarget::Manifest => Ok(PendingSurface::Manifest(
                WorkspaceStore::manifest(&self.entry).map_err(|e| {
                    anyhow::anyhow!("workspace manifest unavailable: {}", e.status())
                })?,
            )),
            SurfaceTarget::ThreatModel(id) => {
                let model = threat_model::read(Path::new(&self.entry.directory), id)
                    .map_err(|e| anyhow::anyhow!("threat model unavailable: {}", e.status()))?;
                // An absent model has no receipts; its first edit constructs it.
                Ok(PendingSurface::Threat(
                    model.unwrap_or_else(|| threat_model::empty(id)),
                ))
            }
        }
    }

    /// Only Surface feature logic may be passed here. Its writes are staged,
    /// then the result and edit are flushed together before returning.
    pub fn execute(
        &self,
        target: &SurfaceTarget,
        operation: &str,
        request_hash: &str,
        action: impl FnOnce(&WorkspaceStore) -> anyhow::Result<String>,
    ) -> anyhow::Result<String> {
        ensure!(
            beale_contracts::swarms::valid_id(operation)
                && beale_contracts::swarm_files::digest(request_hash),
            "invalid Surface operation identity"
        );
        let current = self.load(target)?;
        if let Some(receipt) = current.receipts().iter().find(|r| r.operation == operation) {
            ensure!(
                receipt.request_hash == request_hash,
                "Surface operation reused with changed content"
            );
            // A prior rename may have succeeded before its durability flush
            // reported failure. Reflush before accepting that cached result.
            let directory = Path::new(&self.entry.directory);
            let path = match target {
                SurfaceTarget::Manifest => directory.join("workspace.json"),
                SurfaceTarget::ThreatModel(id) => threat_model::model_path(directory, id)
                    .map_err(|_| anyhow::anyhow!("invalid Surface receipt path"))?,
            };
            OpenOptions::new().write(true).open(&path)?.sync_all()?;
            sync_directory(path.parent().context("Surface receipt parent missing")?)
                .map_err(|_| anyhow::anyhow!("Surface receipt directory flush failed"))?;
            return Ok(receipt.result.clone());
        }
        let stage = Arc::new(SurfaceStage {
            entry: self.entry.clone(),
            pending: Mutex::new(None),
        });
        let mut view = self.store.clone();
        view.surface_stage = Some(stage.clone());
        let result = action(&view)?;
        ensure!(
            result.len() <= beale_contracts::swarms::MAX_DOCUMENT_BYTES / 2,
            "Surface result exceeds receipt bound"
        );
        ensure!(
            serde_json::from_str::<serde_json::Value>(&result).is_ok_and(|value| value.is_object()),
            "Surface mutation result must be an object"
        );
        let mut pending = stage
            .pending
            .lock()
            .map_err(|_| anyhow::anyhow!("Surface staging lock"))?
            .take()
            .context("Surface operation did not stage a mutation")?;
        ensure!(
            pending.matches(target),
            "Surface operation wrote a different target"
        );
        pending.receipts_mut().push(SurfaceReceipt {
            operation: operation.into(),
            request_hash: request_hash.into(),
            result: result.clone(),
        });
        self.write(&pending)?;
        Ok(result)
    }

    /// Call after the leader receipt commits. Failure only leaves recovery
    /// metadata behind; it cannot invalidate the accepted canonical result.
    pub fn forget(&self, target: &SurfaceTarget, operation: &str) -> anyhow::Result<()> {
        let mut current = self.load(target)?;
        if !current.receipts().iter().any(|r| r.operation == operation) {
            return Ok(());
        }
        current.receipts_mut().retain(|r| r.operation != operation);
        self.write(&current)
    }

    fn write(&self, pending: &PendingSurface) -> anyhow::Result<()> {
        let directory = Path::new(&self.entry.directory);
        let result = match pending {
            PendingSurface::Manifest(manifest) => write_manifest(directory, manifest),
            PendingSurface::Threat(model) => threat_model::write(directory, model),
        };
        result.map_err(|e| anyhow::anyhow!("Surface operation persistence failed: {}", e.status()))
    }
}

impl PendingSurface {
    fn matches(&self, target: &SurfaceTarget) -> bool {
        match (self, target) {
            (Self::Manifest(_), SurfaceTarget::Manifest) => true,
            (Self::Threat(model), SurfaceTarget::ThreatModel(id)) => model.surface_id == *id,
            _ => false,
        }
    }
    fn receipts(&self) -> &[SurfaceReceipt] {
        match self {
            Self::Manifest(m) => &m.surface_receipts,
            Self::Threat(m) => &m.surface_receipts,
        }
    }
    fn receipts_mut(&mut self) -> &mut Vec<SurfaceReceipt> {
        match self {
            Self::Manifest(m) => &mut m.surface_receipts,
            Self::Threat(m) => &mut m.surface_receipts,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn staged_surface_failure_has_no_effect_and_file_receipts_survive_reopen() {
        let root = std::env::temp_dir().join(format!(
            "beale-surface-operation-example-{}",
            Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let workspace = root.join("workspace");
        let store = WorkspaceStore::at(root.join("host")).unwrap();
        let created = store
            .create(CreateWorkspaceRequest {
                name: "Example".into(),
                directory: workspace.to_string_lossy().into(),
                rules: vec!["Synthetic authorized scope".into()],
                ..Default::default()
            })
            .unwrap();
        let before = fs::read(workspace.join("workspace.json")).unwrap();
        let input = || {
            vec![WorkspaceSurfaceInput {
                id: String::new(),
                definition: Some(SurfaceDefinition {
                    type_id: "domain".into(),
                    name: "Example domain".into(),
                    locator: Some(SurfaceLocator {
                        kind: "domain".into(),
                        value: "example.test".into(),
                    }),
                    scope: SurfaceScope::InScope as i32,
                    scope_state: SurfaceScopeState::Approved as i32,
                    ..Default::default()
                }),
                relationships: vec![],
            }]
        };
        let prepare = |view: &WorkspaceStore| -> anyhow::Result<String> {
            let saved = view
                .update_surfaces_for_agent(&created.id, 1, input())
                .map_err(|s| anyhow::anyhow!("{s}"))?;
            Ok(json!({"id":saved.surfaces[0].id,"revision":saved.revision}).to_string())
        };
        let target = SurfaceTarget::Manifest;
        let fingerprint = "a".repeat(64);
        let failed = store.with_surface_writer(&created.id, |writer| {
            writer.execute(&target, "operation-example", &fingerprint, |view| {
                prepare(view)?;
                anyhow::bail!("synthetic feature failure after staging")
            })
        });
        assert!(failed.is_err());
        assert_eq!(fs::read(workspace.join("workspace.json")).unwrap(), before);
        for invalid_result in ["null".to_owned(), "{".to_owned(), "x".repeat(524_289)] {
            assert!(
                store
                    .with_surface_writer(&created.id, |writer| {
                        writer.execute(&target, "invalid-result", &fingerprint, |view| {
                            prepare(view)?;
                            Ok(invalid_result)
                        })
                    })
                    .is_err()
            );
            assert_eq!(fs::read(workspace.join("workspace.json")).unwrap(), before);
        }
        let rules = fs::read(workspace.join("RULES.md")).unwrap();
        fs::remove_file(workspace.join("RULES.md")).unwrap();
        fs::create_dir(workspace.join("RULES.md")).unwrap();
        assert!(
            store
                .with_surface_writer(&created.id, |writer| writer.execute(
                    &target,
                    "operation-example",
                    &fingerprint,
                    prepare
                ))
                .is_err()
        );
        assert_eq!(
            fs::read(workspace.join("workspace.json")).unwrap(),
            before,
            "failed companion-file persistence rolls the manifest back"
        );
        fs::remove_dir(workspace.join("RULES.md")).unwrap();
        fs::write(workspace.join("RULES.md"), rules).unwrap();
        let accepted = store
            .with_surface_writer(&created.id, |writer| {
                writer.execute(&target, "operation-example", &fingerprint, prepare)
            })
            .unwrap();
        drop(store);
        let reopened = WorkspaceStore::at(root.join("host")).unwrap();
        let replay = reopened
            .with_surface_writer(&created.id, |writer| {
                writer.execute(&target, "operation-example", &fingerprint, |_| {
                    panic!("replay must not execute the feature")
                })
            })
            .unwrap();
        assert_eq!(accepted, replay);
        let receipt_bytes = fs::read(workspace.join("workspace.json")).unwrap();
        let mut corrupt: serde_json::Value = serde_json::from_slice(&receipt_bytes).unwrap();
        corrupt["surfaceReceipts"][0]["result"] = json!("null");
        fs::write(
            workspace.join("workspace.json"),
            serde_json::to_vec(&corrupt).unwrap(),
        )
        .unwrap();
        assert!(
            reopened
                .with_surface_writer(&created.id, |writer| {
                    writer.execute(&target, "operation-example", &fingerprint, |_| {
                        panic!("corrupt receipt must not execute the feature")
                    })
                })
                .is_err()
        );
        fs::write(workspace.join("workspace.json"), receipt_bytes).unwrap();
        assert!(
            reopened
                .with_surface_writer(&created.id, |writer| writer.execute(
                    &target,
                    "operation-example",
                    &"b".repeat(64),
                    |_| panic!("changed retry must not execute")
                ))
                .is_err()
        );
        let saved = fs::read(workspace.join("workspace.json")).unwrap();
        assert!(
            reopened
                .with_surface_writer(&created.id, |writer| writer.execute(
                    &SurfaceTarget::ThreatModel("S1".into()),
                    "wrong-target",
                    &fingerprint,
                    |view| {
                        view.update_surfaces_for_agent(&created.id, 2, vec![])
                            .map_err(|s| anyhow::anyhow!("{s}"))?;
                        Ok("{}".into())
                    }
                ))
                .is_err()
        );
        assert_eq!(fs::read(workspace.join("workspace.json")).unwrap(), saved);
        reopened
            .with_surface_writer(&created.id, |writer| {
                writer.forget(&target, "operation-example")
            })
            .unwrap();
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(workspace.join("workspace.json")).unwrap()).unwrap();
        assert!(manifest.get("surfaceReceipts").is_none());
        assert_eq!(reopened.surfaces_for_host(&created.id).unwrap().revision, 2);
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }
}
