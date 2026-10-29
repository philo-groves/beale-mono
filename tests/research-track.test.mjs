import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import test from 'node:test';
import { ResearchTrackStore, createResearchTrackTool, researchTrackProgress } from '../packages/research-agent/dist/index.js';
import { AppServerHostService } from '../app-server/dist/hostService.js';

const workspaceId = 'workspace-example-track';
const covered = { covered: true, unmetRequirements: [] };
const open = { covered: false, unmetRequirements: ['surface_map', 'sinks'] };
const surfaces = [
  { id: 'resource-example-a', parentId: null, name: 'Example service', coverageState: open, progressBlocker: null },
  { id: 'resource-example-b', parentId: null, name: 'Example library', coverageState: open, progressBlocker: null },
];

function angle(name, status = 'open', evidenceRefs = []) {
  return { name, status, vector: {
    attackerControl: `Control the ${name} input`, trustBoundary: `Example ${name} authority boundary`,
    failureMode: `Example ${name} policy mismatch`, unauthorizedEffect: `Access the ${name} protected asset`
  }, proofQuestion: `Can ${name} cross the example trust boundary?`,
    evidenceRefs, nextAction: `Test ${name} with a discriminating input.` };
}

function fixture() {
  const directory = mkdtempSync(join(tmpdir(), 'beale-track-example-'));
  const databasePath = join(directory, 'memory.sqlite');
  const store = new ResearchTrackStore({ databasePath, workspaceId });
  const database = new DatabaseSync(databasePath);
  database.exec(`CREATE TABLE memory_nodes (id TEXT PRIMARY KEY, updated_at TEXT NOT NULL,
      status TEXT NOT NULL DEFAULT 'draft', duplicate_of_memory_id TEXT);
    CREATE TABLE memory_node_workspaces (node_id TEXT NOT NULL, workspace_id TEXT NOT NULL,
      workspace_name TEXT NOT NULL, PRIMARY KEY(node_id, workspace_id));
    CREATE TABLE memory_evidence_refs (id TEXT PRIMARY KEY, node_id TEXT NOT NULL, kind TEXT NOT NULL,
      path TEXT, locator_json TEXT NOT NULL DEFAULT '{}', created_at TEXT NOT NULL)`);
  database.prepare('INSERT INTO memory_nodes (id, updated_at) VALUES (?, ?)').run('memory-example-a', '2000-01-01T00:00:00.000Z');
  database.prepare('INSERT INTO memory_nodes (id, updated_at) VALUES (?, ?)').run('memory-other-workspace', '2000-01-01T00:00:00.000Z');
  database.prepare('INSERT INTO memory_node_workspaces VALUES (?, ?, ?)')
    .run('memory-example-a', workspaceId, 'Example workspace');
  database.prepare('INSERT INTO memory_node_workspaces VALUES (?, ?, ?)')
    .run('memory-other-workspace', 'workspace-other', 'Other workspace');
  return { store, database, databasePath, close() { database.close(); store.close(); rmSync(directory, { recursive: true, force: true }); } };
}

let nextEvidenceId = 0;
function recordMemoryEvidence(state, kind = 'code') {
  const id = `evidence-example-${++nextEvidenceId}`;
  state.database.prepare(`INSERT INTO memory_evidence_refs (id, node_id, kind, path, locator_json, created_at)
    VALUES (?, ?, ?, ?, '{}', ?)`).run(id, 'memory-example-a', kind,
    kind === 'code' ? 'src/example.ts' : null, new Date().toISOString());
  return id;
}

async function afterEpochStart() { await new Promise((resolve) => setTimeout(resolve, 5)); }

function interruptedTrackSession(guardianEnabled) {
  return { status: 'active', attempts: [{ id: 'attempt-example-interrupted', parentAttemptId: null }],
    metadata: { appServerRestartLaunch: { schemaVersion: 1, eligible: true,
      launch: { workspaceId, mode: 'research-track', machineId: 'local', targetIds: ['local'],
        promptMarkdown: 'Continue synthetic surface research.', provider: { id: 'openai-codex' }, guardianEnabled } } } };
}

test('existing track tables gain optional launch configuration columns', () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-track-migration-example-'));
  const databasePath = join(directory, 'memory.sqlite');
  const database = new DatabaseSync(databasePath);
  try {
    database.exec(`CREATE TABLE app_server_research_tracks (
      workspace_id TEXT PRIMARY KEY, id TEXT NOT NULL UNIQUE, status TEXT NOT NULL,
      pass INTEGER NOT NULL, scope_id TEXT NOT NULL, provider_json TEXT NOT NULL,
      target_ids_json TEXT NOT NULL, active_session_id TEXT, active_surface_id TEXT,
      epoch_count INTEGER NOT NULL, last_surface_id TEXT, consecutive_surface_epochs INTEGER NOT NULL,
      last_error TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL
    );
    CREATE TABLE app_server_research_track_surfaces (
      workspace_id TEXT NOT NULL, surface_id TEXT NOT NULL, name TEXT NOT NULL, status TEXT NOT NULL,
      impact TEXT NOT NULL, theory TEXT NOT NULL, next_action TEXT NOT NULL,
      evidence_refs_json TEXT NOT NULL, epochs INTEGER NOT NULL, no_progress_epochs INTEGER NOT NULL,
      revisits INTEGER NOT NULL, revisit_after TEXT, last_worked_at TEXT, updated_at TEXT NOT NULL,
      PRIMARY KEY(workspace_id, surface_id)
    );
    CREATE TABLE app_server_surfaces (id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, parent_id TEXT)`);
    database.exec(`INSERT INTO app_server_surfaces VALUES
      ('resource-example-old', '${workspaceId}', NULL),
      ('surface-example-old', '${workspaceId}', 'resource-example-old'),
      ('surface-example-other', 'workspace-example-other', 'resource-example-other');
    INSERT INTO app_server_research_track_surfaces VALUES
      ('${workspaceId}', 'resource-example-old', 'Example root', 'unexamined', 'unknown', '', '', '[]', 0, 0, 0, NULL, NULL, '2000-01-01T00:00:00.000Z'),
      ('${workspaceId}', 'surface-example-old', 'Example child', 'unexamined', 'unknown', '', '', '[]', 0, 0, 0, NULL, NULL, '2000-01-01T00:00:00.000Z'),
      ('workspace-example-other', 'surface-example-other', 'Other example child', 'unexamined', 'unknown', '', '', '[]', 0, 0, 0, NULL, NULL, '2000-01-01T00:00:00.000Z')`);
    const store = new ResearchTrackStore({ databasePath, workspaceId });
    try {
      const columns = database.prepare('PRAGMA table_info(app_server_research_tracks)').all().map((row) => row.name);
      assert.ok(columns.includes('collaboration_json'));
      assert.ok(columns.includes('guardian_enabled'));
      const surfaceColumns = database.prepare('PRAGMA table_info(app_server_research_track_surfaces)').all().map((row) => row.name);
      assert.ok(surfaceColumns.includes('actionability_score'));
      assert.ok(surfaceColumns.includes('score_updated_at'));
      assert.ok(surfaceColumns.includes('validation_json'));
      assert.ok(surfaceColumns.includes('angles_json'));
      assert.ok(surfaceColumns.includes('ranking_json'));
      const migrated = database.prepare(`SELECT surface_id, actionability_score, score_updated_at
        FROM app_server_research_track_surfaces ORDER BY surface_id`).all();
      assert.deepEqual(migrated.map((row) => [row.surface_id, row.actionability_score]),
        [['resource-example-old', 35], ['surface-example-old', 55], ['surface-example-other', 55]]);
      assert.ok(migrated.every((row) => row.score_updated_at > '2000-01-01T00:00:00.000Z'));
    } finally { store.close(); }
  } finally { database.close(); rmSync(directory, { recursive: true, force: true }); }
});

test('track accepts bounded completion and validates cited canonical evidence', () => {
  const state = fixture();
  try {
    const started = state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex', model: 'example-model' },
      targetIds: ['local'], surfaces });
    assert.equal(started.status, 'running');
    assert.equal(started.pass, 1);
    const first = state.store.reserveNext('session-example-one', surfaces);
    assert.equal(first.surface.id, 'resource-example-a');
    const worker = new ResearchTrackStore({ databasePath: state.databasePath, workspaceId, sessionId: 'session-example-one' });
    try {
      assert.throws(() => worker.checkpoint({ surfaceId: first.surface.id, outcome: 'complete', impact: 'high',
        theory: 'Untrusted input reaches privileged parsing.', nextAction: 'Revisit after source changes.',
        evidenceRefs: ['memory-example-a'] }, { ...first.surface, coverageState: covered }), /Use finding:<claim ID>, memory:<memory ID>/);
      assert.throws(() => worker.checkpoint({ surfaceId: first.surface.id, outcome: 'complete', impact: 'high',
        theory: 'Untrusted input reaches privileged parsing.', nextAction: 'Revisit after source changes.',
        evidenceRefs: ['memory:memory-other-workspace'] }, { ...first.surface, coverageState: covered }), /does not exist in this workspace/);
      worker.checkpoint({ surfaceId: first.surface.id, outcome: 'complete', impact: 'high',
        theory: 'Untrusted input reaches privileged parsing.', nextAction: 'Revisit after source changes.',
        evidenceRefs: ['memory:memory-example-a'], angles: [
          angle('example parser entry', 'ruled_out', ['memory:memory-example-a']),
          angle('example state transition', 'ruled_out', ['memory:memory-example-a'])
        ] }, { ...first.surface, coverageState: covered });
    } finally { worker.close(); }
    state.store.finishEpoch('session-example-one', true);
    assert.equal(state.store.get().surfaces.find((work) => work.surfaceId === first.surface.id).status, 'complete');
    assert.equal(state.store.get().surfaces.find((work) => work.surfaceId === first.surface.id).revisitAfter, null);
    const map = [{ ...surfaces[0], coverageState: covered }, surfaces[1]];
    assert.equal(state.store.reserveNext('session-example-two', map).surface.id, 'resource-example-b');
    state.store.finishEpoch('session-example-two', false, 'Example provider error');
    assert.equal(state.store.get().surfaces.find((work) => work.surfaceId === 'resource-example-b').noProgressEpochs, 1);
    assert.equal(state.store.get().activeSessionId, null);
  } finally { state.close(); }
});

test('terminal persistence keeps an automatic pause reason visible', () => {
  const state = fixture();
  try {
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' },
      targetIds: ['local'], surfaces });
    state.store.reserveNext('session-example-paused', surfaces);
    state.store.pauseWithError('Example authorization changed.');
    const finished = state.store.finishEpoch('session-example-paused', true);
    assert.equal(finished.status, 'paused');
    assert.equal(finished.lastError, 'Example authorization changed.');
    assert.equal(finished.activeSessionId, null);
  } finally { state.close(); }
});

test('finishing a user-paused epoch does not invent a pause reason', () => {
  const state = fixture();
  try {
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' },
      targetIds: ['local'], surfaces });
    state.store.reserveNext('session-example-user-pause', surfaces);
    state.store.control('pause');
    const finished = state.store.finishEpoch('session-example-user-pause', false, 'Worker stopped.');
    assert.equal(finished.status, 'paused');
    assert.equal(finished.lastError, null);
  } finally { state.close(); }
});

test('actionability promotes child proof work, decays without evidence, and returns to breadth', async () => {
  const state = fixture();
  try {
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' },
      targetIds: ['local'], surfaces });
    const root = state.store.reserveNext('session-example-root', surfaces);
    assert.equal(root.surface.id, surfaces[0].id);
    const rootWorker = new ResearchTrackStore({ databasePath: state.databasePath, workspaceId,
      sessionId: 'session-example-root' });
    try { rootWorker.checkpoint({ surfaceId: root.surface.id, outcome: 'defer', impact: 'unknown',
      theory: 'A synthetic entry path remains untested.', nextAction: 'Test the child trust boundary.',
      evidenceRefs: [] }, root.surface); }
    finally { rootWorker.close(); }
    state.store.finishEpoch('session-example-root', true);
    const children = [
      { id: 'surface-example-a-one', parentId: surfaces[0].id, name: 'Example input path',
        coverageState: open, progressBlocker: null },
      { id: 'surface-example-a-two', parentId: surfaces[0].id, name: 'Example privilege path',
        coverageState: open, progressBlocker: null },
    ];
    const map = [...surfaces, ...children];
    const firstChild = state.store.reserveNext('session-example-child-one', map);
    assert.equal(firstChild.surface.id, children[0].id);
    await afterEpochStart();
    recordMemoryEvidence(state);
    state.database.prepare('UPDATE memory_nodes SET updated_at=? WHERE id=?')
      .run(new Date().toISOString(), 'memory-example-a');
    const firstWorker = new ResearchTrackStore({ databasePath: state.databasePath, workspaceId,
      sessionId: 'session-example-child-one' });
    try { firstWorker.checkpoint({ surfaceId: firstChild.surface.id, outcome: 'continue', impact: 'high',
      theory: 'The synthetic input reached a privileged boundary.', nextAction: 'Test its effect.',
      evidenceRefs: ['memory:memory-example-a'] }, firstChild.surface); }
    finally { firstWorker.close(); }
    state.store.finishEpoch('session-example-child-one', true);
    assert.ok(state.store.get().surfaces.find((work) => work.surfaceId === children[0].id).actionabilityScore >= 80);
    state.database.prepare('UPDATE memory_nodes SET updated_at=? WHERE id=?')
      .run('2000-01-01T00:00:00.000Z', 'memory-example-a');

    const secondLook = state.store.reserveNext('session-example-child-two', map);
    assert.equal(secondLook.surface.id, children[0].id);
    const secondWorker = new ResearchTrackStore({ databasePath: state.databasePath, workspaceId,
      sessionId: 'session-example-child-two' });
    try { secondWorker.checkpoint({ surfaceId: secondLook.surface.id, outcome: 'defer', impact: 'high',
      theory: 'The effect still needs a discriminating test.', nextAction: 'Test another route.',
      evidenceRefs: ['memory:memory-example-a'] }, secondLook.surface); }
    finally { secondWorker.close(); }
    state.store.finishEpoch('session-example-child-two', true);
    assert.ok(state.store.get().surfaces.find((work) => work.surfaceId === children[0].id).actionabilityScore < 80);

    const alternateRoot = state.store.reserveNext('session-example-breadth', map);
    assert.equal(alternateRoot.surface.id, surfaces[1].id);
    state.store.finishEpoch('session-example-breadth', true);
    const nextChild = state.store.reserveNext('session-example-child-three', map);
    assert.equal(nextChild.surface.id, children[0].id);
    assert.deepEqual(researchTrackProgress(state.store.get()), { investigated: 3, complete: 0, total: 4 });
    state.store.finishEpoch('session-example-child-three', true);

    state.database.prepare('UPDATE app_server_research_track_surfaces SET score_updated_at=? WHERE workspace_id=? AND surface_id=?')
      .run('2000-01-01T00:00:00.000Z', workspaceId, children[0].id);
    assert.equal(state.store.get().surfaces.find((work) => work.surfaceId === children[0].id).actionabilityScore, 0);
  } finally { state.close(); }
});

test('open claims receive depth epochs while periodic breadth still reaches new resources', () => {
  const state = fixture();
  try {
    const map = [{ ...surfaces[0], coverageState: { ...open,
      openLeadIds: ['claim-example-open'], unblockedFindingIds: ['finding-example-open'] } }, surfaces[1]];
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' },
      targetIds: ['local'], surfaces: map });
    const first = state.store.reserveNext('session-example-claim-depth', map);
    assert.equal(first.surface.id, map[0].id);
    assert.equal(first.focus, 'depth');
    state.store.finishEpoch('session-example-claim-depth', true);
    state.database.prepare(`UPDATE app_server_research_tracks SET epoch_count=3 WHERE workspace_id=?`)
      .run(workspaceId);
    state.database.prepare(`UPDATE app_server_research_track_surfaces
      SET actionability_score=90, impact='high', no_progress_epochs=0
      WHERE workspace_id=? AND surface_id=?`).run(workspaceId, map[0].id);
    const breadth = state.store.reserveNext('session-example-claim-breadth', map);
    assert.equal(breadth.surface.id, map[1].id);
    assert.equal(breadth.focus, 'breadth');
  } finally { state.close(); }
});

test('finding resolution outranks an equally actionable unmapped child', () => {
  const state = fixture();
  try {
    const map = [surfaces[0],
      { id: 'surface-example-a', parentId: surfaces[0].id, name: 'Example decoder',
        coverageState: open, progressBlocker: null },
      { id: 'surface-example-b', parentId: surfaces[0].id, name: 'Example broker',
        coverageState: { ...open, unblockedFindingIds: ['finding-example-broker'] }, progressBlocker: null }];
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' },
      targetIds: ['local'], surfaces: map });
    const assigned = state.store.reserveNext('session-example-finding-depth', map);
    assert.equal(assigned.surface.id, map[2].id);
    assert.equal(assigned.focus, 'depth');
  } finally { state.close(); }
});

test('unavailable target validation keeps an open claim from monopolizing depth work', () => {
  const state = fixture();
  try {
    const map = [{ ...surfaces[0], coverageState: { ...open,
      openLeadIds: [], unblockedFindingIds: ['finding-example-device'] } }, surfaces[1]];
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' },
      targetIds: ['local'], surfaces: map });
    state.database.prepare(`UPDATE app_server_research_track_surfaces
      SET validation_json=? WHERE workspace_id=? AND surface_id=?`)
      .run(JSON.stringify({ targetDependent: true, environment: 'none',
        reason: 'The example device needed for the next proof is unavailable.' }), workspaceId, map[0].id);
    const assigned = state.store.reserveNext('session-example-device-breadth', map);
    assert.equal(assigned.surface.id, map[1].id);
    assert.equal(assigned.focus, 'breadth');
  } finally { state.close(); }
});

test('metadata edits and human notes do not raise priority; new source evidence does', async () => {
  const state = fixture();
  try {
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' },
      targetIds: ['local'], surfaces: [surfaces[0]] });
    for (const [sessionId, kind] of [['session-example-edit', null], ['session-example-note', 'human_note'],
      ['session-example-source', 'code']]) {
      state.store.reserveNext(sessionId, [surfaces[0]]);
      await afterEpochStart();
      if (kind) recordMemoryEvidence(state, kind);
      state.database.prepare('UPDATE memory_nodes SET updated_at=? WHERE id=?')
        .run(new Date().toISOString(), 'memory-example-a');
      const worker = new ResearchTrackStore({ databasePath: state.databasePath, workspaceId, sessionId });
      try { worker.checkpoint({ surfaceId: surfaces[0].id, outcome: 'defer', impact: 'high',
        theory: 'An example path needs direct validation.', nextAction: 'Test the example input.',
        evidenceRefs: ['memory:memory-example-a'], angles: [angle('example source path')] }, surfaces[0]); }
      finally { worker.close(); }
      const work = state.store.get().surfaces[0];
      if (kind === 'code') {
        assert.equal(work.actionabilityScore, 33);
        assert.equal(work.noProgressEpochs, 0);
        assert.deepEqual(work.priority, { baseScore: 78, actionability: 33, impactAdjustment: 30,
          surfaceRankingAdjustment: 0,
          testableAngleBonus: 15, targetValidationPenalty: 0, noProgressPenalty: 0 });
      } else {
        assert.equal(work.noProgressEpochs, kind === null ? 1 : 2);
        assert.equal(work.actionabilityScore, kind === null ? 17 : 0);
      }
      state.store.finishEpoch(sessionId, true);
    }
  } finally { state.close(); }
});

test('five scored surface factors affect selection and replace qualitative impact adjustment', () => {
  const state = fixture();
  const ranking = (score) => Object.fromEntries(['impact', 'reachability', 'relevance', 'exposure', 'precedence']
    .map((factor) => [factor, { score, basis: `Synthetic ${factor} assessment.` }]));
  const ranked = [
    { ...surfaces[0], foundation: { ranking: ranking(1) } },
    { ...surfaces[1], foundation: { ranking: ranking(10) } },
  ];
  try {
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' },
      targetIds: ['local'], surfaces: ranked });
    state.database.prepare(`UPDATE app_server_research_track_surfaces SET impact='critical'
      WHERE workspace_id=? AND surface_id=?`).run(workspaceId, ranked[1].id);
    const work = state.store.get().surfaces;
    assert.equal(work[0].priority.surfaceRankingAdjustment, -40);
    assert.equal(work[1].priority.surfaceRankingAdjustment, 50);
    assert.equal(work[1].priority.impactAdjustment, 0);
    assert.equal(work[1].priority.baseScore, 85);
    assert.equal(state.store.reserveNext('session-example-ranked', ranked).surface.id, ranked[1].id);
    const reversed = [
      { ...ranked[0], foundation: { ranking: ranking(10) } },
      { ...ranked[1], foundation: { ranking: ranking(1) } },
    ];
    const refreshed = state.store.syncSurfaces(reversed).surfaces;
    assert.equal(refreshed.find((surface) => surface.surfaceId === ranked[0].id).priority.surfaceRankingAdjustment, 50);
    assert.equal(refreshed.find((surface) => surface.surfaceId === ranked[1].id).priority.surfaceRankingAdjustment, -40);
  } finally { state.close(); }
});

test('new finding evidence and completed runbook cells qualify as track progress', async () => {
  const state = fixture();
  try {
    state.database.exec(`CREATE TABLE app_server_research_claims (id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL,
      updated_at TEXT NOT NULL, status TEXT NOT NULL, duplicate_of_claim_id TEXT);
      CREATE TABLE app_server_claim_evidence (id TEXT PRIMARY KEY, claim_id TEXT NOT NULL, kind TEXT NOT NULL,
        reference_id TEXT, content_hash TEXT, created_at TEXT NOT NULL);
      CREATE TABLE app_server_runbooks (id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, updated_at TEXT NOT NULL);
      CREATE TABLE app_server_runbook_executions (run_id TEXT PRIMARY KEY, runbook_id TEXT NOT NULL,
        workspace_id TEXT NOT NULL, completed_at TEXT, status TEXT NOT NULL);
      CREATE TABLE app_server_runbook_cell_executions (run_id TEXT NOT NULL, status TEXT NOT NULL)`);
    state.database.prepare(`INSERT INTO app_server_research_claims VALUES (?, ?, ?, 'observed', NULL)`)
      .run('claim-example', workspaceId, '2000-01-01T00:00:00.000Z');
    state.database.prepare('INSERT INTO app_server_runbooks VALUES (?, ?, ?)')
      .run('runbook-example', workspaceId, '2000-01-01T00:00:00.000Z');
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' },
      targetIds: ['local'], surfaces: [surfaces[0]] });
    for (const [sessionId, ref] of [['session-example-finding', 'finding:claim-example'],
      ['session-example-runbook', 'runbook:runbook-example']]) {
      state.store.reserveNext(sessionId, [surfaces[0]]);
      await afterEpochStart();
      const now = new Date().toISOString();
      if (ref.startsWith('finding:')) {
        state.database.prepare(`INSERT INTO app_server_claim_evidence VALUES
          ('claim-evidence-example', 'claim-example', 'artifact', 'artifact-example', NULL, ?)`).run(now);
      } else {
        state.database.prepare(`INSERT INTO app_server_runbook_executions VALUES
          ('run-example', 'runbook-example', ?, ?, 'succeeded')`).run(workspaceId, now);
        state.database.prepare(`INSERT INTO app_server_runbook_cell_executions VALUES
          ('run-example', 'succeeded')`).run();
      }
      const worker = new ResearchTrackStore({ databasePath: state.databasePath, workspaceId, sessionId });
      try { worker.checkpoint({ surfaceId: surfaces[0].id, outcome: 'defer', impact: 'medium',
        theory: 'A synthetic path has new test evidence.', nextAction: 'Inspect the next example path.',
        evidenceRefs: [ref] }, surfaces[0]); }
      finally { worker.close(); }
      assert.equal(state.store.get().surfaces[0].actionabilityScore, ref.startsWith('finding:') ? 60 : 85);
      state.store.finishEpoch(sessionId, true);
    }
  } finally { state.close(); }
});

test('a testable high-impact path keeps depth priority and retains distinct attempts across epochs', () => {
  const state = fixture();
  try {
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' },
      targetIds: ['local'], surfaces });
    const first = state.store.reserveNext('session-example-first-angle', surfaces);
    const firstWorker = new ResearchTrackStore({ databasePath: state.databasePath, workspaceId,
      sessionId: 'session-example-first-angle' });
    try {
      firstWorker.checkpoint({ surfaceId: first.surface.id, outcome: 'defer', impact: 'high',
        theory: 'An example authority boundary may be crossed.', nextAction: 'Test the authority path.',
        evidenceRefs: [], angles: [angle('authority path'), angle('lifecycle path')],
        validation: { targetDependent: false, environment: 'none', reason: 'Source-level proof is available.' } }, first.surface);
    } finally { firstWorker.close(); }
    state.store.finishEpoch('session-example-first-angle', true);
    const legacyAngles = state.store.get().surfaces.find((work) => work.surfaceId === first.surface.id)
      .angles.map(({ vector: _vector, ...entry }) => entry);
    state.database.prepare(`UPDATE app_server_research_track_surfaces SET angles_json=?
      WHERE workspace_id=? AND surface_id=?`).run(JSON.stringify(legacyAngles), workspaceId, first.surface.id);
    assert.equal(state.store.reserveNext('session-example-second-angle', surfaces).surface.id, first.surface.id);
    state.database.prepare('UPDATE memory_nodes SET updated_at=? WHERE id=?')
      .run(new Date(Date.now() + 1_000).toISOString(), 'memory-example-a');
    const secondWorker = new ResearchTrackStore({ databasePath: state.databasePath, workspaceId,
      sessionId: 'session-example-second-angle' });
    try {
      secondWorker.checkpoint({ surfaceId: first.surface.id, outcome: 'defer', impact: 'high',
        theory: 'The authority path was ruled out; the lifecycle path remains open.',
        nextAction: 'Test the lifecycle path.', evidenceRefs: [],
        angles: [angle('authority path', 'ruled_out', ['memory:memory-example-a'])] }, first.surface);
    } finally { secondWorker.close(); }
    const recorded = state.store.get().surfaces.find((work) => work.surfaceId === first.surface.id);
    assert.deepEqual(recorded.angles.map(({ name, status }) => [name, status]),
      [['authority path', 'ruled_out'], ['lifecycle path', 'open']]);
    assert.deepEqual(recorded.evidenceRefs, ['memory:memory-example-a']);
    assert.equal(recorded.angles[0].vector.failureMode, 'Example authority path policy mismatch');
    assert.equal(recorded.angles[1].vector, undefined);
  } finally { state.close(); }
});

test('high-impact completion accepts a bounded review while structured outcomes require evidence', () => {
  const state = fixture();
  try {
    const map = [{ ...surfaces[0], coverageState: covered }];
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' },
      targetIds: ['local'], surfaces: map });
    state.store.reserveNext('session-example-angles', map);
    state.database.prepare('UPDATE memory_nodes SET updated_at=? WHERE id=?')
      .run(new Date(Date.now() + 1_000).toISOString(), 'memory-example-a');
    const worker = new ResearchTrackStore({ databasePath: state.databasePath, workspaceId,
      sessionId: 'session-example-angles' });
    const base = { surfaceId: map[0].id, outcome: 'complete', impact: 'high',
      theory: 'Two separate example paths need evidence.', nextAction: 'Revisit after changes.',
      evidenceRefs: ['memory:memory-example-a'] };
    try {
      assert.throws(() => worker.checkpoint({ ...base,
        angles: [angle('authority path', 'ruled_out')] }, map[0]), /needs canonical evidence/);
      assert.throws(() => worker.checkpoint({ ...base, angles: [
        angle('authority path', 'ruled_out', ['memory:memory-example-a']),
        { ...angle('renamed authority path', 'ruled_out', ['memory:memory-example-a']),
          vector: angle('authority path').vector }
      ] }, map[0]), /distinct causal vectors/);
      assert.throws(() => worker.checkpoint({ ...base, angles: [
        { ...angle('authority path', 'ruled_out', ['memory:memory-example-a']), vector: undefined }
      ] }, map[0]), /needs an attacker-control-to-impact vector/);
      const completed = worker.checkpoint({ ...base, angles: [
        angle('authority path', 'ruled_out', ['memory:memory-example-a']),
        angle('lifecycle path', 'ruled_out', ['memory:memory-example-a'])
      ] }, map[0]);
      assert.equal(completed.status, 'complete');
      assert.equal(completed.angles.length, 2);
    } finally { worker.close(); }
  } finally { state.close(); }
});

test('track scheduling favors a testable higher-impact surface over a stalled low-impact path', () => {
  const state = fixture();
  try {
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' },
      targetIds: ['local'], surfaces });
    const update = state.database.prepare(`UPDATE app_server_research_track_surfaces
      SET actionability_score=?, impact=?, validation_json=?, no_progress_epochs=?
      WHERE workspace_id=? AND surface_id=?`);
    update.run(82, 'low', JSON.stringify({ targetDependent: true, environment: 'none',
      reason: 'The example hardware is unavailable for the required test.' }), 1,
    workspaceId, surfaces[0].id);
    update.run(55, 'high', JSON.stringify({ targetDependent: false, environment: 'localhost',
      reason: 'The example parser can be checked from source.' }), 0,
    workspaceId, surfaces[1].id);
    assert.equal(state.store.reserveNext('session-example-prioritized', surfaces).surface.id, surfaces[1].id);
  } finally { state.close(); }
});

test('track scheduling rotates away from hardware-dependent work without target validation', () => {
  const state = fixture();
  try {
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' },
      targetIds: ['local'], surfaces });
    state.database.prepare(`UPDATE app_server_research_track_surfaces
      SET actionability_score=70, impact='high', validation_json=?
      WHERE workspace_id=? AND surface_id=?`).run(JSON.stringify({ targetDependent: true,
      environment: 'none', reason: 'The example device is unavailable for target validation.' }),
    workspaceId, surfaces[0].id);
    state.database.prepare(`UPDATE app_server_research_track_surfaces SET actionability_score=55
      WHERE workspace_id=? AND surface_id=?`).run(workspaceId, surfaces[1].id);
    assert.equal(state.store.reserveNext('session-example-hardware-rotation', surfaces).surface.id, surfaces[1].id);
  } finally { state.close(); }
});

test('track scheduling does not favor low-impact work solely because it yields easy evidence', () => {
  const state = fixture();
  try {
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' },
      targetIds: ['local'], surfaces });
    state.database.prepare(`UPDATE app_server_research_track_surfaces
      SET actionability_score=80, impact='low' WHERE workspace_id=? AND surface_id=?`)
      .run(workspaceId, surfaces[0].id);
    state.database.prepare(`UPDATE app_server_research_track_surfaces
      SET actionability_score=65, impact='medium' WHERE workspace_id=? AND surface_id=?`)
      .run(workspaceId, surfaces[1].id);
    assert.equal(state.store.reserveNext('session-example-impact-rotation', surfaces).surface.id, surfaces[1].id);
  } finally { state.close(); }
});

test('track breadth escapes a high-scoring repository family for another resource kind', () => {
  const state = fixture();
  try {
    const map = [
      { id: 'resource-example-repository', resourceId: 'resource-example-repository', resourceKind: 'repository',
        resourceSource: 'explicit_scope', parentId: null, name: 'Example source', coverageState: open, progressBlocker: null },
      { id: 'surface-example-repository-child', resourceId: 'resource-example-repository', resourceKind: 'repository',
        resourceSource: 'explicit_scope', parentId: 'resource-example-repository', name: 'Example parser path', coverageState: open, progressBlocker: null },
      { id: 'resource-example-other', resourceId: 'resource-example-other', resourceKind: 'other',
        resourceSource: 'explicit_scope', parentId: null, name: 'Example platform resource', coverageState: open, progressBlocker: null },
      { id: 'resource-example-binary', resourceId: 'resource-example-binary', resourceKind: 'binary',
        resourceSource: 'runtime_discovery', parentId: null, name: 'Example service executable', coverageState: open, progressBlocker: null },
    ];
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' }, targetIds: ['local'], surfaces: map });
    state.database.prepare(`UPDATE app_server_research_tracks SET epoch_count=8, last_surface_id=? WHERE workspace_id=?`)
      .run(map[1].id, workspaceId);
    state.database.prepare(`UPDATE app_server_research_track_surfaces SET epochs=7,
      actionability_score=95, impact='high' WHERE workspace_id=? AND surface_id=?`)
      .run(workspaceId, map[1].id);
    state.database.prepare(`UPDATE app_server_research_track_surfaces SET epochs=1
      WHERE workspace_id=? AND surface_id=?`).run(workspaceId, map[0].id);
    assert.equal(state.store.reserveNext('session-example-cross-resource', map).surface.id, map[2].id);
  } finally { state.close(); }
});

test('a newly approved service becomes a selectable track root after repository-only epochs', () => {
  const state = fixture();
  try {
    const repository = { id: 'resource-example-repository', resourceId: 'resource-example-repository',
      resourceKind: 'repository', resourceSource: 'explicit_scope', parentId: null,
      name: 'Example source', coverageState: open, progressBlocker: null };
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' },
      targetIds: ['local'], surfaces: [repository] });
    state.database.prepare(`UPDATE app_server_research_tracks SET epoch_count=4, last_surface_id=? WHERE workspace_id=?`)
      .run(repository.id, workspaceId);
    state.database.prepare(`UPDATE app_server_research_track_surfaces SET epochs=4,
      actionability_score=90, impact='high' WHERE workspace_id=? AND surface_id=?`)
      .run(workspaceId, repository.id);
    const service = { id: 'resource-example-service', resourceId: 'resource-example-service',
      resourceKind: 'service', resourceSource: 'runtime_discovery', parentId: null,
      name: 'Example daemon', coverageState: open, progressBlocker: null };
    assert.equal(state.store.reserveNext('session-example-new-service', [repository, service]).surface.id, service.id);
  } finally { state.close(); }
});

test('recurring breadth selects a different resource after every root has been investigated', () => {
  const state = fixture();
  try {
    const map = [
      { id: 'resource-example-repository', resourceId: 'resource-example-repository', resourceKind: 'repository',
        parentId: null, name: 'Example source', coverageState: open, progressBlocker: null },
      { id: 'surface-example-repository-child', resourceId: 'resource-example-repository', resourceKind: 'repository',
        parentId: 'resource-example-repository', name: 'Example parser path', coverageState: open, progressBlocker: null },
      { id: 'resource-example-service', resourceId: 'resource-example-service', resourceKind: 'service',
        parentId: null, name: 'Example daemon', coverageState: open, progressBlocker: null },
    ];
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' }, targetIds: ['local'], surfaces: map });
    state.database.prepare(`UPDATE app_server_research_tracks SET epoch_count=3, last_surface_id=? WHERE workspace_id=?`)
      .run(map[1].id, workspaceId);
    const update = state.database.prepare(`UPDATE app_server_research_track_surfaces
      SET epochs=?, actionability_score=? WHERE workspace_id=? AND surface_id=?`);
    update.run(1, 35, workspaceId, map[0].id);
    update.run(2, 95, workspaceId, map[1].id);
    update.run(1, 35, workspaceId, map[2].id);
    assert.equal(state.store.reserveNext('session-example-recurring-breadth', map).surface.id, map[2].id);
  } finally { state.close(); }
});

test('target-dependent localhost evidence records a gap and raises actionability less than device evidence', async () => {
  const state = fixture();
  try {
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' },
      targetIds: ['local'], surfaces: [surfaces[0]] });
    state.store.reserveNext('session-example-host-proof', [surfaces[0]]);
    await afterEpochStart();
    recordMemoryEvidence(state);
    state.database.prepare('UPDATE memory_nodes SET updated_at=? WHERE id=?')
      .run(new Date(Date.now() + 1000).toISOString(), 'memory-example-a');
    const hostWorker = new ResearchTrackStore({ databasePath: state.databasePath, workspaceId,
      sessionId: 'session-example-host-proof' });
    try {
      const tool = createResearchTrackTool(hostWorker, () => surfaces[0]);
      assert.ok(tool.parameters.properties.angles.items.required.includes('vector'));
      assert.match(tool.parameters.properties.evidenceRefs.description, /finding:<claim ID>, memory:<memory ID>/);
      assert.match(tool.parameters.properties.evidenceRefs.description, /only new source evidence/);
      assert.match(tool.parameters.properties.angles.items.properties.evidenceRefs.description, /added to the checkpoint automatically/);
      const base = { surfaceId: surfaces[0].id, outcome: 'defer', impact: 'medium',
        theory: 'Synthetic input may cross the example trust boundary.', nextAction: 'Test on an example device.',
        evidenceRefs: ['memory:memory-example-a'] };
      const childCheckpoint = await tool.execute({ input: base }, { agentId: 'example-child' });
      assert.equal(childCheckpoint.status, 'error');
      assert.match(childCheckpoint.error.message, /Only the lead agent can checkpoint/);
      assert.equal(state.store.get().activeSurfaceId, surfaces[0].id);
      assert.equal((await tool.execute({ input: base }, { agentId: 'root' })).status, 'complete');
      const invalidEvidence = await tool.execute({ input: { ...base, evidenceRefs: ['evidence-example-a'] } });
      assert.equal(invalidEvidence.status, 'error');
      assert.match(invalidEvidence.error.message, /parent record, not an evidence entry ID/);
      assert.equal((await tool.execute({ input: { ...base,
        validation: { targetDependent: true, environment: 'localhost', reason: 'Unavailable.' } } })).status, 'error');
      assert.equal((await tool.execute({ input: { ...base, angles: [
        { ...angle('example target path'), vector: undefined }
      ], validation: { targetDependent: true, environment: 'localhost', reason: 'Unavailable.' } } })).status, 'error');
      assert.throws(() => hostWorker.checkpoint({ ...base, outcome: 'complete',
        validation: { targetDependent: true, environment: 'localhost',
          reason: 'The synthetic device was not connected during this epoch.' } },
      { ...surfaces[0], coverageState: covered }), /Target-dependent work cannot complete/);
      assert.equal((await tool.execute({ input: { ...base,
        angles: [angle('example target path')],
        validation: { targetDependent: true, environment: 'localhost',
          reason: 'The synthetic device was not connected during this epoch.' } } })).status, 'complete');
    } finally { hostWorker.close(); }
    const afterHost = state.store.get().surfaces[0];
    assert.ok(afterHost.actionabilityScore > 0);
    assert.equal(afterHost.validation.environment, 'localhost');
    state.store.finishEpoch('session-example-host-proof', true);
    state.store.reserveNext('session-example-device-proof', [surfaces[0]]);
    await afterEpochStart();
    recordMemoryEvidence(state);
    const deviceWorker = new ResearchTrackStore({ databasePath: state.databasePath, workspaceId,
      sessionId: 'session-example-device-proof' });
    try {
      deviceWorker.checkpoint({ surfaceId: surfaces[0].id, outcome: 'defer', impact: 'medium',
        theory: 'Synthetic device behavior was observed.', nextAction: 'Check another example path.',
        evidenceRefs: ['memory:memory-example-a'], validation: { targetDependent: true,
          environment: 'device', reason: 'Observed the behavior on the example device.' } }, surfaces[0]);
    } finally { deviceWorker.close(); }
    assert.ok(state.store.get().surfaces[0].actionabilityScore > afterHost.actionabilityScore);
    assert.equal(state.store.get().surfaces[0].validation.environment, 'device');
  } finally { state.close(); }
});

test('worker and launch failures pause a track with an actionable diagnostic', async () => {
  const state = fixture();
  const service = new AppServerHostService({ registry: {
    registryDirectory: join(dirname(state.databasePath), 'registry'),
    resolveWorkspace: (id) => id === workspaceId ? {
      workspaceId, researchProfileId: 'security-research', memoryBackend: 'app-server'
    } : null,
    storageForProfile: () => ({ databasePath: state.databasePath, artifactDirectoryPath: join(dirname(state.databasePath), 'artifacts') })
  }, invokeProtocol: async (operation) => operation === 'session.get' ? { status: 'failed' } : {} });
  try {
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' }, targetIds: ['local'], surfaces });
    state.store.reserveNext('session-example-failed-worker', surfaces);
    await service.recordSessionTerminalState({ request: { launch: { workspaceId, mode: 'research-track' } },
      sessionId: 'session-example-failed-worker', attemptId: 'attempt-example', state: 'failed',
      diagnostic: 'Synthetic worker tool registration failed.' });
    assert.equal(state.store.get().status, 'paused');
    assert.equal(state.store.get().lastError, 'Synthetic worker tool registration failed.');
    assert.equal(state.store.reserveNext('session-example-unwanted-retry', surfaces), null);

    state.store.control('resume');
    assert.equal(state.store.get().lastError, null);
    state.store.reserveNext('session-example-launch-failure', surfaces);
    service.recordResearchTrackEpochLaunchFailure(workspaceId, 'session-example-launch-failure',
      'Synthetic launch preparation failed.');
    assert.equal(state.store.get().status, 'paused');
    assert.equal(state.store.get().lastError, 'Synthetic launch preparation failed.');
    assert.equal(state.store.reserveNext('session-example-second-unwanted-retry', surfaces), null);
    assert.equal(state.store.control('stop').lastError, null);
    assert.equal(state.store.control('stop').status, 'stopped');
  } finally { state.close(); }
});

test('expected epoch rotation preserves checkpoints and identifies missing checkpoints', async () => {
  const state = fixture();
  const service = new AppServerHostService({ registry: {
    registryDirectory: join(dirname(state.databasePath), 'registry'),
    resolveWorkspace: (id) => id === workspaceId ? {
      workspaceId, researchProfileId: 'security-research', memoryBackend: 'app-server'
    } : null,
    storageForProfile: () => ({ databasePath: state.databasePath, artifactDirectoryPath: join(dirname(state.databasePath), 'artifacts') })
  }, invokeProtocol: async (operation) => operation === 'session.get' ? { status: 'stopped' } : {} });
  const request = { launch: { workspaceId, mode: 'research-track' } };
  try {
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' },
      targetIds: ['local'], surfaces: [surfaces[0]] });
    const first = state.store.reserveNext('session-example-rotation-one', [surfaces[0]]);
    const worker = new ResearchTrackStore({ databasePath: state.databasePath, workspaceId,
      sessionId: 'session-example-rotation-one' });
    try {
      worker.checkpoint({ surfaceId: first.surface.id, outcome: 'defer', impact: 'medium',
        theory: 'A synthetic trust boundary requires another proof.', nextAction: 'Check another entry path.',
        evidenceRefs: ['memory:memory-example-a'] }, first.surface);
    } finally { worker.close(); }
    await service.recordSessionTerminalState({ request, sessionId: 'session-example-rotation-one',
      attemptId: 'attempt-example-rotation-one', state: 'stopped', trackRotation: true });
    assert.equal(state.store.get().status, 'running');
    assert.equal(state.store.get().lastError, null);
    assert.equal(state.store.get().surfaces[0].noProgressEpochs, 1);

    state.store.reserveNext('session-example-rotation-two', [surfaces[0]]);
    await service.recordSessionTerminalState({ request, sessionId: 'session-example-rotation-two',
      attemptId: 'attempt-example-rotation-two', state: 'stopped', trackRotation: true });
    assert.equal(state.store.get().status, 'running');
    assert.match(state.store.get().lastError, /time limit without a track checkpoint/);
    assert.equal(state.store.get().surfaces[0].noProgressEpochs, 2);

    state.store.reserveNext('session-example-manual-stop', [surfaces[0]]);
    await service.recordSessionTerminalState({ request, sessionId: 'session-example-manual-stop',
      attemptId: 'attempt-example-manual-stop', state: 'stopped' });
    assert.equal(state.store.get().status, 'paused');
    assert.match(state.store.get().lastError, /no durable progress/);
  } finally { state.close(); }
});

test('checkpoint storage errors pause a track instead of consuming more epochs', async () => {
  const state = fixture();
  try {
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' },
      targetIds: ['local'], surfaces: [surfaces[0]] });
    state.store.reserveNext('session-example-storage-error', [surfaces[0]]);
    state.database.exec(`CREATE TABLE app_server_research_claims (
      id TEXT PRIMARY KEY, updated_at TEXT NOT NULL)`);
    state.database.prepare('INSERT INTO app_server_research_claims VALUES (?, ?)')
      .run('claim-example', new Date().toISOString());
    const worker = new ResearchTrackStore({ databasePath: state.databasePath, workspaceId,
      sessionId: 'session-example-storage-error' });
    try {
      const tool = createResearchTrackTool(worker, () => surfaces[0]);
      const result = await tool.execute({ input: { surfaceId: surfaces[0].id,
        outcome: 'defer', impact: 'medium', theory: 'A synthetic trust boundary needs another proof.',
        nextAction: 'Check another path.', evidenceRefs: ['finding:claim-example'],
        angles: [angle('example claim path', 'supported', ['finding:claim-example'])],
        validation: { targetDependent: true, environment: 'localhost', reason: 'The example target was unavailable during this epoch.' } } });
      assert.equal(result.status, 'error');
      assert.match(result.error.message, /no such column: workspace_id/);
      assert.equal(state.store.get().status, 'paused');
      assert.match(state.store.get().lastError, /checkpoint storage failed: no such column: workspace_id/i);
    } finally { worker.close(); }
  } finally { state.close(); }
});

test('track start stores the selected model, targets, and collaboration on the owning app-server', async () => {
  const state = fixture();
  const service = new AppServerHostService({ registry: {
    registryDirectory: join(dirname(state.databasePath), 'registry'),
    resolveWorkspace: (id) => id === workspaceId ? {
      workspaceId, researchProfileId: 'security-research', memoryBackend: 'app-server'
    } : null,
    storageForProfile: () => ({ databasePath: state.databasePath, artifactDirectoryPath: join(dirname(state.databasePath), 'artifacts') }),
    providerSettings: () => ({ defaultProviderId: 'openai-codex', modelDefaults: {} })
  }, invokeProtocol: async () => ({}) });
  service.workspaceResearchTrackContext = async () => ({ scope: { id: 'scope-example' },
    targets: [{ id: 'local', ready: true }, { id: 'browser:example', ready: true }],
    surfaceMap: { surfaces } });
  try {
    const collaboration = { mode: 'adaptive', subagentMode: 'advanced', providers: [] };
    const track = await service.workspaceResearchTrack(workspaceId, 'start', {
      provider: { id: 'openai-codex', model: 'example-model', reasoningEffort: 'high' },
      targetIds: ['browser:example'], collaboration
    });
    assert.deepEqual(track.provider, { id: 'openai-codex', model: 'example-model', reasoningEffort: 'high' });
    assert.deepEqual(track.targetIds, ['local', 'browser:example']);
    assert.deepEqual(track.collaboration, collaboration);
    assert.equal(track.guardianEnabled, true);
  } finally { state.close(); }
});

test('track epochs share a session, wait for worker exit, and ignore an obsolete stop', async () => {
  const state = fixture();
  let stopTransition = null;
  const epochMarkers = [];
  let currentSurfaces = surfaces;
  const service = new AppServerHostService({ registry: {
    registryDirectory: join(dirname(state.databasePath), 'registry'),
    listWorkspaces: () => [{ workspaceId }],
    resolveWorkspace: (id) => id === workspaceId ? {
      workspaceId, researchProfileId: 'security-research', researchKitId: 'apple-security-bounty',
      memoryBackend: 'app-server'
    } : null,
    storageForProfile: () => ({ databasePath: state.databasePath, artifactDirectoryPath: join(dirname(state.databasePath), 'artifacts') })
  }, invokeProtocol: async (operation, options) => {
    if (operation === 'session.transition') stopTransition = options.input;
    if (operation === 'session.append_event') epochMarkers.push(options.input);
    return operation === 'session.get' ? { status: 'completed' } : {};
  } });
  service.workspaceResearchTrackContext = async () => ({ scope: { id: 'scope-example' },
    targets: [{ id: 'local', ready: true }], surfaceMap: { surfaces: currentSurfaces } });
  try {
    const collaboration = { mode: 'adaptive', subagentMode: 'advanced', providers: [] };
    const started = state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex', model: 'example-model' },
      targetIds: ['local'], collaboration, surfaces });
    assert.deepEqual(state.store.get().collaboration, collaboration);
    const [first] = await service.dueResearchTracks();
    assert.equal(first.request.sessionId, started.id);
    assert.deepEqual(first.request.launch.collaboration, collaboration);
    assert.match(first.request.launch.promptMarkdown, /Selected execution targets: local/);
    assert.match(first.request.launch.promptMarkdown, /^# Continuous research track epoch 1/m);
    assert.match(first.request.launch.promptMarkdown, /Epoch focus: breadth/);
    assert.match(first.request.launch.promptMarkdown, /complementary depth subagent/);
    assert.match(first.request.launch.promptMarkdown, /lead alone records the track checkpoint/);
    assert.match(first.request.launch.promptMarkdown, /pursue the most promising authorized question/);
    assert.match(first.request.launch.promptMarkdown, /listed resources are an inventory/);
    assert.match(first.request.launch.promptMarkdown, /authorized physical iPhone/);
    assert.match(first.request.launch.promptMarkdown, /localhost run alone does not validate target behavior/);
    assert.match(first.request.launch.promptMarkdown, /Official vendor APIs and SDKs may be covered/);
    assert.match(first.request.launch.promptMarkdown, /Third-party applications, customers, and accounts are not authorized/);
    assert.match(first.request.launch.promptMarkdown, /Map concrete subcomponents with stable noun names/);
    assert.match(first.request.launch.promptMarkdown, /Checkpoint at a useful stopping point/);
    assert.match(first.request.launch.promptMarkdown, /Epoch checkpoint recorded\./);
    assert.doesNotMatch(first.request.launch.promptMarkdown, /final response so the host can select/);
    assert.equal(first.request.launch.generateTitle, true);
    assert.equal(first.request.launch.continuation, undefined);
    assert.deepEqual(await service.dueResearchTracks(new Date(), new Set([started.id])), []);
    await service.recordSessionTerminalState({ request: first.request, sessionId: started.id,
      attemptId: 'attempt-example-first', state: 'completed' });
    assert.deepEqual(epochMarkers.map((event) => ({ id: event.id, kind: event.kind, payload: event.payload })), [{
      id: 'track_epoch_resolving_attempt-example-first', kind: 'agent.event',
      payload: { type: 'track_epoch_resolving', attemptId: 'attempt-example-first', agentPath: '/root' }
    }]);
    currentSurfaces = [{ ...surfaces[0], coverageState: { ...open,
      openLeadIds: ['claim-example-open'], unblockedFindingIds: ['finding-example-open'] } }, surfaces[1]];
    const [second] = await service.dueResearchTracks();
    assert.match(second.request.launch.promptMarkdown, /^# Continuous research track epoch 2/m);
    assert.match(second.request.launch.promptMarkdown, /Epoch focus: depth/);
    assert.match(second.request.launch.promptMarkdown, /Linked open leads: claim-example-open/);
    assert.match(second.request.launch.promptMarkdown, /Findings needing resolution: finding-example-open/);
    assert.match(second.request.launch.promptMarkdown, /complementary breadth subagent/);
    assert.equal(second.request.sessionId, started.id);
    assert.equal(second.request.launch.continuation.fallbackPrompt, second.request.launch.promptMarkdown);
    assert.equal(second.request.launch.continuation.resumeAttemptId, undefined);
    assert.equal(second.request.launch.generateTitle, true);
    await service.recordSessionTerminalState({ request: first.request, sessionId: 'session-example-obsolete',
      attemptId: 'attempt-example-obsolete', state: 'stopped' });
    assert.equal(epochMarkers.length, 1);
    assert.equal(state.store.get().status, 'running');
    assert.equal(state.store.get().activeSessionId, started.id);
    await service.recordSessionControlState({ request: first.request, sessionId: started.id,
      attemptId: 'attempt-example-first', state: 'stopped', stopCause: 'track_state' });
    assert.equal(stopTransition.metadata.appServerStopCause, 'track_state');
    assert.equal(stopTransition.metadata.manualControlState, undefined);
    state.store.finishEpoch(started.id, true);
    await service.recordSessionTerminalState({ request: second.request, sessionId: started.id,
      attemptId: 'attempt-example-second', state: 'completed' });
    assert.deepEqual(epochMarkers.map((event) => event.id), [
      'track_epoch_resolving_attempt-example-first', 'track_epoch_resolving_attempt-example-second'
    ]);
  } finally { state.close(); }
});

test('track epochs retain model history until harness context review calls for a fresh attempt', async () => {
  const state = fixture();
  const directory = dirname(state.databasePath);
  let attempts = [];
  let events = [];
  const service = new AppServerHostService({ registry: {
    registryDirectory: join(directory, 'registry'),
    listWorkspaces: () => [{ workspaceId }],
    resolveWorkspace: (id) => id === workspaceId ? {
      workspaceId, workspacePath: directory, researchProfileId: 'security-research',
      memoryBackend: 'app-server'
    } : null,
    storageForProfile: () => ({ databasePath: state.databasePath,
      artifactDirectoryPath: join(directory, 'artifacts') })
  }, invokeProtocol: async (operation) => operation === 'session.get'
    ? { workspaceId, status: 'completed', attempts }
    : operation === 'session.events' ? { events } : {} });
  service.workspaceResearchTrackContext = async () => ({ scope: { id: 'scope-example' },
    targets: [{ id: 'local', ready: true }], surfaceMap: { surfaces } });
  try {
    const track = state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' },
      targetIds: ['local'], surfaces });
    const runDirectory = join(directory, '.beale', 'app-server-runs');
    mkdirSync(runDirectory, { recursive: true });
    const [first] = await service.dueResearchTracks();
    assert.equal(first.request.launch.continuation, undefined);
    attempts = [{ id: 'attempt-example-initial', parentAttemptId: null,
      startedAt: '2026-01-01T01:00:00.000Z' }];
    writeFileSync(join(runDirectory, `${track.id}.capture.json`), '{"agent":{}}');
    events = [
      { kind: 'agent.event', timestamp: '2026-01-01T01:01:00.000Z', payload: {
        type: 'context_composed', agentPath: '/root', estimatedTokens: 32_000 } },
      { kind: 'agent.event', timestamp: '2026-01-01T01:02:00.000Z', payload: {
        type: 'turn_completed', agentPath: '/root', contextWindow: 100_000 } },
    ];
    await service.recordSessionTerminalState({ request: first.request, sessionId: track.id,
      attemptId: attempts[0].id, state: 'completed' });
    const [second] = await service.dueResearchTracks();
    assert.equal(second.request.sessionId, track.id);
    assert.equal(second.request.launch.continuation.resumeAttemptId, attempts[0].id);
    assert.equal(second.request.launch.continuation.resumeFromInitialAttempt, true);
    assert.match(second.request.launch.promptMarkdown, /use retained model history when available/);

    attempts.push({ id: 'attempt-example-second', parentAttemptId: attempts[0].id,
      startedAt: '2026-01-01T02:00:00.000Z' });
    writeFileSync(join(runDirectory, `${track.id}.attempt-example-second.capture.json`), '{"agent":{}}');
    events = [
      { kind: 'agent.event', timestamp: '2026-01-01T02:01:00.000Z', payload: {
        type: 'context_composed', agentPath: '/root', estimatedTokens: 87_000 } },
      { kind: 'agent.event', timestamp: '2026-01-01T02:02:00.000Z', payload: {
        type: 'turn_completed', agentPath: '/root', contextWindow: 100_000 } },
    ];
    await service.recordSessionTerminalState({ request: second.request, sessionId: track.id,
      attemptId: attempts[1].id, state: 'completed' });
    const [third] = await service.dueResearchTracks();
    assert.equal(third.request.sessionId, track.id);
    assert.equal(third.request.launch.continuation.resumeAttemptId, undefined);
    assert.match(third.request.launch.promptMarkdown, /fresh model context after context pressure/);

    attempts.push({ id: 'attempt-example-third', parentAttemptId: null,
      startedAt: '2026-01-01T03:00:00.000Z' });
    writeFileSync(join(runDirectory, `${track.id}.attempt-example-third.capture.json`), '{"agent":{}}');
    events = [
      { kind: 'agent.event', timestamp: '2026-01-01T03:01:00.000Z', payload: {
        type: 'context_composed', agentPath: '/root', estimatedTokens: 20_000 } },
      { kind: 'agent.event', timestamp: '2026-01-01T03:02:00.000Z', payload: {
        type: 'turn_completed', agentPath: '/root', contextWindow: 100_000 } },
    ];
    await service.recordSessionTerminalState({ request: third.request, sessionId: track.id,
      attemptId: attempts[2].id, state: 'completed' });
    const [fourth] = await service.dueResearchTracks();
    assert.equal(fourth.request.launch.continuation.resumeAttemptId, attempts[2].id);
    assert.equal(fourth.request.launch.continuation.resumeFromInitialAttempt, false);
  } finally { state.close(); }
});

test('stopping one track session epoch leaves the track running', async () => {
  const state = fixture();
  const service = new AppServerHostService({ registry: {
    registryDirectory: join(dirname(state.databasePath), 'registry'),
    listWorkspaces: () => [{ workspaceId }],
    resolveWorkspace: (id) => id === workspaceId ? {
      workspaceId, researchProfileId: 'security-research', memoryBackend: 'app-server'
    } : null,
    storageForProfile: () => ({ databasePath: state.databasePath, artifactDirectoryPath: join(dirname(state.databasePath), 'artifacts') })
  }, invokeProtocol: async (operation) => operation === 'session.get' ? { status: 'stopped' } : {} });
  service.workspaceResearchTrackContext = async () => ({ scope: { id: 'scope-example' },
    targets: [{ id: 'local', ready: true }], surfaceMap: { surfaces } });
  try {
    const track = state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' },
      targetIds: ['local'], surfaces });
    const [first] = await service.dueResearchTracks();
    assert.equal(first.request.sessionId, track.id);
    await service.recordSessionTerminalState({ request: first.request, sessionId: track.id,
      attemptId: 'attempt-example-stopped', state: 'stopped' });
    assert.equal(state.store.get().status, 'running');
    assert.equal(state.store.get().activeSessionId, null);
    const [next] = await service.dueResearchTracks();
    assert.equal(next.request.sessionId, track.id);
    assert.equal(state.store.get().status, 'running');
  } finally { state.close(); }
});

test('an orphaned track epoch pauses when Guardian is disabled', async () => {
  const state = fixture();
  const service = new AppServerHostService({ registry: {
    registryDirectory: join(dirname(state.databasePath), 'registry'),
    listWorkspaces: () => [{ workspaceId }],
    resolveWorkspace: (id) => id === workspaceId ? {
      workspaceId, researchProfileId: 'security-research', memoryBackend: 'app-server'
    } : null,
    storageForProfile: () => ({ databasePath: state.databasePath, artifactDirectoryPath: join(dirname(state.databasePath), 'artifacts') })
  }, invokeProtocol: async (operation) => operation === 'session.get' ? interruptedTrackSession(false) : {} });
  service.workspaceResearchTrackContext = async () => ({ scope: { id: 'scope-example' },
    targets: [{ id: 'local', ready: true }], surfaceMap: { surfaces } });
  try {
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' }, guardianEnabled: false,
      targetIds: ['local'], surfaces });
    state.store.reserveNext('session-example-orphaned', surfaces);
    state.database.prepare('UPDATE app_server_research_tracks SET updated_at=? WHERE workspace_id=?')
      .run('2000-01-01T00:00:00.000Z', workspaceId);
    assert.deepEqual(await service.dueResearchTracks(new Date(), new Set()), []);
    assert.equal(state.store.get().status, 'paused');
    assert.equal(state.store.get().activeSessionId, null);
    assert.match(state.store.get().lastError, /without a terminal session record/);
  } finally { state.close(); }
});

test('a recently recovered session keeps its assigned track surface during startup', async () => {
  const state = fixture();
  let sessionUpdatedAt = new Date().toISOString();
  const service = new AppServerHostService({ registry: {
    registryDirectory: join(dirname(state.databasePath), 'registry'),
    listWorkspaces: () => [{ workspaceId }],
    resolveWorkspace: (id) => id === workspaceId ? {
      workspaceId, researchProfileId: 'security-research', memoryBackend: 'app-server'
    } : null,
    storageForProfile: () => ({ databasePath: state.databasePath,
      artifactDirectoryPath: join(dirname(state.databasePath), 'artifacts') })
  }, invokeProtocol: async (operation) => operation === 'session.get'
    ? { ...interruptedTrackSession(false), updatedAt: sessionUpdatedAt } : {} });
  service.workspaceResearchTrackContext = async () => ({ scope: { id: 'scope-example' },
    targets: [{ id: 'local', ready: true }], surfaceMap: { surfaces } });
  try {
    const track = state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' },
      guardianEnabled: false, targetIds: ['local'], surfaces });
    const assigned = state.store.reserveNext(track.id, surfaces);
    state.database.prepare('UPDATE app_server_research_tracks SET updated_at=? WHERE workspace_id=?')
      .run('2000-01-01T00:00:00.000Z', workspaceId);
    assert.deepEqual(await service.dueResearchTracks(new Date(), new Set()), []);
    assert.equal(state.store.get().status, 'running');
    assert.equal(state.store.get().activeSurfaceId, assigned.surface.id);
    sessionUpdatedAt = '2000-01-01T00:00:00.000Z';
    assert.deepEqual(await service.dueResearchTracks(new Date(), new Set()), []);
    assert.equal(state.store.get().status, 'paused');
  } finally { state.close(); }
});

test('track epochs retain a disabled Guardian setting across scheduling', async () => {
  const state = fixture();
  const service = new AppServerHostService({ registry: {
    registryDirectory: join(dirname(state.databasePath), 'registry'),
    listWorkspaces: () => [{ workspaceId }],
    resolveWorkspace: (id) => id === workspaceId ? {
      workspaceId, researchProfileId: 'security-research', memoryBackend: 'app-server'
    } : null,
    storageForProfile: () => ({ databasePath: state.databasePath, artifactDirectoryPath: join(dirname(state.databasePath), 'artifacts') })
  }, invokeProtocol: async () => ({}) });
  service.workspaceResearchTrackContext = async () => ({ scope: { id: 'scope-example' },
    targets: [{ id: 'local', ready: true }], surfaceMap: { surfaces } });
  try {
    const track = state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' },
      guardianEnabled: false, targetIds: ['local'], surfaces });
    assert.equal(track.guardianEnabled, false);
    const [due] = await service.dueResearchTracks();
    assert.equal(due.request.launch.guardianEnabled, false);
    assert.doesNotMatch(due.request.launch.promptMarkdown, /Track collaboration:/);
  } finally { state.close(); }
});

test('Guardian recovers an orphaned track epoch and reviews the interrupted work in the next epoch', async () => {
  const state = fixture();
  const transitions = [];
  let transitionAttempts = 0;
  const service = new AppServerHostService({ registry: {
    registryDirectory: join(dirname(state.databasePath), 'registry'),
    listWorkspaces: () => [{ workspaceId }],
    resolveWorkspace: (id) => id === workspaceId ? {
      workspaceId, researchProfileId: 'security-research', memoryBackend: 'app-server'
    } : null,
    storageForProfile: () => ({ databasePath: state.databasePath, artifactDirectoryPath: join(dirname(state.databasePath), 'artifacts') })
  }, invokeProtocol: async (operation, options) => {
    if (operation === 'session.get') return interruptedTrackSession(true);
    if (operation === 'session.transition') {
      transitionAttempts += 1;
      if (transitionAttempts === 1) throw new Error('Synthetic persistence interruption.');
      transitions.push(options.input);
      return {};
    }
    return {};
  } });
  service.workspaceResearchTrackContext = async () => ({ scope: { id: 'scope-example' },
    targets: [{ id: 'local', ready: true }], surfaceMap: { surfaces } });
  try {
    const track = state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' },
      targetIds: ['local'], surfaces });
    const first = state.store.reserveNext(track.id, surfaces);
    state.database.prepare('UPDATE app_server_research_track_surfaces SET no_progress_epochs=2 WHERE workspace_id=? AND surface_id=?')
      .run(workspaceId, first.surface.id);
    state.database.prepare('UPDATE app_server_research_tracks SET updated_at=? WHERE workspace_id=?')
      .run('2000-01-01T00:00:00.000Z', workspaceId);
    assert.deepEqual(await service.dueResearchTracks(new Date(), new Set()), []);
    assert.equal(state.store.get().status, 'running');
    assert.equal(state.store.get().activeSessionId, track.id);
    const [next] = await service.dueResearchTracks(new Date(), new Set());
    assert.equal(state.store.get().status, 'running');
    assert.equal(state.store.get().activeSessionId, track.id);
    assert.equal(state.store.get().surfaces.find((work) => work.surfaceId === first.surface.id).noProgressEpochs, 2);
    assert.equal(transitions.length, 1);
    assert.equal(transitions[0].status, 'failed');
    assert.equal(transitions[0].attemptId, 'attempt-example-interrupted');
    assert.match(next.request.launch.promptMarkdown, /Guardian recovery review: The prior worker ended/u);
  } finally { state.close(); }
});

test('Guardian resumes a track paused by the earlier orphan policy after reviewing its session', async () => {
  const state = fixture();
  const transitions = [];
  const service = new AppServerHostService({ registry: {
    registryDirectory: join(dirname(state.databasePath), 'registry'),
    listWorkspaces: () => [{ workspaceId }],
    resolveWorkspace: (id) => id === workspaceId ? {
      workspaceId, researchProfileId: 'security-research', memoryBackend: 'app-server'
    } : null,
    storageForProfile: () => ({ databasePath: state.databasePath, artifactDirectoryPath: join(dirname(state.databasePath), 'artifacts') })
  }, invokeProtocol: async (operation, options) => {
    if (operation === 'session.get') return interruptedTrackSession(true);
    if (operation === 'session.transition') { transitions.push(options.input); return {}; }
    return {};
  } });
  service.workspaceResearchTrackContext = async () => ({ scope: { id: 'scope-example' },
    targets: [{ id: 'local', ready: true }], surfaceMap: { surfaces } });
  try {
    const track = state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' },
      targetIds: ['local'], surfaces });
    state.store.reserveNext(track.id, surfaces);
    state.store.finishEpoch(track.id, false, 'The research worker ended without a terminal session record.');
    state.store.pauseWithError('The research worker ended without a terminal session record. Review the session before resuming.');
    const [next] = await service.dueResearchTracks(new Date(), new Set());
    assert.equal(state.store.get().status, 'running');
    assert.equal(state.store.get().activeSessionId, track.id);
    assert.equal(transitions.length, 1);
    assert.match(next.request.launch.promptMarkdown, /Guardian recovery review: The prior worker ended/u);
  } finally { state.close(); }
});

test('startup recovery advances a checkpointed track epoch without replaying it', async () => {
  const state = fixture();
  let terminalTransition = null;
  let started = null;
  const service = new AppServerHostService({ registry: {
    registryDirectory: join(dirname(state.databasePath), 'registry'),
    listWorkspaces: () => [{ workspaceId }],
    resolveWorkspace: (id) => id === workspaceId ? {
      workspaceId, researchProfileId: 'security-research', memoryBackend: 'app-server'
    } : null,
    storageForProfile: () => ({ databasePath: state.databasePath, artifactDirectoryPath: join(dirname(state.databasePath), 'artifacts') })
  }, invokeProtocol: async (operation, options) => {
    if (operation === 'session.recover_interrupted') return { sessionIds: [started.id], interruptedSessions: 1 };
    if (operation === 'session.get') return { workspaceId, status: 'paused',
      metadata: { appServerRestartLaunch: { schemaVersion: 1, eligible: true,
        launch: { workspaceId, mode: 'research-track', promptMarkdown: 'Synthetic surface investigation.',
          machineId: 'local', targetIds: ['local'], provider: { id: 'openai-codex', model: 'example-model' } } } },
      attempts: [{ id: 'attempt-example-interrupted', parentAttemptId: null,
        status: 'paused', metadata: { interruptedByRecovery: true } }] };
    if (operation === 'session.transition') { terminalTransition = options.input; return {}; }
    throw new Error(`Unexpected operation: ${operation}`);
  } });
  service.workspaceResearchTrackContext = async () => ({ scope: { id: 'scope-example' },
    targets: [{ id: 'local', ready: true }], surfaceMap: { surfaces } });
  try {
    started = state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' },
      targetIds: ['local'], surfaces });
    const first = state.store.reserveNext(started.id, surfaces);
    const worker = new ResearchTrackStore({ databasePath: state.databasePath, workspaceId, sessionId: started.id });
    try { worker.checkpoint({ surfaceId: first.surface.id, outcome: 'defer', impact: 'medium',
      theory: 'A synthetic trust boundary needs another proof.', nextAction: 'Inspect a different entry path.',
      evidenceRefs: [] }, first.surface); }
    finally { worker.close(); }
    const recovery = await service.recoverInterruptedSessions();
    assert.equal(recovery.recovered.length, 0);
    assert.equal(recovery.skippedSessions, 1);
    assert.equal(terminalTransition.status, 'completed');
    assert.equal(state.store.get().activeSessionId, null);
    assert.equal((await service.dueResearchTracks())[0].request.sessionId, started.id);
    assert.equal(state.store.get().activeSurfaceId, surfaces[1].id);
  } finally { state.close(); }
});

test('a high-impact surface can complete after a bounded assessment', () => {
  const state = fixture();
  try {
    const map = [{ ...surfaces[0], coverageState: covered }];
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' }, targetIds: ['local'], surfaces: map });
    const first = state.store.reserveNext('session-example-first', map);
    const worker = new ResearchTrackStore({ databasePath: state.databasePath, workspaceId, sessionId: 'session-example-first' });
    try { worker.checkpoint({ surfaceId: first.surface.id, outcome: 'complete', impact: 'critical',
      theory: 'Reviewed a privileged boundary.', nextAction: 'Revisit if the source changes.',
      evidenceRefs: [] }, map[0]); }
    finally { worker.close(); }
    state.store.finishEpoch('session-example-first', true);
    assert.equal(state.store.reserveNext('session-example-final', map), null);
    assert.equal(state.store.get().status, 'complete');
  } finally { state.close(); }
});

test('track respects external blockers and can start a new audit pass', () => {
  const state = fixture();
  try {
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' }, targetIds: ['local'], surfaces });
    const blocked = [{ ...surfaces[0], progressBlocker: { reason: 'An authorized account is unavailable.' } }, surfaces[1]];
    assert.equal(state.store.syncSurfaces(blocked).surfaces.find((work) => work.surfaceId === surfaces[0].id).status, 'blocked');
    assert.equal(state.store.reserveNext('session-example-two', blocked).surface.id, surfaces[1].id);
    state.store.finishEpoch('session-example-two', true);
    const worker = new ResearchTrackStore({ databasePath: state.databasePath, workspaceId, sessionId: 'session-example-three' });
    try {
      const assigned = state.store.reserveNext('session-example-three', blocked);
      assert.equal(assigned.surface.id, surfaces[1].id);
      state.database.prepare('UPDATE memory_nodes SET updated_at=? WHERE id=?')
        .run(new Date().toISOString(), 'memory-example-a');
      worker.checkpoint({ surfaceId: surfaces[1].id, outcome: 'complete', impact: 'medium',
        theory: 'Reviewed the library trust boundary.', nextAction: 'Revisit after source changes.',
        evidenceRefs: ['memory:memory-example-a'] }, { ...surfaces[1], coverageState: covered });
    } finally { worker.close(); }
    state.store.finishEpoch('session-example-three', true);
    const map = [blocked[0], { ...surfaces[1], coverageState: covered }];
    assert.equal(state.store.reserveNext('session-example-four', map), null);
    assert.equal(state.store.get().status, 'waiting');
    assert.equal(state.store.control('resume').status, 'running');
    assert.equal(state.store.syncSurfaces([surfaces[0], map[1]]).surfaces.find((work) => work.surfaceId === surfaces[0].id).status, 'deferred');
    assert.equal(state.store.reserveNext('session-example-five', [surfaces[0], map[1]]).surface.id, surfaces[0].id);
    state.store.finishEpoch('session-example-five', false);
    state.store.control('stop');
    assert.equal(state.store.start({ scopeId: 'scope-example-new', provider: { id: 'openai-codex' },
      targetIds: ['local'], surfaces }).pass, 2);
  } finally { state.close(); }
});

test('stopping an active track permits an immediate new pass with a distinct session', () => {
  const state = fixture();
  try {
    const first = state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' },
      targetIds: ['local'], surfaces });
    state.store.reserveNext(first.id, surfaces);
    const stopped = state.store.control('stop');
    assert.equal(stopped.status, 'stopped');
    assert.equal(stopped.activeSessionId, null);
    assert.equal(stopped.activeSurfaceId, null);
    assert.equal(stopped.surfaces.find((surface) => surface.surfaceId === surfaces[0].id).status, 'deferred');
    state.database.prepare('UPDATE app_server_research_tracks SET active_session_id=? WHERE workspace_id=?')
      .run(first.id, workspaceId);
    const second = state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' },
      targetIds: ['local'], surfaces });
    assert.equal(second.pass, 2);
    assert.notEqual(second.id, first.id);
    assert.equal(second.createdAt, second.updatedAt);
    assert.equal(state.store.finishEpoch(first.id, true).id, second.id);
    assert.equal(state.store.get().status, 'running');
    const history = state.database.prepare('SELECT snapshot_json FROM app_server_research_track_history WHERE workspace_id=?')
      .get(workspaceId);
    assert.equal(JSON.parse(history.snapshot_json).status, 'stopped');
  } finally { state.close(); }
});

test('a completed parent stays complete while a child remains open', () => {
  const state = fixture();
  try {
    const map = [
      { ...surfaces[0], coverageState: covered },
      { ...surfaces[1], parentId: surfaces[0].id },
    ];
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' }, targetIds: ['local'], surfaces: map });
    state.database.prepare(`UPDATE app_server_research_track_surfaces SET status='complete'
      WHERE workspace_id=? AND surface_id=?`).run(workspaceId, surfaces[0].id);
    assert.equal(state.store.syncSurfaces(map).surfaces.find((work) => work.surfaceId === surfaces[0].id).status, 'complete');
    assert.equal(state.store.reserveNext('session-example-child', map).surface.id, surfaces[1].id);
  } finally { state.close(); }
});

test('three epochs without durable progress pause the track', () => {
  const state = fixture();
  try {
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' }, targetIds: ['local'], surfaces: [surfaces[0]] });
    for (let index = 0; index < 3; index += 1) {
      const sessionId = `session-example-no-progress-${index}`;
      assert.ok(state.store.reserveNext(sessionId, [surfaces[0]]));
      state.store.finishEpoch(sessionId, true);
    }
    assert.equal(state.store.get().status, 'paused');
    assert.match(state.store.get().lastError, /no durable progress/);
  } finally { state.close(); }
});

test('repeated no-progress work rotates to another available surface', () => {
  const state = fixture();
  try {
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' }, targetIds: ['local'], surfaces });
    const first = state.store.reserveNext('session-example-stalled', surfaces);
    state.database.prepare(`UPDATE app_server_research_track_surfaces SET no_progress_epochs=2
      WHERE workspace_id=? AND surface_id=?`).run(workspaceId, first.surface.id);
    const finished = state.store.finishEpoch('session-example-stalled', true);
    assert.equal(finished.status, 'running');
    assert.equal(finished.lastError, null);
    assert.notEqual(state.store.reserveNext('session-example-other', surfaces).surface.id, first.surface.id);
  } finally { state.close(); }
});

test('report-ready findings do not reopen completed surface work', () => {
  const state = fixture();
  try {
    const map = [{ ...surfaces[0], coverageState: { covered: false,
      unmetRequirements: ['unblocked_findings'], unblockedFindingIds: ['finding-example'] } }];
    state.database.exec('CREATE TABLE app_server_research_claims (id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, status TEXT NOT NULL, updated_at TEXT NOT NULL)');
    state.database.prepare('INSERT INTO app_server_research_claims VALUES (?, ?, ?, ?)')
      .run('finding-example', workspaceId, 'report_ready', new Date().toISOString());
    state.store.start({ scopeId: 'scope-example', provider: { id: 'openai-codex' }, targetIds: ['local'], surfaces: map });
    state.store.reserveNext('session-example-report-ready', map);
    state.database.prepare('UPDATE memory_nodes SET updated_at=? WHERE id=?').run(new Date().toISOString(), 'memory-example-a');
    const worker = new ResearchTrackStore({ databasePath: state.databasePath, workspaceId, sessionId: 'session-example-report-ready' });
    try { worker.checkpoint({ surfaceId: map[0].id, outcome: 'complete', impact: 'medium',
      theory: 'The finding is ready for reporting.', nextAction: 'Revisit after a source change.',
      evidenceRefs: ['memory:memory-example-a'] }, map[0]); }
    finally { worker.close(); }
    state.store.finishEpoch('session-example-report-ready', true);
    assert.equal(state.store.reserveNext('session-example-next', map), null);
    assert.equal(state.store.get().surfaces[0].status, 'complete');
    assert.equal(state.store.get().status, 'complete');
  } finally { state.close(); }
});
