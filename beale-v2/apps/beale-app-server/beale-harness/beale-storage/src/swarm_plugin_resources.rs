//! Attempt-scoped immutable plugin bundles and replayable resource reads.
use super::*;
use crate::swarm_files::{ResearchFiles, hash};
use beale_contracts::{swarm_plugins::*, swarm_worker::ExecutionIdentity};

pub(super) const SCHEMA: &str = "
CREATE TABLE swarm_plugin_resources (
 scan_id TEXT NOT NULL REFERENCES swarm_scans(id), agent_id TEXT NOT NULL,
 attempt_id TEXT NOT NULL, generation INTEGER NOT NULL, plugin TEXT NOT NULL,
 bundle TEXT NOT NULL, manifest TEXT NOT NULL,
 PRIMARY KEY(scan_id,agent_id,attempt_id,plugin)
);
";
pub struct PreparedPluginBundle {
    pub manifest: ResourceManifest,
    pub bytes: Vec<Vec<u8>>,
}
pub enum PluginResourceInput {
    Catalog(Vec<String>),
    Snapshot(PreparedPluginBundle),
    Retained,
    Unavailable(String),
}
/// Each execution gets a private resource copy. Resuming an agent must not
/// overwrite files modified by a prior attempt or invalidate its old references.
pub fn plugin_resource_directory(
    execution: &ExecutionIdentity,
    bundle: &str,
    plugin: &str,
) -> Result<String> {
    execution.validate().map_err(anyhow::Error::msg)?;
    ResourceRequest::Prepare {
        plugin: plugin.into(),
    }
    .validate()
    .map_err(anyhow::Error::msg)?;
    ensure!(
        beale_contracts::swarm_files::digest(bundle),
        "invalid plugin bundle digest"
    );
    let scope = hash(&serde_json::to_vec(execution)?);
    Ok(format!("plugins/{scope}/{bundle}/{plugin}"))
}
fn identity(r: &WorkerRequest) -> ExecutionIdentity {
    ExecutionIdentity {
        workspace_id: r.workspace_id.clone(),
        scan_id: r.scan_id.clone(),
        agent_id: r.agent_id.clone(),
        attempt_id: r.attempt_id.clone(),
        generation: r.generation,
    }
}
fn prepared(
    db: &Connection,
    r: &WorkerRequest,
    plugin: &str,
) -> Result<Option<(String, ResourceManifest)>> {
    let value:Option<(String,String)>=db.query_row("SELECT bundle,manifest FROM swarm_plugin_resources WHERE scan_id=?1 AND agent_id=?2 AND attempt_id=?3 AND generation=?4 AND plugin=?5",params![r.scan_id,r.agent_id,r.attempt_id,i64::try_from(r.generation)?,plugin],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
    value
        .map(|(bundle, raw)| {
            let manifest: ResourceManifest = serde_json::from_str(&raw)?;
            manifest.validate().map_err(anyhow::Error::msg)?;
            ensure!(
                manifest.plugin == plugin && hash(&serde_json::to_vec(&manifest)?) == bundle,
                "plugin bundle integrity failure"
            );
            Ok((bundle, manifest))
        })
        .transpose()
}
fn available(db: &Connection, plugin: &str, workspace: &str) -> Result<bool> {
    Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM plugin_installs WHERE name=?1) AND (COALESCE((SELECT all_workspaces FROM extension_scope_modes WHERE kind='plugin' AND extension_id=?1),1)=1 OR EXISTS(SELECT 1 FROM extension_workspace_scope WHERE kind='plugin' AND extension_id=?1 AND workspace_id=?2))",params![plugin,workspace],|row|row.get(0))?)
}
impl SwarmStore {
    pub fn prepared_plugin_manifest(
        &self,
        r: &WorkerRequest,
        plugin: &str,
    ) -> Result<Option<(String, ResourceManifest)>> {
        self.db.with_connection(|db| prepared(db, r, plugin))
    }
    pub fn worker_plugin_resources(
        &self,
        files: &ResearchFiles,
        bearer: &str,
        r: &WorkerRequest,
        now: i64,
        input: PluginResourceInput,
    ) -> Result<MutationAcknowledgement> {
        let WorkerOperation::PluginResources { request } = &r.operation else {
            anyhow::bail!("not a plugin resource request");
        };
        let (catalog, snapshot, unavailable) = match input {
            PluginResourceInput::Catalog(v) => (Some(v), None, None),
            PluginResourceInput::Snapshot(v) => (None, Some(v), None),
            PluginResourceInput::Retained => (None, None, None),
            PluginResourceInput::Unavailable(v) => (None, None, Some(v)),
        };
        self.dispatch_mutation(bearer,WORKER_AUDIENCE,r,now,|tx,_| {
            let retained=match request {ResourceRequest::Prepare {plugin}=>prepared(tx,r,plugin)?,_=>None};
            let result=if let Some((bundle,manifest))=retained {ResourceResult::Prepared {bundle,manifest}}
            else if matches!(request,ResourceRequest::Prepare {plugin} if !available(tx,plugin,&r.workspace_id)?) {ResourceResult::Unavailable {message:"Plugin is no longer available in this workspace.".into()}}
            else if let Some(message)=unavailable {ResourceResult::Unavailable {message}} else {match request {
                ResourceRequest::Catalog {}=>{
                    let mut plugins=Vec::new();
                    for plugin in catalog.context("plugin catalog missing")? {if available(tx,&plugin,&r.workspace_id)? {plugins.push(plugin);}}
                    ResourceResult::Catalog {plugins}
                },
                ResourceRequest::Prepare {plugin}=>{
                    let (bundle,manifest)=match prepared(tx,r,plugin)? {
                        Some(saved)=>saved,
                        None=>{
                            let snapshot=snapshot.context("plugin snapshot missing")?;
                            snapshot.manifest.validate().map_err(anyhow::Error::msg)?;
                            ensure!(snapshot.manifest.plugin==*plugin && snapshot.bytes.len()==snapshot.manifest.files.len(),"invalid plugin resource capture");
                            for (bytes,file) in snapshot.bytes.iter().zip(&snapshot.manifest.files) {
                                ensure!(bytes.len() as u64==file.size && hash(bytes)==file.sha256,"plugin capture integrity mismatch");
                                files.put_blob(bytes)?;
                            }
                            let raw=serde_json::to_string(&snapshot.manifest)?;
                            let bundle=hash(raw.as_bytes());
                            tx.execute("INSERT INTO swarm_plugin_resources VALUES(?1,?2,?3,?4,?5,?6,?7)",params![r.scan_id,r.agent_id,r.attempt_id,i64::try_from(r.generation)?,plugin,bundle,raw])?;
                            (bundle,snapshot.manifest)
                        }
                    };
                    ResourceResult::Prepared {bundle,manifest}
                },
                ResourceRequest::Read {plugin,bundle,path,offset}=>{
                    let (saved,manifest)=prepared(tx,r,plugin)?.context("plugin bundle not prepared for this execution")?;
                    ensure!(saved==*bundle,"plugin bundle identity changed");
                    let file=manifest.files.iter().find(|file|file.path==*path).context("plugin resource not declared")?.clone();
                    ensure!(*offset<=file.size,"invalid plugin resource offset");
                    let bytes=files.read_blob(&file.sha256,file.size)?;
                    let start=usize::try_from(*offset)?;
                    let bytes=bytes[start..(start+RESOURCE_CHUNK_BYTES).min(bytes.len())].to_vec();
                    ResourceResult::Chunk {bundle:saved,file,offset:*offset,sha256:hash(&bytes),bytes}
                }
            }};
            if let ResourceResult::Unavailable {message}=&result {
                let mut state=read_scan(tx,&r.scan_id)?;
                let sequence=next(u64::try_from(state.events.len())?)?;
                state.events.push(LifecycleEvent {id:format!("plugin-resources-{}",hash(serde_json::to_string(r)?.as_bytes())),agent_id:r.agent_id.clone(),sequence,kind:"plugin-resources-unavailable".into(),payload:json!({"attempt_id":r.attempt_id,"generation":r.generation,"warning":message,"plugin":match request {ResourceRequest::Catalog {}=>"catalog",ResourceRequest::Prepare {plugin}|ResourceRequest::Read {plugin,..}=>plugin.as_str()}})});
                write_scan(tx,&mut state)?;
            }
            let reply=ResourceReply {identity:identity(r),result};
            reply.validate_for(&identity(r),request).map_err(anyhow::Error::msg)?;
            Ok(serde_json::to_value(reply)?)
        })
    }
    /// Host-generated model guidance; paths name only the private workspace.
    pub fn worker_plugin_guidance(&self, execution: &ExecutionIdentity) -> Result<String> {
        self.db.with_connection(|db| {
            let rows=db.prepare("SELECT bundle,manifest FROM swarm_plugin_resources WHERE scan_id=?1 AND agent_id=?2 AND attempt_id=?3 AND generation=?4 ORDER BY plugin")?.query_map(params![execution.scan_id,execution.agent_id,execution.attempt_id,i64::try_from(execution.generation)?],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
            let mut result=String::new();
            for (bundle,raw) in rows {
                let manifest:ResourceManifest=serde_json::from_str(&raw)?;manifest.validate().map_err(anyhow::Error::msg)?;
                ensure!(hash(&serde_json::to_vec(&manifest)?)==bundle,"plugin resource integrity failure");
                result.push_str(&format!("- Plugin `{}`: read `{}/PLUGIN.md` in your assigned workspace.\n",manifest.plugin,plugin_resource_directory(execution,&bundle,&manifest.plugin)?));
            }
            let state=read_scan(db,&execution.scan_id)?;
            for event in state.events.iter().filter(|e|e.agent_id==execution.agent_id && e.kind=="plugin-resources-unavailable" && e.payload["attempt_id"]==execution.attempt_id && e.payload["generation"]==execution.generation) {
                if let Some(warning)=event.payload["warning"].as_str() {result.push_str(&format!("- Plugin `{}` preparation warning: {warning}\n",event.payload["plugin"].as_str().unwrap_or("catalog")));}
            }
            Ok(result)
        })
    }
}
