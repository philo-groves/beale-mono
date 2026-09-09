import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, mkdirSync, writeFileSync, readdirSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { MemoryGraphStore } from '@beale/research-agent';
import { initializeWorkspaceProjectAsync, runWorkspaceCheckpoint, runWorkspaceMaintenance } from '../dist/workspaceCheckpoints.js';

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
    graph.save({ type: 'invariant', title: 'Example boundary', status: 'suspected' });
    graph.close();
    const before = ticks;
    const results = await Promise.all([runWorkspaceCheckpoint(options, 'First checkpoint'), runWorkspaceCheckpoint(options, 'Queued checkpoint')]);
    assert.equal(results[0].status, 'committed', results[0].error);
    assert.equal(results[1].status, 'unchanged', results[1].error);
    assert.ok(ticks > before, 'Git publication must yield the host event loop');
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
