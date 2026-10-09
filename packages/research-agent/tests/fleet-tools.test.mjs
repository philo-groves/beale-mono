import assert from 'node:assert/strict';
import test from 'node:test';
import { createFleetTools } from '../dist/fleet-tools.js';

test('Fleet model tools expose VM status without SSH metadata', async () => {
  const calls = [];
  const tools = createFleetTools(async (operation, input) => {
    calls.push({ operation, input });
    return { enabled: true, available: true, machines: [{
      id: 'tart:example-worker', name: 'example-worker', backend: 'tart', state: 'stopped', base: false,
      privilege: 'standard', sshConfigured: true, sshHost: '192.0.2.10', sshUser: 'example',
    }] };
  });
  const list = tools.find((tool) => tool.descriptor.name === 'fleet.list');
  const result = await list.execute({ id: 'action-example', toolName: 'fleet.list', input: {} });
  assert.equal(result.status, 'complete');
  assert.equal(result.output.machines[0].sshHost, undefined);
  assert.equal(result.output.machines[0].sshUser, undefined);
  const clone = tools.find((tool) => tool.descriptor.name === 'fleet.clone');
  await clone.execute({ id: 'action-clone', toolName: 'fleet.clone', input: { baseId: 'tart:example-base', name: 'example-copy' } });
  assert.deepEqual(calls[1], { operation: 'fleet.clone', input: { baseId: 'tart:example-base', name: 'example-copy' } });
});
