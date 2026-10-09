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
    machineId() { return 'machine-example'; },
    async reserve() { return {}; },
    async release() { return {}; },
    assertReserved() {},
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

test('Fleet transfers a source workspace through another primary and preserves session ownership', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-relay-'));
  const source = join(root, 'source');
  mkdirSync(source);
  writeFileSync(join(source, 'workspace.json'), JSON.stringify({ schemaVersion: 2, workspaceId: 'workspace-example', directories: [], checkpointIntervalMs: 600000, researchAuthority: 'files' }));
  writeFileSync(join(source, 'note.md'), 'before');
  const sourceStore = new FleetWorkspaceStore({ registryDirectory: join(root, 'source-registry') });
  const relayStore = new FleetWorkspaceStore({ registryDirectory: join(root, 'relay-registry'), registerFleetWorkspace() {} });
  const calls = [];
  const fleet = {
    machineId() { return 'machine-source'; },
    async connectedRemoteAppServer() { return { url: 'https://worker.example.ts.net', operatorToken: 'synthetic-token' }; },
    async callRemote(_serverId, operation, input) {
      calls.push({ operation, input });
      if (operation === 'fleet.state') return { enabled: true, role: 'primary', machines: [{ id: 'tart:worker', base: false, sshConfigured: true, state: 'running' }] };
      if (operation === 'fleet.relay_stage') return input.action === 'finish'
        ? { workspaceId: input.workspaceId } : relayStore.guestStage(input);
      if (operation === 'fleet.relay_export') return relayStore.guestExport(input);
      if (operation === 'fleet.complete') {
        writeFileSync(join(root, 'relay-registry', 'fleet-workspaces', 'workspace-example', 'note.md'), 'after');
        return { imported: 1, conflicts: 0, candidateRecords: 0 };
      }
      return {};
    },
  };
  try {
    const controller = new FleetRemoteController(fleet, sourceStore);
    const prepared = await controller.prepare({ workspaceId: 'workspace-example', workspacePath: source, name: 'Example workspace', researchKitId: 'general' },
      'run-example', 'remote:server-example:tart:worker');
    assert.equal(prepared.remoteMachineId, 'tart:worker');
    assert.equal(prepared.ownerMachineId, 'machine-source');
    assert.equal(readFileSync(join(root, 'relay-registry', 'fleet-workspaces', 'workspace-example', 'note.md'), 'utf8'), 'before');
    const [first, second] = await Promise.all([controller.complete('run-example'), controller.complete('run-example')]);
    assert.deepEqual(first, second);
    assert.deepEqual(first, { imported: 1, conflicts: 0, candidateRecords: 0 });
    assert.equal(readFileSync(join(source, 'note.md'), 'utf8'), 'after');
    assert.equal(calls.filter((call) => call.operation === 'fleet.reserve').length, 1);
    assert.deepEqual(calls.find((call) => call.operation === 'fleet.reserve').input,
      { machineId: 'tart:worker', ownerMachineId: 'machine-source', sessionId: 'run-example', workspaceId: 'workspace-example' });
    assert.equal(calls.filter((call) => call.operation === 'fleet.complete').length, 1);
    assert.equal(calls.filter((call) => call.operation === 'fleet.release').length, 1);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
