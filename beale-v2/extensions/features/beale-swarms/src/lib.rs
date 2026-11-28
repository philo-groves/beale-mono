//! Declarative Swarms. Execution and model-facing tools belong to the session harness.
use beale_contracts::{
    swarm_workflows::WorkflowDefinition,
    swarms::{DefinitionSnapshot, ProfileSnapshot, ReviewApplicability, ReviewRuleSnapshot},
};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, LazyLock, RwLock},
};
pub mod local;
mod profiles;
pub use profiles::{ProfileId, task_profiles};

pub const FEATURE_ID: &str = "swarms";
pub const FEATURE_NAME: &str = "Swarms";
pub const FEATURE_DESCRIPTION: &str =
    "Declarative leader, deputy, and worker research workflows with parent-mediated handoffs.";
pub const TOOL_COUNT: u32 = 7;
pub const HAS_PROMPT_EXTENSION: bool = false;
pub const MAX_CONCURRENT: u32 = 64;
pub const BASIC_ID: &str = "basic";
pub const VDH_VVS_ID: &str = "vdh_vvs";
pub const OAI_948_ID: &str = "oai_948";
pub const BASIC_NAME: &str = "Basic Subagents";
pub const BASIC_DESCRIPTION: &str = "One coordinating leader, Research Deputies for distinct areas, and bounded specialist workers.";
pub const FEATURE: beale_feature_api::Feature = beale_feature_api::Feature {
    id: FEATURE_ID,
    name: FEATURE_NAME,
    description: FEATURE_DESCRIPTION,
    prompt_extension: "",
    use_case: "Define research delegation and evidence handoffs.",
    tool_names: &[
        tool_names::HANDOFF,
        tool_names::LIST,
        tool_names::SPAWN,
        tool_names::MESSAGE,
        tool_names::STOP,
        tool_names::MESSAGE_PARENT,
        tool_names::MATERIALIZE,
    ],
    definitions: None,
    review: beale_feature_api::ToolReview::Exempt,
    subagents: false,
    migrations: &[],
    startup: &[],
};

#[derive(Clone, Copy, Debug)]
pub struct Edge {
    pub from: ProfileId,
    pub to: ProfileId,
    pub bidirectional: bool,
}
#[derive(Clone, Copy, Debug)]
pub struct NodeLayout {
    pub role: ProfileId,
    pub x: f32,
    pub y: f32,
}
pub struct SwarmDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub nodes: Vec<ProfileId>,
    pub edges: Vec<Edge>,
    pub layout: Vec<NodeLayout>,
    pub snapshot: DefinitionSnapshot,
}
fn build(definition: WorkflowDefinition) -> Result<SwarmDefinition, String> {
    definition.validate()?;
    let mut profiles = Vec::new();
    for node in &definition.nodes {
        let profile = ProfileId::from_id(&node.profile.id).ok_or("unknown specialist profile")?;
        if profile.revision() != node.profile.revision || profile.role() != node.role {
            return Err("profile revision or role mismatch".into());
        }
        profiles.push(ProfileSnapshot {
            profile: node.profile.clone(),
            role: node.role,
            instruction: profile.instruction().into(),
        });
    }
    let snapshot=DefinitionSnapshot{id:definition.id.clone(),revision:definition.revision,
        definition:serde_json::to_value(&definition).map_err(|_|"invalid definition")?,profiles,
        leader_rule:ReviewRuleSnapshot{id:"coordinator-only".into(),revision:1,applicability:ReviewApplicability::Leader,
            instruction:"Allow tools for coordination and managerial purposes. Dedicated research must be delegated to deputies or workers.".into()}};
    snapshot_view(snapshot)
}

// Profile identities supply presentation labels only. Saved instructions, roles,
// revisions, topology, and review rules come exclusively from the snapshot.
fn snapshot_view(snapshot: DefinitionSnapshot) -> Result<SwarmDefinition, String> {
    let definition = snapshot.workflow()?;
    let mut nodes = Vec::new();
    let mut layout = Vec::new();
    for node in &definition.nodes {
        let profile =
            ProfileId::from_id(&node.profile.id).ok_or("unsupported saved profile identity")?;
        nodes.push(profile);
        layout.push(NodeLayout {
            role: profile,
            x: node.x as f32,
            y: node.y as f32,
        });
    }
    let mut edges = Vec::new();
    for d in &definition.delegation {
        for child in &d.children {
            edges.push(Edge {
                from: ProfileId::from_id(&d.parent).ok_or("unknown parent")?,
                to: ProfileId::from_id(child).ok_or("unknown child")?,
                bidirectional: false,
            });
        }
    }
    Ok(SwarmDefinition {
        id: definition.id,
        name: definition.name,
        description: definition.description,
        nodes,
        edges,
        layout,
        snapshot,
    })
}
static BUNDLED: LazyLock<Vec<Arc<SwarmDefinition>>> = LazyLock::new(|| {
    let definitions: Vec<WorkflowDefinition> =
        serde_json::from_str(include_str!("../swarms.json")).expect("bundled swarm definitions");
    definitions
        .into_iter()
        .map(|d| Arc::new(build(d).expect("valid bundled swarm")))
        .collect()
});
static CUSTOM: LazyLock<RwLock<HashMap<String, Arc<SwarmDefinition>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
// Saved views must never replace a new-scan catalog entry, including bundled IDs.
static RESTORED: LazyLock<RwLock<SnapshotCache>> =
    LazyLock::new(|| RwLock::new(SnapshotCache::new(1024)));

struct SnapshotCache {
    capacity: usize,
    entries: VecDeque<([u8; 32], Arc<SwarmDefinition>)>,
}

impl SnapshotCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: VecDeque::new(),
        }
    }

    fn restore(&mut self, raw: &str) -> Result<Arc<SwarmDefinition>, String> {
        let snapshot = validate_snapshot(raw)?;
        let canonical = serde_json::to_vec(&snapshot).map_err(|_| "invalid swarm snapshot")?;
        let key: [u8; 32] = Sha256::digest(canonical).into();
        if let Some(index) = self.entries.iter().position(|(saved, _)| *saved == key) {
            let entry = self
                .entries
                .remove(index)
                .ok_or("snapshot cache entry unavailable")?;
            let view = Arc::clone(&entry.1);
            self.entries.push_back(entry);
            return Ok(view);
        }
        // Validate and build before eviction: invalid input must not displace valid views.
        let view = Arc::new(snapshot_view(snapshot)?);
        if self.capacity > 0 {
            if self.entries.len() == self.capacity {
                self.entries.pop_front();
            }
            self.entries.push_back((key, Arc::clone(&view)));
        }
        Ok(view)
    }
}

impl SwarmDefinition {
    pub fn is_top_level(&self, profile: ProfileId) -> bool {
        self.snapshot.profiles.iter().any(|p| {
            p.profile.id == profile.id() && p.role != beale_contracts::swarms::Role::Worker
        })
    }
    pub fn entry_role(&self) -> Option<ProfileId> {
        self.snapshot
            .profiles
            .iter()
            .find(|p| p.role == beale_contracts::swarms::Role::Leader)
            .and_then(|p| ProfileId::from_id(&p.profile.id))
    }
}
pub fn bundled() -> &'static [Arc<SwarmDefinition>] {
    &BUNDLED
}
pub fn swarm_definition(id: &str) -> Option<Arc<SwarmDefinition>> {
    BUNDLED
        .iter()
        .find(|d| d.id == id)
        .cloned()
        .or_else(|| CUSTOM.read().ok()?.get(id).cloned())
}
pub fn entry_role(id: &str) -> Option<ProfileId> {
    swarm_definition(id).map(|_| ProfileId::Leader)
}
pub fn has_hierarchy(id: &str) -> bool {
    swarm_definition(id).is_some()
}
pub fn template_snapshot(id: &str) -> Option<String> {
    serde_json::to_string(&swarm_definition(id)?.snapshot).ok()
}
/// Custom definitions are content addressed. No mutable registry entry can change a saved scan.
pub fn register_template(raw: &str) -> Result<String, String> {
    let mut definition = WorkflowDefinition::decode(raw)?;
    let canonical = serde_json::to_vec(&definition).map_err(|_| "invalid template")?;
    let id = format!("custom:{:x}", Sha256::digest(canonical));
    definition.id = id.clone();
    let mut registry = CUSTOM.write().map_err(|_| "swarm registry unavailable")?;
    if !registry.contains_key(&id) {
        if registry.len() >= 1024 {
            return Err("too many swarm revisions".into());
        }
        registry.insert(id.clone(), Arc::new(build(definition)?));
    }
    Ok(id)
}
/// Restore saved profiles and rules, independent of installed template revisions.
pub fn validate_snapshot(raw: &str) -> Result<DefinitionSnapshot, String> {
    if raw.len() > 1_048_576 {
        return Err("swarm snapshot too large".into());
    }
    let snapshot: DefinitionSnapshot =
        serde_json::from_str(raw).map_err(|_| "invalid swarm snapshot")?;
    snapshot.workflow()?;
    if snapshot
        .profiles
        .iter()
        .any(|p| p.instruction.trim().is_empty() || p.instruction.len() > 32768)
        || !beale_contracts::swarms::valid_id(&snapshot.leader_rule.id)
        || snapshot.leader_rule.revision == 0
        || snapshot.leader_rule.instruction.trim().is_empty()
        || snapshot.leader_rule.instruction.len() > 32768
    {
        return Err("invalid profile or leader rule snapshot".into());
    }
    Ok(snapshot)
}

/// Restore a saved view without consulting or mutating the new-scan catalog.
/// The complete snapshot keys the cache so revisions of the same ID can coexist.
/// Eviction releases only the cache's reference; active callers retain their views.
pub fn restore_snapshot(raw: &str) -> Result<Arc<SwarmDefinition>, String> {
    RESTORED
        .write()
        .map_err(|_| "swarm registry unavailable")?
        .restore(raw)
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextPolicy {
    EvidenceOnly,
    Shared,
}

pub static BASIC_SWARM: LazyLock<Arc<SwarmDefinition>> = LazyLock::new(|| Arc::clone(&BUNDLED[0]));
pub static VDH_VVS_SWARM: LazyLock<Arc<SwarmDefinition>> =
    LazyLock::new(|| Arc::clone(&BUNDLED[1]));
pub static OAI_948_SWARM: LazyLock<Arc<SwarmDefinition>> =
    LazyLock::new(|| Arc::clone(&BUNDLED[2]));

/// Create durable definition state only. Allocation and execution are separate stages.
/// No Subagents does not initialize the Swarms repository.
pub fn create_scan(
    db: beale_storage::HostDatabase,
    mut config: beale_contracts::swarms::ScanConfig,
    selection: &str,
    enabled: bool,
) -> anyhow::Result<Option<beale_storage::swarm_state::ScanState>> {
    use beale_contracts::swarms::ScanMode;
    config.mode = if selection.is_empty() {
        ScanMode::Local {}
    } else {
        anyhow::ensure!(enabled, "Swarms is disabled for new scans");
        let definition = swarm_definition(selection)
            .ok_or_else(|| anyhow::anyhow!("unknown Swarm definition"))?;
        ScanMode::Swarm {
            snapshot: Box::new(definition.snapshot.clone()),
        }
    };
    config
        .validate_creation(enabled, true)
        .map_err(anyhow::Error::msg)?;
    if matches!(config.mode, ScanMode::Local {}) {
        return Ok(None);
    }
    beale_storage::swarm_state::SwarmStore::open(db)?
        .create_scan(config, enabled, true)
        .map(Some)
}

/// Role-specific tools are assembled by the host from saved agent identity.
pub mod tool_names {
    pub const HANDOFF: &str = "swarm_handoff";
    pub const MATERIALIZE: &str = "swarm_materialize_artifact";
    pub const LIST: &str = "swarm_list_subagents";
    pub const SPAWN: &str = "swarm_spawn_subagent";
    pub const MESSAGE: &str = "swarm_message_subagent";
    pub const STOP: &str = "swarm_stop_subagent";
    pub const MESSAGE_PARENT: &str = "swarm_message_parent";
}

/// Hierarchy control tools. They change only host-owned Swarm coordination
/// state, never the workspace, and their authority is enforced structurally by
/// the host (direct-child relationships, saved routes and profiles). A leader
/// keeps them under the coordinator-only rule without a model review, so a
/// review-service outage cannot stop it from delegating or reading results.
/// Artifact materialization writes files and remains reviewed for a leader.
pub fn coordination_tool(name: &str) -> bool {
    use tool_names::*;
    matches!(
        name,
        HANDOFF | LIST | SPAWN | MESSAGE | STOP | MESSAGE_PARENT
    )
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    fn saved(name: &str) -> String {
        let mut snapshot = BASIC_SWARM.snapshot.clone();
        snapshot.definition["name"] = serde_json::json!(name);
        serde_json::to_string(&snapshot).unwrap()
    }

    #[test]
    fn eviction_releases_unused_views_and_preserves_active_views() {
        let mut cache = SnapshotCache::new(2);
        let raw_a = saved("Example A");
        let raw_b = saved("Example B");
        let a = cache.restore(&raw_a).unwrap();
        let snapshot_a = a.snapshot.clone();
        let weak_a = Arc::downgrade(&a);
        let b = cache.restore(&raw_b).unwrap();
        let weak_b = Arc::downgrade(&b);
        drop(b);
        assert!(Arc::ptr_eq(&a, &cache.restore(&raw_a).unwrap()));
        let c = cache.restore(&saved("Example C")).unwrap();
        assert_eq!(cache.entries.len(), 2);
        assert!(
            weak_b.upgrade().is_none(),
            "the least recently used view must be released"
        );
        assert_eq!(c.name, "Example C");
        let reopened_b = cache.restore(&raw_b).unwrap();
        assert_eq!(reopened_b.name, "Example B");
        assert_eq!(a.snapshot, snapshot_a);
        assert_eq!(
            Arc::strong_count(&a),
            1,
            "only the active caller retains the evicted view"
        );
        drop(a);
        assert!(weak_a.upgrade().is_none());
        let reopened_a = cache.restore(&raw_a).unwrap();
        assert_eq!(reopened_a.snapshot, snapshot_a);
        assert_eq!(cache.entries.len(), 2);
    }

    #[test]
    fn invalid_snapshots_do_not_evict_or_reorder_cached_views() {
        let mut cache = SnapshotCache::new(2);
        let a = cache.restore(&saved("Example A")).unwrap();
        let b = cache.restore(&saved("Example B")).unwrap();
        assert!(cache.restore("{}").is_err());
        let unsupported = saved("Unsupported Example").replace("mapper", "example_unknown");
        assert!(validate_snapshot(&unsupported).is_ok());
        assert!(cache.restore(&unsupported).is_err());
        assert_eq!(cache.entries.len(), 2);
        assert_eq!(Arc::strong_count(&a), 2);
        assert_eq!(Arc::strong_count(&b), 2);
        cache.restore(&saved("Example C")).unwrap();
        assert_eq!(Arc::strong_count(&a), 1);
        assert_eq!(Arc::strong_count(&b), 2);
    }
}
