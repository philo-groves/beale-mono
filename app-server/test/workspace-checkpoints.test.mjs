import assert from 'node:assert/strict';
import { test } from 'node:test';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, writeFileSync, readdirSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { DatabaseSync } from 'node:sqlite';
import { AppServerSessionStore, MemoryGraphStore, ResearchResourceCatalog, checkpointWorkspace, listWorkspaceResearchEdits, publishWorkspaceFiles, publishWorkspaceResearch, readWorkspaceResearchCacheState, workspaceContentHash } from '@beale/research-agent';
import { initializeWorkspaceProjectAsync, previewWorkspaceCheckpointRepair, runWorkspaceCheckpoint, runWorkspaceMaintenance } from '../dist/workspaceCheckpoints.js';
import { AppServerWorkerDatabaseCoordinator } from '../dist/workerDatabaseBroker.js';

test('checkpoint worker previews and repairs oversized untracked investigation files', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-checkpoint-repair-example-'));
  const workspaceRoot = join(directory, 'workspace');
  const options = { workspaceRoot, workspaceId: 'workspace-example', databasePath: join(directory, 'runtime', 'memory.sqlite'), artifactDirectoryPath: join(directory, 'runtime', 'artifacts') };
  try {
    await initializeWorkspaceProjectAsync(workspaceRoot, options.workspaceId);
    mkdirSync(options.artifactDirectoryPath, { recursive: true });
    const path = join(workspaceRoot, 'investigations', 'example', 'generated.bin');
    mkdirSync(join(workspaceRoot, 'investigations', 'example'), { recursive: true });
    writeFileSync(path, Buffer.alloc(5 * 1024 * 1024 + 1));
    const failed = await runWorkspaceCheckpoint(options, 'Preflight oversized file');
    assert.equal(failed.status, 'failed');
    const preview = await previewWorkspaceCheckpointRepair(workspaceRoot);
    assert.deepEqual(preview, failed.repair);
    const repaired = await runWorkspaceCheckpoint(options, 'Repair oversized file', undefined, undefined, undefined, { repairFingerprint: preview.fingerprint });
    assert.equal(repaired.status, 'committed', repaired.error);
    assert.equal(existsSync(path), false);
    assert.equal(existsSync(join(workspaceRoot, preview.candidates[0].destinationPath)), true);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('session checkpointing automatically migrates an oversized monolithic prior-art export', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-checkpoint-prior-art-recovery-'));
  const workspaceRoot = join(directory, 'workspace');
  const databasePath = join(directory, 'runtime', 'memory.sqlite');
  const artifactDirectoryPath = join(directory, 'runtime', 'artifacts');
  const options = { workspaceRoot, workspaceId: 'workspace-example', databasePath, artifactDirectoryPath };
  try {
    await initializeWorkspaceProjectAsync(workspaceRoot, options.workspaceId);
    mkdirSync(join(directory, 'runtime'), { recursive: true });
    mkdirSync(artifactDirectoryPath, { recursive: true });
    const locator = 'https://example.test/reference';
    const catalog = new ResearchResourceCatalog({
      databasePath,
      workspaceId: options.workspaceId,
      explicitResources: [{ id: 'asset-example', kind: 'documentation', direction: 'in_scope', locator, source: 'explicit_scope' }],
    });
    try {
      const body = 'a'.repeat(6 * 1024 * 1024);
      catalog.priorArt.save(catalog.list()[0].id, 'action-example', {
        requestedUrl: locator, url: locator, fetchedAt: '2026-09-01T00:00:00.000Z', contentType: 'text/plain',
        contentHash: workspaceContentHash(body), etag: null, lastModified: null, title: 'Example reference', text: body, links: [],
      });
    } finally { catalog.close(); }
    const database = new DatabaseSync(databasePath, { readOnly: true });
    let legacyResources;
    try {
      legacyResources = JSON.stringify({
        schemaVersion: 1,
        workspaceId: options.workspaceId,
        resources: database.prepare('SELECT * FROM app_server_research_resources WHERE workspace_id=?').all(options.workspaceId),
        priorArt: database.prepare('SELECT * FROM resource_prior_art WHERE workspace_id=?').all(options.workspaceId),
      }, null, 2) + '\n';
    } finally { database.close(); }
    assert.equal(Buffer.byteLength(legacyResources) > 5 * 1024 * 1024, true);
    publishWorkspaceFiles(workspaceRoot, { 'references/resources.json': legacyResources });
    assert.equal(checkpointWorkspace(workspaceRoot, 'Simulate legacy prior-art publication').status, 'committed');

    const recovered = await runWorkspaceCheckpoint(options, 'Before research session');
    assert.equal(recovered.status, 'committed', recovered.error);
    const resources = JSON.parse(readFileSync(join(workspaceRoot, 'references', 'resources.json'), 'utf8'));
    assert.equal(resources.schemaVersion, 2);
    assert.equal(resources.priorArt[0].data_json, undefined);
    assert.match(resources.priorArt[0].dataRef.path, /^references\/prior-art\/[a-f0-9]{64}\.json$/u);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('session checkpoint recovers an interrupted publication before checking direct research edits', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-checkpoint-interrupted-publication-example-'));
  const workspaceRoot = join(directory, 'workspace');
  const databasePath = join(directory, 'runtime', 'memory.sqlite');
  const artifactDirectoryPath = join(directory, 'runtime', 'artifacts');
  const options = { workspaceRoot, workspaceId: 'workspace-example', databasePath, artifactDirectoryPath };
  try {
    await initializeWorkspaceProjectAsync(workspaceRoot, options.workspaceId);
    mkdirSync(artifactDirectoryPath, { recursive: true });
    const graph = new MemoryGraphStore({
      workspaceRoot,
      databasePath,
      context: { workspaceId: options.workspaceId, workspaceName: 'Example', subjectId: 'subject-example', subjectName: 'Example' }
    });
    let memory;
    try {
      memory = graph.save({ type: 'invariant', title: 'Example boundary', body: 'Original example.', status: 'suspected' });
      assert.equal((await runWorkspaceCheckpoint(options, 'Initial publication')).status, 'committed');
      graph.correct(memory.id, memory.revision, { body: 'Revised example.' });
    } finally { graph.close(); }

    const publicationPath = join(workspaceRoot, '.git', 'beale', 'publication.json');
    const indexPath = join(workspaceRoot, 'references', 'research-index.json');
    const previousPublication = readFileSync(publicationPath, 'utf8');
    const previousIndex = readFileSync(indexPath, 'utf8');
    publishWorkspaceResearch(options);

    const nextIndex = JSON.parse(readFileSync(publicationPath, 'utf8'));
    const files = Object.fromEntries(Object.keys(nextIndex.files).map((path) => [path, readFileSync(join(workspaceRoot, path), 'utf8')]));
    writeFileSync(publicationPath, previousPublication);
    writeFileSync(indexPath, previousIndex);
    const pendingPath = join(workspaceRoot, '.git', 'beale', 'pending-publication.json');
    writeFileSync(pendingPath, JSON.stringify({ files, index: nextIndex }));
    assert.deepEqual(listWorkspaceResearchEdits(workspaceRoot).map((edit) => edit.state), ['modified']);

    const recovered = await runWorkspaceCheckpoint(options, 'Before research session');
    assert.equal(recovered.status, 'committed', recovered.error);
    assert.equal(existsSync(pendingPath), false);
    assert.deepEqual(listWorkspaceResearchEdits(workspaceRoot), []);
    assert.match(readFileSync(join(workspaceRoot, 'memories', `${memory.id}.md`), 'utf8'), /Revised example\./);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('workspace creation, queued checkpoints, and housekeeping keep the host event loop responsive', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-checkpoint-worker-'));
  const workspaceRoot = join(directory, 'workspace');
  const databasePath = join(directory, 'runtime', 'memory.sqlite');
  const artifactDirectoryPath = join(directory, 'runtime', 'artifacts');
  const options = { workspaceRoot, workspaceId: 'workspace-example', databasePath, artifactDirectoryPath };
  let ticks = 0;
  const timer = setInterval(() => ticks++, 10);
  try {
    await initializeWorkspaceProjectAsync(workspaceRoot, options.workspaceId);
    assert.ok(ticks > 0, 'initial Git setup must yield the host event loop');
    mkdirSync(artifactDirectoryPath, { recursive: true });
    const graph = new MemoryGraphStore({ workspaceRoot, databasePath, context: { workspaceId: options.workspaceId, workspaceName: 'Example', subjectId: 'subject-example', subjectName: 'Example' } });
    const memory = graph.save({ id: 'memory-z-parent-example', type: 'invariant', title: 'Example boundary', body: 'Original file-authority body.', status: 'suspected' });
    const duplicateMemory = graph.save({ id: 'memory-a-duplicate-example', type: 'invariant', title: 'Repeated example boundary', body: 'Duplicate file-authority body.', status: 'suspected' });
    graph.markDuplicate(duplicateMemory.id, { expectedRevision: duplicateMemory.revision, parentMemoryId: memory.id, reason: 'Synthetic duplicate for index ordering coverage.' });
    graph.close();
    const sessions = new AppServerSessionStore({ databasePath });
    sessions.create({ id: 'session-example', workspaceId: options.workspaceId, attemptId: 'attempt-example', title: 'Example session', prompt: 'Inspect the example.', provider: 'openai-codex', model: 'gpt-example', reasoningEffort: 'high' });
    sessions.close();
    mkdirSync(join(workspaceRoot, 'investigations', 'investigation-example'), { recursive: true });
    writeFileSync(join(workspaceRoot, 'investigations', 'investigation-example', 'proof-plan.md'), 'Synthetic file-native proof plan.\n');
    const before = ticks;
    const coordinator = new AppServerWorkerDatabaseCoordinator();
    let brokeredRequests = 0;
    const dispatch = coordinator.dispatch.bind(coordinator);
    coordinator.dispatch = (request) => { brokeredRequests += 1; dispatch(request); };
    const results = await Promise.all([
      runWorkspaceCheckpoint(options, 'First checkpoint', undefined, undefined, coordinator),
      runWorkspaceCheckpoint(options, 'Queued checkpoint', undefined, undefined, coordinator),
    ]);
    assert.equal(results[0].status, 'committed', results[0].error);
    assert.equal(results[1].status, 'unchanged', results[1].error);
    assert.ok(brokeredRequests > 0, 'file-authority checkpoints must refresh their app-server-owned derived index projection');
    assert.ok(ticks > before, 'Git checkpointing must yield the host event loop');
    const memoryPath = join(workspaceRoot, 'memories', `${memory.id}.md`);
    writeFileSync(memoryPath, readFileSync(memoryPath, 'utf8').replace('Original file-authority body.', 'Edited file-authority body.'));
    const imported = await runWorkspaceCheckpoint(options, 'Import validated file-authority edit', undefined, undefined, coordinator);
    assert.equal(imported.status, 'committed', imported.error);
    const refreshed = new MemoryGraphStore({ workspaceRoot, databasePath, context: { workspaceId: options.workspaceId, workspaceName: 'Example', subjectId: 'subject-example', subjectName: 'Example' } });
    assert.equal(refreshed.get(memory.id).body, 'Edited file-authority body.');
    refreshed.close();
    const exported = await runWorkspaceCheckpoint(options, 'Explicit research synchronization', undefined, undefined, coordinator, { exportResearch: true });
    assert.equal(exported.status, 'unchanged', exported.error);
    const released = await runWorkspaceCheckpoint(options, 'Release derived research index', undefined, undefined, coordinator, { researchIndexAction: 'release' });
    assert.equal(released.status, 'unchanged', released.error);
    assert.equal(released.researchIndex.state, 'released');
    const releasedDatabase = new DatabaseSync(databasePath, { readOnly: true });
    assert.equal(releasedDatabase.prepare('SELECT COUNT(*) AS count FROM memory_node_workspaces WHERE workspace_id=?').get(options.workspaceId).count, 0);
    assert.equal(releasedDatabase.prepare('SELECT COUNT(*) AS count FROM memory_nodes WHERE id=?').get(memory.id).count, 0);
    assert.equal(releasedDatabase.prepare('SELECT COUNT(*) AS count FROM memory_nodes WHERE id=?').get(duplicateMemory.id).count, 0);
    assert.equal(releasedDatabase.prepare('SELECT COUNT(*) AS count FROM app_server_sessions WHERE workspace_id=?').get(options.workspaceId).count, 1);
    releasedDatabase.close();
    rmSync(join(workspaceRoot, '.beale', 'research-cache.json'));
    const rebuilt = await runWorkspaceCheckpoint(options, 'Automatically rebuild derived research index', undefined, undefined, coordinator);
    assert.equal(rebuilt.status, 'unchanged', rebuilt.error);
    assert.equal(readWorkspaceResearchCacheState(workspaceRoot).state, 'ready');
    const rebuiltGraph = new MemoryGraphStore({ workspaceRoot, databasePath, context: { workspaceId: options.workspaceId, workspaceName: 'Example', subjectId: 'subject-example', subjectName: 'Example' } });
    assert.equal(rebuiltGraph.get(memory.id).body, 'Edited file-authority body.');
    assert.equal(rebuiltGraph.get(duplicateMemory.id).duplicateOfMemoryId, memory.id);
    rebuiltGraph.close();
    writeFileSync(join(workspaceRoot, 'evidence', 'candidate-verifier-example.json'), '{"result":"candidate"}\n');
    const candidateEvidence = await runWorkspaceCheckpoint(options, 'Checkpoint candidate evidence', undefined, undefined, coordinator);
    assert.equal(candidateEvidence.status, 'committed', candidateEvidence.error);
    const untypedClaim = join(workspaceRoot, 'claims', 'claim-untyped-example.json');
    writeFileSync(untypedClaim, '{"revision":1}');
    const rejected = await runWorkspaceCheckpoint(options, 'Reject untyped record creation', undefined, undefined, coordinator);
    assert.equal(rejected.status, 'failed');
    assert.match(rejected.error, /typed research creation/);
    rmSync(untypedClaim);
    writeFileSync(join(workspaceRoot, 'scratch', 'example.txt'), 'incomplete research');
    const maintenance = await runWorkspaceMaintenance({ workspacePath: workspaceRoot });
    assert.equal(maintenance.lastRun.status, 'completed');
    assert.equal(maintenance.lastRun.movedFileCount, 1);
    assert.deepEqual(readdirSync(join(workspaceRoot, 'scratch')), []);
  } finally {
    clearInterval(timer);
    rmSync(directory, { recursive: true, force: true });
  }
});
