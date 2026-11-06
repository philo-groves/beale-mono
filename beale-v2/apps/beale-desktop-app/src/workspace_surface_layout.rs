use std::collections::{HashMap, HashSet, VecDeque};

use beale_contracts::v1::WorkspaceSurface;

/// Place related surfaces together before isolated surfaces.
/// An absent relationship is not inferred from a surface's type or name.
pub(crate) fn layout(surfaces: &[WorkspaceSurface], width: f32, height: f32) -> Vec<[f32; 2]> {
    if surfaces.is_empty() {
        return Vec::new();
    }
    let indices: HashMap<&str, usize> = surfaces
        .iter()
        .enumerate()
        .map(|(index, surface)| (surface.id.as_str(), index))
        .collect();
    let mut neighbors = vec![Vec::new(); surfaces.len()];
    let mut edges = HashSet::new();
    for (source, surface) in surfaces.iter().enumerate() {
        if let Some(parent_id) = surface
            .definition
            .as_ref()
            .map(|definition| definition.parent_id.as_str())
            && let Some(&target) = indices.get(parent_id)
            && source != target
            && edges.insert((source.min(target), source.max(target)))
        {
            neighbors[source].push(target);
            neighbors[target].push(source);
        }
        for relationship in &surface.relationships {
            if let Some(&target) = indices.get(relationship.target_id.as_str())
                && source != target
                && edges.insert((source.min(target), source.max(target)))
            {
                neighbors[source].push(target);
                neighbors[target].push(source);
            }
        }
    }
    let mut seen = vec![false; surfaces.len()];
    let mut groups = Vec::new();
    for start in 0..surfaces.len() {
        if seen[start] {
            continue;
        }
        seen[start] = true;
        let mut queue = VecDeque::from([start]);
        let mut members = Vec::new();
        while let Some(index) = queue.pop_front() {
            members.push(index);
            for &neighbor in &neighbors[index] {
                if !seen[neighbor] {
                    seen[neighbor] = true;
                    queue.push_back(neighbor);
                }
            }
        }
        groups.push(members);
    }
    groups.sort_by(|left, right| {
        right
            .len()
            .cmp(&left.len())
            .then_with(|| surfaces[left[0]].id.cmp(&surfaces[right[0]].id))
    });

    let mut positions = vec![[0.0, 0.0]; surfaces.len()];
    let mut boxes = Vec::new();
    for members in groups {
        if members.len() == 1 {
            boxes.push((88.0, 88.0, vec![(members[0], [44.0, 44.0])]));
            continue;
        }
        let hub = *members
            .iter()
            .max_by_key(|&&index| (neighbors[index].len(), std::cmp::Reverse(index)))
            .unwrap();
        let mut distances = vec![usize::MAX; surfaces.len()];
        distances[hub] = 0;
        let mut queue = VecDeque::from([hub]);
        while let Some(index) = queue.pop_front() {
            for &neighbor in &neighbors[index] {
                if distances[neighbor] == usize::MAX {
                    distances[neighbor] = distances[index] + 1;
                    queue.push_back(neighbor);
                }
            }
        }
        let depth = members.iter().map(|&index| distances[index]).max().unwrap();
        let mut radius = 0.0_f32;
        let mut points = vec![(hub, [0.0, 0.0])];
        for ring in 1..=depth {
            let mut ring_members: Vec<_> = members
                .iter()
                .copied()
                .filter(|&index| distances[index] == ring)
                .collect();
            ring_members.sort_by(|&left, &right| surfaces[left].id.cmp(&surfaces[right].id));
            radius = (radius + 68.0).max(ring_members.len() as f32 * 48.0 / std::f32::consts::TAU);
            for (slot, index) in ring_members.iter().enumerate() {
                let angle = -std::f32::consts::FRAC_PI_2
                    + std::f32::consts::TAU * slot as f32 / ring_members.len() as f32;
                points.push((*index, [radius * angle.cos(), radius * angle.sin()]));
            }
        }
        let size = 2.0 * (radius + 28.0);
        boxes.push((
            size,
            size,
            points
                .into_iter()
                .map(|(index, [x, y])| (index, [x + size / 2.0, y + size / 2.0]))
                .collect(),
        ));
    }

    let total_area: f32 = boxes.iter().map(|(w, h, _)| (w + 24.0) * (h + 24.0)).sum();
    let target_width = (total_area * (width / height.max(1.0)).max(0.5))
        .sqrt()
        .clamp(200.0, (width - 48.0).max(200.0));
    let mut rows: Vec<Vec<usize>> = vec![Vec::new()];
    let mut row_width = 0.0;
    for (index, (box_width, _, _)) in boxes.iter().enumerate() {
        let next = if row_width == 0.0 {
            *box_width
        } else {
            row_width + 24.0 + box_width
        };
        if next > target_width && row_width > 0.0 {
            rows.push(Vec::new());
            row_width = 0.0;
        }
        rows.last_mut().unwrap().push(index);
        row_width += if row_width == 0.0 {
            *box_width
        } else {
            24.0 + box_width
        };
    }
    let total_height: f32 = rows
        .iter()
        .map(|row| row.iter().map(|&index| boxes[index].1).fold(0.0, f32::max) + 24.0)
        .sum();
    let mut y = if rows.len() == 1 && total_height > height {
        (height - total_height) / 2.0
    } else {
        ((height - total_height) / 2.0).max(150.0)
    };
    for row in rows {
        let row_height = row.iter().map(|&index| boxes[index].1).fold(0.0, f32::max);
        let width_used: f32 = row.iter().map(|&index| boxes[index].0 + 24.0).sum::<f32>() - 24.0;
        let mut x = if width_used > width {
            (width - width_used) / 2.0
        } else {
            ((width - width_used) / 2.0).max(24.0)
        };
        for index in row {
            let (box_width, box_height, points) = &boxes[index];
            for &(surface, [local_x, local_y]) in points {
                positions[surface] = [x + local_x, y + (row_height - box_height) / 2.0 + local_y];
            }
            x += box_width + 24.0;
        }
        y += row_height + 24.0;
    }
    positions
}

#[cfg(test)]
mod tests {
    use super::*;
    use beale_contracts::v1::{SurfaceDefinition, SurfaceRelationship};

    fn surface(id: &str, target: Option<&str>) -> WorkspaceSurface {
        WorkspaceSurface {
            id: id.to_owned(),
            relationships: target
                .into_iter()
                .map(|target_id| SurfaceRelationship {
                    target_id: target_id.to_owned(),
                    label: "calls".to_owned(),
                    type_id: String::new(),
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn connected_surfaces_cluster_and_unconnected_surfaces_use_multiple_rows() {
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
        assert!(
            positions[3..]
                .windows(2)
                .any(|pair| pair[0][1] != pair[1][1])
        );
        assert_eq!(positions, layout(&surfaces, 1000.0, 700.0));
    }

    #[test]
    fn three_unconnected_surfaces_do_not_form_a_single_row() {
        let surfaces = [
            surface("one", None),
            surface("two", None),
            surface("three", None),
        ];
        let positions = layout(&surfaces, 1000.0, 700.0);
        assert!(
            positions
                .iter()
                .any(|position| position[1] != positions[0][1])
        );
    }

    #[test]
    fn parent_links_place_children_with_their_repository() {
        let mut child = surface("S2", None);
        child.definition = Some(SurfaceDefinition {
            parent_id: "S1".to_owned(),
            ..Default::default()
        });
        let positions = layout(
            &[surface("S1", None), child, surface("S3", None)],
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
}
