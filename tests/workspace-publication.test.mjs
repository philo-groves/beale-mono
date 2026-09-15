import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { spawnSync } from 'node:child_process';
import { DatabaseSync } from 'node:sqlite';
import {
  initializeWorkspaceProject, checkpointWorkspaceResearch, importWorkspaceResearchFile,
  releaseWorkspaceResearchIndex, rebuildWorkspaceResearchIndex, listWorkspaceResearchEdits,
  publishWorkspaceResearch, workspaceContentHash,
  MemoryGraphStore, FindingStore, RunbookStore, ReportStore, CampaignTrackStore, ResearchResourceCatalog,
  createResearchStorageLayout, ensureResearchStorageLayout,
} from '../packages/research-agent/dist/index.js';

test('publication splits and deduplicates large prior-art bodies and rebuilds their complete history', () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-publication-prior-art-example-'));
  const workspaceRoot = join(directory, 'workspace');
  const databasePath = join(directory, 'runtime', 'memory.sqlite');
  const artifactDirectoryPath = join(directory, 'runtime', 'artifacts');
  mkdirSync(workspaceRoot);
  mkdirSync(join(directory, 'runtime'));
  const options = { workspaceRoot, workspaceId: 'workspace-example', databasePath, artifactDirectoryPath };
  initializeWorkspaceProject(workspaceRoot, options.workspaceId);
  const locator = 'https://example.test/reference';
  const catalog = new ResearchResourceCatalog({
    databasePath,
    workspaceId: options.workspaceId,
    explicitResources: [{ id: 'asset-example', kind: 'documentation', direction: 'in_scope', locator, source: 'explicit_scope' }],
  });
  let catalogOpen = true;
  try {
    const resourceId = catalog.list()[0].id;
    const body = 'a'.repeat(6 * 1024 * 1024);
    const document = {
      requestedUrl: locator, url: locator, fetchedAt: '2026-09-01T00:00:00.000Z', contentType: 'text/plain',
      contentHash: workspaceContentHash(body), etag: null, lastModified: null, title: 'Example reference', text: body, links: [],
    };
    catalog.priorArt.save(resourceId, 'action-example-one', document, { revision: 'build-example-001' });
    catalog.priorArt.save(resourceId, 'action-example-two', { ...document, fetchedAt: '2026-09-02T00:00:00.000Z' }, { revision: 'build-example-001' });

    const checkpoint = checkpointWorkspaceResearch(options, 'Publish bounded prior-art payloads');
    assert.equal(checkpoint.status, 'committed', checkpoint.error);
    const resources = JSON.parse(readFileSync(join(workspaceRoot, 'references', 'resources.json'), 'utf8'));
    assert.equal(resources.schemaVersion, 2);
    assert.equal(resources.priorArt.length, 2);
    assert.equal(resources.priorArt[0].data_json, undefined);
    assert.equal(resources.priorArt[0].dataRef.path, resources.priorArt[1].dataRef.path);
    assert.equal(readFileSync(join(workspaceRoot, resources.priorArt[0].dataRef.path), 'utf8').length > 5 * 1024 * 1024, true);
    assert.equal(readFileSync(join(workspaceRoot, 'references', 'resources.json')).length < 128 * 1024, true);

    catalog.close();
    catalogOpen = false;
    releaseWorkspaceResearchIndex(options);
    rebuildWorkspaceResearchIndex(options);
    const database = new DatabaseSync(databasePath, { readOnly: true });
    try {
      const rows = database.prepare('SELECT data_json FROM resource_prior_art WHERE workspace_id=? ORDER BY action_id').all(options.workspaceId);
      assert.equal(rows.length, 2);
      assert.equal(JSON.parse(rows[0].data_json).text.length, body.length);
      assert.equal(JSON.parse(rows[1].data_json).fetchedAt, '2026-09-02T00:00:00.000Z');
    } finally { database.close(); }
  } finally {
    if (catalogOpen) catalog.close();
    rmSync(directory, { recursive: true, force: true });
  }
});

test('publication preserves directory evidence references without reading them as files', () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-publication-directory-evidence-example-'));
  const workspaceRoot = join(directory, 'workspace');
  const databasePath = join(directory, 'runtime', 'memory.sqlite');
  const artifactDirectoryPath = join(directory, 'runtime', 'artifacts');
  mkdirSync(workspaceRoot);
  const options = { workspaceRoot, workspaceId: 'workspace-example', databasePath, artifactDirectoryPath };
  initializeWorkspaceProject(workspaceRoot, options.workspaceId);
  const evidenceDirectory = join(workspaceRoot, 'investigations', 'investigation-example', 'captured-output');
  mkdirSync(evidenceDirectory, { recursive: true });
  writeFileSync(join(evidenceDirectory, 'result.txt'), 'synthetic result\n');
  const context = { workspaceId: options.workspaceId, workspaceName: 'Example Research', subjectId: 'subject-example', subjectName: 'Example Subject' };
  const graph = new MemoryGraphStore({ workspaceRoot, databasePath, context });
  try {
    const memory = graph.save({
      type: 'invariant',
      title: 'Directory evidence reference example',
      body: 'A synthetic result collection is retained by its workspace path.',
      status: 'suspected',
      evidence: [{
        kind: 'command',
        pathBase: 'workspace',
        path: 'investigations/investigation-example/captured-output',
        locator: {},
        summary: 'Directory containing synthetic command output.',
      }],
    });
    const checkpoint = checkpointWorkspaceResearch(options, 'Publish directory evidence reference');
    assert.equal(checkpoint.status, 'committed', checkpoint.error);
    const published = readFileSync(join(workspaceRoot, 'memories', `${memory.id}.md`), 'utf8');
    assert.match(published, /investigations\/investigation-example\/captured-output/u);
  } finally {
    graph.close();
    rmSync(directory, { recursive: true, force: true });
  }
});

test('publication preserves a workspace-root evidence reference without rejecting the checkpoint', () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-publication-root-evidence-example-'));
  const workspaceRoot = join(directory, 'workspace');
  const databasePath = join(directory, 'runtime', 'memory.sqlite');
  const artifactDirectoryPath = join(directory, 'runtime', 'artifacts');
  mkdirSync(workspaceRoot);
  const options = { workspaceRoot, workspaceId: 'workspace-example', databasePath, artifactDirectoryPath };
  initializeWorkspaceProject(workspaceRoot, options.workspaceId);
  const context = { workspaceId: options.workspaceId, workspaceName: 'Example Research', subjectId: 'subject-example', subjectName: 'Example Subject' };
  const graph = new MemoryGraphStore({ workspaceRoot, databasePath, context });
  try {
    const memory = graph.save({
      type: 'trajectory',
      title: 'Workspace-root command example',
      body: 'A synthetic command ran from the research workspace root.',
      status: 'suspected',
      evidence: [{
        kind: 'command',
        pathBase: 'workspace',
        path: '.',
        locator: {},
        summary: 'The command used the workspace root as its working directory.',
      }],
    });
    const checkpoint = checkpointWorkspaceResearch(options, 'Publish workspace-root evidence reference');
    assert.equal(checkpoint.status, 'committed', checkpoint.error);
    const published = readFileSync(join(workspaceRoot, 'memories', `${memory.id}.md`), 'utf8');
    assert.match(published, /"path": "\."/u);
  } finally {
    graph.close();
    rmSync(directory, { recursive: true, force: true });
  }
});

test('publication gives opaque legacy record IDs stable filesystem-safe paths', () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-publication-opaque-id-example-'));
  const workspaceRoot = join(directory, 'workspace');
  const databasePath = join(directory, 'runtime', 'memory.sqlite');
  const artifactDirectoryPath = join(directory, 'runtime', 'artifacts');
  mkdirSync(workspaceRoot);
  const options = { workspaceRoot, workspaceId: 'workspace-example', databasePath, artifactDirectoryPath };
  initializeWorkspaceProject(workspaceRoot, options.workspaceId);
  const context = { workspaceId: options.workspaceId, workspaceName: 'Example Research', subjectId: 'subject-example', subjectName: 'Example Subject' };
  const graph = new MemoryGraphStore({ workspaceRoot, databasePath, context });
  const opaqueId = 'legacy:example/memory';
  try {
    const memory = graph.save({
      id: opaqueId,
      type: 'invariant',
      title: 'Legacy opaque identifier example',
      body: 'Original synthetic body.',
      status: 'suspected',
    });
    const checkpoint = checkpointWorkspaceResearch(options, 'Publish opaque legacy record');
    assert.equal(checkpoint.status, 'committed', checkpoint.error);

    const memoryPath = `memories/encoded-${workspaceContentHash(opaqueId)}.md`;
    const absoluteMemoryPath = join(workspaceRoot, memoryPath);
    const published = readFileSync(absoluteMemoryPath, 'utf8');
    assert.match(published, /"id": "legacy:example\/memory"/u);
    assert.equal(existsSync(join(workspaceRoot, 'memories', 'legacy:example', 'memory.md')), false);

    writeFileSync(absoluteMemoryPath, published.replace('Original synthetic body.', 'Revised synthetic body.'));
    importWorkspaceResearchFile(options, memoryPath, memory.revision);
    assert.equal(graph.get(opaqueId).body, 'Revised synthetic body.');
    assert.equal(checkpointWorkspaceResearch(options, 'Publish revised opaque legacy record').status, 'committed');
  } finally {
    graph.close();
    rmSync(directory, { recursive: true, force: true });
  }
});

test('completed execution evidence remains pinned when its workspace candidate is revised', () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-publication-pinned-example-'));
  const workspaceRoot = join(directory, 'workspace');
  const databasePath = join(directory, 'runtime', 'memory.sqlite');
  const artifactDirectoryPath = join(directory, 'runtime', 'artifacts');
  mkdirSync(workspaceRoot);
  const options = { workspaceRoot, workspaceId: 'workspace-example', databasePath, artifactDirectoryPath };
  initializeWorkspaceProject(workspaceRoot, options.workspaceId);
  const layout = ensureResearchStorageLayout(createResearchStorageLayout(options));
  const context = { workspaceId: options.workspaceId, workspaceName: 'Example Research', subjectId: 'subject-example', subjectName: 'Example Subject' };
  const runbooks = new RunbookStore(databasePath, layout, context);
  const candidatePath = 'investigations/investigation-example/proof.sh';
  const candidate = join(workspaceRoot, candidatePath);
  mkdirSync(join(workspaceRoot, 'investigations', 'investigation-example'), { recursive: true });
  writeFileSync(candidate, '#!/bin/sh\necho first-example\n');
  try {
    const created = runbooks.create({
      title: 'Pinned evidence example',
      purpose: 'Retain the exact candidate used by a completed execution.',
      cells: [{ kind: 'code', source: 'Execute the synthetic candidate.', features: ['runtime'], language: 'sh' }],
    }).runbook;
    const cell = runbooks.get(created.id).cells.find((entry) => entry.kind === 'code');
    runbooks.configure({
      id: created.id,
      expectedRevision: created.revision,
      enabledFeatures: ['runtime'],
      cellExecutors: [{
        cellId: cell.id,
        executor: { kind: 'tart-vm', vmName: 'example-vm', workspacePath: candidatePath, runAs: 'guest', argv: [] },
      }],
    });
    const runId = 'runbook_run_pinned-example';
    const startedAt = new Date().toISOString();
    runbooks.beginExecution(created.id, runId, [cell.id], 'vm');
    runbooks.beginCellExecution(created.id, runId, cell.id, 'vm');
    const firstHash = workspaceContentHash(readFileSync(candidate));
    runbooks.completeCellExecution({
      id: created.id,
      runId,
      cellId: cell.id,
      status: 'succeeded',
      startedAt,
      completedAt: new Date().toISOString(),
      durationMs: 1,
      stdout: `${firstHash}  ${candidate}\n`,
      exitCode: 0,
      proofTarget: 'vm',
    });
    runbooks.completeExecution({
      id: created.id,
      runId,
      status: 'succeeded',
      startedAt,
      completedAt: new Date().toISOString(),
      durationMs: 1,
      proofTarget: 'vm',
    });

    const first = checkpointWorkspaceResearch(options, 'Publish completed execution');
    assert.equal(first.status, 'committed', first.error);
    const executionPath = join(workspaceRoot, 'evidence', `execution-${runId}.json`);
    const pinnedExecution = readFileSync(executionPath, 'utf8');
    assert.match(pinnedExecution, new RegExp(`evidence/raw/${firstHash}`));
    assert.doesNotMatch(pinnedExecution, new RegExp(candidate.replace(/[.*+?^${}()|[\]\\]/gu, '\\$&')));

    writeFileSync(candidate, '#!/bin/sh\necho second-example\n');
    const secondHash = workspaceContentHash(readFileSync(candidate));
    publishWorkspaceResearch(options);
    assert.equal(readFileSync(executionPath, 'utf8'), pinnedExecution);
    assert.deepEqual(listWorkspaceResearchEdits(workspaceRoot), []);
    const second = checkpointWorkspaceResearch(options, 'Publish revised candidate');
    assert.equal(second.status, 'committed', second.error);
    assert.equal(readFileSync(executionPath, 'utf8'), pinnedExecution);
    assert.ok(existsSync(join(workspaceRoot, 'evidence', `${firstHash}.json`)));
    assert.ok(existsSync(join(workspaceRoot, 'evidence', `${secondHash}.json`)));
    assert.deepEqual(listWorkspaceResearchEdits(workspaceRoot), []);
  } finally {
    runbooks.close();
    rmSync(directory, { recursive: true, force: true });
  }
});

test('canonical snapshots isolate workspace records and imports preserve revision/evidence validation', () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-publication-example-'));
  const workspaceRoot = join(directory, 'workspace');
  const databasePath = join(directory, 'runtime', 'memory.sqlite');
  const artifactDirectoryPath = join(directory, 'runtime', 'artifacts');
  mkdirSync(workspaceRoot);
  const options = { workspaceRoot, workspaceId: 'workspace-example', databasePath, artifactDirectoryPath };
  initializeWorkspaceProject(workspaceRoot, options.workspaceId);
  const layout = ensureResearchStorageLayout(createResearchStorageLayout(options));
  const context = { workspaceId: options.workspaceId, workspaceName: 'Example Research', subjectId: 'subject-example', subjectName: 'Example Subject' };
  const graph = new MemoryGraphStore({ workspaceRoot, databasePath, context });
  const other = new MemoryGraphStore({ workspaceRoot, databasePath, context: { ...context, workspaceId: 'workspace-other', subjectId: 'subject-other' } });
  const claims = new FindingStore(graph);
  const runbooks = new RunbookStore(databasePath, layout, context);
  const reports = new ReportStore(databasePath, layout, context);
  try {
    const memory = graph.save({ type: 'invariant', title: 'Example parser boundary', summary: 'An example observation.', body: 'Original explanation.', status: 'suspected' });
    const foreign = other.save({ type: 'invariant', title: 'Other workspace knowledge', status: 'suspected' });
    const claim = claims.create({ title: 'Example hypothesis', summary: 'A synthetic hypothesis.', classification: 'security.primitive', rating: 'medium' });
    const runbook = runbooks.create({ title: 'Example procedure', purpose: 'Repeat the example.', cells: [{ kind: 'code', source: 'echo example', features: ['runtime'], language: 'shell' }] }).runbook;
    const report = reports.create({ title: 'Example report', summary: 'Example summary.', content: '# Example report\n' }).report;
    const checkpoint = checkpointWorkspaceResearch(options, 'Canonical research checkpoint');
    assert.equal(checkpoint.status, 'committed', checkpoint.error);
    assert.ok(existsSync(join(workspaceRoot, 'memories', `${memory.id}.md`)));
    assert.equal(existsSync(join(workspaceRoot, 'memories', `${foreign.id}.md`)), false);
    assert.equal(checkpointWorkspaceResearch(options, 'Unchanged publication').status, 'unchanged');

    const claimPath = `claims/${claim.id}.json`;
    const exported = JSON.parse(readFileSync(join(workspaceRoot, claimPath), 'utf8'));
    const publishedClaim = readFileSync(join(workspaceRoot, claimPath), 'utf8');
    exported.updated_at = '2026-09-01T00:00:00.000Z';
    writeFileSync(join(workspaceRoot, claimPath), JSON.stringify(exported));
    importWorkspaceResearchFile(options, claimPath, claim.revision);
    assert.equal(readFileSync(join(workspaceRoot, claimPath), 'utf8'), publishedClaim);
    assert.equal(claims.get(claim.id).revision, claim.revision);

    exported.updated_at = JSON.parse(publishedClaim).updated_at;
    exported.status = 'verified';
    writeFileSync(join(workspaceRoot, claimPath), JSON.stringify(exported));
    assert.throws(() => importWorkspaceResearchFile(options, claimPath, claim.revision), /host-managed/);
    assert.equal(claims.get(claim.id).status, 'hypothesis');
    exported.status = 'hypothesis';
    exported.summary = 'An explicitly revised hypothesis.';
    writeFileSync(join(workspaceRoot, claimPath), JSON.stringify(exported));
    importWorkspaceResearchFile(options, claimPath, claim.revision);
    assert.equal(claims.get(claim.id).summary, exported.summary);
    assert.equal(checkpointWorkspaceResearch(options, 'Imported claim').status, 'committed');

    const memoryPath = `memories/${memory.id}.md`;
    const publishedMemory = readFileSync(join(workspaceRoot, memoryPath), 'utf8');
    const driftedMemory = publishedMemory.replace(/"updated_at": "[^"]+"/u, '"updated_at": "2026-09-01T00:00:00.000Z"');
    assert.notEqual(driftedMemory, publishedMemory);
    writeFileSync(join(workspaceRoot, memoryPath), driftedMemory);
    importWorkspaceResearchFile(options, memoryPath, memory.revision);
    assert.equal(readFileSync(join(workspaceRoot, memoryPath), 'utf8'), publishedMemory);
    assert.equal(graph.get(memory.id).revision, memory.revision);

    writeFileSync(join(workspaceRoot, memoryPath), readFileSync(join(workspaceRoot, memoryPath), 'utf8').replace('Original explanation.', 'Revised explanation.'));
    importWorkspaceResearchFile(options, memoryPath, memory.revision);
    assert.equal(graph.get(memory.id).body, 'Revised explanation.');
    assert.equal(checkpointWorkspaceResearch(options, 'Imported memory').status, 'committed');

    const reportPath = `reports/${report.id}/report.md`;
    writeFileSync(join(workspaceRoot, reportPath), '# Revised example report\n');
    importWorkspaceResearchFile(options, reportPath, report.revision);
    assert.match(reports.get(report.id).content, /Revised/);
    assert.equal(checkpointWorkspaceResearch(options, 'Imported report').status, 'committed');

    const runbookPath = `runbooks/${runbook.id}/runbook.ipynb`;
    const notebook = JSON.parse(readFileSync(join(workspaceRoot, runbookPath), 'utf8'));
    notebook.cells[1].source = ['echo revised-example'];
    writeFileSync(join(workspaceRoot, runbookPath), JSON.stringify(notebook));
    importWorkspaceResearchFile(options, runbookPath, runbook.revision);
    assert.equal(runbooks.get(runbook.id).contentRevision, runbook.contentRevision + 1);
    assert.equal(checkpointWorkspaceResearch(options, 'Imported procedure').status, 'committed');
    const tracks = new CampaignTrackStore({ databasePath, context });
    try {
      const track = tracks.ensureForSession({ sessionId: 'session-example', objective: 'Example investigation', source: 'runtime' });
      const attributed = checkpointWorkspaceResearch({ ...options, sessionId: 'session-example' }, 'Linked investigation checkpoint');
      assert.equal(attributed.status, 'committed', attributed.error);
      const investigation = JSON.parse(readFileSync(join(workspaceRoot, 'investigations', track.id, 'record.json'), 'utf8'));
      assert.equal(investigation.schemaVersion, 2);
      assert.deepEqual(investigation.sessions.map((entry) => entry.session_id), ['session-example']);
      for (const field of ['resources', 'questions', 'experiments', 'observations', 'nextActions', 'memoryClaimReviews', 'researchClaimReviews']) {
        assert.ok(Array.isArray(investigation[field]), field);
      }
      assert.ok(existsSync(join(workspaceRoot, 'references', 'campaign-state.json')));
      const message = spawnSync('git', ['log', '-1', '--format=%B'], { cwd: workspaceRoot, encoding: 'utf8', windowsHide: true }).stdout.trim();
      assert.equal(message, `Linked investigation checkpoint\n\nInvestigation-ID: ${track.id}\nSession-ID: session-example`);

      const released = releaseWorkspaceResearchIndex(options);
      assert.equal(released.state, 'released');
      const releasedDatabase = new DatabaseSync(databasePath, { readOnly: true });
      for (const table of ['memory_node_workspaces', 'app_server_research_claims', 'app_server_runbooks', 'app_server_reports', 'campaign_tracks']) {
        const clause = table === 'memory_node_workspaces' ? 'workspace_id=?' : 'workspace_id=?';
        assert.equal(releasedDatabase.prepare(`SELECT COUNT(*) AS count FROM ${table} WHERE ${clause}`).get(options.workspaceId).count, 0, table);
      }
      releasedDatabase.close();
      const rebuilt = rebuildWorkspaceResearchIndex(options);
      assert.equal(rebuilt.state, 'ready');
      assert.equal(graph.get(memory.id).body, 'Revised explanation.');
      assert.equal(claims.get(claim.id).summary, exported.summary);
      assert.match(reports.get(report.id).content, /Revised/);
      assert.equal(runbooks.get(runbook.id).contentRevision, runbook.contentRevision + 1);
      assert.equal(tracks.get(track.id).id, track.id);
    } finally { tracks.close(); }
  } finally {
    reports.close(); runbooks.close(); claims.close(); graph.close(); other.close();
    rmSync(directory, { recursive: true, force: true });
  }
});
