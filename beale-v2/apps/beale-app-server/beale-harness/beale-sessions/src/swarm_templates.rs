//! Host-owned custom Swarm templates. Scan creation snapshots the selected definition.

use beale_contracts::v1::SwarmTemplate;
use std::{fs, path::Path};

const MAX_FILES: usize = 64;
const MAX_TEMPLATE_BYTES: u64 = 16_384;

pub(super) fn load(root: &Path) -> Vec<SwarmTemplate> {
    let directory = root.join("swarm-templates");
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
        let Ok(id) = beale_swarms::register_template(&definition_json) else {
            continue;
        };
        let Some(swarm) = beale_swarms::swarm_definition(&id) else {
            continue;
        };
        templates.push(SwarmTemplate {
            id,
            name: swarm.name.to_owned(),
            definition_json,
        });
    }
    templates.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    templates
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_edits_get_new_ids_without_changing_saved_snapshots() {
        let root = std::env::temp_dir().join(format!(
            "beale-swarm-template-test-{}",
            uuid::Uuid::new_v4()
        ));
        let directory = root.join("swarm-templates");
        fs::create_dir_all(&directory).unwrap();
        let raw = include_str!(
            "../../../../../extensions/features/beale-swarms/examples/example-review.json"
        );
        fs::write(directory.join("example.json"), raw).unwrap();
        fs::write(directory.join("invalid.json"), "{}").unwrap();
        let mut workerless = beale_swarms::BASIC_SWARM.snapshot.workflow().unwrap();
        workerless
            .nodes
            .retain(|node| node.role != beale_contracts::swarms::Role::Worker);
        workerless.delegation.retain(|edge| edge.parent == "leader");
        fs::write(
            directory.join("workerless.json"),
            serde_json::to_string(&workerless).unwrap(),
        )
        .unwrap();
        let first = load(&root);
        assert_eq!(first.len(), 1);
        let saved = beale_swarms::template_snapshot(&first[0].id).unwrap();
        let mut edited: serde_json::Value = serde_json::from_str(raw).unwrap();
        edited["name"] = serde_json::json!("Updated Example");
        fs::write(directory.join("example.json"), edited.to_string()).unwrap();
        let second = load(&root);
        assert_eq!(second.len(), 1);
        assert_ne!(first[0].id, second[0].id);
        assert_eq!(
            beale_swarms::restore_snapshot(&saved).unwrap().id,
            first[0].id
        );
        assert_eq!(
            beale_swarms::template_snapshot(&first[0].id).unwrap(),
            saved
        );
        let _ = fs::remove_dir_all(root);
    }
}
