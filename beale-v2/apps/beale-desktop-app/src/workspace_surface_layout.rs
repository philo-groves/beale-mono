use std::collections::{HashMap, HashSet};

use beale_contracts::v1::{SurfaceRelationship, SurfaceScope, SurfaceScopeState, WorkspaceSurface};

pub(crate) const GRAPH_DEFAULT_MAX: usize = 24;
pub(crate) const INVENTORY_PAGE_SIZE: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum View {
    Graph,
    Inventory,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Sort {
    #[default]
    Recorded,
    Name,
    Type,
    Connections,
}

#[derive(Default)]
pub(crate) struct Explorer {
    pub view: Option<View>,
    pub sort: Sort,
    pub page: usize,
    pub page_anchor: Option<String>,
    pub focus_path: Vec<String>,
    pub focus_scroll: Vec<f32>,
    pub show_out_of_scope: bool,
    pub query: String,
    pub neighborhood: Option<String>,
    pub return_to: Option<ReturnTo>,
}

pub(crate) struct ReturnTo {
    pub view: View,
    pub sort: Sort,
    pub focus: Focus,
    pub show_out_of_scope: bool,
    pub query: String,
    pub page: usize,
    pub page_anchor: Option<String>,
}

impl Explorer {
    pub fn view(&self, count: usize) -> View {
        self.view.unwrap_or(if count > GRAPH_DEFAULT_MAX {
            View::Inventory
        } else {
            View::Graph
        })
    }
}

pub(crate) fn name(surface: &WorkspaceSurface) -> &str {
    surface
        .definition
        .as_ref()
        .map(|d| d.name.trim())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            surface
                .definition
                .as_ref()
                .and_then(|d| d.locator.as_ref())
                .map(|l| l.value.as_str())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or(&surface.id)
}

/// Index only recorded relationships; locator similarity never grants scope or adds edges.
pub(crate) struct Analysis {
    pub indices: HashMap<String, usize>,
    pub incoming: Vec<usize>,
    pub outgoing: Vec<usize>,
    pub neighbors: Vec<HashSet<usize>>,
    pub boundary: Vec<bool>,
    pub dangling: usize,
    pub links: usize,
}

impl Analysis {
    pub fn new(surfaces: &[WorkspaceSurface]) -> Self {
        let mut result = Self {
            indices: surfaces
                .iter()
                .enumerate()
                .map(|(i, s)| (s.id.clone(), i))
                .collect(),
            incoming: vec![0; surfaces.len()],
            outgoing: vec![0; surfaces.len()],
            neighbors: vec![HashSet::new(); surfaces.len()],
            boundary: vec![false; surfaces.len()],
            dangling: 0,
            links: 0,
        };
        for (source, surface) in surfaces.iter().enumerate() {
            for link in &surface.relationships {
                result.outgoing[source] += 1;
                let Some(&target) = result.indices.get(&link.target_id) else {
                    result.dangling += 1;
                    continue;
                };
                result.links += 1;
                result.incoming[target] += 1;
                if source != target {
                    result.neighbors[source].insert(target);
                    result.neighbors[target].insert(source);
                }
                let excluded = |i: usize| {
                    surfaces[i]
                        .definition
                        .as_ref()
                        .is_some_and(|d| d.scope == SurfaceScope::OutOfScope as i32)
                };
                if excluded(source) != excluded(target) {
                    result.boundary[source] = true;
                    result.boundary[target] = true;
                }
            }
        }
        result
    }

    pub fn degree(&self, index: usize) -> usize {
        self.incoming[index] + self.outgoing[index]
    }

    pub fn matching(
        &self,
        surfaces: &[WorkspaceSurface],
        focus: Focus,
        query: &str,
        explorer: &Explorer,
    ) -> Vec<usize> {
        let root = explorer
            .neighborhood
            .as_ref()
            .and_then(|id| self.indices.get(id))
            .copied();
        if explorer.neighborhood.is_some() && root.is_none() {
            return Vec::new();
        }
        let mut result: Vec<_> = surfaces
            .iter()
            .enumerate()
            .filter(|(i, s)| {
                (explorer.show_out_of_scope || !out_of_scope(s))
                    && matches(s, focus, query)
                    && root.is_none_or(|root| root == *i || self.neighbors[root].contains(i))
            })
            .map(|(i, _)| i)
            .collect();
        match explorer.sort {
            Sort::Recorded => {}
            Sort::Name => result.sort_by_cached_key(|&i| {
                (name(&surfaces[i]).to_lowercase(), surfaces[i].id.clone())
            }),
            Sort::Type => result.sort_by_cached_key(|&i| {
                (
                    surfaces[i]
                        .definition
                        .as_ref()
                        .map_or("", |d| d.type_id.as_str())
                        .to_lowercase(),
                    name(&surfaces[i]).to_lowercase(),
                    i,
                )
            }),
            Sort::Connections => result.sort_by_key(|&i| (std::cmp::Reverse(self.degree(i)), i)),
        }
        // Place the neighborhood anchor first, independent of sort order.
        if let Some(root) = root.and_then(|root| result.iter().position(|&i| i == root)) {
            let index = result.remove(root);
            result.insert(0, index);
        }
        result
    }
}

pub(crate) fn page(indices: &[usize], requested: usize, size: usize) -> (usize, &[usize]) {
    let page = requested.min(indices.len().saturating_sub(1) / size);
    let start = (page * size).min(indices.len());
    (page, &indices[start..(start + size).min(indices.len())])
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Focus {
    #[default]
    All,
    Review,
    Unevidenced,
    Excluded,
}

pub(crate) fn evidence_count(surface: &WorkspaceSurface) -> usize {
    surface
        .definition
        .iter()
        .flat_map(|definition| &definition.evidence_refs)
        .chain(
            surface
                .relationships
                .iter()
                .flat_map(|link| &link.evidence_refs),
        )
        .filter(|reference| !reference.reference.trim().is_empty())
        .map(|reference| (reference.kind.as_str(), reference.reference.as_str()))
        .collect::<HashSet<_>>()
        .len()
}

pub(crate) fn needs_review(surface: &WorkspaceSurface) -> bool {
    surface.definition.as_ref().is_none_or(|definition| {
        definition.scope_state != SurfaceScopeState::Approved as i32
            || !matches!(
                SurfaceScope::try_from(definition.scope),
                Ok(SurfaceScope::InScope | SurfaceScope::OutOfScope)
            )
    })
}

pub(crate) fn out_of_scope(surface: &WorkspaceSurface) -> bool {
    surface
        .definition
        .as_ref()
        .is_some_and(|definition| definition.scope == SurfaceScope::OutOfScope as i32)
}

pub(crate) fn matches(surface: &WorkspaceSurface, focus: Focus, query: &str) -> bool {
    let focused = match focus {
        Focus::All => true,
        Focus::Review => needs_review(surface),
        Focus::Unevidenced => evidence_count(surface) == 0,
        Focus::Excluded => out_of_scope(surface),
    };
    let query = query.trim().to_lowercase();
    focused
        && query.split_whitespace().all(|term| {
            let definition = surface.definition.as_ref();
            match term.split_once(':') {
                Some(("type", value)) => {
                    definition.is_some_and(|d| d.type_id.to_lowercase().contains(value))
                }
                Some(("id", value)) => surface.id.to_lowercase() == value,
                Some(("scope", value)) => {
                    definition.is_some_and(|d| match value {
                        "in" => d.scope == SurfaceScope::InScope as i32,
                        "out" => d.scope == SurfaceScope::OutOfScope as i32,
                        "unknown" => !matches!(
                            SurfaceScope::try_from(d.scope),
                            Ok(SurfaceScope::InScope | SurfaceScope::OutOfScope)
                        ),
                        _ => false,
                    }) || (value == "unknown" && definition.is_none())
                }
                _ => {
                    surface.id.to_lowercase().contains(term)
                        || definition.is_some_and(|d| {
                            [&d.name, &d.type_id, &d.description]
                                .iter()
                                .any(|v| v.to_lowercase().contains(term))
                                || d.locator
                                    .as_ref()
                                    .is_some_and(|l| l.value.to_lowercase().contains(term))
                        })
                }
            }
        })
}

fn is_containment(link: &SurfaceRelationship) -> bool {
    link.type_id == "contains"
}

pub(crate) struct Hierarchy {
    pub parent: Vec<Option<usize>>,
    pub children: Vec<Vec<usize>>,
}

pub(crate) fn hierarchy(surfaces: &[WorkspaceSurface]) -> Hierarchy {
    let indices: HashMap<&str, usize> = surfaces
        .iter()
        .enumerate()
        .map(|(i, surface)| (surface.id.as_str(), i))
        .collect();
    let mut parent = vec![None; surfaces.len()];
    let mut children = vec![Vec::new(); surfaces.len()];
    for (source, surface) in surfaces.iter().enumerate() {
        for link in &surface.relationships {
            if is_containment(link)
                && let Some(&target) = indices.get(link.target_id.as_str())
                && source != target
                && parent[target].is_none()
            {
                parent[target] = Some(source);
                children[source].push(target);
            }
        }
    }
    Hierarchy { parent, children }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beale_contracts::v1::SurfaceRelationship;

    #[test]
    fn large_scope_paging_keeps_every_record_reachable() {
        let surfaces: Vec<_> = (0..10_000)
            .map(|i| surface(&format!("S{i}"), Some("S0")))
            .collect();
        let analysis = Analysis::new(&surfaces);
        let explorer = Explorer {
            neighborhood: Some("S0".into()),
            ..Default::default()
        };
        assert_eq!(explorer.view(surfaces.len()), View::Inventory);
        let matches = analysis.matching(&surfaces, Focus::All, "", &explorer);
        assert_eq!(matches.len(), 10_000);
        let mut seen = HashSet::new();
        for requested in 0..matches.len().div_ceil(INVENTORY_PAGE_SIZE) {
            let (_, slice) = page(&matches, requested, INVENTORY_PAGE_SIZE);
            seen.extend(slice.iter().copied());
            assert!(slice.len() <= INVENTORY_PAGE_SIZE);
        }
        assert_eq!(seen.len(), surfaces.len());
        assert_eq!(page(&[], usize::MAX, INVENTORY_PAGE_SIZE), (0, &[][..]));
        assert_eq!(page(&matches[..3], usize::MAX, INVENTORY_PAGE_SIZE).0, 0);
    }

    #[test]
    fn analysis_preserves_direction_cycles_missing_targets_and_explicit_boundaries() {
        use beale_contracts::v1::SurfaceDefinition;
        let mut surfaces = vec![
            surface("S1", Some("S2")),
            surface("S2", Some("S1")),
            surface("S3", Some("missing")),
            surface("S4", None),
        ];
        surfaces[1].definition = Some(SurfaceDefinition {
            scope: SurfaceScope::OutOfScope as i32,
            ..Default::default()
        });
        let analysis = Analysis::new(&surfaces);
        assert_eq!(analysis.links, 2);
        assert_eq!(analysis.dangling, 1);
        assert_eq!(analysis.incoming, vec![1, 1, 0, 0]);
        assert_eq!(analysis.outgoing, vec![1, 1, 1, 0]);
        assert_eq!(analysis.boundary, vec![true, true, false, false]);
        let mut explorer = Explorer {
            neighborhood: Some("S1".into()),
            ..Default::default()
        };
        assert_eq!(
            analysis.matching(&surfaces, Focus::All, "", &explorer),
            vec![0]
        );
        assert!(
            analysis
                .matching(&surfaces, Focus::Excluded, "", &explorer)
                .is_empty()
        );
        explorer.show_out_of_scope = true;
        assert_eq!(
            analysis.matching(&surfaces, Focus::All, "", &explorer),
            vec![0, 1]
        );
        assert_eq!(
            analysis.matching(&surfaces, Focus::Excluded, "", &explorer),
            vec![1]
        );
        let missing = Explorer {
            neighborhood: Some("deleted-example".into()),
            ..Default::default()
        };
        assert!(
            analysis
                .matching(&surfaces, Focus::All, "", &missing)
                .is_empty()
        );
    }

    #[test]
    fn inventory_sort_is_deterministic_for_names_types_and_link_counts() {
        use beale_contracts::v1::SurfaceDefinition;
        let mut surfaces = vec![
            surface("S1", None),
            surface("S2", Some("S3")),
            surface("S3", None),
        ];
        for (surface, (name, kind)) in surfaces.iter_mut().zip([
            ("Zulu", "repository"),
            ("Alpha", "service"),
            ("Beta", "binary"),
        ]) {
            surface.definition = Some(SurfaceDefinition {
                name: name.into(),
                type_id: kind.into(),
                ..Default::default()
            });
        }
        let analysis = Analysis::new(&surfaces);
        for (sort, expected) in [
            (Sort::Name, vec![1, 2, 0]),
            (Sort::Type, vec![2, 0, 1]),
            (Sort::Connections, vec![1, 2, 0]),
        ] {
            let explorer = Explorer {
                sort,
                ..Default::default()
            };
            assert_eq!(
                analysis.matching(&surfaces, Focus::All, "", &explorer),
                expected
            );
        }
    }

    #[test]
    fn scoped_search_combines_terms_without_inferring_wildcard_authorization() {
        use beale_contracts::v1::{SurfaceDefinition, SurfaceLocator};
        let mut record = surface("S1", None);
        record.definition = Some(SurfaceDefinition {
            name: "Example parser".into(),
            type_id: "code.module".into(),
            locator: Some(SurfaceLocator {
                kind: "domain".into(),
                value: "*.example.test".into(),
            }),
            scope: SurfaceScope::OutOfScope as i32,
            ..Default::default()
        });
        assert!(matches(
            &record,
            Focus::All,
            "type:code scope:out parser *.example.test"
        ));
        assert!(matches(&record, Focus::All, "id:S1"));
        assert!(!matches(&record, Focus::All, "scope:in"));
        assert!(!matches(&record, Focus::All, "id:S10"));
        assert!(!matches(&record, Focus::All, "other.example.test"));
    }

    fn surface(id: &str, target: Option<&str>) -> WorkspaceSurface {
        WorkspaceSurface {
            id: id.to_owned(),
            relationships: target
                .into_iter()
                .map(|target_id| SurfaceRelationship {
                    target_id: target_id.to_owned(),
                    label: "calls".to_owned(),
                    type_id: String::new(),
                    evidence_refs: Vec::new(),
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn review_distinguishes_approval_from_recorded_scope() {
        use beale_contracts::v1::SurfaceDefinition;
        let mut record = surface("S1", None);
        assert!(needs_review(&record));
        record.definition = Some(SurfaceDefinition {
            scope: SurfaceScope::InScope as i32,
            scope_state: SurfaceScopeState::Approved as i32,
            ..Default::default()
        });
        assert!(!needs_review(&record));
        record.definition.as_mut().unwrap().scope = SurfaceScope::OutOfScope as i32;
        assert!(!needs_review(&record));
        assert!(matches(&record, Focus::Excluded, ""));
        record.definition.as_mut().unwrap().scope = SurfaceScope::Unspecified as i32;
        assert!(needs_review(&record));
        record.definition.as_mut().unwrap().scope = 999;
        assert!(needs_review(&record));
        record.definition.as_mut().unwrap().scope = SurfaceScope::InScope as i32;
        record.definition.as_mut().unwrap().scope_state = SurfaceScopeState::Candidate as i32;
        assert!(needs_review(&record));
    }

    #[test]
    fn evidence_counts_unique_nonempty_references_across_definition_and_links() {
        use beale_contracts::v1::{SurfaceDefinition, SurfaceEvidence};
        let reference = SurfaceEvidence {
            kind: "artifact".into(),
            reference: "artifacts/example.txt".into(),
            note: String::new(),
        };
        let mut record = surface("S1", Some("S2"));
        record.definition = Some(SurfaceDefinition {
            evidence_refs: vec![reference.clone(), SurfaceEvidence::default()],
            ..Default::default()
        });
        record.relationships[0].evidence_refs = vec![
            reference.clone(),
            SurfaceEvidence {
                reference: "artifacts/example-connection.txt".into(),
                ..reference
            },
        ];
        assert_eq!(evidence_count(&record), 2);
        assert!(!matches(&record, Focus::Unevidenced, ""));
    }

    #[test]
    fn search_matches_recorded_properties_and_combines_with_focus() {
        use beale_contracts::v1::{SurfaceDefinition, SurfaceLocator};
        let mut record = surface("S1", None);
        record.definition = Some(SurfaceDefinition {
            name: "Example parser".into(),
            type_id: "code.module".into(),
            description: "Decodes example requests".into(),
            locator: Some(SurfaceLocator {
                kind: "path".into(),
                value: "src/example.rs".into(),
            }),
            scope: SurfaceScope::InScope as i32,
            scope_state: SurfaceScopeState::Candidate as i32,
            ..Default::default()
        });
        for query in [" S1 ", "PARSER", "code.module", "decodes", "src/example.rs"] {
            assert!(matches(&record, Focus::Review, query), "{query}");
        }
        assert!(!matches(&record, Focus::Excluded, "parser"));
        assert!(!matches(&record, Focus::All, "missing"));
    }
}
