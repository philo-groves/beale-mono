import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, readdirSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { DatabaseSync } from 'node:sqlite';
import { AppServerSessionStore, MemoryGraphStore, readWorkspaceResearchCacheState } from '@beale/research-agent';
import { initializeWorkspaceProjectAsync, runWorkspaceCheckpoint, runWorkspaceMaintenance } from '../dist/workspaceCheckpoints.js';
import { AppServerWorkerDatabaseCoordinator } from '../dist/workerDatabaseBroker.js';

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
    const memory = graph.save({ type: 'invariant', title: 'Example boundary', body: 'Original file-authority body.', status: 'suspected' });
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
    assert.equal(releasedDatabase.prepare('SELECT COUNT(*) AS count FROM app_server_sessions WHERE workspace_id=?').get(options.workspaceId).count, 1);
    releasedDatabase.close();
    rmSync(join(workspaceRoot, '.beale', 'research-cache.json'));
    const rebuilt = await runWorkspaceCheckpoint(options, 'Automatically rebuild derived research index', undefined, undefined, coordinator);
    assert.equal(rebuilt.status, 'unchanged', rebuilt.error);
    assert.equal(readWorkspaceResearchCacheState(workspaceRoot).state, 'ready');
    const rebuiltGraph = new MemoryGraphStore({ workspaceRoot, databasePath, context: { workspaceId: options.workspaceId, workspaceName: 'Example', subjectId: 'subject-example', subjectName: 'Example' } });
    assert.equal(rebuiltGraph.get(memory.id).body, 'Edited file-authority body.');
    rebuiltGraph.close();
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
