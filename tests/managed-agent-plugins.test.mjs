import assert from 'node:assert/strict';
import { chmodSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { AgentPluginRegistry } from '../packages/research-agent/dist/agent-plugin-registry.js';

const pluginRoot = resolve('managed-plugins/apple-security-devices');
const serverPath = join(pluginRoot, 'server.mjs');
const targetFlagsPluginRoot = resolve('managed-plugins/apple-target-flags');

test('managed apple-target-flags is importable as a guidance-only Agent Plugin', () => {
  const registryRoot = mkdtempSync(join(tmpdir(), 'beale-managed-target-flags-plugin-'));
  try {
    const registry = new AgentPluginRegistry(registryRoot, { builtinPlugins: [] });
    const state = registry.addFromFilesystem(targetFlagsPluginRoot);
    assert.equal(state.plugins.length, 1);
    assert.equal(state.plugins[0].name, 'apple-target-flags');
    assert.equal(state.plugins[0].status, 'ready');
    assert.deepEqual(state.plugins[0].skills.map((skill) => skill.id), ['apple-target-flags']);
    assert.deepEqual(state.plugins[0].mcpServers, []);

    const runtime = registry.getAppServerRuntime();
    assert.deepEqual(runtime.selectedSkillIds, ['apple-target-flags']);
    assert.deepEqual(runtime.allowedMcpServers, []);
  } finally {
    rmSync(registryRoot, { recursive: true, force: true });
  }
});

test('managed apple-security-devices is importable through the Beale Agent Plugin registry', () => {
  const registryRoot = mkdtempSync(join(tmpdir(), 'beale-managed-plugin-registry-'));
  try {
    const registry = new AgentPluginRegistry(registryRoot, { builtinPlugins: [] });
    const state = registry.addFromFilesystem(pluginRoot);
    assert.equal(state.specVersion, '1.0.0');
    assert.equal(state.plugins.length, 1);
    assert.equal(state.plugins[0].name, 'apple-security-devices');
    assert.equal(state.plugins[0].status, 'ready');
    assert.equal(state.plugins[0].enabled, true);
    assert.deepEqual(state.plugins[0].skills.map((skill) => skill.id), ['apple-security-devices']);
    assert.deepEqual(state.plugins[0].mcpServers.map((server) => ({
      name: server.name,
      transport: server.transport,
      valid: server.valid
    })), [{ name: 'devices', transport: 'stdio', valid: true }]);

    const runtime = registry.getAppServerRuntime();
    assert.deepEqual(runtime.selectedSkillIds, ['apple-security-devices']);
    assert.deepEqual(runtime.allowedMcpServers, ['apple-security-devices.devices']);
    const runtimeConfig = JSON.parse(readFileSync(runtime.mcpConfigPath, 'utf8'));
    assert.deepEqual(runtimeConfig.servers['apple-security-devices.devices'], {
      type: 'stdio',
      command: 'node',
      args: [serverPath],
      cwd: pluginRoot,
      env: {
        PLUGIN_ROOT: pluginRoot,
        PLUGIN_DATA: join(registryRoot, 'agent-plugin-data', state.plugins[0].id)
      }
    });
  } finally {
    rmSync(registryRoot, { recursive: true, force: true });
  }
});

test('apple-security-devices MCP surface auto-reviews Tart operations and confirms external device mutations', () => {
  const messages = runServer([
    { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
    { jsonrpc: '2.0', id: 2, method: 'tools/list', params: {} }
  ]);
  assert.equal(messages[0].result.serverInfo.name, 'apple-security-devices');
  const tools = messages[1].result.tools;
  const expectedReadTools = [
    'environment_status',
    'list_tart_vms',
    'inspect_tart_vm',
    'tart_vm_ip',
    'list_physical_iphones',
    'describe_physical_iphone',
    'inspect_darwin_vm',
    'list_darwin_vm_runs',
    'read_darwin_vm_log'
  ];
  const expectedWriteTools = [
    'start_tart_vm',
    'stop_tart_vm',
    'exec_tart_vm',
    'copy_to_tart_vm',
    'copy_from_tart_vm',
    'install_physical_iphone_app',
    'launch_physical_iphone_app',
    'start_darwin_vm',
    'stop_darwin_vm',
    'run_darwin_vm_console_command'
  ];
  const autoReviewedTartTools = [
    'start_tart_vm',
    'stop_tart_vm',
    'exec_tart_vm',
    'copy_to_tart_vm',
    'copy_from_tart_vm'
  ];
  assert.deepEqual(tools.map((tool) => tool.name).sort(), [...expectedReadTools, ...expectedWriteTools].sort());
  assert.equal(tools.some((tool) => /simulator|simctl/iu.test(tool.name)), false);
  for (const tool of tools) {
    const policy = tool.annotations['beale.io/tool'];
    if (expectedReadTools.includes(tool.name)) {
      assert.equal(tool.annotations.readOnlyHint, true);
      assert.equal(policy.sideEffects, 'read');
      assert.equal(policy.confirmation, 'never');
    } else {
      assert.equal(tool.annotations.readOnlyHint, false);
      assert.equal(policy.sideEffects, 'write');
      assert.equal(policy.confirmation, autoReviewedTartTools.includes(tool.name) ? 'never' : 'always');
    }
  }
});

test('apple-security-devices rejects Simulator research before any host action', () => {
  const messages = runServer([
    { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
    {
      jsonrpc: '2.0',
      id: 2,
      method: 'tools/call',
      params: { name: 'tart_vm_ip', arguments: { vmName: 'ios-simulator-lab' } }
    }
  ]);
  assert.equal(messages[1].result.isError, true);
  assert.match(messages[1].result.content[0].text, /iOS Simulator security research is prohibited/u);
  assert.match(messages[1].result.content[0].text, /physical iPhone/u);
});

test('apple-security-devices reports capabilities without claiming iOS Simulator support', () => {
  const messages = runServer([
    { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
    { jsonrpc: '2.0', id: 2, method: 'tools/call', params: { name: 'environment_status', arguments: {} } }
  ]);
  assert.equal(messages[1].result.isError, undefined);
  const status = JSON.parse(messages[1].result.content[0].text);
  assert.equal(status.iosSimulatorSupported, false);
  assert.equal(typeof status.tart.available, 'boolean');
  assert.equal(typeof status.physicalIphone.available, 'boolean');
  assert.equal(status.darwinVm.available, true);
});

test('apple-security-devices allows name-bound Tart concurrency by default but supports explicit exclusivity', () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-fake-tart-'));
  const fakeTart = join(directory, 'tart');
  try {
    writeFileSync(fakeTart, `#!/usr/bin/env node
const args = process.argv.slice(2);
if (args[0] === 'list') {
  process.stdout.write(JSON.stringify([
    { Name: 'selected-vm', Running: false, State: 'stopped' },
    { Name: 'unrelated-vm', Running: true, State: 'running' }
    ]));
  process.exit(0);
}
if (args[0] === 'run' && args[1] === 'selected-vm') {
  if (args.includes('--net-host')) {
    process.stderr.write('default startup unexpectedly requested privileged host-only networking');
    process.exit(98);
  }
  setTimeout(() => process.exit(0), 1500);
  return;
}
process.stderr.write('unexpected fake Tart command');
process.exit(99);
`);
    chmodSync(fakeTart, 0o755);
    const messages = runServer([
      { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
      {
        jsonrpc: '2.0', id: 2, method: 'tools/call',
        params: { name: 'start_tart_vm', arguments: { vmName: 'selected-vm', waitSeconds: 0 } }
      },
      {
        jsonrpc: '2.0', id: 3, method: 'tools/call',
        params: { name: 'start_tart_vm', arguments: { vmName: 'selected-vm', requireExclusive: true, waitSeconds: 0 } }
      }
    ], {
      APPLE_SECURITY_TEST_PLATFORM: 'darwin',
      APPLE_SECURITY_TART_COMMAND: fakeTart
    });
    const concurrentResponse = messages.find((message) => message.id === 2);
    const exclusiveResponse = messages.find((message) => message.id === 3);
    assert.ok(concurrentResponse);
    assert.ok(exclusiveResponse);
    assert.equal(concurrentResponse.result.isError, undefined, concurrentResponse.result.content[0].text);
    const concurrentStart = JSON.parse(concurrentResponse.result.content[0].text);
    assert.equal(concurrentStart.started, true);
    assert.equal(concurrentStart.posture.network, 'shared-nat');
    assert.deepEqual(concurrentStart.concurrentRunningVms, ['unrelated-vm']);
    assert.equal(exclusiveResponse.result.isError, true);
    assert.match(exclusiveResponse.result.content[0].text, /Exclusive start requested for selected-vm/u);
    assert.match(exclusiveResponse.result.content[0].text, /unrelated-vm/u);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('apple-security-devices returns the real diagnostic when Tart exits during startup', () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-fake-tart-start-failure-'));
  const fakeTart = join(directory, 'tart');
  try {
    writeFileSync(fakeTart, `#!/usr/bin/env node
const args = process.argv.slice(2);
if (args[0] === 'list') {
  process.stdout.write(JSON.stringify([{ Name: 'selected-vm', Running: false, State: 'stopped' }]));
  process.exit(0);
}
if (args[0] === 'run' && args[1] === 'selected-vm') {
  process.stderr.write('root privileges are required for the selected network mode');
  process.exit(2);
}
process.stderr.write('unexpected fake Tart command');
process.exit(99);
`);
    chmodSync(fakeTart, 0o755);
    const messages = runServer([
      { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
      {
        jsonrpc: '2.0', id: 2, method: 'tools/call',
        params: { name: 'start_tart_vm', arguments: { vmName: 'selected-vm', networkMode: 'host-only', waitSeconds: 0 } }
      }
    ], {
      APPLE_SECURITY_TEST_PLATFORM: 'darwin',
      APPLE_SECURITY_TART_COMMAND: fakeTart
    });
    assert.equal(messages[1].result.isError, true);
    assert.match(messages[1].result.content[0].text, /root privileges are required for the selected network mode/u);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('apple-security-devices resolves an explicitly named Tart IP with concurrent guests', () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-fake-tart-ip-'));
  const fakeTart = join(directory, 'tart');
  try {
    writeFileSync(fakeTart, `#!/usr/bin/env node
const args = process.argv.slice(2);
if (args[0] === 'list') {
  process.stdout.write(JSON.stringify([
    { Name: 'selected-vm', Running: true, State: 'running' },
    { Name: 'unrelated-vm', Running: true, State: 'running' }
  ]));
  process.exit(0);
}
if (args[0] === 'ip' && args[1] === 'selected-vm') {
  process.stdout.write('192.0.2.25');
  process.exit(0);
}
process.stderr.write('unexpected fake Tart command');
process.exit(99);
`);
    chmodSync(fakeTart, 0o755);
    const messages = runServer([
      { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
      {
        jsonrpc: '2.0', id: 2, method: 'tools/call',
        params: { name: 'tart_vm_ip', arguments: { vmName: 'selected-vm', waitSeconds: 0 } }
      }
    ], {
      APPLE_SECURITY_TEST_PLATFORM: 'darwin',
      APPLE_SECURITY_TART_COMMAND: fakeTart
    });
    assert.equal(messages[1].result.isError, undefined, messages[1].result.content[0].text);
    const resolution = JSON.parse(messages[1].result.content[0].text);
    assert.equal(resolution.address, '192.0.2.25');
    assert.deepEqual(resolution.concurrentRunningVms, ['unrelated-vm']);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('apple-security-devices falls back to configured SSH when Tart Guest Agent is unavailable', () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-fake-tart-ssh-fallback-'));
  const fakeTart = join(directory, 'tart');
  const fakeSsh = join(directory, 'ssh');
  const fakeRunner = join(directory, 'runner');
  const runnerLog = join(directory, 'runner.log');
  const identity = join(directory, 'identity');
  const knownHosts = join(directory, 'known-hosts');
  try {
    writeFileSync(fakeTart, `#!/usr/bin/env node
const args = process.argv.slice(2);
if (args[0] === 'exec') {
  process.stderr.write('Tart Guest Agent is unavailable');
  process.exit(1);
}
if (args[0] === 'ip') {
  process.stdout.write('192.0.2.25');
  process.exit(0);
}
process.stderr.write('unexpected fake Tart command');
process.exit(99);
`);
    writeFileSync(fakeSsh, `#!/usr/bin/env node
process.stderr.write('SSH must be launched through the configured host runner');
process.exit(97);
`);
    writeFileSync(fakeRunner, `#!/usr/bin/env node
const { appendFileSync } = require('node:fs');
appendFileSync(process.env.APPLE_SECURITY_TEST_RUNNER_LOG, JSON.stringify(process.argv.slice(2)) + '\\n');
process.stdout.write('fallback-ok');
`);
    writeFileSync(identity, 'test identity');
    writeFileSync(knownHosts, 'test known host');
    chmodSync(fakeTart, 0o755);
    chmodSync(fakeSsh, 0o755);
    chmodSync(fakeRunner, 0o755);
    const messages = runServer([
      { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
      {
        jsonrpc: '2.0', id: 2, method: 'tools/call',
        params: { name: 'exec_tart_vm', arguments: { vmName: 'selected-vm', argv: ['/usr/bin/printf', '%s', 'safe value'], timeoutSeconds: 10 } }
      }
    ], {
      APPLE_SECURITY_TEST_PLATFORM: 'darwin',
      APPLE_SECURITY_TART_COMMAND: fakeTart,
      APPLE_SECURITY_SSH_COMMAND: fakeSsh,
      APPLE_SECURITY_COMMAND_RUNNER: fakeRunner,
      APPLE_SECURITY_TEST_RUNNER_LOG: runnerLog,
      APPLE_SECURITY_SSH_IDENTITY: identity,
      APPLE_SECURITY_SSH_KNOWN_HOSTS: knownHosts
    });
    assert.equal(messages[1].result.isError, undefined, messages[1].result.content[0].text);
    const result = JSON.parse(messages[1].result.content[0].text);
    assert.equal(result.transport, 'ssh');
    assert.equal(result.stdout, 'fallback-ok');
    const runnerCalls = readFileSync(runnerLog, 'utf8').trim().split('\n').map((line) => JSON.parse(line));
    assert.equal(runnerCalls.length, 2);
    for (const call of runnerCalls) assert.deepEqual(call.slice(0, 3), ['run', '--', fakeSsh]);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('apple-security-devices copies bounded files through the Tart Guest Agent in both directions', () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-fake-tart-copy-'));
  const fakeTart = join(directory, 'tart');
  const source = join(directory, 'host-source');
  const guest = join(directory, 'guest-file');
  const downloaded = join(directory, 'host-download');
  try {
    writeFileSync(fakeTart, `#!/usr/bin/env node
const { createHash } = require('node:crypto');
const { chmodSync, existsSync, readFileSync, renameSync, statSync, unlinkSync, writeFileSync } = require('node:fs');
const args = process.argv.slice(2);
if (args[0] === 'list') {
  process.stdout.write(JSON.stringify([{ Name: 'selected-vm', Running: true, State: 'running' }]));
  process.exit(0);
}
if (args[0] !== 'exec') {
  process.stderr.write('unexpected fake Tart command');
  process.exit(99);
}
const attachInput = args[1] === '-i';
const commandIndex = attachInput ? 3 : 2;
const command = args[commandIndex];
const commandArgs = args.slice(commandIndex + 1);
if (command === '/usr/bin/true') process.exit(0);
if (command === '/bin/test' && commandArgs[0] === '-e') process.exit(existsSync(commandArgs[1]) ? 0 : 1);
if (command === '/bin/test' && commandArgs[0] === '-f') process.exit(existsSync(commandArgs[1]) && statSync(commandArgs[1]).isFile() ? 0 : 1);
if (command === '/bin/dd') {
  const output = commandArgs.find((item) => item.startsWith('of='))?.slice(3);
  const chunks = [];
  process.stdin.on('data', (chunk) => chunks.push(Buffer.from(chunk)));
  process.stdin.on('end', () => { writeFileSync(output, Buffer.concat(chunks)); process.exit(0); });
  return;
}
if (command === '/bin/chmod') { chmodSync(commandArgs[1], Number.parseInt(commandArgs[0], 8)); process.exit(0); }
if (command === '/usr/bin/stat' && commandArgs[0] === '-f') {
  const value = commandArgs[1] === '%z' ? statSync(commandArgs[2]).size : (statSync(commandArgs[2]).mode & 0o777).toString(8);
  process.stdout.write(String(value) + '\\n'); process.exit(0);
}
if (command === '/usr/bin/shasum') {
  const path = commandArgs.at(-1);
  process.stdout.write(createHash('sha256').update(readFileSync(path)).digest('hex') + '  ' + path + '\\n'); process.exit(0);
}
if (command === '/bin/mv') { renameSync(commandArgs.at(-2), commandArgs.at(-1)); process.exit(0); }
if (command === '/bin/rm') { if (existsSync(commandArgs.at(-1))) unlinkSync(commandArgs.at(-1)); process.exit(0); }
if (command === '/bin/cat') { process.stdout.write(readFileSync(commandArgs[0])); process.exit(0); }
process.stderr.write('unexpected fake guest command: ' + command);
process.exit(98);
`);
    writeFileSync(source, Buffer.from([0, 1, 2, 3, 10, 13, 255, 128, 65]));
    chmodSync(fakeTart, 0o755);
    chmodSync(source, 0o750);
    const uploadMessages = runServer([
      { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
      {
        jsonrpc: '2.0', id: 2, method: 'tools/call',
        params: { name: 'copy_to_tart_vm', arguments: { vmName: 'selected-vm', localPath: source, guestPath: guest, timeoutSeconds: 10 } }
      }
    ], {
      APPLE_SECURITY_TEST_PLATFORM: 'darwin',
      APPLE_SECURITY_TART_COMMAND: fakeTart
    });
    const downloadMessages = runServer([
      { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
      {
        jsonrpc: '2.0', id: 3, method: 'tools/call',
        params: { name: 'copy_from_tart_vm', arguments: { vmName: 'selected-vm', guestPath: guest, localPath: downloaded, timeoutSeconds: 10 } }
      }
    ], {
      APPLE_SECURITY_TEST_PLATFORM: 'darwin',
      APPLE_SECURITY_TART_COMMAND: fakeTart
    });
    assert.equal(uploadMessages[1].result.isError, undefined, uploadMessages[1].result.content[0].text);
    assert.equal(downloadMessages[1].result.isError, undefined, downloadMessages[1].result.content[0].text);
    const upload = JSON.parse(uploadMessages[1].result.content[0].text);
    const download = JSON.parse(downloadMessages[1].result.content[0].text);
    assert.equal(upload.direction, 'host-to-guest');
    assert.equal(download.direction, 'guest-to-host');
    assert.equal(upload.transport, 'guest-agent');
    assert.equal(download.transport, 'guest-agent');
    assert.equal(upload.mode, '750');
    assert.equal(download.mode, '750');
    assert.equal(upload.sha256, download.sha256);
    assert.deepEqual(readFileSync(guest), readFileSync(source));
    assert.deepEqual(readFileSync(downloaded), readFileSync(source));
    assert.equal(statSync(guest).mode & 0o777, 0o750);
    assert.equal(statSync(downloaded).mode & 0o777, 0o750);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('apple-security-devices copies files through the private host runner when Guest Agent is unavailable', () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-fake-tart-runner-copy-'));
  const fakeTart = join(directory, 'tart');
  const fakeSsh = join(directory, 'ssh');
  const fakeScp = join(directory, 'scp');
  const fakeRunner = join(directory, 'runner');
  const runnerLog = join(directory, 'runner.log');
  const identity = join(directory, 'identity');
  const knownHosts = join(directory, 'known-hosts');
  const source = join(directory, 'runner-source');
  const guest = join(directory, 'runner-guest');
  const downloaded = join(directory, 'runner-download');
  try {
    writeFileSync(fakeTart, `#!/usr/bin/env node
const args = process.argv.slice(2);
if (args[0] === 'list') {
  process.stdout.write(JSON.stringify([{ Name: 'selected-vm', Running: true, State: 'running' }]));
  process.exit(0);
}
if (args[0] === 'exec') { process.stderr.write('Guest Agent unavailable'); process.exit(1); }
if (args[0] === 'ip') { process.stdout.write('192.0.2.25'); process.exit(0); }
process.exit(99);
`);
    writeFileSync(fakeSsh, `#!/usr/bin/env node
const { spawnSync } = require('node:child_process');
const command = process.argv.at(-1);
const result = spawnSync('/bin/sh', ['-c', command], { stdio: 'inherit' });
process.exit(result.status ?? 98);
`);
    writeFileSync(fakeScp, `#!/usr/bin/env node
const { copyFileSync } = require('node:fs');
const args = process.argv.slice(2);
const source = args.at(-2);
const destination = args.at(-1);
const separator = destination.indexOf(':');
if (!source || separator < 0) process.exit(96);
copyFileSync(source, destination.slice(separator + 1));
`);
    writeFileSync(fakeRunner, `#!/usr/bin/env node
const { appendFileSync } = require('node:fs');
const { spawnSync } = require('node:child_process');
const args = process.argv.slice(2);
appendFileSync(process.env.APPLE_SECURITY_TEST_RUNNER_LOG, JSON.stringify(args.slice(0, 3)) + '\\n');
const result = spawnSync(args[2], args.slice(3), { stdio: ['ignore', 'inherit', 'inherit'] });
process.exit(result.status ?? 97);
`);
    writeFileSync(identity, 'test identity');
    writeFileSync(knownHosts, 'test known host');
    writeFileSync(source, Buffer.from([240, 159, 141, 142, 0, 10, 255]));
    for (const executable of [fakeTart, fakeSsh, fakeScp, fakeRunner]) chmodSync(executable, 0o755);
    const environment = {
      APPLE_SECURITY_TEST_PLATFORM: 'darwin',
      APPLE_SECURITY_TART_COMMAND: fakeTart,
      APPLE_SECURITY_SSH_COMMAND: fakeSsh,
      APPLE_SECURITY_SCP_COMMAND: fakeScp,
      APPLE_SECURITY_COMMAND_RUNNER: fakeRunner,
      APPLE_SECURITY_TEST_RUNNER_LOG: runnerLog,
      APPLE_SECURITY_SSH_IDENTITY: identity,
      APPLE_SECURITY_SSH_KNOWN_HOSTS: knownHosts
    };
    const uploadMessages = runServer([
      { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
      {
        jsonrpc: '2.0', id: 2, method: 'tools/call',
        params: { name: 'copy_to_tart_vm', arguments: { vmName: 'selected-vm', localPath: source, guestPath: guest, timeoutSeconds: 10 } }
      }
    ], environment);
    const downloadMessages = runServer([
      { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
      {
        jsonrpc: '2.0', id: 3, method: 'tools/call',
        params: { name: 'copy_from_tart_vm', arguments: { vmName: 'selected-vm', guestPath: guest, localPath: downloaded, timeoutSeconds: 10 } }
      }
    ], environment);
    assert.equal(uploadMessages[1].result.isError, undefined, uploadMessages[1].result.content[0].text);
    assert.equal(downloadMessages[1].result.isError, undefined, downloadMessages[1].result.content[0].text);
    const upload = JSON.parse(uploadMessages[1].result.content[0].text);
    const download = JSON.parse(downloadMessages[1].result.content[0].text);
    assert.equal(upload.transport, 'ssh');
    assert.equal(download.transport, 'ssh');
    assert.deepEqual(readFileSync(guest), readFileSync(source));
    assert.deepEqual(readFileSync(downloaded), readFileSync(source));
    const calls = readFileSync(runnerLog, 'utf8').trim().split('\n').map((line) => JSON.parse(line));
    assert.ok(calls.length >= 10);
    assert.ok(calls.some((call) => call[2] === fakeScp));
    assert.ok(calls.some((call) => call[2] === fakeSsh));
    for (const call of calls) assert.deepEqual(call.slice(0, 2), ['run', '--']);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('apple-security-devices rejects over-limit and clobbering transfers before opening a guest transport', () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-fake-tart-copy-guards-'));
  const fakeTart = join(directory, 'tart');
  const source = join(directory, 'source');
  const destination = join(directory, 'destination');
  try {
    writeFileSync(fakeTart, `#!/usr/bin/env node
const args = process.argv.slice(2);
if (args[0] === 'list') {
  process.stdout.write(JSON.stringify([{ Name: 'selected-vm', Running: true, State: 'running' }]));
  process.exit(0);
}
process.stderr.write('guest transport should not have opened');
process.exit(99);
`);
    writeFileSync(source, 'too large');
    writeFileSync(destination, 'keep me');
    chmodSync(fakeTart, 0o755);
    const messages = runServer([
      { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
      {
        jsonrpc: '2.0', id: 2, method: 'tools/call',
        params: { name: 'copy_to_tart_vm', arguments: { vmName: 'selected-vm', localPath: source, guestPath: '/tmp/destination', maxBytes: 1 } }
      },
      {
        jsonrpc: '2.0', id: 3, method: 'tools/call',
        params: { name: 'copy_from_tart_vm', arguments: { vmName: 'selected-vm', guestPath: '/tmp/source', localPath: destination } }
      }
    ], {
      APPLE_SECURITY_TEST_PLATFORM: 'darwin',
      APPLE_SECURITY_TART_COMMAND: fakeTart
    });
    const oversized = messages.find((message) => message.id === 2);
    const clobbering = messages.find((message) => message.id === 3);
    assert.equal(oversized.result.isError, true);
    assert.match(oversized.result.content[0].text, /transfer limit/u);
    assert.equal(clobbering.result.isError, true);
    assert.match(clobbering.result.content[0].text, /already exists/u);
    assert.equal(readFileSync(destination, 'utf8'), 'keep me');
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

function runServer(messages, extraEnvironment = {}) {
  const pluginData = mkdtempSync(join(tmpdir(), 'beale-apple-security-plugin-data-'));
  try {
    const result = spawnSync(process.execPath, [serverPath], {
      input: `${messages.map((message) => JSON.stringify(message)).join('\n')}\n`,
      encoding: 'utf8',
      env: { ...process.env, PLUGIN_ROOT: pluginRoot, PLUGIN_DATA: pluginData, ...extraEnvironment },
      timeout: 5000
    });
    assert.equal(result.status, 0, result.stderr);
    return result.stdout.trim().split('\n').filter(Boolean).map((line) => JSON.parse(line));
  } finally {
    rmSync(pluginData, { recursive: true, force: true });
  }
}
