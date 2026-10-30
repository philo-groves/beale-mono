import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import test from 'node:test';
import { strToU8, zipSync } from '../packages/research-agent/node_modules/fflate/esm/index.mjs';
import { recordRunbookExecution } from './fixtures/record-runbook-execution.mjs';
import {
  MemoryGraphStore, ResearchClaimStore, RunbookStore, ReportStore, TrackFunnelStore,
  ResearchTrackStore,
  createResearchStorageLayout, ensureResearchStorageLayout, createResearchToolRegistry,
  createTriageTools, createReportTools,
} from '../packages/research-agent/dist/index.js';
import { AppServerHostService } from '../app-server/dist/hostService.js';

function fixture() {
  const close = [];
  return mkdtemp(join(tmpdir(), 'beale-track-funnel-example-')).then((root) => {
    const context = { workspaceId: 'workspace-example-funnel', workspaceName: 'Example workspace',
      subjectId: 'subject-example', subjectName: 'Example subject', sessionId: 'session-example' };
    const graph = new MemoryGraphStore({ workspaceRoot: root, databasePath: join(root, 'memory.sqlite'), context });
    close.push(graph);
    const claims = new ResearchClaimStore(graph); close.unshift(claims);
    const layout = ensureResearchStorageLayout(createResearchStorageLayout({
      workspaceRoot: root,
      databasePath: graph.databasePath,
      artifactDirectoryPath: join(root, 'artifacts'),
    }));
    const runbooks = new RunbookStore(graph.databasePath, layout, graph.getContext()); close.unshift(runbooks);
    const reports = new ReportStore(graph.databasePath, layout, graph.getContext(), { packetCandidateRoots: [root] }); close.unshift(reports);
    const funnel = new TrackFunnelStore({ databasePath: graph.databasePath, workspaceId: context.workspaceId }); close.unshift(funnel);
    return { root, graph, claims, runbooks, reports, funnel,
      async close() { for (const item of close) item.close(); await rm(root, { recursive: true, force: true }); } };
  });
}

function observed(claims, title) {
  return claims.createObserved({ title, classification: 'security.primitive', rating: 'high',
    sourceRevision: 'git:example-revision-1', environmentFingerprint: 'example-environment-a',
    summary: 'An example pre-auth input reaches a sensitive sink.',
    impact: 'An example protected value could cross a trust boundary.',
    evidence: [{ kind: 'code', referenceId: 'src/example.ts:10', summary: 'Example source path.' }],
  }, undefined, 'agent-example-discovery');
}

function recordWorkerActivity(databasePath, sessionId, assignment, action, offset) {
  const database = new DatabaseSync(databasePath);
  try {
    database.exec(`CREATE TABLE IF NOT EXISTS app_server_session_events (
      session_id TEXT NOT NULL, event_offset INTEGER NOT NULL, event_json TEXT NOT NULL,
      PRIMARY KEY (session_id, event_offset))`);
    const attempt = database.prepare(`SELECT assigned_at FROM app_server_track_worker_attempts
      WHERE active_session_id=? AND role=? AND claim_id=?`)
      .get(sessionId, assignment.role, assignment.claimId);
    const timestamp = new Date(Date.parse(attempt.assigned_at) + offset * 1000).toISOString();
    database.prepare(`INSERT INTO app_server_session_events (session_id, event_offset, event_json) VALUES (?, ?, ?)`)
      .run(sessionId, offset, JSON.stringify({ kind: 'agent.event', timestamp,
        payload: { type: 'subagent.activity', action,
          status: action === 'spawned' ? 'running' : action, role: assignment.role,
          trackClaimId: assignment.claimId } }));
  } finally { database.close(); }
}

test('track funnel is empty without eligible findings and cools unchanged investigation', async () => {
  const state = await fixture();
  try {
    assert.deepEqual(state.funnel.reserveForSession('epoch-empty', 1), []);
    let finding = observed(state.claims, 'Example sensitive parser path');
    assert.deepEqual(state.funnel.reserveForSession('epoch-one', 1).map((item) => item.role), ['investigator']);
    assert.deepEqual(state.funnel.activeForSession('epoch-one').map((item) => item.claimId), [finding.id]);
    assert.deepEqual(state.funnel.reserveForSession('epoch-one', 2).map((item) => item.role),
      ['investigator'], 'a recovered epoch can reclaim a prior assignment under the same track session');
    state.funnel.finishSession('epoch-one');
    assert.deepEqual(state.funnel.reserveForSession('epoch-two', 2), []);
    finding = state.claims.revise(finding.id, { expectedRevision: finding.revision,
      reason: 'A distinct entry path was identified.', summary: 'An example input reaches the sink on a distinct path.' });
    assert.equal(state.funnel.reserveForSession('epoch-three', 3)[0]?.claimRevision, finding.revision);
    state.funnel.finishSession('epoch-three');
  } finally { await state.close(); }
});

test('track worker cards show the assigned finding and role-specific eligible queue', async () => {
  const state = await fixture();
  try {
    const first = observed(state.claims, 'Example parser boundary');
    const second = observed(state.claims, 'Example callback boundary');
    assert.deepEqual(state.funnel.cardsForSession('epoch-example'), []);
    const [assignment] = state.funnel.reserveForSession('epoch-example', 1, 1);
    assert.deepEqual(state.funnel.cardsForSession('epoch-example'), [], 'a reservation is not yet a running worker');
    recordWorkerActivity(state.graph.databasePath, 'epoch-example', assignment, 'spawned', 1);
    const cards = state.funnel.cardsForSession('epoch-example');
    assert.equal(cards.length, 1);
    assert.equal(cards[0].role, 'investigator');
    assert.equal(cards[0].claimId, assignment.claimId);
    assert.equal(cards[0].title, assignment.claimId === first.id ? first.title : second.title);
    assert.ok(Number.isFinite(Date.parse(cards[0].assignedAt)));
    assert.deepEqual(cards[0].queued, [assignment.claimId === first.id
      ? { claimId: second.id, title: second.title }
      : { claimId: first.id, title: first.title }]);
    state.funnel.finishSession('epoch-example');
    assert.deepEqual(state.funnel.cardsForSession('epoch-example'), []);
  } finally { await state.close(); }
});

test('track worker cards disappear individually when workers complete before the epoch ends', async () => {
  const state = await fixture();
  try {
    observed(state.claims, 'Example parser boundary');
    observed(state.claims, 'Example callback boundary');
    const assignments = state.funnel.reserveForSession('epoch-example', 1, 2);
    assert.equal(assignments.length, 2);
    recordWorkerActivity(state.graph.databasePath, 'epoch-example', assignments[0], 'spawned', 1);
    recordWorkerActivity(state.graph.databasePath, 'epoch-example', assignments[1], 'spawned', 2);
    assert.equal(state.funnel.cardsForSession('epoch-example').length, 2);
    recordWorkerActivity(state.graph.databasePath, 'epoch-example', assignments[0], 'completed', 3);
    assert.deepEqual(state.funnel.cardsForSession('epoch-example').map((card) => card.claimId), [assignments[1].claimId]);
    recordWorkerActivity(state.graph.databasePath, 'epoch-example', assignments[1], 'completed', 4);
    assert.deepEqual(state.funnel.cardsForSession('epoch-example'), []);
    assert.equal(state.funnel.activeForSession('epoch-example').length, 2,
      'reservations remain until the epoch ends, even after cards disappear');
  } finally { await state.close(); }
});

test('track worker runtime upgrade retries reservations made before workers could start', async () => {
  const state = await fixture();
  try {
    const finding = observed(state.claims, 'Example retryable parser path');
    assert.deepEqual(state.funnel.reserveForSession('legacy-epoch', 1).map((item) => item.role), ['investigator']);
    state.funnel.finishSession('legacy-epoch');
    const db = new DatabaseSync(state.graph.databasePath);
    try {
      db.exec('ALTER TABLE app_server_track_worker_attempts DROP COLUMN worker_runtime_version');
      db.prepare(`UPDATE app_server_track_worker_attempts SET attempts_on_revision=2,
        next_eligible_at='2999-01-01T00:00:00.000Z'
        WHERE workspace_id=? AND claim_id=?`).run(state.graph.getContext().workspaceId, finding.id);
    } finally { db.close(); }
    const upgraded = new TrackFunnelStore({ databasePath: state.graph.databasePath,
      workspaceId: state.graph.getContext().workspaceId });
    try {
      assert.deepEqual(upgraded.reserveForSession('fixed-epoch', 2).map((item) => item.role), ['investigator']);
      upgraded.finishSession('fixed-epoch');
    } finally { upgraded.close(); }
  } finally { await state.close(); }
});

test('chain work is sparse and tied to linked component evidence', async () => {
  const state = await fixture();
  try {
    const primitive = observed(state.claims, 'Example authorization primitive');
    let chain = state.claims.create({ title: 'Example composed authority crossing',
      classification: 'security.chain', rating: 'high', componentClaimIds: [primitive.id],
      summary: 'The example primitive may combine with a second boundary.' });
    assert.deepEqual(state.funnel.reserveForSession('epoch-chain-one', 0).map((item) => item.role),
      ['investigator', 'chain_analyst']);
    state.funnel.finishSession('epoch-chain-one');
    assert.equal(state.funnel.reserveForSession('epoch-chain-two', 1).some((item) => item.role === 'chain_analyst'), false);
    const database = new DatabaseSync(state.graph.databasePath);
    try {
      database.prepare(`UPDATE app_server_track_worker_attempts SET next_eligible_at='2000-01-01T00:00:00.000Z'
        WHERE workspace_id=? AND role='chain_analyst' AND claim_id=?`).run('workspace-example-funnel', chain.id);
    } finally { database.close(); }
    assert.equal(state.funnel.reserveForSession('epoch-chain-second-review', 2)
      .some((item) => item.role === 'chain_analyst'), true);
    state.funnel.finishSession('epoch-chain-second-review');
    assert.equal(state.funnel.reserveForSession('epoch-chain-exhausted', 3)
      .some((item) => item.role === 'chain_analyst'), false);
    chain = state.claims.revise(chain.id, { expectedRevision: chain.revision,
      reason: 'The composition edge gained new evidence.',
      summary: 'The example primitive reaches a second authority boundary.' });
    assert.equal(state.funnel.reserveForSession('epoch-chain-three', 4).find((item) => item.role === 'chain_analyst')?.claimRevision,
      chain.revision);
    state.funnel.finishSession('epoch-chain-three');
    const observedChain = state.claims.createObserved({ title: 'Example observed composition',
      classification: 'security.chain', rating: 'high', componentClaimIds: [primitive.id],
      evidence: [{ kind: 'code', referenceId: 'src/example-chain.ts:20', summary: 'Example composition path.' }],
    });
    assert.throws(() => state.claims.revise(primitive.id, { expectedRevision: primitive.revision,
      reason: 'Attempt a circular composition.', classification: 'security.chain',
      componentClaimIds: [observedChain.id] }), /cycle/);
  } finally { await state.close(); }
});

test('the host runs eligible finding workers while discovery components cool', async () => {
  const state = await fixture();
  const workspaceId = 'workspace-example-funnel';
  const surface = { id: 'surface-example-service', parentId: null, name: 'Example service',
    coverageState: { covered: false, unmetRequirements: ['sinks'] }, progressBlocker: null };
  const track = new ResearchTrackStore({ databasePath: state.graph.databasePath, workspaceId });
  try {
    track.start({ scopeId: 'scope-example', provider: { id: 'openai-codex', model: 'example-model' },
      targetIds: ['local'], surfaces: [surface] });
    const database = new DatabaseSync(state.graph.databasePath);
    try {
      database.prepare(`UPDATE app_server_research_track_surfaces SET status='deferred', no_progress_epochs=2,
        revisit_after=? WHERE workspace_id=? AND surface_id=?`).run(
          new Date(Date.now() + 60 * 60_000).toISOString(), workspaceId, surface.id);
    } finally { database.close(); }
    observed(state.claims, 'Example queued parser finding');
    const host = new AppServerHostService({ registry: {
      registryDirectory: join(state.root, 'registry'),
      listWorkspaces: () => [{ workspaceId }],
      resolveWorkspace: (id) => id === workspaceId ? { workspaceId, researchProfileId: 'security-research',
        memoryBackend: 'app-server' } : null,
      storageForProfile: () => ({ databasePath: state.graph.databasePath,
        artifactDirectoryPath: join(state.root, 'artifacts') }),
    }, invokeProtocol: async () => ({}) });
    host.workspaceResearchTrackContext = async () => ({ scope: { id: 'scope-example' },
      targets: [{ id: 'local', ready: true }], surfaceMap: { surfaces: [surface] } });
    const due = await host.dueResearchTracks(new Date(), new Set(), new Set(), workspaceId);
    assert.equal(due.length, 1);
    assert.match(due[0].request.launch.promptMarkdown, /No discovery component is ready/);
    assert.equal(track.get().activeSurfaceId, null);
    const [assignment] = state.funnel.activeForSession(track.get().id);
    assert.equal(assignment?.role, 'investigator');
    const status = await host.workspaceResearchTrack(workspaceId, 'status');
    assert.deepEqual(status.workers, [], 'a reservation does not represent a started worker');
    recordWorkerActivity(state.graph.databasePath, track.get().id, assignment, 'spawned', 1);
    const runningStatus = await host.workspaceResearchTrack(workspaceId, 'status');
    assert.deepEqual(runningStatus.workers.map((worker) => worker.role), ['investigator']);
    assert.equal(runningStatus.workers[0].title, 'Example queued parser finding');
    await host.recordResearchTrackEpochLaunchFailure(workspaceId, track.get().id, 'Synthetic launch failure.');
    assert.deepEqual(state.funnel.activeForSession(track.get().id), []);
  } finally { track.close(); await state.close(); }
});

test('a completed discovery pass can still process newly eligible findings', async () => {
  const state = await fixture();
  const workspaceId = 'workspace-example-funnel';
  const surface = { id: 'surface-example-complete', parentId: null, name: 'Example completed service',
    coverageState: { covered: true, unmetRequirements: [] }, progressBlocker: null };
  const track = new ResearchTrackStore({ databasePath: state.graph.databasePath, workspaceId });
  try {
    track.start({ scopeId: 'scope-example', provider: { id: 'openai-codex', model: 'example-model' },
      targetIds: ['local'], surfaces: [surface] });
    const database = new DatabaseSync(state.graph.databasePath);
    try {
      database.prepare(`UPDATE app_server_research_track_surfaces SET status='complete'
        WHERE workspace_id=? AND surface_id=?`).run(workspaceId, surface.id);
      database.prepare(`UPDATE app_server_research_tracks SET status='complete' WHERE workspace_id=?`).run(workspaceId);
    } finally { database.close(); }
    observed(state.claims, 'Example late finding');
    const host = new AppServerHostService({ registry: {
      registryDirectory: join(state.root, 'registry'),
      listWorkspaces: () => [{ workspaceId }],
      resolveWorkspace: (id) => id === workspaceId ? { workspaceId, researchProfileId: 'security-research',
        memoryBackend: 'app-server' } : null,
      storageForProfile: () => ({ databasePath: state.graph.databasePath,
        artifactDirectoryPath: join(state.root, 'artifacts') }),
    }, invokeProtocol: async () => ({}) });
    host.workspaceResearchTrackContext = async () => ({ scope: { id: 'scope-example' },
      targets: [{ id: 'local', ready: true }], surfaceMap: { surfaces: [surface] } });
    const due = await host.dueResearchTracks(new Date(), new Set(), new Set(), workspaceId);
    assert.equal(due.length, 1);
    assert.match(due[0].request.launch.promptMarkdown, /finding work epoch/);
    assert.equal(track.get().activeSurfaceId, null);
    assert.deepEqual(state.funnel.activeForSession(track.get().id).map((item) => item.role), ['investigator']);
    await host.recordResearchTrackEpochLaunchFailure(workspaceId, track.get().id, 'Synthetic launch failure.');
  } finally { track.close(); await state.close(); }
});

test('verified findings flow through policy triage, portable replay packet, and reporting', async () => {
  const state = await fixture();
  try {
    let finding = observed(state.claims, 'Example callback authority crossing');
    const book = state.runbooks.create({ title: 'Example callback replay', purpose: 'Replay the authority crossing.',
      cells: [{ kind: 'code', language: 'sh', source: './example-replay', features: ['runtime'] }] }).runbook;
    recordRunbookExecution(state.runbooks, book.id, 'example-original-run', {
      sourceRevision: finding.sourceRevision, environmentFingerprint: finding.environmentFingerprint,
      actorId: 'agent-example-discovery',
    });
    finding = state.claims.transition(finding.id, { expectedRevision: finding.revision, toStatus: 'reproduced',
      reason: 'The example proof ran.', reproductionRunbookId: book.id,
      stepsToReproduce: 'Run the example replay and inspect the protected callback.',
      evidence: [{ kind: 'runbook_execution', referenceId: 'example-original-run', summary: 'Example proof ran.' }],
    }, undefined, 'agent-example-discovery');
    finding = state.claims.transition(finding.id, { expectedRevision: finding.revision, toStatus: 'verified',
      reason: 'A separate reviewer checked the proof and contrary evidence.',
      evidence: [{ kind: 'independent_verification', referenceId: 'example-original-run',
        summary: 'Example review held.', independent: true }],
    }, undefined, 'agent-example-reviewer', { freshSubagentContext: true });
    assert.deepEqual(state.funnel.reserveForSession('epoch-triage', 1).map((item) => item.role), ['triager']);
    state.funnel.finishSession('epoch-triage');
    const reportTools = createResearchToolRegistry(createReportTools(state.reports, {
      requireVerifiedFinding: true, requireSubmissionPacket: true, claimStore: state.claims,
    }));
    const reportInput = { title: 'Example callback issue', summary: 'Example report.',
      content: '# Example callback issue\n\nReproduction and impact.', sourceFindingId: finding.id };
    let outcome = await reportTools.execute({ id: 'report-before-triage', toolName: 'report.create',
      actionClass: 'synthesize', input: reportInput });
    assert.equal(outcome.result.status, 'error');
    assert.match(outcome.result.error.message, /Reportable/);
    finding = state.claims.revise(finding.id, { expectedRevision: finding.revision, reason: 'Example policy threshold assessed.',
      securityTracking: { reportability: { decision: 'not_reportable',
        basis: 'The first example program rule excludes this configuration.', policyRefs: ['example-policy:scope'] } } });
    assert.deepEqual(state.funnel.reserveForSession('epoch-not-reportable', 2), []);
    finding = state.claims.revise(finding.id, { expectedRevision: finding.revision, reason: 'Example program policy assessed.',
      securityTracking: { reportability: { decision: 'reportable', basis: 'The authorized pre-auth boundary crosses the program threshold.',
        policyRefs: ['example-policy:scope', 'example-policy:impact'] } } });
    recordRunbookExecution(state.runbooks, book.id, 'example-portable-run', {
      sourceRevision: finding.sourceRevision, environmentFingerprint: 'example-environment-b',
      actorId: 'agent-example-triage',
    });
    const files = {
      'README.md': strToU8('Run poc/replay.sh on the authorized example target.\n'),
      'poc/replay.sh': strToU8('#!/bin/sh\n./example-replay\n'),
    };
    const manifest = { schemaVersion: 1, claimId: finding.id, replayRunId: 'example-portable-run',
      prerequisites: 'An authorized example target and shell.', stepsToReproduce: 'Run poc/replay.sh and inspect the callback.',
      expectedResult: 'The protected callback remains private.', observedResult: 'The protected callback crosses the example boundary.',
      policyRefs: ['example-policy:scope', 'example-policy:impact'],
      files: Object.entries(files).map(([path, bytes]) => ({ path,
        contentHash: `sha256:${createHash('sha256').update(bytes).digest('hex')}` })),
    };
    const packetPath = join(state.root, 'example-poc.zip');
    await writeFile(packetPath, Buffer.from(zipSync({ ...files, 'manifest.json': strToU8(JSON.stringify(manifest)) })));
    const triageTools = createResearchToolRegistry(createTriageTools(state.claims, state.runbooks, state.reports));
    const invalidPacketPath = join(state.root, 'incomplete.zip');
    await writeFile(invalidPacketPath, Buffer.from(zipSync({ 'README.md': strToU8('No proof or manifest.') })));
    outcome = await triageTools.execute({ id: 'triage-incomplete-example', toolName: 'triage.package',
      actionClass: 'synthesize', input: { claimId: finding.id, expectedRevision: finding.revision,
        packetPath: invalidPacketPath, replayRunId: 'example-portable-run', reason: 'Incomplete example packet.' } });
    assert.equal(outcome.result.status, 'error');
    assert.match(outcome.result.error.message, /README.md, manifest.json, and a poc\/ file/);
    outcome = await triageTools.execute({ id: 'triage-package-example', toolName: 'triage.package',
      actionClass: 'synthesize', input: { claimId: finding.id, expectedRevision: finding.revision,
        packetPath, replayRunId: 'example-portable-run', reason: 'The full proof replayed in a distinct example environment.' } });
    assert.equal(outcome.result.status, 'complete', outcome.result.error?.message);
    finding = outcome.result.output.finding;
    assert.deepEqual(state.funnel.reserveForSession('epoch-report', 2).map((item) => item.role), ['reporter']);
    outcome = await reportTools.execute({ id: 'report-after-triage', toolName: 'report.create',
      actionClass: 'synthesize', input: reportInput });
    assert.equal(outcome.result.status, 'complete', outcome.result.error?.message);
    assert.equal(outcome.result.output.report.submissionPacket.contentHash, finding.securityTracking.triage.packet.contentHash);
    assert.equal(outcome.result.output.sourceFinding.status, 'report_ready');
    state.funnel.finishSession('epoch-report');
    assert.deepEqual(state.funnel.reserveForSession('epoch-done', 3), []);
  } finally { await state.close(); }
});
