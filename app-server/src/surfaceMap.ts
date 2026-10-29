import { existsSync } from 'node:fs';
import { DatabaseSync } from 'node:sqlite';
import { assessSurfaceClaims, assessSurfaceCoverage, hasThreatModelReview,
  type SurfaceFoundation, type SurfaceProgressBlocker } from '@beale/research-agent';
import type { BealeWorkspaceSurfaceMap } from '@beale/app-server-runtime/protocol';

type Row = Record<string, unknown>;
type Surface = BealeWorkspaceSurfaceMap['surfaces'][number];
type SurfaceHistoryEvent = NonNullable<Surface['history']>[number];
type StoredFoundation = SurfaceFoundation & { progressBlocker: SurfaceProgressBlocker | null };

function tableExists(database: DatabaseSync, name: string): boolean {
  return Boolean(database.prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?").get(name));
}
function tableHasColumn(database: DatabaseSync, table: string, column: string): boolean {
  return Boolean(database.prepare(`SELECT 1 FROM pragma_table_info('${table}') WHERE name=?`).get(column));
}

function string(value: unknown): string { return typeof value === 'string' ? value : ''; }
function object(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : null;
}
function parsed(value: unknown): Record<string, unknown> | null {
  if (typeof value !== 'string') return null;
  try { return object(JSON.parse(value) as unknown); } catch { return null; }
}
function foundation(value: unknown): StoredFoundation | null {
  const record = parsed(value);
  const coverage = object(record?.coverage);
  if (!coverage || !Array.isArray(coverage.openQuestions) || !Array.isArray(coverage.unexploredConnections)) return null;
  const blocker = object(record?.progressBlocker);
  const progressBlocker = blocker && typeof blocker.reason === 'string' && typeof blocker.actorId === 'string'
    && typeof blocker.setAt === 'string'
    ? { reason: blocker.reason, actorId: blocker.actorId, setAt: blocker.setAt } : null;
  return { ...record, progressBlocker } as unknown as StoredFoundation;
}
function displayFoundation(value: SurfaceFoundation | null): Surface['foundation'] {
  const coverage = value?.coverage;
  return coverage ? { coverage: {
    summary: coverage.summary, gaps: coverage.gaps, opportunities: coverage.opportunities,
    mappingComplete: coverage.mappingComplete === true,
    sinksInvestigated: coverage.sinksInvestigated === true,
    openQuestions: Array.isArray(coverage.openQuestions) ? coverage.openQuestions : [],
    unexploredConnections: Array.isArray(coverage.unexploredConnections) ? coverage.unexploredConnections : []
  }, threatModelReviewed: hasThreatModelReview(value), discovery: typeof value.discovery === 'string' ? value.discovery : '',
  ranking: value.ranking ?? null,
  techStack: Array.isArray(value.techStack) ? value.techStack : [],
  assets: Array.isArray(value.assets) ? value.assets : [],
  bugHistory: { summary: value.bugHistory?.summary ?? '', limits: value.bugHistory?.limits ?? '',
    entries: Array.isArray(value.bugHistory?.entries) ? value.bugHistory.entries : [] },
  threatModel: value.threatModel } : null;
}

/** App-server-owned, path-free projection of the durable workspace surface map. */
export function readWorkspaceSurfaceMap(databasePath: string, workspaceId: string, sessionId?: string): BealeWorkspaceSurfaceMap {
  const empty = (): BealeWorkspaceSurfaceMap => ({ schemaVersion: 1, workspaceId, surfaces: [], agents: [],
    ...(sessionId ? { sessionSurfaceIds: [] } : {}) });
  if (!existsSync(databasePath)) return empty();
  const database = new DatabaseSync(databasePath, { readOnly: true });
  try {
    if (!tableExists(database, 'app_server_research_resources')) return empty();
    const roots = (database.prepare(`SELECT id, name, locator, scope_asset_id, rationale, kind, direction, review_status, source, discovered_at, updated_at
      FROM app_server_research_resources WHERE workspace_id=? ORDER BY discovered_at, id`).all(workspaceId) as Row[])
      .filter((row) => row.source === 'explicit_scope'
        || row.direction !== 'out_of_scope' && row.review_status === 'relevant');
    const eligibleRootIds = new Set(roots.map((row) => string(row.id)));
    const resourceKinds = new Map(roots.map((row) => [string(row.id), string(row.kind)]));
    const resourceSources = new Map(roots.map((row) => [string(row.id), string(row.source)]));
    const resourceDirections = new Map(roots.map((row) => [string(row.id), string(row.direction)]));
    const children = tableExists(database, 'app_server_surfaces')
      ? (database.prepare(`SELECT id, resource_id, parent_id, name, description, foundation_json, created_at, updated_at
          FROM app_server_surfaces WHERE workspace_id=? ORDER BY created_at, id`).all(workspaceId) as Row[])
        .filter((row) => eligibleRootIds.has(string(row.resource_id))) : [];
    const rootRows = tableExists(database, 'app_server_surface_roots')
      ? database.prepare('SELECT resource_id, foundation_json, updated_at FROM app_server_surface_roots WHERE workspace_id=?').all(workspaceId) as Row[] : [];
    const rootFoundations = new Map<string, StoredFoundation | null>(rootRows
      .map((row) => [string(row.resource_id), foundation(row.foundation_json)]));
    const rootUpdates = new Map(rootRows.map((row) => [string(row.resource_id), string(row.updated_at)]));
    const rootUpdatedAt = (row: Row): string => {
      const resourceAt = string(row.updated_at);
      const surfaceAt = rootUpdates.get(string(row.id)) ?? '';
      return surfaceAt > resourceAt ? surfaceAt : resourceAt;
    };
    const firstTouches = new Map<string, { at: string; agentId: string; sessionId: string }>((tableExists(database, 'app_server_surface_touches')
      ? database.prepare('SELECT surface_id, first_touched_at, first_agent_id, first_session_id FROM app_server_surface_touches WHERE workspace_id=?').all(workspaceId) as Row[] : [])
      .map((row) => [string(row.surface_id), { at: string(row.first_touched_at), agentId: string(row.first_agent_id),
        sessionId: string(row.first_session_id) }]));
    const sessionModels = new Map<string, { provider: string | null; model: string }>((tableExists(database, 'app_server_sessions')
      ? database.prepare("SELECT id, json_extract(document_json, '$.provider') AS provider, json_extract(document_json, '$.model') AS model FROM app_server_sessions WHERE workspace_id=?")
        .all(workspaceId) as Row[] : [])
      .map((row): [string, { provider: string | null; model: string }] => [string(row.id), {
        provider: string(row.provider) || null, model: string(row.model) }]));
    const firstTouchedAuthor = (id: string): { provider: string | null; model: string } | null => {
      const touch = firstTouches.get(id);
      return touch?.agentId === 'root' ? sessionModels.get(touch.sessionId) ?? null : null;
    };
    const focusHistory = new Map<string, SurfaceHistoryEvent[]>();
    if (tableExists(database, 'app_server_surface_focus_events')) {
      const rows = database.prepare(`SELECT surface_id, agent_id, reason, moved_at FROM (
        SELECT surface_id, agent_id, reason, moved_at,
          ROW_NUMBER() OVER (PARTITION BY surface_id ORDER BY moved_at DESC, id DESC) AS event_rank
        FROM app_server_surface_focus_events WHERE workspace_id=?
      ) WHERE event_rank <= 40`).all(workspaceId) as Row[];
      for (const row of rows) {
        const id = string(row.surface_id);
        const event: SurfaceHistoryEvent = { kind: 'focused', at: string(row.moved_at), actorId: string(row.agent_id),
          detail: string(row.reason) === 'research' ? null : string(row.reason) };
        focusHistory.set(id, [...(focusHistory.get(id) ?? []), event]);
      }
    }
    const updateHistory = new Map<string, SurfaceHistoryEvent[]>();
    if (tableExists(database, 'app_server_surface_update_events')) {
      const rows = database.prepare(`SELECT surface_id, actor_id, kind, changed_at FROM (
        SELECT surface_id, actor_id, kind, changed_at,
          ROW_NUMBER() OVER (PARTITION BY surface_id ORDER BY changed_at DESC, id DESC) AS event_rank
        FROM app_server_surface_update_events WHERE workspace_id=?
      ) WHERE event_rank <= 40`).all(workspaceId) as Row[];
      for (const row of rows) {
        const id = string(row.surface_id);
        const kind = string(row.kind);
        if (kind !== 'updated' && kind !== 'blocked' && kind !== 'unblocked') continue;
        updateHistory.set(id, [...(updateHistory.get(id) ?? []),
          { kind, at: string(row.changed_at), actorId: string(row.actor_id) }]);
      }
    }
    const history = (id: string, createdAt: string, updatedAt: string): SurfaceHistoryEvent[] => {
      const touch = firstTouches.get(id);
      const focusEvents = (focusHistory.get(id) ?? []).filter((event) =>
        !touch || event.at !== touch.at || event.actorId !== touch.agentId);
      const updateEvents = updateHistory.get(id) ?? [];
      const latestTrackedUpdate = updateEvents.reduce((latest, event) => event.at > latest ? event.at : latest, createdAt);
      const recent = [
        ...updateEvents,
        ...focusEvents,
        ...(updatedAt > latestTrackedUpdate
          ? [{ kind: 'snapshot_update' as const, at: updatedAt, actorId: null }] : [])
      ].sort((left, right) => right.at.localeCompare(left.at)).slice(0, 40);
      return [
        ...recent,
        ...(touch ? [{ kind: 'first_touch' as const, at: touch.at, actorId: touch.agentId }] : []),
        { kind: 'created' as const, at: createdAt, actorId: null }
      ].sort((left, right) => right.at.localeCompare(left.at));
    };
    const raw = [
      ...roots.map((row) => ({ id: string(row.id), resourceId: string(row.id),
        resourceKind: string(row.kind), resourceSource: string(row.source), resourceDirection: string(row.direction),
        resourceLocator: string(row.locator), scopeAssetId: row.scope_asset_id === null ? null : string(row.scope_asset_id),
        parentId: null, name: string(row.name),
        description: string(row.rationale), updatedAt: rootUpdatedAt(row),
        firstTouchedAt: firstTouches.get(string(row.id))?.at ?? null,
        firstTouchedBy: firstTouches.get(string(row.id))?.agentId ?? null,
        firstTouchedModel: firstTouchedAuthor(string(row.id))?.model || null,
        firstTouchedProvider: firstTouchedAuthor(string(row.id))?.provider ?? null,
        history: history(string(row.id), string(row.discovered_at), rootUpdatedAt(row)),
        foundation: rootFoundations.get(string(row.id)) ?? null })),
      ...children.map((row) => ({ id: string(row.id), resourceId: string(row.resource_id),
        resourceKind: resourceKinds.get(string(row.resource_id)) ?? '',
        resourceSource: resourceSources.get(string(row.resource_id)) ?? '',
        resourceDirection: resourceDirections.get(string(row.resource_id)) ?? '', parentId: string(row.parent_id),
        name: string(row.name), description: string(row.description), updatedAt: string(row.updated_at),
        firstTouchedAt: firstTouches.get(string(row.id))?.at ?? null,
        firstTouchedBy: firstTouches.get(string(row.id))?.agentId ?? null,
        firstTouchedModel: firstTouchedAuthor(string(row.id))?.model || null,
        firstTouchedProvider: firstTouchedAuthor(string(row.id))?.provider ?? null,
        history: history(string(row.id), string(row.created_at), string(row.updated_at)),
        foundation: foundation(row.foundation_json) }))
    ];
    const claims = tableExists(database, 'app_server_research_claims')
      ? (database.prepare(`SELECT id, status, ${tableHasColumn(database, 'app_server_research_claims', 'stale_from_status')
        ? 'stale_from_status' : 'NULL AS stale_from_status'}, ${tableHasColumn(database, 'app_server_research_claims', 'surface_id')
        ? 'surface_id' : 'NULL AS surface_id'}, security_tracking_json
        FROM app_server_research_claims WHERE workspace_id=? AND duplicate_of_claim_id IS NULL ORDER BY id`).all(workspaceId) as Row[])
        .map((claim) => ({ id: string(claim.id), status: string(claim.status),
          staleFromStatus: claim.stale_from_status === null ? null : string(claim.stale_from_status),
          surfaceId: claim.surface_id === null ? null : string(claim.surface_id),
          securityTracking: parsed(claim.security_tracking_json) })) : [];
    const surfaces = raw.map((surface) => {
      const { openLeadIds, unblockedFindingIds } = assessSurfaceClaims(surface.id, claims);
      return { ...surface, progressBlocker: surface.foundation?.progressBlocker ?? null,
        foundation: displayFoundation(surface.foundation),
        coverageState: assessSurfaceCoverage(surface.foundation, openLeadIds, unblockedFindingIds) };
    });
    const visibleSurfaceIds = new Set(surfaces.map((surface) => surface.id));
    const agents = tableExists(database, 'app_server_surface_focus') && tableExists(database, 'app_server_sessions')
      ? (database.prepare(`SELECT focus.session_id, sessions.title AS session_title, focus.agent_id, focus.surface_id
          FROM app_server_surface_focus AS focus
          JOIN app_server_sessions AS sessions ON sessions.id = focus.session_id AND sessions.status = 'active'
          WHERE focus.workspace_id=? ORDER BY focus.session_id, focus.agent_id`).all(workspaceId) as Row[])
        .filter((row) => visibleSurfaceIds.has(string(row.surface_id)))
        .map((row) => ({ sessionId: string(row.session_id), sessionTitle: string(row.session_title),
          agentId: string(row.agent_id), surfaceId: string(row.surface_id) })) : [];
    const sessionSurfaceIds = new Set<string>();
    if (sessionId) {
      if (tableExists(database, 'app_server_surface_touches')) {
        for (const row of database.prepare('SELECT surface_id FROM app_server_surface_touches WHERE workspace_id=? AND first_session_id=?')
          .all(workspaceId, sessionId) as Row[]) sessionSurfaceIds.add(string(row.surface_id));
      }
      if (tableExists(database, 'app_server_surface_focus_events')) {
        for (const row of database.prepare('SELECT surface_id FROM app_server_surface_focus_events WHERE workspace_id=? AND session_id=?')
          .all(workspaceId, sessionId) as Row[]) sessionSurfaceIds.add(string(row.surface_id));
      }
      if (tableExists(database, 'app_server_surface_update_events')) {
        for (const row of database.prepare('SELECT surface_id FROM app_server_surface_update_events WHERE workspace_id=? AND session_id=?')
          .all(workspaceId, sessionId) as Row[]) sessionSurfaceIds.add(string(row.surface_id));
      }
      if (tableExists(database, 'app_server_surfaces') && tableHasColumn(database, 'app_server_surfaces', 'created_session_id')) {
        for (const row of database.prepare('SELECT id FROM app_server_surfaces WHERE workspace_id=? AND created_session_id=?')
          .all(workspaceId, sessionId) as Row[]) sessionSurfaceIds.add(string(row.id));
      }
    }
    return { schemaVersion: 1, workspaceId, surfaces, agents,
      ...(sessionId ? { sessionSurfaceIds: surfaces.filter((surface) => sessionSurfaceIds.has(surface.id)).map((surface) => surface.id) } : {}) };
  } finally {
    database.close();
  }
}
