import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { FleetService } from '../dist/fleet.js';

test('Fleet registers only operator-designated bases and never runs one directly', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-test-'));
  const machines = new Map([['example-base', false]]);
  const launched = [];
  const runner = {
    async run(command, args) {
      assert.equal(command, 'tart');
      if (args[0] === 'list') return JSON.stringify([...machines].map(([Name, Running]) => ({ Name, Running })));
      if (args[0] === 'clone') {
        machines.set(args[2], false);
        return '';
      }
      if (args[0] === 'stop') {
        machines.set(args[1], false);
        return '';
      }
      throw new Error('Unexpected test command.');
    },
    launch(command, args) { launched.push({ command, args }); machines.set(args[1], true); },
  };
  try {
    const fleet = new FleetService(join(root, 'fleet.json'), 'darwin', runner);
    assert.equal((await fleet.state()).machines[0]?.base, false);
    await assert.rejects(fleet.clone('tart:example-base', 'example-copy'), /registered base/);
    await fleet.configure({ action: 'set-machine', machineId: 'tart:example-base', base: true, privilege: 'elevated' });
    await assert.rejects(fleet.start('tart:example-base'), /runnable/);
    const cloned = await fleet.clone('tart:example-base', 'example-copy');
    assert.equal(cloned.machines.find((machine) => machine.name === 'example-copy')?.base, false);
    assert.equal(cloned.machines.find((machine) => machine.name === 'example-copy')?.privilege, 'elevated');
    await fleet.start('tart:example-copy');
    assert.deepEqual(launched, [{ command: 'tart', args: ['run', 'example-copy', '--no-graphics'] }]);
    await fleet.configure({ action: 'set-machine', machineId: 'tart:example-copy', sshUser: 'example' });
    await fleet.configure({ action: 'select-machine', workspaceId: 'workspace-example', machineId: 'tart:example-copy' });
    assert.equal((await fleet.state()).lastMachineByWorkspace['workspace-example'], 'tart:example-copy');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('Fleet tests draft SSH settings without saving them or starting a base', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-ssh-test-'));
  const machines = new Map([['example-base', false], ['example-worker', true]]);
  const calls = [];
  let sshError = null;
  let rejectAgent = false;
  const runner = {
    async run(command, args) {
      calls.push({ command, args });
      if (command === 'tart' && args[0] === 'list') return JSON.stringify([...machines].map(([Name, Running]) => ({ Name, Running })));
      if (command === 'tart' && args[0] === 'ip') {
        if (rejectAgent && args.includes('agent')) throw new Error('guest agent unavailable');
        return '192.0.2.10\n';
      }
      if (command === 'ssh') {
        if (sshError) throw sshError;
        return '';
      }
      throw new Error('Unexpected test command.');
    },
    launch() { throw new Error('SSH test must not start a VM.'); },
  };
  try {
    const path = join(root, 'fleet.json');
    const fleet = new FleetService(path, 'darwin', runner);
    await fleet.configure({ action: 'set-machine', machineId: 'tart:example-base', base: true, sshUser: 'saved' });
    assert.match((await fleet.testSsh({ machineId: 'tart:example-base', sshHost: '', sshUser: 'draft' })).message, /Clone and start a worker/);
    const before = readFileSync(path, 'utf8');
    const result = await fleet.testSsh({ machineId: 'tart:example-worker', sshHost: '', sshUser: 'example', sshIdentityFile: '~/example-key', sshKnownHostsFile: join(root, 'known_hosts') });
    assert.equal(result.success, true);
    const ssh = calls.find((call) => call.command === 'ssh');
    assert.ok(ssh);
    assert.ok(ssh.args.includes('example@192.0.2.10'));
    assert.ok(calls.some((call) => call.command === 'tart' && call.args.includes('agent')));
    assert.ok(ssh.args.includes(`UserKnownHostsFile=${join(root, 'known_hosts')}`));
    assert.ok(ssh.args.includes('StrictHostKeyChecking=accept-new'));
    assert.ok(ssh.args.some((arg) => arg.endsWith('/example-key')));
    rejectAgent = true;
    assert.equal((await fleet.testSsh({ machineId: 'tart:example-worker', sshHost: '', sshUser: 'example' })).success, true);
    assert.ok(calls.some((call) => call.command === 'tart' && call.args.includes('--wait') && !call.args.includes('agent')));
    sshError = new Error('synthetic SSH failure');
    assert.deepEqual(await fleet.testSsh({ machineId: 'tart:example-worker', sshHost: '192.0.2.10', sshUser: 'example' }), {
      success: false,
      message: 'SSH failed. Check the guest user, identity file, known-hosts file, and Remote Login. These settings have not been saved. Address checked: 192.0.2.10.',
    });
    sshError = Object.assign(new Error('synthetic SSH failure'), { stderr: 'Host key verification failed.' });
    assert.match((await fleet.testSsh({ machineId: 'tart:example-worker', sshHost: '192.0.2.10', sshUser: 'example' })).message, /SSH detail: Host key verification failed\./);
    sshError = Object.assign(new Error('synthetic SSH failure'), { stderr: 'ssh: connect to host 192.0.2.10 port 22: No route to host' });
    assert.match((await fleet.testSsh({ machineId: 'tart:example-worker', sshHost: '192.0.2.10', sshUser: 'example' })).message, /app-server could not reach this VM/);
    assert.equal(readFileSync(path, 'utf8'), before);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('Fleet leases persist by machine and session and reject conflicting control', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-owner-test-'));
  let running = false;
  const runner = {
    async run(command, args) {
      assert.equal(command, 'tart');
      if (args[0] === 'list') return JSON.stringify([{ Name: 'example-worker', Running: running }, { Name: 'example-second', Running: false }]);
      if (args[0] === 'stop') { running = false; return ''; }
      throw new Error('Unexpected Fleet test command.');
    },
    launch() { running = true; },
  };
  try {
    const path = join(root, 'fleet.json');
    const fleet = new FleetService(path, 'darwin', runner);
    const owner = { machineId: 'machine-example', sessionId: 'session-example', workspaceId: 'workspace-example' };
    await fleet.reserve('tart:example-worker', owner);
    assert.deepEqual((await new FleetService(path, 'darwin', runner).state()).machines.find((machine) => machine.id === 'tart:example-worker').owner,
      { machineId: owner.machineId, sessionId: owner.sessionId });
    await assert.rejects(fleet.reserve('tart:example-worker', { machineId: 'machine-other', sessionId: 'session-example' }), /already reserved/);
    await assert.rejects(fleet.reserve('tart:example-second', { machineId: 'machine-other', sessionId: 'session-other', workspaceId: 'workspace-example' }), /already staged/);
    await assert.rejects(fleet.start('tart:example-worker'), /reserved/);
    await fleet.start('tart:example-worker', owner);
    await assert.rejects(fleet.stop('tart:example-worker', { machineId: owner.machineId, sessionId: 'session-other' }), /reserved/);
    await fleet.stop('tart:example-worker', owner);
    await assert.rejects(fleet.release('tart:example-worker', { machineId: 'machine-other', sessionId: owner.sessionId }), /Only the owning/);
    await fleet.release('tart:example-worker', owner);
    assert.equal((await fleet.state()).machines.find((machine) => machine.id === 'tart:example-worker').owner, null);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
