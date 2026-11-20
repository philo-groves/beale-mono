use crate::FleetStore;
use beale_contracts::v1::{FleetAllowedResearchTool, FleetResearchTool};
use beale_storage::HostDatabase;

#[derive(Debug)]
pub struct ResearchToolSettingsError;

struct CatalogEntry {
    id: &'static str,
    name: &'static str,
    category: &'static str,
    description: &'static str,
}

const CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        id: "ripgrep",
        name: "ripgrep",
        category: "Source",
        description: "Fast search across source and artifacts.",
    },
    CatalogEntry {
        id: "opengrep",
        name: "Opengrep",
        category: "Source",
        description: "Rule-based source scanning.",
    },
    CatalogEntry {
        id: "git",
        name: "Git",
        category: "Source",
        description: "Repository history and changes.",
    },
    CatalogEntry {
        id: "radare2",
        name: "radare2",
        category: "Binary",
        description: "Scriptable binary inspection and analysis.",
    },
    CatalogEntry {
        id: "ghidra",
        name: "Ghidra",
        category: "Binary",
        description: "Disassembly and decompilation.",
    },
    CatalogEntry {
        id: "llvm",
        name: "LLVM tools",
        category: "Binary",
        description: "Object, symbol, and disassembly utilities.",
    },
    CatalogEntry {
        id: "jadx",
        name: "JADX",
        category: "Mobile",
        description: "Android APK and DEX decompilation.",
    },
    CatalogEntry {
        id: "ipsw",
        name: "ipsw",
        category: "Mobile",
        description: "Apple firmware and binary inspection.",
    },
    CatalogEntry {
        id: "python",
        name: "Python 3",
        category: "Runtimes",
        description: "Runtime for research scripts.",
    },
    CatalogEntry {
        id: "java",
        name: "Java JDK",
        category: "Runtimes",
        description: "Runtime required by Java-based tools.",
    },
    CatalogEntry {
        id: "node",
        name: "Node.js",
        category: "Runtimes",
        description: "Runtime for JavaScript research tooling.",
    },
];

pub(crate) fn contains(id: &str) -> bool {
    CATALOG.iter().any(|entry| entry.id == id)
}

pub(crate) fn catalog(
    mut settings: impl FnMut(&str) -> Result<(bool, bool), ()>,
) -> Result<Vec<FleetResearchTool>, ()> {
    CATALOG
        .iter()
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
