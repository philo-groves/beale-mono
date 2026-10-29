import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import test from 'node:test';
import { AppServerSessionStore, ResearchResourceCatalog, ResearchTrackStore, SurfaceStore } from '../packages/research-agent/dist/index.js';
import { AppServerHostService } from '../app-server/dist/hostService.js';
import { readWorkspaceSurfaceMap } from '../app-server/dist/surfaceMap.js';
import { decodeBealeWorkspaceSurfaceMap, decodeBealeWorkspaceSurfaceMapRequest } from '../packages/app-server-runtime/dist/protocol.js';

test('surface map projects eligible hierarchy, touch state, coverage, and active agent focus', () => {
  assert.deepEqual(decodeBealeWorkspaceSurfaceMapRequest({ workspaceId: 'workspace-example', sessionId: 'session-example-one' }),
    { workspaceId: 'workspace-example', sessionId: 'session-example-one' });
  assert.throws(() => decodeBealeWorkspaceSurfaceMapRequest({ workspaceId: 'workspace-example', sessionId: '' }), /Invalid workspace surface map request/);
  const directory = mkdtempSync(join(tmpdir(), 'beale-surface-map-example-'));
  const databasePath = join(directory, 'memory.sqlite');
  const workspaceId = 'workspace-example';
  const sessions = new AppServerSessionStore({ databasePath });
  const resources = new ResearchResourceCatalog({ databasePath, workspaceId, explicitResources: [
    { id: 'asset-example', direction: 'in_scope', kind: 'service', locator: 'service.example.test', name: 'Example service', source: 'explicit_scope' },
    { id: 'asset-excluded', direction: 'out_of_scope', kind: 'service', locator: 'excluded.example.test', name: 'Excluded service', source: 'explicit_scope' }
  ] });
  const surfaces = new SurfaceStore({ databasePath, workspaceId, sessionId: 'session-example-one', resources });
  try {
    const first = sessions.create({ id: 'session-example-one', workspaceId, attemptId: 'attempt-example-one',
      title: 'Map service', prompt: 'Map authorized service boundaries.', provider: 'openai-codex', model: 'gpt-example',
      reasoningEffort: 'high' });
    const root = resources.list().find((resource) => resource.direction === 'in_scope');
    const child = surfaces.create(root.id, 'Request parser', 'Untrusted request boundary.');
    assert.deepEqual(readWorkspaceSurfaceMap(databasePath, workspaceId, 'session-example-one').sessionSurfaceIds, [child.id]);
    assert.equal(readWorkspaceSurfaceMap(databasePath, workspaceId).surfaces.find((surface) => surface.id === child.id).firstTouchedAt, null);
    surfaces.focus('root', child.id);
    surfaces.focus('/root/scout', root.id);
    const foundation = {
      discovery: 'Example service request parsing is reachable through its public API.',
      ranking: Object.fromEntries(['impact', 'reachability', 'relevance', 'exposure', 'precedence']
        .map((factor) => [factor, { score: 7, basis: `Synthetic ${factor} basis.` }])),
      techStack: [{ technology: 'Example Parser', role: 'Request parsing', version: '2.x', environment: 'Example service',
        evidence: 'Synthetic package manifest', confidence: 'observed' }],
      assets: [{ name: 'src/request_parser.ts', status: 'in_progress' }],
      bugHistory: { summary: 'Earlier search note.', limits: 'Search limited to public sources.', entries: [
        { identifier: 'EXAMPLE-ADVISORY-001', impact: 'Malformed requests may bypass validation.', affectedVersions: '1.x',
          reference: 'https://example.test/advisory', notes: 'Applicability to 2.x is unknown.' }
      ] },
      coverage: { summary: 'Mapped input to sink.', gaps: 'No remaining gaps.', opportunities: 'Review future parser modes.', mappingComplete: true,
        sinksInvestigated: true, openQuestions: [], unexploredConnections: [] },
      threatModel: { diagram: 'flowchart LR\n  input["Request"] --> parser["Parser"]',
        securityObjectives: 'Protect example state.', riskImpact: 'Unauthorized change.', assumptions: 'Service enabled.',
        invariants: 'Only validated requests reach dispatch.', sinks: 'State update.', mitigations: 'Validation before dispatch.',
        attackerControl: 'Request bytes.', trustBoundaries: 'Network to parser.',
        assetsAtRisk: 'Example state.', plausibleImpacts: 'Unauthorized change.' }
    };
    surfaces.update(child.id, child.revision, foundation);
    surfaces.update(root.id, 1, foundation);
    const legacyDatabase = new DatabaseSync(databasePath);
    try {
      const row = legacyDatabase.prepare('SELECT foundation_json FROM app_server_surfaces WHERE id=?').get(child.id);
      const stored = JSON.parse(row.foundation_json);
      delete stored.threatModelReviewed;
      legacyDatabase.prepare('UPDATE app_server_surfaces SET foundation_json=? WHERE id=?')
        .run(JSON.stringify(stored), child.id);
    } finally { legacyDatabase.close(); }
    surfaces.focus('root', root.id);
    surfaces.focus('root', child.id);
    const map = readWorkspaceSurfaceMap(databasePath, workspaceId);
    assert.deepEqual(new Set(readWorkspaceSurfaceMap(databasePath, workspaceId, 'session-example-one').sessionSurfaceIds),
      new Set([root.id, child.id]));
    assert.equal(map.surfaces.length, 3);
    assert.equal(map.surfaces.find((surface) => surface.scopeAssetId === 'asset-example').resourceLocator, 'service.example.test');
    assert.equal(map.surfaces.find((surface) => surface.scopeAssetId === 'asset-excluded').resourceDirection, 'out_of_scope');
    assert.equal(map.surfaces.find((surface) => surface.id === child.id).parentId, root.id);
    assert.equal(map.surfaces.find((surface) => surface.id === child.id).resourceKind, 'service');
    assert.equal(map.surfaces.find((surface) => surface.id === child.id).resourceSource, 'explicit_scope');
    assert.equal(map.surfaces.find((surface) => surface.id === child.id).firstTouchedBy, 'root');
    assert.equal(map.surfaces.find((surface) => surface.id === root.id).firstTouchedBy, '/root/scout');
    assert.equal(map.surfaces.find((surface) => surface.id === child.id).firstTouchedModel, 'gpt-example');
    assert.equal(map.surfaces.find((surface) => surface.id === child.id).firstTouchedProvider, 'openai-codex');
    assert.equal(map.surfaces.find((surface) => surface.id === root.id).firstTouchedModel, null);
    assert.equal(map.surfaces.find((surface) => surface.id === root.id).firstTouchedProvider, null);
    assert.equal(map.surfaces.find((surface) => surface.id === child.id).updatedAt, surfaces.get(child.id).updatedAt);
    assert.equal(map.surfaces.find((surface) => surface.id === root.id).updatedAt, surfaces.get(root.id).updatedAt);
    assert.deepEqual(new Set(map.surfaces.find((surface) => surface.id === child.id).history.map((event) => event.kind)),
      new Set(['updated', 'focused', 'first_touch', 'created']));
    assert.equal(map.surfaces.find((surface) => surface.id === child.id).history.filter((event) => event.kind === 'first_touch').length, 1);
    assert.ok(map.surfaces.find((surface) => surface.id === child.id).history.some((event) => event.kind === 'updated' && event.actorId === 'host'));
    assert.equal(map.surfaces.find((surface) => surface.id === child.id).foundation.coverage.summary, 'Mapped input to sink.');
    assert.deepEqual(map.surfaces.find((surface) => surface.id === child.id).foundation.ranking, foundation.ranking);
    assert.equal(map.surfaces.find((surface) => surface.id === child.id).foundation.threatModel.attackerControl, 'Request bytes.');
    assert.equal(map.surfaces.find((surface) => surface.id === child.id).foundation.threatModel.diagram, foundation.threatModel.diagram);
    const canonical = decodeBealeWorkspaceSurfaceMap({ ...map, surfaces: map.surfaces.map((surface) =>
      surface.id === child.id ? { ...surface, foundation: { ...surface.foundation, threatModel: {
        ...surface.foundation.threatModel, securityObjectives: ['Protect example state.'],
        risks: ['Unauthorized change.'], assumptions: ['Service enabled.'] } } } : surface) });
    assert.deepEqual(canonical.surfaces.find((surface) => surface.id === child.id).foundation.threatModel.risks, ['Unauthorized change.']);
    assert.throws(() => decodeBealeWorkspaceSurfaceMap({ ...map, surfaces: map.surfaces.map((surface) =>
      surface.id === child.id ? { ...surface, foundation: { ...surface.foundation, threatModel: {
        ...surface.foundation.threatModel, risks: [7] } } } : surface) }), /invalid workspace surface map/);
    assert.throws(() => decodeBealeWorkspaceSurfaceMap({ ...map, surfaces: map.surfaces.map((surface) =>
      surface.id === child.id ? { ...surface, foundation: { ...surface.foundation, ranking: {
        ...surface.foundation.ranking, exposure: { score: 11, basis: 'Synthetic exposure basis.' } } } } : surface) }),
    /invalid workspace surface map/);
    assert.equal(map.surfaces.find((surface) => surface.id === child.id).foundation.threatModelReviewed, true);
    assert.deepEqual(map.surfaces.find((surface) => surface.id === child.id).coverageState.openLeadIds, []);
    assert.equal(map.surfaces.find((surface) => surface.id === child.id).foundation.bugHistory.limits, 'Search limited to public sources.');
    assert.equal(map.surfaces.find((surface) => surface.id === child.id).foundation.discovery, foundation.discovery);
    assert.equal(map.surfaces.find((surface) => surface.id === child.id).foundation.techStack[0].technology, 'Example Parser');
    assert.deepEqual(map.surfaces.find((surface) => surface.id === child.id).foundation.assets,
      [{ name: 'src/request_parser.ts', status: 'in_progress' }]);
    assert.equal(map.surfaces.find((surface) => surface.id === child.id).foundation.bugHistory.entries[0].identifier, 'EXAMPLE-ADVISORY-001');
    assert.ok(map.surfaces.filter((surface) => surface.resourceDirection !== 'out_of_scope').every((surface) => surface.firstTouchedAt));
    assert.equal(map.surfaces.filter((surface) => surface.resourceDirection !== 'out_of_scope').every((surface) => surface.coverageState.covered), true);
    assert.equal(map.surfaces.every((surface) => surface.progressBlocker === null), true);
    const reason = 'Public routes were tested without a session; an authorized account is required for authenticated requests. Provide a test account to resume.';
    surfaces.update(child.id, surfaces.get(child.id).revision, { ...foundation,
      coverage: { ...foundation.coverage, gaps: 'Authenticated request paths remain untested.' } },
    undefined, undefined, { reason }, '/root/scout');
    const blocked = decodeBealeWorkspaceSurfaceMap(readWorkspaceSurfaceMap(databasePath, workspaceId));
    assert.equal(blocked.surfaces.find((surface) => surface.id === child.id).progressBlocker.reason, reason);
    assert.ok(blocked.surfaces.find((surface) => surface.id === child.id).history.some((event) =>
      event.kind === 'blocked' && event.actorId === '/root/scout'));
    assert.deepEqual(blocked.surfaces.find((surface) => surface.id === child.id).coverageState.unmetRequirements, []);
    assert.deepEqual(blocked.surfaces.find((surface) => surface.id === root.id).coverageState.unmetRequirements, []);
    assert.throws(() => decodeBealeWorkspaceSurfaceMap({ ...blocked, surfaces: blocked.surfaces.map((surface) =>
      surface.id === child.id ? { ...surface, progressBlocker: { reason } } : surface) }), /invalid workspace surface map/);
    assert.throws(() => decodeBealeWorkspaceSurfaceMap({ ...blocked, surfaces: blocked.surfaces.map((surface) =>
      surface.id === root.id ? { ...surface, scopeAssetId: 42 } : surface) }), /invalid workspace surface map/);
    assert.throws(() => decodeBealeWorkspaceSurfaceMap({ ...blocked, surfaces: blocked.surfaces.map((surface) =>
      surface.id === child.id ? { ...surface, resourceKind: 7 } : surface) }), /invalid workspace surface map/);
    assert.throws(() => decodeBealeWorkspaceSurfaceMap({ ...blocked, surfaces: blocked.surfaces.map((surface) =>
      surface.id === child.id ? { ...surface, firstTouchedBy: 7 } : surface) }), /invalid workspace surface map/);
    assert.throws(() => decodeBealeWorkspaceSurfaceMap({ ...blocked, surfaces: blocked.surfaces.map((surface) =>
      surface.id === child.id ? { ...surface, firstTouchedModel: 7 } : surface) }), /invalid workspace surface map/);
    assert.throws(() => decodeBealeWorkspaceSurfaceMap({ ...blocked, surfaces: blocked.surfaces.map((surface) =>
      surface.id === child.id ? { ...surface, firstTouchedProvider: 7 } : surface) }), /invalid workspace surface map/);
    assert.throws(() => decodeBealeWorkspaceSurfaceMap({ ...blocked, surfaces: blocked.surfaces.map((surface) =>
      surface.id === child.id ? { ...surface, updatedAt: 7 } : surface) }), /invalid workspace surface map/);
    assert.throws(() => decodeBealeWorkspaceSurfaceMap({ ...blocked, surfaces: blocked.surfaces.map((surface) =>
      surface.id === child.id ? { ...surface, history: [{ kind: 'updated', at: 7, actorId: 'root' }] } : surface) }), /invalid workspace surface map/);
    assert.throws(() => decodeBealeWorkspaceSurfaceMap({ ...blocked, surfaces: blocked.surfaces.map((surface) =>
      surface.id === child.id ? { ...surface, foundation: { ...surface.foundation, threatModel: { attackerControl: 7 } } } : surface) }), /invalid workspace surface map/);
    assert.throws(() => decodeBealeWorkspaceSurfaceMap({ ...blocked, surfaces: blocked.surfaces.map((surface) =>
      surface.id === child.id ? { ...surface, foundation: { ...surface.foundation,
        threatModel: { ...surface.foundation.threatModel, diagram: 7 } } } : surface) }), /invalid workspace surface map/);
    assert.throws(() => decodeBealeWorkspaceSurfaceMap({ ...blocked, surfaces: blocked.surfaces.map((surface) =>
      surface.id === child.id ? { ...surface, foundation: { ...surface.foundation, techStack: [{ technology: 7 }] } } : surface) }), /invalid workspace surface map/);
    assert.throws(() => decodeBealeWorkspaceSurfaceMap({ ...blocked, surfaces: blocked.surfaces.map((surface) =>
      surface.id === child.id ? { ...surface, foundation: { ...surface.foundation, assets: [{ name: 'example.ts', status: 'queued' }] } } : surface) }), /invalid workspace surface map/);
    assert.throws(() => decodeBealeWorkspaceSurfaceMap({ ...blocked, surfaces: blocked.surfaces.map((surface) =>
      surface.id === child.id ? { ...surface, foundation: { ...surface.foundation, threatModelReviewed: 'yes' } } : surface) }), /invalid workspace surface map/);
    assert.throws(() => decodeBealeWorkspaceSurfaceMap({ ...blocked, surfaces: blocked.surfaces.map((surface) =>
      surface.id === child.id ? { ...surface, coverageState: { ...surface.coverageState, openLeadIds: [7] } } : surface) }), /invalid workspace surface map/);
    assert.throws(() => decodeBealeWorkspaceSurfaceMap({ ...blocked, surfaces: blocked.surfaces.map((surface) =>
      surface.id === child.id ? { ...surface, foundation: { ...surface.foundation,
        bugHistory: { ...surface.foundation.bugHistory, entries: [{ identifier: 'EXAMPLE-ADVISORY-002', impact: 7 }] } } } : surface) }), /invalid workspace surface map/);
    const discovered = resources.discover({ kind: 'binary', name: 'Example daemon',
      locator: 'binary.example.test/daemon', rationale: 'Example authorized service executable.' });
    assert.equal(readWorkspaceSurfaceMap(databasePath, workspaceId).surfaces.some((surface) => surface.id === discovered.id), false);
    resources.recordReview(discovered.id, { decision: 'relevant', reason: 'Covered by the synthetic workspace scope.', source: 'auto_review' });
    const approved = readWorkspaceSurfaceMap(databasePath, workspaceId).surfaces.find((surface) => surface.id === discovered.id);
    assert.equal(approved.resourceKind, 'binary');
    assert.equal(approved.resourceSource, 'runtime_discovery');
    assert.deepEqual(new Set(map.agents.map((agent) => agent.agentId)), new Set(['root', '/root/scout']));
    assert.equal(map.agents[0].sessionTitle, 'Map service');
    const otherSessionSurfaces = new SurfaceStore({ databasePath, workspaceId, sessionId: 'session-example-two', resources });
    try {
      const otherChild = otherSessionSurfaces.create(root.id, 'Response serializer', 'Example response boundary.');
      otherSessionSurfaces.focus('root', root.id);
      assert.deepEqual(new Set(readWorkspaceSurfaceMap(databasePath, workspaceId, 'session-example-two').sessionSurfaceIds),
        new Set([root.id, otherChild.id]));
      assert.deepEqual(new Set(readWorkspaceSurfaceMap(databasePath, workspaceId, 'session-example-one').sessionSurfaceIds),
        new Set([root.id, child.id]));
      otherSessionSurfaces.update(child.id, otherSessionSurfaces.get(child.id).revision);
      assert.deepEqual(new Set(readWorkspaceSurfaceMap(databasePath, workspaceId, 'session-example-two').sessionSurfaceIds),
        new Set([root.id, otherChild.id, child.id]));
    } finally { otherSessionSurfaces.close(); }
    assert.throws(() => decodeBealeWorkspaceSurfaceMap({ ...map, sessionSurfaceIds: [7] }), /invalid workspace surface map/);
    const claimDb = new DatabaseSync(databasePath);
    try {
      claimDb.exec(`CREATE TABLE app_server_research_claims (
        id TEXT, workspace_id TEXT, status TEXT, stale_from_status TEXT, surface_id TEXT,
        duplicate_of_claim_id TEXT, security_tracking_json TEXT
      )`);
      claimDb.prepare(`INSERT INTO app_server_research_claims VALUES (?, ?, ?, NULL, ?, NULL, ?)`)
        .run('lead-example', workspaceId, 'hypothesis', child.id, '{}');
      const active = decodeBealeWorkspaceSurfaceMap(readWorkspaceSurfaceMap(databasePath, workspaceId));
      assert.deepEqual(active.surfaces.find((surface) => surface.id === child.id).coverageState.openLeadIds, ['lead-example']);
      assert.equal(active.surfaces.find((surface) => surface.id === child.id).coverageState.covered, false);
      assert.equal(active.surfaces.find((surface) => surface.id === root.id).coverageState.covered, true);
      claimDb.prepare("UPDATE app_server_research_claims SET status='rejected' WHERE id='lead-example'").run();
      assert.equal(readWorkspaceSurfaceMap(databasePath, workspaceId).surfaces.find((surface) => surface.id === child.id).coverageState.covered, true);
    } finally { claimDb.close(); }
    const database = new DatabaseSync(databasePath, { readOnly: true });
    try {
      assert.equal(database.prepare("SELECT COUNT(*) AS count FROM sqlite_master WHERE type='trigger' AND name LIKE 'app_server_session_activity_%'").get().count, 0);
      assert.equal(database.prepare('SELECT COUNT(*) AS count FROM app_server_session_activity_events').get().count, 0);
    } finally { database.close(); }
    sessions.transition(first.id, { status: 'completed', summary: 'Complete.', expectedRevision: first.revision });
    assert.deepEqual(readWorkspaceSurfaceMap(databasePath, workspaceId).agents, []);
    assert.deepEqual(readWorkspaceSurfaceMap(databasePath, 'workspace-other').surfaces, []);
  } finally {
    surfaces.close(); resources.close(); sessions.close(); rmSync(directory, { recursive: true, force: true });
  }
});

test('existing surface tables migrate to record the creating session', () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-surface-session-migration-example-'));
  const databasePath = join(directory, 'memory.sqlite');
  const workspaceId = 'workspace-example';
  const resources = new ResearchResourceCatalog({ databasePath, workspaceId, explicitResources: [
    { id: 'asset-example', direction: 'in_scope', kind: 'service', locator: 'service.example.test',
      name: 'Example service', source: 'explicit_scope' }
  ] });
  try {
    const database = new DatabaseSync(databasePath);
    try {
      database.exec(`CREATE TABLE app_server_surfaces (
        id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, resource_id TEXT NOT NULL,
        parent_id TEXT NOT NULL, name TEXT NOT NULL, description TEXT NOT NULL,
        foundation_json TEXT, revision INTEGER NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL
      )`);
    } finally { database.close(); }
    const surfaces = new SurfaceStore({ databasePath, workspaceId, sessionId: 'session-example', resources });
    try {
      const child = surfaces.create(resources.list()[0].id, 'Parser', 'Example parser boundary.');
      assert.deepEqual(readWorkspaceSurfaceMap(databasePath, workspaceId, 'session-example').sessionSurfaceIds, [child.id]);
    } finally { surfaces.close(); }
  } finally { resources.close(); rmSync(directory, { recursive: true, force: true }); }
});

test('new workspace scope resources seed root surfaces before the first research session', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-surface-scope-example-'));
  const databasePath = join(directory, 'memory.sqlite');
  const workspaceId = 'workspace-scope-example';
  const database = new DatabaseSync(databasePath);
  try {
    database.exec(`
      CREATE TABLE workspaces (id TEXT PRIMARY KEY, workspace_path TEXT NOT NULL);
      CREATE TABLE scope_versions (
        id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, version INTEGER NOT NULL,
        status TEXT NOT NULL, workspace_name TEXT NOT NULL, scope_owner TEXT NOT NULL,
        description_markdown TEXT NOT NULL, active_from TEXT NOT NULL, expires_at TEXT,
        created_at TEXT NOT NULL
      );
      CREATE TABLE scope_assets (
        id TEXT PRIMARY KEY, scope_version_id TEXT NOT NULL, direction TEXT NOT NULL,
        kind TEXT NOT NULL, value TEXT NOT NULL, sensitivity TEXT NOT NULL,
        attributes_json TEXT NOT NULL, created_at TEXT NOT NULL
      );
    `);
    database.prepare('INSERT INTO workspaces VALUES (?, ?)').run(workspaceId, directory);
    const insertScope = database.prepare('INSERT INTO scope_versions VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)');
    const insertAsset = database.prepare('INSERT INTO scope_assets VALUES (?, ?, ?, ?, ?, ?, ?, ?)');
    insertScope.run('scope-example-first', workspaceId, 1, 'active', 'Example workspace', 'ExampleCo',
      'Authorized example resources.', '2026-01-01T00:00:00Z', null, '2026-01-01T00:00:00Z');
    insertAsset.run('asset-example-service', 'scope-example-first', 'in_scope', 'service',
      'api.example.test', 'public', '{}', '2026-01-01T00:00:01Z');
    insertAsset.run('asset-example-excluded', 'scope-example-first', 'out_of_scope', 'domain',
      'excluded.example.test', 'public', '{}', '2026-01-01T00:00:02Z');
    insertAsset.run('asset-example-credential', 'scope-example-first', 'in_scope', 'other',
      'secret://example-credential', 'secret', '{"legacyKind":"credential_ref"}', '2026-01-01T00:00:03Z');
    const workspace = { workspaceId, workspacePath: directory, researchProfileId: 'security-research', memoryBackend: 'app-server' };
    const service = new AppServerHostService({ registry: {
      registryDirectory: join(directory, 'registry'),
      resolveWorkspace: (identifier) => identifier === workspaceId ? workspace : null,
      storageForProfile: () => ({ databasePath, artifactDirectoryPath: join(directory, 'artifacts') })
    } });

    assert.equal(readWorkspaceSurfaceMap(databasePath, workspaceId).surfaces.length, 0);
    const first = await service.workspaceSurfaceMap(workspaceId);
    assert.equal(first.surfaces.length, 2);
    assert.equal(first.surfaces.find((surface) => surface.scopeAssetId === 'asset-example-service').name, 'api.example.test');
    assert.equal(first.surfaces.find((surface) => surface.scopeAssetId === 'asset-example-excluded').resourceDirection, 'out_of_scope');
    const firstId = first.surfaces.find((surface) => surface.scopeAssetId === 'asset-example-service').id;
    assert.equal(database.prepare('SELECT scope_asset_id FROM app_server_research_resources WHERE id=?').get(firstId).scope_asset_id,
      'asset-example-service');
    const before = database.prepare('SELECT updated_at FROM app_server_research_resources WHERE id=?').get(firstId).updated_at;
    assert.equal((await service.workspaceSurfaceMap(workspaceId)).surfaces.find((surface) => surface.scopeAssetId === 'asset-example-service').id, firstId);
    workspace.memoryBackend = 'disabled';
    assert.equal((await service.workspaceSurfaceMap(workspaceId)).surfaces.length, 2);
    workspace.memoryBackend = 'app-server';
    assert.equal(database.prepare('SELECT updated_at FROM app_server_research_resources WHERE id=?').get(firstId).updated_at, before);
    const track = new ResearchTrackStore({ databasePath, workspaceId });
    try {
      assert.equal(track.start({ scopeId: 'scope-example-first', provider: { id: 'openai-codex' },
        targetIds: ['local'], surfaces: first.surfaces }).surfaces.length, 1);
    } finally { track.close(); }

    database.prepare("UPDATE scope_versions SET status='inactive' WHERE id='scope-example-first'").run();
    insertScope.run('scope-example-second', workspaceId, 2, 'active', 'Example workspace', 'ExampleCo',
      'Updated authorized example resources.', '2026-02-01T00:00:00Z', null, '2026-02-01T00:00:00Z');
    insertAsset.run('asset-example-repository', 'scope-example-second', 'in_scope', 'repo',
      'https://github.com/example-org/example-repository', 'public', '{}', '2026-02-01T00:00:01Z');
    const second = await service.workspaceSurfaceMap(workspaceId);
    assert.equal(second.surfaces.length, 1);
    assert.equal(second.surfaces[0].name, 'https://github.com/example-org/example-repository');
    assert.notEqual(second.surfaces[0].id, firstId);
  } finally {
    database.close();
    rmSync(directory, { recursive: true, force: true });
  }
});
