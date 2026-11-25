//! Validated, bundled graph definitions. The catalog is the source of graph topology and scheduling.

use super::{Edge, GraphDefinition, GraphMode, Handoff, NodeLayout, ProfileId, Stage, WhenEmpty};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;

use super::profiles::profile;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    schema_version: u32,
    graphs: Vec<Manifest>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    id: String,
    revision: u32,
    name: String,
    description: String,
    entry: String,
    scheduler: Scheduler,
    nodes: Vec<Node>,
    delegation: Vec<Delegation>,
    handoffs: Vec<HandoffSpec>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scheduler {
    mode: GraphMode,
    max_active_top_level: usize,
    #[serde(default)]
    default_stage_minutes: u32,
    #[serde(default)]
    grace_minutes: u32,
    #[serde(default)]
    stages: Vec<StageSpec>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Node {
    id: String,
    profile: String,
    x: f32,
    y: f32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Delegation {
    parents: Vec<String>,
    children: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HandoffSpec {
    from: String,
    to: String,
    #[serde(default)]
    bidirectional: bool,
    payload: String,
    reverse_payload: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StageSpec {
    role: String,
    when_empty: WhenEmpty,
}

fn role(id: &str) -> Result<ProfileId, String> {
    ProfileId::from_id(id).ok_or_else(|| format!("unknown graph role: {id}"))
}

fn payload(id: &str) -> Result<&'static str, String> {
    match id {
        "task@1" => Ok("task@1"),
        "ranked_targets@1" => Ok("ranked_targets@1"),
        _ => Err(format!("unknown handoff payload: {id}")),
    }
}

fn build(manifest: Manifest) -> Result<GraphDefinition, String> {
    if manifest.id.is_empty()
        || manifest.revision != 1
        || manifest.name.trim().is_empty()
        || manifest.name.len() > 200
        || manifest.description.trim().is_empty()
        || manifest.description.len() > 4_096
        || manifest.nodes.is_empty()
        || manifest.nodes.len() > 64
    {
        return Err("invalid graph metadata".to_owned());
    }
    let mut seen = HashSet::new();
    let mut nodes = Vec::new();
    let mut layout = Vec::new();
    for node in manifest.nodes {
        let parsed = role(&node.id)?;
        if !seen.insert(parsed)
            || node.profile != format!("{}@{}", node.id, profile(parsed).revision)
            || !node.x.is_finite()
            || !node.y.is_finite()
        {
            return Err("invalid graph node".to_owned());
        }
        nodes.push(parsed);
        layout.push(NodeLayout {
            role: parsed,
            x: node.x,
            y: node.y,
        });
    }
    let entry = role(&manifest.entry)?;
    if !seen.contains(&entry) || !entry.is_top_level() {
        return Err("invalid graph entry".to_owned());
    }
    let top_count = nodes.iter().filter(|role| role.is_top_level()).count();
    let scheduler = manifest.scheduler;
    if scheduler.max_active_top_level == 0
        || scheduler.max_active_top_level > top_count
        || (scheduler.mode == GraphMode::Single && top_count != 1)
        || (scheduler.mode == GraphMode::RotatingStages
            && (scheduler.max_active_top_level != 1
                || !(1..=1_440).contains(&scheduler.default_stage_minutes)
                || scheduler.grace_minutes == 0))
        || (scheduler.mode != GraphMode::RotatingStages
            && (!scheduler.stages.is_empty()
                || scheduler.default_stage_minutes != 0
                || scheduler.grace_minutes != 0))
    {
        return Err("invalid graph scheduler".to_owned());
    }
    let mut stages = Vec::new();
    let mut stage_roles = HashSet::new();
    for stage in scheduler.stages {
        let parsed = role(&stage.role)?;
        if !parsed.is_top_level() || !seen.contains(&parsed) || !stage_roles.insert(parsed) {
            return Err("invalid graph stage".to_owned());
        }
        stages.push(Stage {
            role: parsed,
            when_empty: stage.when_empty,
        });
    }
    if scheduler.mode == GraphMode::RotatingStages
        && (stage_roles.len() != top_count || stages.first().is_none_or(|s| s.role != entry))
    {
        return Err("incomplete graph stages".to_owned());
    }
    let mut edges = Vec::new();
    let mut delegated = HashSet::new();
    for rule in manifest.delegation {
        if rule.parents.is_empty() || rule.children.is_empty() {
            return Err("empty delegation rule".to_owned());
        }
        for parent in &rule.parents {
            let from = role(parent)?;
            for child in &rule.children {
                let to = role(child)?;
                if !from.is_top_level()
                    || to.is_top_level()
                    || !seen.contains(&from)
                    || !seen.contains(&to)
                    || !delegated.insert((from, to))
                {
                    return Err("invalid delegation rule".to_owned());
                }
                edges.push(Edge {
                    from,
                    to,
                    bidirectional: true,
                });
            }
        }
    }
    let mut handoffs = Vec::new();
    let mut routes = HashSet::new();
    for handoff in manifest.handoffs {
        let from = role(&handoff.from)?;
        let to = role(&handoff.to)?;
        if handoff.payload == "ranked_targets@1"
            && (from != ProfileId::AuditTargets || to != ProfileId::Discovery)
        {
            return Err("ranked target handoff requires Audit Targets to Discovery".to_owned());
        }
        if from == to
            || !from.is_top_level()
            || !to.is_top_level()
            || !seen.contains(&from)
            || !seen.contains(&to)
            || !routes.insert((from, to))
            || (handoff.bidirectional && !routes.insert((to, from)))
            || (!handoff.bidirectional && handoff.reverse_payload.is_some())
        {
            return Err("invalid handoff route".to_owned());
        }
        let forward = payload(&handoff.payload)?;
        let reverse = handoff
            .reverse_payload
            .as_deref()
            .map(payload)
            .transpose()?
            .unwrap_or(forward);
        handoffs.push(Handoff {
            from,
            to,
            payload: forward,
        });
        if handoff.bidirectional {
            handoffs.push(Handoff {
                from: to,
                to: from,
                payload: reverse,
            });
        }
        edges.push(Edge {
            from,
            to,
            bidirectional: handoff.bidirectional,
        });
    }
    Ok(GraphDefinition {
        id: Box::leak(manifest.id.into_boxed_str()),
        revision: manifest.revision,
        name: Box::leak(manifest.name.into_boxed_str()),
        description: Box::leak(manifest.description.into_boxed_str()),
        entry,
        mode: scheduler.mode,
        max_active_top_level: scheduler.max_active_top_level,
        default_stage_minutes: scheduler.default_stage_minutes,
        grace_minutes: scheduler.grace_minutes,
        stages: Box::leak(stages.into_boxed_slice()),
        nodes: Box::leak(nodes.into_boxed_slice()),
        edges: Box::leak(edges.into_boxed_slice()),
        handoffs: Box::leak(handoffs.into_boxed_slice()),
        layout: Box::leak(layout.into_boxed_slice()),
    })
}

pub(super) fn load(id: &str) -> GraphDefinition {
    let catalog: Catalog =
        serde_json::from_str(include_str!("../graphs.json")).expect("bundled graph catalog");
    assert_eq!(catalog.schema_version, 1, "bundled graph schema version");
    let mut ids = HashSet::new();
    for graph in &catalog.graphs {
        assert!(ids.insert(&graph.id), "duplicate bundled graph ID");
    }
    let manifest = catalog
        .graphs
        .into_iter()
        .find(|graph| graph.id == id)
        .expect("registered bundled graph");
    build(manifest).expect("valid bundled graph")
}

pub(super) fn custom_id(raw: &str) -> Result<String, String> {
    if raw.len() > 16_384 {
        return Err("graph template is too large".to_owned());
    }
    let manifest: Manifest = serde_json::from_str(raw).map_err(|_| "invalid graph template")?;
    if manifest.id.is_empty()
        || manifest.id.len() > 64
        || !manifest
            .id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err("invalid custom graph ID".to_owned());
    }
    let digest = Sha256::digest(raw.as_bytes());
    Ok(format!("custom:{}:{digest:x}", manifest.id))
}

pub(super) fn load_custom(raw: &str, id: &str) -> Result<GraphDefinition, String> {
    if custom_id(raw)? != id {
        return Err("graph snapshot ID does not match its definition".to_owned());
    }
    let mut manifest: Manifest = serde_json::from_str(raw).map_err(|_| "invalid graph template")?;
    if manifest.scheduler.mode == GraphMode::Single {
        return Err("custom single-agent graphs are not supported".to_owned());
    }
    if !manifest
        .nodes
        .iter()
        .any(|node| role(&node.id).is_ok_and(|role| !role.is_top_level()))
    {
        return Err("custom graph needs a specialist role".to_owned());
    }
    if manifest.nodes.iter().any(|node| {
        role(&node.id).is_ok_and(ProfileId::is_top_level)
            && !manifest.delegation.iter().any(|rule| {
                rule.parents.contains(&node.id)
                    && rule
                        .children
                        .iter()
                        .any(|child| role(child).is_ok_and(|role| !role.is_top_level()))
            })
    }) {
        return Err("every top-level role must have a specialist delegation".to_owned());
    }
    manifest.id = id.to_owned();
    build(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_graphs_validate() {
        for id in [
            "simple",
            "cloudflare_harness",
            "openai_pipeline",
            "sequential_pipeline",
        ] {
            let graph = load(id);
            assert_eq!(graph.id, id);
            assert_eq!(graph.revision, 1);
        }
    }

    #[test]
    fn rejects_unknown_profiles_and_invalid_routes() {
        let mut value: serde_json::Value =
            serde_json::from_str(include_str!("../graphs.json")).unwrap();
        value["graphs"][0]["nodes"][1]["profile"] = "other@1".into();
        let catalog: Catalog = serde_json::from_value(value.clone()).unwrap();
        assert!(build(catalog.graphs.into_iter().next().unwrap()).is_err());

        value["graphs"][0]["nodes"][1]["profile"] = "scout@1".into();
        value["graphs"][0]["delegation"][0]["parents"][0] = "scout".into();
        let catalog: Catalog = serde_json::from_value(value).unwrap();
        assert!(build(catalog.graphs.into_iter().next().unwrap()).is_err());
    }

    #[test]
    fn bundled_routes_and_stage_policy_match_current_flows() {
        let pipeline = load("openai_pipeline");
        assert_eq!(pipeline.handoffs.len(), 8);
        assert_eq!(pipeline.max_active_top_level, 5);
        assert!(pipeline.handoffs.iter().any(|route| {
            route.from == ProfileId::AuditTargets
                && route.to == ProfileId::Discovery
                && route.payload == "ranked_targets@1"
        }));
        assert!(pipeline.handoffs.iter().any(|route| {
            route.from == ProfileId::Discovery
                && route.to == ProfileId::AuditTargets
                && route.payload == "task@1"
        }));
        let sequential = load("sequential_pipeline");
        assert_eq!(sequential.handoffs.len(), 9);
        assert_eq!(sequential.default_stage_minutes, 60);
        assert_eq!(sequential.grace_minutes, 10);
        assert_eq!(sequential.stages.len(), 5);
        assert_eq!(sequential.stages[0].when_empty, WhenEmpty::Continue);
        assert_eq!(sequential.stages[2].when_empty, WhenEmpty::Advance);
    }
}
