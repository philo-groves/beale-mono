import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { FleetService } from '../dist/fleet.js';

test('connected primary sessions request one remote browser and attach with a session token', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-browser-primary-'));
  const requests = [];
  const fleet = new FleetService(join(root, 'fleet.json'), 'darwin', { async run() { return '[]'; }, launch() {} });
  fleet.connectedRemoteAppServer = async () => ({ id: 'server-example', name: 'Example',
    url: 'https://worker.example.test', operatorToken: 'synthetic-operator-token' });
  fleet.callRemote = async () => ({ enabled: true, role: 'primary', requiredWorkspaceIds: [], optionalWorkspaceIds: [], machines: [] });
  const previousFetch = globalThis.fetch;
  globalThis.fetch = async (url, init) => {
    requests.push({ url: String(url), body: JSON.parse(init.body) });
    if (String(url).endsWith('/attachments')) return Response.json({ transport: { token: 'synthetic-session-token' } });
    return Response.json({ session: { sessionId: 'session-example' } });
  };
  try {
    assert.deepEqual(await fleet.remoteLaunch('server-example', 'workspace-example', 'Research the example scope.', 'local'),
      { sessionId: 'session-example' });
    assert.equal(requests[0].body.launch.browserRelay, true);
    assert.equal(requests[0].body.launch.machineId, 'local');
    assert.deepEqual(await fleet.remoteBrowserAttach('server-example', 'session-example'), {
      url: 'wss://worker.example.test/v1/sessions/session-example/browser', token: 'synthetic-session-token',
    });
    assert.equal(requests[1].url, 'https://worker.example.test/v1/sessions/session-example/attachments');
  } finally { globalThis.fetch = previousFetch; rmSync(root, { recursive: true, force: true }); }
});

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
    const path = join(root, 'fleet.json');
    const fleet = new FleetService(path, 'darwin', runner);
    assert.equal((await fleet.state()).machines[0]?.base, false);
    await assert.rejects(fleet.clone('tart:example-base', 'example-copy'), /registered base/);
    await fleet.configure({ action: 'set-machine', machineId: 'tart:example-base', base: true, privilege: 'elevated', sshUser: 'example' });
    await assert.rejects(fleet.start('tart:example-base'), /runnable/);
    const cloned = await fleet.clone('tart:example-base', 'example-copy');
    assert.equal(cloned.machines.find((machine) => machine.name === 'example-copy')?.base, false);
    assert.equal(cloned.machines.find((machine) => machine.name === 'example-copy')?.privilege, 'elevated');
    await fleet.start('tart:example-copy');
    assert.deepEqual(launched, [{ command: 'tart', args: ['run', 'example-copy', '--no-graphics'] }]);
    await fleet.configure({ action: 'set-machine', machineId: 'tart:example-copy', sshUser: 'example' });
    await fleet.configure({ action: 'select-machine', workspaceId: 'workspace-example', machineId: 'tart:example-base' });
    assert.equal((await fleet.state()).lastMachineByWorkspace['workspace-example'], 'tart:example-base');
    const owner = { machineId: 'machine-example', sessionId: 'session-example' };
    const [first, again] = await Promise.all([
      fleet.cloneForSession('tart:example-base', owner), fleet.cloneForSession('tart:example-base', owner),
    ]);
    assert.equal(first.id, again.id);
    assert.match(first.name, /^beale-example-base-[a-f0-9]{16}$/);
    assert.deepEqual((await fleet.state()).machines.find((machine) => machine.id === first.id)?.owner, owner);
    assert.equal(new FleetService(path, 'darwin', runner).isSessionClone(first.id), true);
    assert.equal((await fleet.state()).machines.find((machine) => machine.id === 'tart:example-base')?.state, 'stopped');
    await fleet.release(first.id, owner);
    const next = await fleet.cloneForSession('tart:example-base', owner);
    assert.equal(next.name, first.name);
    assert.equal(next.id, first.id);
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

test('Fleet includes bases from reachable app servers when another peer fails', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-remote-bases-'));
  try {
    const path = join(root, 'fleet.json');
    writeFileSync(path, JSON.stringify({ version: 2, machineId: 'machine-example', role: 'primary', enabled: true,
      appServers: {
        'server-good': { name: 'Example remote', url: 'https://good.example.ts.net:47174', operatorToken: 'synthetic-operator-token' },
        'server-bad': { name: 'Offline remote', url: 'https://bad.example.ts.net:47174', operatorToken: 'synthetic-operator-token' },
      }
    }));
    const fleet = new FleetService(path, 'darwin');
    const calls = [];
    fleet.callRemote = async (serverId, operation, _input, timeoutMs) => {
      calls.push({ serverId, operation, timeoutMs });
      if (serverId === 'server-bad') throw new Error('Synthetic peer unavailable');
      return { enabled: true, role: 'primary', available: true, machines: [
        { id: 'tart:example-base', name: 'example-base', base: true, state: 'stopped', sshConfigured: true },
        { id: 'tart:example-worker', name: 'example-worker', base: false, state: 'stopped', sshConfigured: true },
      ] };
    };
    assert.deepEqual((await fleet.remoteMachines()).map(({ id, name }) => ({ id, name })), [
      { id: 'remote:server-good:tart:example-base', name: 'Example remote / example-base' }
    ]);
    assert.ok(calls.every((call) => call.operation === 'fleet.state' && call.timeoutMs === 12_000));
    fleet.callRemote = async () => { throw new Error('Synthetic peers unavailable'); };
    await assert.rejects(fleet.remoteMachines(), /Could not load VM inventory/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
