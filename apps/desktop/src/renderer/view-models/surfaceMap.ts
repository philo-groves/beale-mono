import type { BealeWorkspaceSurfaceMap } from '@beale/app-server-runtime/protocol';

export type MapSurface = BealeWorkspaceSurfaceMap['surfaces'][number];

/** Keep hierarchy context without counting unrelated ancestors as session activity. */
export function sessionSurfaceHierarchy(map: BealeWorkspaceSurfaceMap, sessionId: string): BealeWorkspaceSurfaceMap {
  const byId = new Map(map.surfaces.map((surface) => [surface.id, surface]));
  const visible = new Set<string>();
  for (const id of map.sessionSurfaceIds ?? []) {
    let current = byId.get(id);
    const seen = new Set<string>();
    while (current && !seen.has(current.id)) {
      visible.add(current.id);
      seen.add(current.id);
      current = current.parentId ? byId.get(current.parentId) : undefined;
    }
  }
  return { ...map, surfaces: map.surfaces.filter((surface) => visible.has(surface.id)),
    agents: map.agents.filter((agent) => agent.sessionId === sessionId && visible.has(agent.surfaceId)) };
}

export function sessionSurfaceCounts(map: BealeWorkspaceSurfaceMap | null): { total: number; touched: number; completed: number } {
  if (!map) return { total: 0, touched: 0, completed: 0 };
  const byId = new Map(map.surfaces.map((surface) => [surface.id, surface]));
  const tracked = [...new Set(map.sessionSurfaceIds ?? [])].map((id) => byId.get(id)).filter((surface) => surface !== undefined);
  const completed = tracked.filter((surface) => surface.coverageState.covered).length;
  return { total: tracked.length, touched: tracked.length - completed, completed };
}

/** Keep each selected surface's path and descendant branch visible. */
export function filterSurfaceHierarchy(
  surfaces: readonly MapSurface[], query: string, showUntouched: boolean
): MapSurface[] {
  const byId = new Map(surfaces.map((surface) => [surface.id, surface]));
  const children = new Map<string, MapSurface[]>();
  for (const surface of surfaces) if (surface.parentId && byId.has(surface.parentId)) {
    children.set(surface.parentId, [...(children.get(surface.parentId) ?? []), surface]);
  }
  const eligible = new Set(surfaces.filter((surface) => showUntouched || surfaceCoveragePercent(surface) > 0)
    .map((surface) => surface.id));
  const addAncestors = (id: string, into: Set<string>): void => {
    let current = byId.get(id);
    const seen = new Set<string>();
    while (current && !seen.has(current.id)) {
      into.add(current.id);
      seen.add(current.id);
      current = current.parentId ? byId.get(current.parentId) : undefined;
    }
  };
  for (const id of [...eligible]) addAncestors(id, eligible);
  const words = query.toLocaleLowerCase().trim().split(/\s+/u).filter(Boolean);
  if (!words.length) return surfaces.filter((surface) => eligible.has(surface.id));
  const visible = new Set<string>();
  const addDescendants = (id: string): void => {
    const queue = [id];
    const walked = new Set<string>();
    while (queue.length) {
      const current = queue.pop()!;
      if (walked.has(current)) continue;
      walked.add(current);
      visible.add(current);
      for (const child of children.get(current) ?? []) queue.push(child.id);
    }
  };
  for (const surface of surfaces) {
    if (!eligible.has(surface.id)) continue;
    const haystack = `${surface.name} ${surface.description}`.toLocaleLowerCase();
    if (!words.every((word) => haystack.includes(word))) continue;
    addAncestors(surface.id, visible);
    addDescendants(surface.id);
  }
  return surfaces.filter((surface) => visible.has(surface.id));
}

export function surfaceCoveragePercent(surface: MapSurface): number {
  if (surface.coverageState.covered) return 100;
  const modeled = surface.foundation?.threatModelReviewed === true
    || Object.values(surface.foundation?.threatModel ?? {}).some((value) => typeof value === 'string' && value.trim()
      || Array.isArray(value) && value.some((item) => typeof item === 'string' && item.trim()));
  if (!modeled) return surface.firstTouchedAt || surface.foundation ? 25 : 0;
  const unmet = surface.coverageState.unmetRequirements;
  return Math.min(99, 50 + (unmet.includes('open_leads') ? 0 : 25)
    + (unmet.includes('unblocked_findings') ? 0 : 25));
}

export interface SurfaceLayoutNode { surface: MapSurface; x: number; y: number; depth: number }
export function layoutSurfaceHierarchy(surfaces: readonly MapSurface[]): { nodes: SurfaceLayoutNode[]; width: number; height: number } {
  const byId = new Map(surfaces.map((surface) => [surface.id, surface]));
  const children = new Map<string, MapSurface[]>();
  for (const surface of surfaces) if (surface.parentId && byId.has(surface.parentId)) {
    children.set(surface.parentId, [...(children.get(surface.parentId) ?? []), surface]);
  }
  const sort = (items: MapSurface[]) => items.sort((left, right) => left.name.localeCompare(right.name));
  const roots = sort(surfaces.filter((surface) => !surface.parentId || !byId.has(surface.parentId)));
  const nodes: SurfaceLayoutNode[] = [];
  const visited = new Set<string>();
  let leaf = 0;
  const visit = (surface: MapSurface, depth: number): number => {
    if (visited.has(surface.id)) return leaf++;
    visited.add(surface.id);
    const descendants = sort(children.get(surface.id) ?? []).filter((item) => !visited.has(item.id));
    const positions = descendants.map((child) => visit(child, depth + 1));
    const row = positions.length ? (positions[0]! + positions.at(-1)!) / 2 : leaf++;
    nodes.push({ surface, x: 24 + depth * 270, y: 24 + row * 96, depth });
    return row;
  };
  for (const root of roots) { visit(root, 0); leaf += 0.35; }
  for (const surface of surfaces) if (!visited.has(surface.id)) visit(surface, 0);
  return { nodes, width: Math.max(280, ...nodes.map((node) => node.x + 232)), height: Math.max(120, ...nodes.map((node) => node.y + 84)) };
}
