//! Host-owned custom graph templates. Sessions retain a copy of the selected file.

use beale_contracts::v1::GraphTemplate;
use std::{fs, path::Path};

const MAX_FILES: usize = 64;
const MAX_TEMPLATE_BYTES: u64 = 16_384;

pub(super) fn load(root: &Path) -> Vec<GraphTemplate> {
    let directory = root.join("graph-templates");
    if !directory.is_dir()
        || fs::symlink_metadata(&directory).is_ok_and(|meta| meta.file_type().is_symlink())
    {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut paths = entries
        .take(MAX_FILES * 4)
        .flatten()
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    paths.sort();
    let mut templates = Vec::new();
    for path in paths.into_iter().take(MAX_FILES) {
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.is_file() || metadata.len() > MAX_TEMPLATE_BYTES {
            continue;
        }
        let Ok(definition_json) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(id) = beale_graphs::register_template(&definition_json) else {
            continue;
        };
        let Some(graph) = beale_graphs::graph_definition(&id) else {
            continue;
        };
        templates.push(GraphTemplate {
            id,
            name: graph.name.to_owned(),
            definition_json,
        });
    }
    templates.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    templates
}

#[cfg(test)]
mod tests {
    use super::*;
    use beale_contracts::v1::{SessionGraphConfig, SubmitSessionTurnRequest};
    use beale_graphs::GraphStore;
    use beale_storage::{HostDatabase, SessionRow};
    use prost::Message;
    use uuid::Uuid;

    #[test]
    fn loads_custom_file_and_restores_saved_definition_after_file_changes() {
        let root =
            std::env::temp_dir().join(format!("beale-graph-template-example-{}", Uuid::new_v4()));
        let directory = root.join("graph-templates");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("example.json");
        let first = serde_json::json!({
            "id":"example_review","revision":1,"name":"Example Review",
            "description":"Synthetic review flow.","entry":"discovery_harness",
            "scheduler":{"mode":"concurrent","max_active_top_level":2},
            "nodes":[
                {"id":"discovery_harness","profile":"discovery_harness@1","x":0,"y":0},
                {"id":"validation_system","profile":"validation_system@1","x":100,"y":0},
                {"id":"reviewer","profile":"reviewer@1","x":50,"y":100}
            ],
            "delegation":[{"parents":["discovery_harness","validation_system"],"children":["reviewer"]}],
            "handoffs":[{"from":"discovery_harness","to":"validation_system","bidirectional":true,"payload":"task@1"}]
        }).to_string();
        fs::write(&path, &first).unwrap();
        let listed = load(&root);
        assert_eq!(listed.len(), 1);
        let old_id = listed[0].id.clone();
        let db = HostDatabase::open(&root).unwrap();
        crate::features::migrate(&db).unwrap();
        let session = SessionRow {
            id: Uuid::new_v4().to_string(),
            workspace_id: "workspace-example".to_owned(),
            title: "Example review".to_owned(),
            created_at_ms: 100,
            last_message_at_ms: 100,
        };
        let request = SubmitSessionTurnRequest {
            graph_id: old_id.clone(),
            ..Default::default()
        };
        let encoded = crate::encoded_graph_config(&request).unwrap();
        let config = SessionGraphConfig::decode(encoded.as_slice()).unwrap();
        assert_eq!(config.template_json, first);
        db.create_peer_graph_session(&session, &old_id, &encoded, None)
            .unwrap();
        let second = first.replace("Example Review", "Example Revised Review");
        fs::write(&path, second).unwrap();
        assert_ne!(load(&root)[0].id, old_id);
        crate::restore_graph_template(&db, &session.id).unwrap();
        assert_eq!(
            beale_graphs::template_snapshot(&old_id).as_deref(),
            Some(first.as_str())
        );
        drop(db);
        fs::remove_dir_all(root).unwrap();
    }
}
