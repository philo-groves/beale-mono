//! Native features compiled into this host, read from each crate's descriptor.
//!
//! Session assembly, subagent tool lists, review policy, and the host feature
//! catalog all iterate this list instead of restating per-feature details.

use beale_feature_api::Feature;
use beale_storage::HostDatabase;

/// Session-capable native features in this build, in extension-catalog order.
pub fn native_features() -> Vec<&'static Feature> {
    vec![
        #[cfg(feature = "fleet")]
        &beale_fleet::FEATURE,
        #[cfg(feature = "webfetch")]
        &beale_webfetch::FEATURE,
        #[cfg(feature = "surfaces")]
        &beale_surfaces::FEATURE,
        #[cfg(feature = "findings")]
        &beale_findings::FEATURE,
        #[cfg(feature = "memories")]
        &beale_memories::FEATURE,
        #[cfg(feature = "agent-feedback")]
        &beale_agent_feedback::FEATURE,
        #[cfg(feature = "runbooks")]
        &beale_runbooks::FEATURE,
        #[cfg(feature = "goal-mode")]
        &beale_goals::FEATURE,
        #[cfg(feature = "graphs")]
        &beale_graphs::FEATURE,
    ]
}

/// Applies every compiled feature's schema migrations and startup statements.
/// Features own their tables; a build without a feature never creates them.
pub fn migrate(db: &HostDatabase) -> anyhow::Result<()> {
    migrate_features(db, &native_features())
}

/// Applies the given features' migrations and startup statements.
pub fn migrate_features(db: &HostDatabase, features: &[&Feature]) -> anyhow::Result<()> {
    for feature in features {
        if !feature.migrations.is_empty() {
            db.migrate_schema(feature.id, feature.migrations)?;
        }
        if !feature.startup.is_empty() {
            db.run_startup_statements(feature.id, feature.startup)?;
        }
    }
    Ok(())
}

/// Whether a compiled feature is enabled and scoped to the workspace.
pub(crate) fn applies(db: &HostDatabase, id: &str, workspace_id: &str) -> anyhow::Result<bool> {
    Ok(native_features().iter().any(|feature| feature.id == id)
        && db.feature_enabled(id)?
        && db.extension_applies_to("feature", id, workspace_id)?)
}

/// Whether a feature tool call needs Auto-Review.
pub(crate) fn reviews(name: &str) -> bool {
    native_features()
        .iter()
        .any(|feature| feature.reviews(name))
}

/// Whether graph subagents receive a feature.
#[cfg(all(test, feature = "graphs"))]
pub(crate) fn subagents_receive(id: &str) -> bool {
    native_features()
        .iter()
        .any(|feature| feature.id == id && feature.subagents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors_are_unique_and_match_their_static_definitions() {
        let features = native_features();
        let mut ids = std::collections::HashSet::new();
        let mut tools = std::collections::HashSet::new();
        for feature in &features {
            assert!(ids.insert(feature.id), "duplicate feature {}", feature.id);
            assert!(
                !feature.use_case.is_empty(),
                "{} has no use case",
                feature.id
            );
            for name in feature.tool_names {
                assert!(tools.insert(*name), "tool {name} is owned twice");
                assert!(!matches!(
                    *name,
                    "read" | "write" | "edit" | "bash" | "plugin_call"
                ));
            }
            if let Some(mut names) = feature.definition_names() {
                let mut declared: Vec<String> = feature
                    .tool_names
                    .iter()
                    .map(|name| (*name).to_owned())
                    .collect();
                names.sort();
                declared.sort();
                assert_eq!(names, declared, "{} definitions drifted", feature.id);
            }
        }
    }

    #[test]
    fn startup_applies_each_compiled_feature_schema_once() {
        let root =
            std::env::temp_dir().join(format!("beale-feature-schemas-{}", uuid::Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        migrate(&db).unwrap();
        migrate(&db).unwrap();
        for feature in native_features() {
            assert_eq!(
                db.schema_version(feature.id).unwrap(),
                feature.migrations.len() as i64,
                "{}",
                feature.id
            );
        }
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn uncompiled_or_unknown_features_never_apply() {
        let root = std::env::temp_dir().join(format!("beale-features-{}", uuid::Uuid::new_v4()));
        let db = HostDatabase::open(&root).unwrap();
        assert!(!applies(&db, "example-unknown-feature", "workspace-example").unwrap());
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
}
