use std::collections::{HashMap, HashSet, VecDeque};

use beale_contracts::v1::{SurfaceScope, SurfaceScopeState, WorkspaceSurface};

pub(crate) const NODE_MAX_WIDTH: f32 = 232.0;
pub(crate) const NODE_HEIGHT: f32 = 148.0;
pub(crate) const COLUMN_GAP: f32 = 116.0;
pub(crate) const ROW_GAP: f32 = 72.0;

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

pub(crate) fn matches(surface: &WorkspaceSurface, focus: Focus, query: &str) -> bool {
    let focused = match focus {
        Focus::All => true,
        Focus::Review => needs_review(surface),
        Focus::Unevidenced => evidence_count(surface) == 0,
        Focus::Excluded => surface
            .definition
            .as_ref()
            .is_some_and(|definition| definition.scope == SurfaceScope::OutOfScope as i32),
    };
    let query = query.trim().to_lowercase();
    focused
        && (query.is_empty()
            || surface.id.to_lowercase().contains(&query)
            || surface.definition.as_ref().is_some_and(|definition| {
                [
                    &definition.name,
                    &definition.type_id,
                    &definition.description,
                ]
                .iter()
                .any(|value| value.to_lowercase().contains(&query))
                    || definition
                        .locator
                        .as_ref()
                        .is_some_and(|locator| locator.value.to_lowercase().contains(&query))
            }))
}

/// Pack recorded connected components into readable rows. A serpentine order keeps
/// adjacent records close when a component wraps; no relationships are inferred.
pub(crate) fn layout(surfaces: &[WorkspaceSurface], width: f32, height: f32) -> Vec<[f32; 2]> {
    let indices: HashMap<&str, usize> = surfaces
        .iter()
        .enumerate()
        .map(|(index, surface)| (surface.id.as_str(), index))
        .collect();
    let mut neighbors = vec![Vec::new(); surfaces.len()];
    let mut incoming = vec![0_usize; surfaces.len()];
    let mut edges = HashSet::new();
    for (source, surface) in surfaces.iter().enumerate() {
        for relationship in &surface.relationships {
            if let Some(&target) = indices.get(relationship.target_id.as_str())
                && source != target
            {
                incoming[target] += 1;
                if edges.insert((source.min(target), source.max(target))) {
                    neighbors[source].push(target);
                    neighbors[target].push(source);
                }
            }
        }
    }
    let mut seen = vec![false; surfaces.len()];
    let mut groups = Vec::new();
    for start in 0..surfaces.len() {
        if seen[start] {
            continue;
        }
        let mut members = Vec::new();
        let mut queue = VecDeque::from([start]);
        seen[start] = true;
        while let Some(index) = queue.pop_front() {
            members.push(index);
            for &next in &neighbors[index] {
                if !seen[next] {
                    seen[next] = true;
                    queue.push_back(next);
                }
            }
        }
        // Prefer an explicit source, then the most connected record, with stable ties.
        let root = *members
            .iter()
            .min_by_key(|&&index| {
                (
                    incoming[index],
                    std::cmp::Reverse(neighbors[index].len()),
                    index,
                )
            })
            .unwrap();
        let mut ordered = Vec::new();
        let mut visited = HashSet::from([root]);
        let mut queue = VecDeque::from([root]);
        while let Some(index) = queue.pop_front() {
            ordered.push(index);
            for &next in &neighbors[index] {
                if visited.insert(next) {
                    queue.push_back(next);
                }
            }
        }
        groups.push(ordered);
    }
    groups.sort_by_key(|group| (std::cmp::Reverse(group.len()), group[0]));
    // Choose the grid that maximizes card size while fitting every record.
    let fit = |columns: usize| {
        let rows = surfaces.len().div_ceil(columns).max(1);
        let grid_width = columns as f32 * (NODE_MAX_WIDTH + COLUMN_GAP) - COLUMN_GAP;
        let grid_height = rows as f32 * (NODE_HEIGHT + ROW_GAP) - ROW_GAP + 52.0;
        ((width - 48.0).max(1.0) / grid_width).min((height - 24.0).max(1.0) / grid_height)
    };
    let columns = (1..=surfaces.len().max(1))
        .max_by(|&left, &right| {
            fit(left)
                .total_cmp(&fit(right))
                .then_with(|| right.cmp(&left))
        })
        .unwrap();
    let mut positions = vec![[0.0, 0.0]; surfaces.len()];
    let mut slot = 0;
    for group in groups {
        for index in group {
            let row = slot / columns;
            let col = if row % 2 == 0 {
                slot % columns
            } else {
                columns - 1 - slot % columns
            };
            positions[index] = [
                NODE_MAX_WIDTH / 2.0 + col as f32 * (NODE_MAX_WIDTH + COLUMN_GAP),
                NODE_HEIGHT / 2.0 + row as f32 * (NODE_HEIGHT + ROW_GAP),
            ];
            slot += 1;
        }
    }
    positions
}

#[cfg(test)]
mod tests {
    use super::*;
    use beale_contracts::v1::SurfaceRelationship;

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
                    source_element_id: String::new(),
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn connected_surfaces_cluster_with_stable_positions() {
        let surfaces = [
            surface("hub", Some("left")),
            surface("left", None),
            surface("right", Some("hub")),
            surface("free-a", None),
            surface("free-b", None),
            surface("free-c", None),
        ];
        let positions = layout(&surfaces, 1000.0, 700.0);
        let distance = |a: usize, b: usize| {
            ((positions[a][0] - positions[b][0]).powi(2)
                + (positions[a][1] - positions[b][1]).powi(2))
            .sqrt()
        };
        assert!(distance(0, 1) < distance(0, 3));
        assert!(distance(0, 2) < distance(0, 3));
        assert_eq!(positions, layout(&surfaces, 1000.0, 700.0));
    }

    #[test]
    fn narrow_view_wraps_unconnected_surfaces() {
        let surfaces = [
            surface("one", None),
            surface("two", None),
            surface("three", None),
        ];
        let positions = layout(&surfaces, 600.0, 700.0);
        assert!(
            positions
                .iter()
                .any(|position| position[1] != positions[0][1])
        );
    }

    #[test]
    fn explicit_contains_relationship_places_surfaces_together() {
        let positions = layout(
            &[
                surface("S1", Some("S2")),
                surface("S2", None),
                surface("S3", None),
            ],
            1000.0,
            700.0,
        );
        let distance = |left: usize, right: usize| {
            ((positions[left][0] - positions[right][0]).powi(2)
                + (positions[left][1] - positions[right][1]).powi(2))
            .sqrt()
        };
        assert!(distance(0, 1) < distance(0, 2));
    }

    #[test]
    fn dense_related_surfaces_leave_room_for_cards() {
        let mut surfaces = vec![surface("hub", None)];
        surfaces.extend((0..16).map(|index| surface(&format!("leaf-{index}"), Some("hub"))));
        let positions = layout(&surfaces, 1400.0, 1000.0);
        for (index, &[x, y]) in positions.iter().enumerate() {
            for &[other_x, other_y] in &positions[index + 1..] {
                assert!(
                    (x - other_x).abs() >= NODE_MAX_WIDTH + 8.0
                        || (y - other_y).abs() >= NODE_HEIGHT + 8.0,
                    "cards overlap at {x}, {y} and {other_x}, {other_y}"
                );
            }
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
