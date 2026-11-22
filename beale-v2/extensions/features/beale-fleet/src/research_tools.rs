use crate::FleetStore;
use beale_contracts::{
    research_tools::{CATALOG, available_on},
    v1::{FleetAllowedResearchTool, FleetResearchTool},
};
use beale_storage::HostDatabase;

#[derive(Debug)]
pub struct ResearchToolSettingsError;

pub(crate) fn contains(id: &str) -> bool {
    CATALOG
        .iter()
        .any(|entry| entry.id == id && available_on(entry.id, std::env::consts::OS))
}

pub(crate) fn catalog(
    mut settings: impl FnMut(&str) -> Result<(bool, bool), ()>,
) -> Result<Vec<FleetResearchTool>, ()> {
    catalog_for_os(std::env::consts::OS, &mut settings)
}

fn catalog_for_os(
    os: &str,
    mut settings: impl FnMut(&str) -> Result<(bool, bool), ()>,
) -> Result<Vec<FleetResearchTool>, ()> {
    CATALOG
        .iter()
        .filter(|entry| available_on(entry.id, os))
        .map(|entry| {
            let (allow_locally, allow_in_vms) = settings(entry.id)?;
            Ok(FleetResearchTool {
                id: entry.id.to_owned(),
                name: entry.name.to_owned(),
                category: entry.category.to_owned(),
                description: entry.description.to_owned(),
                allow_locally,
                allow_in_vms,
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
                .filter(|tool| tool.allow_locally || tool.allow_in_vms)
                .map(|tool| FleetAllowedResearchTool {
                    id: tool.id,
                    name: tool.name,
                    allow_locally: tool.allow_locally,
                    allow_in_vms: tool.allow_in_vms,
                })
                .collect()
        })
}

pub fn prompt_note(tools: &[FleetAllowedResearchTool]) -> Option<String> {
    if tools.is_empty() {
        return None;
    }
    let entries = tools
        .iter()
        .map(|tool| {
            let location = match (tool.allow_locally, tool.allow_in_vms) {
                (true, true) => "locally and in VMs",
                (true, false) => "locally",
                (false, true) => "in VMs",
                (false, false) => return String::new(),
            };
            format!("{} ({location})", tool.name)
        })
        .filter(|entry| !entry.is_empty())
        .collect::<Vec<_>>();
    (!entries.is_empty()).then(|| {
        format!(
            "The following research tools may optionally be installed and used during research where allowed: {}. Follow normal review and authorized scope.",
            entries.join(", ")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_specific_tools_follow_host_os() {
        let ids = |os| {
            catalog_for_os(os, |_| Ok((false, false)))
                .unwrap()
                .into_iter()
                .map(|tool| tool.id)
                .collect::<Vec<_>>()
        };
        let macos = ids("macos");
        let linux = ids("linux");
        let windows = ids("windows");
        for id in ["otool", "jtool2", "dig"] {
            assert!(macos.contains(&id.to_owned()));
            assert!(!windows.contains(&id.to_owned()));
        }
        assert!(!linux.contains(&"otool".to_owned()));
        assert!(linux.contains(&"jtool2".to_owned()));
        assert!(linux.contains(&"dig".to_owned()));
    }

    #[test]
    fn prompt_note_lists_only_allowed_locations() {
        assert_eq!(prompt_note(&[]), None);
        let tools = vec![
            FleetAllowedResearchTool {
                id: "ripgrep".into(),
                name: "ripgrep".into(),
                allow_locally: true,
                allow_in_vms: false,
            },
            FleetAllowedResearchTool {
                id: "ghidra".into(),
                name: "Ghidra".into(),
                allow_locally: false,
                allow_in_vms: true,
            },
        ];
        assert_eq!(
            prompt_note(&tools).unwrap(),
            "The following research tools may optionally be installed and used during research where allowed: ripgrep (locally), Ghidra (in VMs). Follow normal review and authorized scope."
        );
    }
}
