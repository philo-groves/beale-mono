use crate::FleetStore;
use beale_contracts::{
    research_tools::{CATALOG, available_on},
    v1::{FleetAllowedResearchTool, FleetResearchTool, ResearchToolPreference},
};
use beale_storage::HostDatabase;
#[derive(Debug)]
pub struct ResearchToolSettingsError;
pub(crate) fn contains(id: &str) -> bool {
    CATALOG.iter().any(|entry| entry.id == id)
}
/// Show all platforms in settings; filter guidance at the execution boundary.
pub(crate) fn catalog(
    mut settings: impl FnMut(&str) -> Result<i32, ()>,
) -> Result<Vec<FleetResearchTool>, ()> {
    CATALOG
        .iter()
        .map(|entry| {
            Ok(FleetResearchTool {
                id: entry.id.into(),
                name: entry.name.into(),
                category: entry.category.into(),
                description: entry.description.into(),
                preference: settings(entry.id)?,
            })
        })
        .collect()
}
pub fn allowed_tools(
    db: &HostDatabase,
) -> Result<Vec<FleetAllowedResearchTool>, ResearchToolSettingsError> {
    catalog(|id| db.research_tool_settings(id).map_err(|_| ()))
        .map_err(|_| ResearchToolSettingsError)
        .map(|tools| {
            tools
                .into_iter()
                .map(|tool| FleetAllowedResearchTool {
                    id: tool.id,
                    name: tool.name,
                    preference: tool.preference,
                })
                .collect()
        })
}
pub fn prompt_note_for_os(tools: &[FleetAllowedResearchTool], os: &str) -> Option<String> {
    let entries = tools
        .iter()
        .filter(|t| available_on(&t.id, os))
        .filter_map(|tool| {
            let label = match ResearchToolPreference::try_from(tool.preference).ok()? {
                ResearchToolPreference::Allow => "Allow: may install/use",
                ResearchToolPreference::Recommend => "Recommend: preferred, optional",
                ResearchToolPreference::Priority => "Priority: top choice for relevant tasks",
            };
            Some(format!("{} ({label})", tool.name))
        })
        .collect::<Vec<_>>();
    (!entries.is_empty()).then(|| format!("Research-tool guidance for the actual {os} execution environment, locally or remotely: {}. These are preferences, not execution permissions. Follow recorded authorization and normal review.",entries.join(", ")))
}
pub fn prompt_note(tools: &[FleetAllowedResearchTool]) -> Option<String> {
    prompt_note_for_os(tools, std::env::consts::OS)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preferences_are_mutually_exclusive_and_follow_execution_os() {
        let tools = catalog(|id| {
            Ok(if id == "ripgrep" {
                2
            } else if id == "dig" {
                1
            } else {
                0
            })
        })
        .unwrap()
        .into_iter()
        .map(|t| FleetAllowedResearchTool {
            id: t.id,
            name: t.name,
            preference: t.preference,
        })
        .collect::<Vec<_>>();
        let linux = prompt_note_for_os(&tools, "linux").unwrap();
        assert!(
            linux.contains("Priority: top choice")
                && linux.contains("Recommend: preferred")
                && linux.contains("Allow: may install/use")
        );
        assert!(linux.contains("dig (") && !linux.contains("otool ("));
        let windows = prompt_note_for_os(&tools, "windows").unwrap();
        assert!(!windows.contains("dig (") && !windows.contains("otool ("));
        assert!(
            prompt_note_for_os(&tools, "macos")
                .unwrap()
                .contains("otool (")
        );
        assert!(ResearchToolPreference::try_from(3).is_err());
        assert!(catalog(|_| Ok(0)).unwrap().iter().any(|t| t.id == "otool"));
    }
}
