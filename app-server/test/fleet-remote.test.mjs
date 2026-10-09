import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { BEALE_APP_SERVER_CONTROL_VERSION } from '@beale/app-server-runtime/protocol';
import { FleetRemoteController } from '../dist/fleetRemote.js';
import { FleetWorkspaceStore } from '../dist/fleetWorkspace.js';

test('Fleet stages a workspace and returns changed files', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-route-'));
  const primary = join(root, 'primary');
  mkdirSync(primary);
  mkdirSync(join(primary, 'sources'));
  writeFileSync(join(primary, 'workspace.json'), JSON.stringify({ schemaVersion: 2, workspaceId: 'workspace-example', directories: [], checkpointIntervalMs: 600000, researchAuthority: 'files' }));
  writeFileSync(join(primary, 'note.md'), 'before');
  writeFileSync(join(primary, 'sources', 'target.txt'), 'source copy');
  const guest = new FleetWorkspaceStore({ registryDirectory: join(root, 'guest'), registerFleetWorkspace() {} });
  const primaryStore = new FleetWorkspaceStore({ registryDirectory: join(root, 'primary-registry') });
  const fleet = {
    async state() { return { role: 'primary', enabled: true, available: true, machines: [{ id: 'tart:example-worker', state: 'running', base: false, sshConfigured: true }] }; },
  };
  const controller = new FleetRemoteController(fleet, primaryStore);
  controller.connect = async () => ({ machineId: 'tart:example-worker', url: 'http://127.0.0.1:65432', operatorToken: 'test-token' });
  const previousFetch = globalThis.fetch;
  let finishInput;
  globalThis.fetch = async (_url, init) => {
    const { operation, input } = JSON.parse(init.body);
    let result;
    if (operation === 'fleet.stage' && input.action === 'finish') {
      finishInput = input;
      result = { workspaceId: input.workspaceId };
    } else {
      result = operation === 'fleet.stage' ? await guest.guestStage(input) : guest.guestExport(input);
    }
    return Response.json({ controlVersion: BEALE_APP_SERVER_CONTROL_VERSION, result });
  };
  try {
    await controller.prepare({ workspaceId: 'workspace-example', workspacePath: primary, workspaceDirectories: [primary], name: 'Example workspace', researchKitId: 'general' }, 'run-example', 'tart:example-worker');
    assert.equal(finishInput.researchKitId, 'general');
    assert.equal(readFileSync(join(root, 'guest', 'fleet-workspaces', 'workspace-example', 'note.md'), 'utf8'), 'before');
    assert.equal(readFileSync(join(root, 'guest', 'fleet-workspaces', 'workspace-example', 'sources', 'target.txt'), 'utf8'), 'source copy');
    writeFileSync(join(root, 'guest', 'fleet-workspaces', 'workspace-example', 'note.md'), 'after');
    const completed = await controller.complete('run-example');
    assert.deepEqual(completed, { imported: 1, conflicts: 0, candidateRecords: 0 });
    assert.equal(readFileSync(join(primary, 'note.md'), 'utf8'), 'after');
    assert.equal(readFileSync(join(primary, 'sources', 'target.txt'), 'utf8'), 'source copy');
  } finally {
    globalThis.fetch = previousFetch;
    rmSync(root, { recursive: true, force: true });
  }
});

test('Fleet enforces a workspace VM requirement on local sessions', async () => {
  const fleet = {
    async state() {
      return {
        role: 'primary', enabled: true,
        requiredWorkspaceIds: [], optionalWorkspaceIds: [],
        machines: [{ base: true, sshConfigured: false }],
      };
    },
  };
  const controller = new FleetRemoteController(fleet, {});
  await assert.rejects(controller.validateLocal('workspace-example', null), /requires a Fleet VM/);
  await controller.validateLocal('workspace-example', 'quick-chat');
  fleet.state = async () => ({
    role: 'primary', enabled: false, requiredWorkspaceIds: [], optionalWorkspaceIds: [], machines: [{ base: true, sshConfigured: false }],
  });
  await controller.validateLocal('workspace-example', null);
});
