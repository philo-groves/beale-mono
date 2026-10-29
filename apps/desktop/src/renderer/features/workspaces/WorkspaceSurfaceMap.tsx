import { useMemo, useState, type JSX } from 'react';
import { TriangleAlert } from 'lucide-react';
import type { BealeWorkspaceSurfaceMap } from '@beale/app-server-runtime/protocol';
import type { ScopeAssetKind } from '@shared/types';
import { filterSurfaceHierarchy, layoutSurfaceHierarchy, sessionSurfaceCounts, surfaceCoveragePercent, type MapSurface } from '../../view-models/surfaceMap';
import { surfaceAssetKind } from './WorkspaceResourceKinds';
import { WorkspaceTitleSearch } from './WorkspaceTitleSearch';

export function WorkspaceSurfaceMap({ map, workspaceName, context = 'workspace', resourceKind = null, loading = false, error = null, selectedSurfaceId = null, onSelectSurface }: {
  map: BealeWorkspaceSurfaceMap | null;
  workspaceName: string;
  context?: 'workspace' | 'session';
  resourceKind?: ScopeAssetKind | null;
  loading?: boolean;
  error?: string | null;
  selectedSurfaceId?: string | null;
  onSelectSurface?: (surface: MapSurface) => void;
}): JSX.Element {
  const [query, setQuery] = useState('');
  const [showUntouched, setShowUntouched] = useState(true);
  const kindSurfaces = useMemo(() => map?.surfaces.filter((surface) =>
    resourceKind === null || surfaceAssetKind(surface.resourceKind) === resourceKind) ?? [], [map, resourceKind]);
  const visibleSurfaces = useMemo(() => filterSurfaceHierarchy(kindSurfaces, query, showUntouched),
    [kindSurfaces, query, showUntouched]);
  const layout = useMemo(() => layoutSurfaceHierarchy(visibleSurfaces), [visibleSurfaces]);
  const nodes = new Map(layout.nodes.map((node) => [node.surface.id, node]));
  const agents = map?.agents.filter((agent) => nodes.has(agent.surfaceId)) ?? [];

  const sessionCounts = sessionSurfaceCounts(map);
  return <section className={`workspace-surface-map${context === 'session' ? ' session-surface-map' : ''}`} aria-label={`${context === 'session' ? 'Session' : 'Workspace'} surfaces`} id={context === 'session' ? 'session-surfaces-panel' : 'workspace-dashboard-surfaces-panel'} role="tabpanel">
    <header className="workspace-surface-map-heading">
      <div className="settings-form-heading">
        <div className="workspace-catalog-title-row"><h2>{context === 'session' ? 'Surfaces' : `${workspaceName.trim() || 'Workspace'} Surfaces`}</h2><WorkspaceTitleSearch label="surfaces" query={query} onChange={setQuery} /></div>
        <p>{context === 'session' ? 'Surfaces touched during this session.' : 'System components and their current research context.'}</p>
      </div>
      {map ? <div className="workspace-surface-map-count">{context === 'session'
        ? `${sessionCounts.touched} Touched · ${sessionCounts.completed} Completed`
        : `${kindSurfaces.filter((surface) => surfaceCoveragePercent(surface) > 0).length} of ${kindSurfaces.length} touched · ${kindSurfaces.filter((surface) => surface.coverageState.covered).length} complete · ${kindSurfaces.filter((surface) => surface.progressBlocker).length} blocked`}</div> : null}
    </header>
    {error ? <p className="workspace-surface-map-status" role="alert">{error}</p> : !map
      ? <p className="workspace-surface-map-status" role="status">{loading ? 'Loading surfaces…' : 'Surface map unavailable.'}</p>
      : <>
    <div className="workspace-surface-map-filters">
      <label className="workspace-surface-map-untouched">
        <input type="checkbox" checked={showUntouched} onChange={(event) => setShowUntouched(event.target.checked)} />
        Show untouched surfaces
      </label>
    </div>
    {layout.nodes.length === 0 ? <p className="workspace-surface-map-empty">{query.trim()
      ? 'No surfaces match this filter.'
      : kindSurfaces.length && !showUntouched
        ? 'No surfaces have coverage yet. Show untouched surfaces to see the full map.'
        : 'No surfaces have been mapped in this workspace yet.'}</p> : <div className="workspace-surface-map-scroll">
      <div className="workspace-surface-map-canvas" style={{ width: layout.width, height: layout.height }}>
        <svg aria-hidden="true" className="workspace-surface-map-edges" width={layout.width} height={layout.height}>
          {layout.nodes.flatMap((node) => {
            const parent = node.surface.parentId ? nodes.get(node.surface.parentId) : null;
            if (!parent) return [];
            const x1 = parent.x + 220; const x2 = node.x;
            const y1 = parent.y + 36; const y2 = node.y + 36;
            return [<path key={node.surface.id} d={`M ${x1} ${y1} C ${x1 + 22} ${y1}, ${x2 - 22} ${y2}, ${x2} ${y2}`} />];
          })}
        </svg>
        {layout.nodes.map((node) => {
          const coverage = surfaceCoveragePercent(node.surface);
          return <button key={node.surface.id} className={`workspace-surface-node${selectedSurfaceId === node.surface.id ? ' selected' : ''}${node.surface.coverageState.covered ? ' covered' : ''}${node.surface.progressBlocker ? ' blocked' : ''}`}
            style={{ left: node.x, top: node.y, backgroundImage: `linear-gradient(90deg, var(--surface-coverage-fill) ${coverage}%, transparent ${coverage}%)` }}
            type="button" aria-label={`${node.surface.name}, ${coverage}% coverage${node.surface.progressBlocker ? ', blocked' : ''}`}
            aria-pressed={selectedSurfaceId === node.surface.id} onClick={() => onSelectSurface?.(node.surface)} title={`${node.surface.name}${node.surface.progressBlocker ? ` · Blocked: ${node.surface.progressBlocker.reason}` : ''}`}>
            <span className="workspace-surface-node-name">{node.surface.name}</span>
            {node.surface.progressBlocker ? <TriangleAlert aria-hidden="true" className="workspace-surface-node-blocked" size={16} /> : null}
          </button>;
        })}
        {agents.map((agent, index) => {
          const node = nodes.get(agent.surfaceId)!;
          const clustered = agents.slice(0, index).filter((candidate) => candidate.surfaceId === agent.surfaceId).length;
          return <span key={`${agent.sessionId}:${agent.agentId}`} className={`workspace-surface-agent-dot${agent.agentId === 'root' ? ' root' : ''}`}
            style={{ left: node.x + 16 + clustered * 16, top: node.y + 63 }}
            title={`${agent.agentId === 'root' ? 'Main agent' : agent.agentId} · ${agent.sessionTitle}`} />;
        })}
      </div>
    </div>}
    </>}
  </section>;
}
