import assert from 'node:assert/strict';
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import net from 'node:net';
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

test('apple-security-devices MCP surface auto-reviews VM operations and confirms physical device mutations', () => {
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
  const autoReviewedVmTools = [
    'start_tart_vm',
    'stop_tart_vm',
    'exec_tart_vm',
    'copy_to_tart_vm',
    'copy_from_tart_vm',
    'start_darwin_vm',
    'stop_darwin_vm',
    'run_darwin_vm_console_command'
  ];
  assert.deepEqual(tools.map((tool) => tool.name).sort(), [...expectedReadTools, ...expectedWriteTools].sort());
  for (const name of ['copy_to_tart_vm', 'copy_from_tart_vm']) {
    const transfer = tools.find((tool) => tool.name === name);
    assert.equal(transfer.inputSchema.properties.maxBytes.maximum, 4 * 1024 * 1024 * 1024);
    assert.equal(transfer.inputSchema.properties.maxBytes.default, 4 * 1024 * 1024 * 1024);
    assert.equal(transfer.inputSchema.properties.timeoutSeconds.default, 900);
    assert.equal(transfer.inputSchema.properties.timeoutSeconds.maximum, 3600);
    assert.match(transfer.description, /Stream one regular file/u);
  }
  const tartExec = tools.find((tool) => tool.name === 'exec_tart_vm');
  assert.equal(tartExec.inputSchema.properties.timeoutSeconds.default, 60);
  assert.equal(tartExec.inputSchema.properties.timeoutSeconds.maximum, 1800);
  for (const name of ['inspect_darwin_vm', 'start_darwin_vm']) {
    const tool = tools.find((candidate) => candidate.name === name);
    assert.ok(tool.inputSchema.required.includes('checkoutRoot'));
  }
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
      assert.equal(policy.confirmation, autoReviewedVmTools.includes(tool.name) ? 'never' : 'always');
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
  assert.equal(typeof status.darwinVm.available, 'boolean');
});

test('apple-security-devices reattaches a verified Darwin VM after an MCP restart', () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-fake-darwin-restart-'));
  const pluginData = join(directory, 'plugin-data');
  const checkoutRoot = join(directory, 'checkout');
  const qemuDirectory = join(checkoutRoot, 'qemu-sptm', 'build');
  const firmwareDirectory = join(checkoutRoot, 'firmware');
  const qemu = join(qemuDirectory, 'qemu-system-aarch64');
  const capture = join(directory, 'qemu-capture.jsonl');
  let pid = null;
  try {
    mkdirSync(pluginData, { recursive: true });
    mkdirSync(qemuDirectory, { recursive: true });
    mkdirSync(firmwareDirectory, { recursive: true });
    for (const name of ['bootkc', 'dtree', 'ramdisk.tc', 'ramdisk.dmg']) {
      writeFileSync(join(firmwareDirectory, name), `synthetic-${name}`);
    }
    writeFileSync(qemu, `#!/usr/bin/env node
const { appendFileSync, existsSync, unlinkSync } = require('node:fs');
const net = require('node:net');
const args = process.argv.slice(2);
const chardev = args[args.indexOf('-chardev') + 1] || '';
const socketPath = chardev.match(/(?:^|,)path=([^,]+)/)?.[1];
const bootArguments = args[args.indexOf('-args') + 1] || '';
appendFileSync(process.env.APPLE_SECURITY_TEST_QEMU_CAPTURE, JSON.stringify({ bootArguments }) + '\\n');
if (!socketPath) process.exit(64);
if (existsSync(socketPath)) unlinkSync(socketPath);
const server = net.createServer((socket) => socket.on('data', (chunk) => socket.write('guest:' + chunk.toString('utf8'))));
server.listen(socketPath);
process.on('SIGTERM', () => server.close(() => {
  if (existsSync(socketPath)) unlinkSync(socketPath);
  process.exit(0);
}));
setInterval(() => {}, 1000);
`);
    chmodSync(qemu, 0o755);
    const environment = {
      APPLE_SECURITY_TEST_PLATFORM: 'darwin',
      APPLE_SECURITY_TEST_QEMU_CAPTURE: capture
    };
    const startedMessages = runServerWithPluginData([
      { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
      {
        jsonrpc: '2.0', id: 2, method: 'tools/call',
        params: { name: 'start_darwin_vm', arguments: { checkoutRoot, memoryMiB: 2048, bootArguments: 'debug=0x14e' } }
      }
    ], pluginData, environment);
    assert.equal(startedMessages[1].result.isError, undefined, startedMessages[1].result.content[0].text);
    const started = JSON.parse(startedMessages[1].result.content[0].text);
    const runId = started.run.runId;
    const recordPath = join(pluginData, 'darwin-vm-runs', runId, 'run.json');
    const record = JSON.parse(readFileSync(recordPath, 'utf8'));
    pid = record.pid;
    assert.equal(waitUntil(() => existsSync(record.serialSocketPath), 2000), true);
    const captured = JSON.parse(readFileSync(capture, 'utf8').trim());
    assert.match(captured.bootArguments, /^rd=md0 serial=3 -v -noprogress wdt=-1 wlan-olyhal-abort /u);
    assert.match(captured.bootArguments, /debug=0x14e$/u);

    const listedMessages = runServerWithPluginData([
      { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
      { jsonrpc: '2.0', id: 2, method: 'tools/call', params: { name: 'list_darwin_vm_runs', arguments: {} } }
    ], pluginData, environment);
    const listed = JSON.parse(listedMessages[1].result.content[0].text);
    assert.equal(listed.runs[0].state, 'running');
    assert.equal(listed.runs[0].controllable, true);

    const consoleMessages = runServerWithPluginData([
      { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
      {
        jsonrpc: '2.0', id: 2, method: 'tools/call',
        params: { name: 'run_darwin_vm_console_command', arguments: { runId, command: 'id', readMilliseconds: 100 } }
      }
    ], pluginData, environment);
    assert.equal(consoleMessages[1].result.isError, undefined, consoleMessages[1].result.content[0].text);
    assert.match(JSON.parse(consoleMessages[1].result.content[0].text).output, /guest:id/u);

    const stoppedMessages = runServerWithPluginData([
      { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
      { jsonrpc: '2.0', id: 2, method: 'tools/call', params: { name: 'stop_darwin_vm', arguments: { runId } } }
    ], pluginData, environment);
    assert.equal(stoppedMessages[1].result.isError, undefined, stoppedMessages[1].result.content[0].text);
    assert.equal(JSON.parse(stoppedMessages[1].result.content[0].text).run.state, 'stopped');
    assert.equal(waitUntil(() => !processExists(pid), 2000), true);
    pid = null;
  } finally {
    if (pid && processExists(pid)) process.kill(pid, 'SIGTERM');
    rmSync(directory, { recursive: true, force: true });
  }
});

test('apple-security-devices refuses to adopt a live PID with mismatched QEMU identity', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-fake-darwin-mismatch-'));
  const pluginData = join(directory, 'plugin-data');
  const checkoutRoot = join(directory, 'checkout');
  const qemuDirectory = join(checkoutRoot, 'qemu-sptm', 'build');
  const runId = 'darwin_00000000-0000-4000-8000-000000000123';
  const runRoot = join(pluginData, 'darwin-vm-runs', runId);
  const serialSocketPath = join(tmpdir(), `beale-darwin-${runId.slice(-12)}.sock`);
  const serialLogPath = join(runRoot, 'serial.log');
  const qemuLogPath = join(runRoot, 'qemu.log');
  const socketServer = net.createServer();
  try {
    mkdirSync(qemuDirectory, { recursive: true });
    mkdirSync(runRoot, { recursive: true });
    writeFileSync(join(qemuDirectory, 'qemu-system-aarch64'), 'synthetic-qemu');
    if (existsSync(serialSocketPath)) rmSync(serialSocketPath, { force: true });
    await new Promise((resolvePromise, rejectPromise) => {
      socketServer.once('error', rejectPromise);
      socketServer.listen(serialSocketPath, resolvePromise);
    });
    writeFileSync(join(runRoot, 'run.json'), `${JSON.stringify({
      version: 1,
      runId,
      pid: process.pid,
      checkoutRoot,
      serialSocketPath,
      serialLogPath,
      qemuLogPath,
      startedAt: new Date().toISOString(),
      memoryMiB: 2048,
      state: 'running'
    }, null, 2)}\n`);

    const listedMessages = runServerWithPluginData([
      { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
      { jsonrpc: '2.0', id: 2, method: 'tools/call', params: { name: 'list_darwin_vm_runs', arguments: {} } }
    ], pluginData, { APPLE_SECURITY_TEST_PLATFORM: 'darwin' });
    const listed = JSON.parse(listedMessages[1].result.content[0].text);
    assert.equal(listed.runs[0].state, 'orphaned');
    assert.equal(listed.runs[0].controllable, false);

    const stoppedMessages = runServerWithPluginData([
      { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
      { jsonrpc: '2.0', id: 2, method: 'tools/call', params: { name: 'stop_darwin_vm', arguments: { runId } } }
    ], pluginData, { APPLE_SECURITY_TEST_PLATFORM: 'darwin' });
    assert.equal(stoppedMessages[1].result.isError, true);
    assert.match(stoppedMessages[1].result.content[0].text, /cannot be safely controlled/u);
    assert.equal(processExists(process.pid), true);
  } finally {
    await new Promise((resolvePromise) => socketServer.close(resolvePromise));
    if (existsSync(serialSocketPath)) rmSync(serialSocketPath, { force: true });
    rmSync(directory, { recursive: true, force: true });
  }
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
        params: { name: 'exec_tart_vm', arguments: { vmName: 'selected-vm', argv: ['/usr/bin/printf', '%s', 'safe value'], timeoutSeconds: 1700 } }
      },
      {
        jsonrpc: '2.0', id: 3, method: 'tools/call',
        params: { name: 'exec_tart_vm', arguments: { vmName: 'selected-vm', transport: 'ssh', argv: ['/usr/bin/printf', '%s', 'explicit ssh'], timeoutSeconds: 1700 } }
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
    const explicit = JSON.parse(messages[2].result.content[0].text);
    assert.equal(explicit.transport, 'ssh');
    assert.equal(explicit.stdout, 'fallback-ok');
    const runnerCalls = readFileSync(runnerLog, 'utf8').trim().split('\n').map((line) => JSON.parse(line));
    assert.equal(runnerCalls.length, 4);
    for (const call of runnerCalls) assert.deepEqual(call.slice(0, 3), ['run', '--', fakeSsh]);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('apple-security-devices rejects an invalid explicit Tart execution timeout', () => {
  const messages = runServer([
    { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
    {
      jsonrpc: '2.0', id: 2, method: 'tools/call',
      params: { name: 'exec_tart_vm', arguments: { vmName: 'selected-vm', argv: ['/usr/bin/true'], timeoutSeconds: 1801 } }
    }
  ], { APPLE_SECURITY_TEST_PLATFORM: 'darwin' });
  assert.equal(messages[1].result.isError, true);
  assert.match(messages[1].result.content[0].text, /timeoutSeconds must be an integer from 1 to 1800/u);
});

test('apple-security-devices guest-agent-only policy never opens SSH or a host command runner', () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-fake-tart-guest-agent-only-'));
  const fakeTart = join(directory, 'tart');
  const fakeSsh = join(directory, 'ssh');
  const fakeRunner = join(directory, 'runner');
  const invocationLog = join(directory, 'invocations.log');
  const identity = join(directory, 'identity');
  const knownHosts = join(directory, 'known-hosts');
  try {
    writeFileSync(fakeTart, `#!/usr/bin/env node
const args = process.argv.slice(2);
if (args[0] === 'exec') {
  process.stderr.write('Tart Guest Agent is unavailable');
  process.exit(1);
}
process.exit(99);
`);
    const unexpectedTransport = `#!/usr/bin/env node
const { appendFileSync } = require('node:fs');
appendFileSync(process.env.APPLE_SECURITY_TEST_INVOCATION_LOG, process.argv[1] + '\\n');
process.exit(97);
`;
    writeFileSync(fakeSsh, unexpectedTransport);
    writeFileSync(fakeRunner, unexpectedTransport);
    writeFileSync(identity, 'test identity');
    writeFileSync(knownHosts, 'test known host');
    writeFileSync(join(directory, 'host-config.json'), JSON.stringify({
      tartTransportPolicy: 'guest-agent-only',
      commandRunner: fakeRunner
    }));
    for (const executable of [fakeTart, fakeSsh, fakeRunner]) chmodSync(executable, 0o755);
    const messages = runServer([
      { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
      {
        jsonrpc: '2.0', id: 2, method: 'tools/call',
        params: { name: 'exec_tart_vm', arguments: { vmName: 'selected-vm', argv: ['/usr/bin/true'], timeoutSeconds: 10 } }
      }
    ], {
      APPLE_SECURITY_TEST_PLATFORM: 'darwin',
      APPLE_SECURITY_TART_COMMAND: fakeTart,
      APPLE_SECURITY_SSH_COMMAND: fakeSsh,
      APPLE_SECURITY_SSH_IDENTITY: identity,
      APPLE_SECURITY_SSH_KNOWN_HOSTS: knownHosts,
      APPLE_SECURITY_TEST_INVOCATION_LOG: invocationLog,
      PLUGIN_DATA: directory
    });
    assert.equal(messages[1].result.isError, true);
    assert.match(messages[1].result.content[0].text, /Guest Agent is required by the configured transport policy/u);
    assert.match(messages[1].result.content[0].text, /SSH and host command runners are disabled/u);
    assert.throws(() => readFileSync(invocationLog, 'utf8'), /ENOENT/u);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('apple-security-devices uses a native helper before exec argv', () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-fake-tart-fd-sanitizer-'));
  const fakeTart = join(directory, 'tart');
  const invocationLog = join(directory, 'invocations.log');
  try {
    writeFileSync(fakeTart, `#!/usr/bin/env node
const { appendFileSync } = require('node:fs');
const args = process.argv.slice(2);
if (args[0] !== 'exec') process.exit(99);
appendFileSync(process.env.APPLE_SECURITY_TEST_INVOCATION_LOG, JSON.stringify(args) + '\\n');
const attachInput = args[1] === '-i';
const commandIndex = attachInput ? 3 : 2;
const command = args[commandIndex];
const commandArgs = args.slice(commandIndex + 1);
if (command === '/bin/ls' && commandArgs.at(-1) === '/dev/fd') {
  process.stdout.write('0\\n1\\n2\\n3\\n4\\n5\\n');
  process.exit(0);
}
if (command === '/bin/dd' && commandArgs.some((item) => item.startsWith('of=/tmp/.beale-tart-exec-'))) {
  process.stdin.resume();
  process.stdin.on('end', () => process.exit(0));
  return;
}
if (command === '/bin/chmod' && commandArgs.at(-1).startsWith('/tmp/.beale-tart-exec-')) process.exit(0);
if (command === '/bin/mv' && commandArgs.at(-1) === '/tmp/.beale-tart-exec-v3') process.exit(0);
if (command.startsWith('/tmp/.beale-tart-exec-') && commandArgs[0] === '--beale-probe') {
  process.stdout.write('6\\n');
  process.exit(0);
}
if (command.startsWith('/tmp/.beale-tart-exec-')) {
  process.stdout.write('sanitized-ok');
  process.exit(0);
}
process.stderr.write('unexpected fake Tart exec');
process.exit(98);
`);
    chmodSync(fakeTart, 0o755);
    const messages = runServer([
      { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
      {
        jsonrpc: '2.0', id: 2, method: 'tools/call',
        params: { name: 'exec_tart_vm', arguments: { vmName: 'selected-vm', argv: ['/usr/bin/printf', '%s', 'safe value'], timeoutSeconds: 10 } }
      },
      {
        jsonrpc: '2.0', id: 3, method: 'tools/call',
        params: { name: 'exec_tart_vm', arguments: { vmName: 'selected-vm', argv: ['/usr/bin/printf', '%s', 'second value'], timeoutSeconds: 10 } }
      }
    ], {
      APPLE_SECURITY_TEST_PLATFORM: 'darwin',
      APPLE_SECURITY_TART_COMMAND: fakeTart,
      APPLE_SECURITY_TEST_TART_EXEC_HELPER: fakeTart,
      APPLE_SECURITY_TEST_INVOCATION_LOG: invocationLog
    });
    assert.equal(messages[1].result.isError, undefined, messages[1].result.content[0].text);
    const result = JSON.parse(messages[1].result.content[0].text);
    assert.equal(result.transport, 'guest-agent');
    assert.equal(result.stdout, 'sanitized-ok');
    assert.equal(messages[2].result.isError, undefined, messages[2].result.content[0].text);
    const calls = readFileSync(invocationLog, 'utf8').trim().split('\n').map((line) => JSON.parse(line));
    assert.deepEqual(calls[0], ['exec', 'selected-vm', '/bin/ls', '-1', '/dev/fd']);
    assert.equal(calls[1][0], 'exec');
    assert.equal(calls[1][1], '-i');
    assert.equal(calls[1][3], '/bin/dd');
    assert.equal(calls[2][2], '/bin/chmod');
    assert.equal(calls[3][2], '/bin/mv');
    assert.equal(calls[4][2], '/tmp/.beale-tart-exec-v3');
    assert.equal(calls[4][3], '--beale-probe');
    assert.deepEqual(calls[5].slice(3), [
      '/usr/bin/printf',
      '%s',
      'safe value'
    ]);
    assert.deepEqual(calls[6].slice(3), [
      '/usr/bin/printf',
      '%s',
      'second value'
    ]);
    assert.equal(calls.filter((call) => call.includes('--beale-probe')).length, 1);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('apple-security-devices repairs a guest helper removed by an external VM reset', () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-fake-tart-helper-repair-'));
  const fakeTart = join(directory, 'tart');
  const helperState = join(directory, 'helper-present');
  const invocationLog = join(directory, 'invocations.log');
  const executionLog = join(directory, 'executions.log');
  try {
    writeFileSync(fakeTart, `#!/usr/bin/env node
const { appendFileSync, existsSync, rmSync, writeFileSync } = require('node:fs');
const args = process.argv.slice(2);
appendFileSync(process.env.APPLE_SECURITY_TEST_INVOCATION_LOG, JSON.stringify(args) + '\\n');
if (args[0] !== 'exec') process.exit(99);
const attachInput = args[1] === '-i';
const commandIndex = attachInput ? 3 : 2;
const command = args[commandIndex];
const commandArgs = args.slice(commandIndex + 1);
if (command === '/bin/ls' && commandArgs.at(-1) === '/dev/fd') {
  process.stdout.write('0\\n1\\n2\\n3\\n4\\n5\\n');
  process.exit(0);
}
if (command === '/bin/dd') {
  process.stdin.resume();
  process.stdin.on('end', () => process.exit(0));
  return;
}
if (command === '/bin/chmod') process.exit(0);
if (command === '/bin/mv') {
  writeFileSync(process.env.APPLE_SECURITY_TEST_HELPER_STATE, 'present');
  process.exit(0);
}
if (command === '/tmp/.beale-tart-exec-v3') {
  if (!existsSync(process.env.APPLE_SECURITY_TEST_HELPER_STATE)) {
    process.stderr.write('Error: unknown (2): fork/exec /tmp/.beale-tart-exec-v3: no such file or directory');
    process.exit(1);
  }
  if (commandArgs[0] === '--beale-probe') {
    process.stdout.write('6\\n');
    process.exit(0);
  }
  appendFileSync(process.env.APPLE_SECURITY_TEST_EXECUTION_LOG, JSON.stringify(commandArgs) + '\\n');
  if (commandArgs.at(-1) === 'first') rmSync(process.env.APPLE_SECURITY_TEST_HELPER_STATE);
  process.stdout.write(commandArgs.at(-1));
  process.exit(0);
}
process.stderr.write('unexpected fake Tart exec');
process.exit(98);
`);
    chmodSync(fakeTart, 0o755);
    const messages = runServer([
      { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
      {
        jsonrpc: '2.0', id: 2, method: 'tools/call',
        params: { name: 'exec_tart_vm', arguments: { vmName: 'example-vm', argv: ['/usr/bin/printf', 'first'], timeoutSeconds: 10 } }
      },
      {
        jsonrpc: '2.0', id: 3, method: 'tools/call',
        params: { name: 'exec_tart_vm', arguments: { vmName: 'example-vm', argv: ['/usr/bin/printf', 'second'], timeoutSeconds: 10 } }
      }
    ], {
      APPLE_SECURITY_TEST_PLATFORM: 'darwin',
      APPLE_SECURITY_TART_COMMAND: fakeTart,
      APPLE_SECURITY_TEST_TART_EXEC_HELPER: fakeTart,
      APPLE_SECURITY_TEST_INVOCATION_LOG: invocationLog,
      APPLE_SECURITY_TEST_EXECUTION_LOG: executionLog,
      APPLE_SECURITY_TEST_HELPER_STATE: helperState
    });
    assert.equal(messages[1].result.isError, undefined, messages[1].result.content[0].text);
    assert.equal(messages[2].result.isError, undefined, messages[2].result.content[0].text);
    assert.equal(JSON.parse(messages[2].result.content[0].text).stdout, 'second');
    const calls = readFileSync(invocationLog, 'utf8').trim().split('\n').map((line) => JSON.parse(line));
    const executions = readFileSync(executionLog, 'utf8').trim().split('\n').map((line) => JSON.parse(line));
    assert.equal(calls.filter((call) => call.includes('/bin/dd')).length, 2);
    assert.deepEqual(executions, [
      ['/usr/bin/printf', 'first'],
      ['/usr/bin/printf', 'second']
    ]);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('apple-security-devices recycles Guest Agent descriptor pressure before running requested argv', () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-fake-tart-fd-recovery-'));
  const fakeTart = join(directory, 'tart');
  const statePath = join(directory, 'state');
  const invocationLog = join(directory, 'invocations.log');
  try {
    writeFileSync(fakeTart, `#!/usr/bin/env node
const { appendFileSync, existsSync, writeFileSync } = require('node:fs');
const args = process.argv.slice(2);
appendFileSync(process.env.APPLE_SECURITY_TEST_INVOCATION_LOG, JSON.stringify(args) + '\\n');
if (args[0] !== 'exec') process.exit(99);
const attachInput = args[1] === '-i';
const commandIndex = attachInput ? 3 : 2;
const command = args[commandIndex];
const commandArgs = args.slice(commandIndex + 1);
if (command === '/bin/ls' && commandArgs.at(-1) === '/dev/fd') {
  const count = existsSync(process.env.APPLE_SECURITY_TEST_STATE) ? 6 : 120;
  process.stdout.write(Array.from({ length: count }, (_, index) => String(index)).join('\\n') + '\\n');
  process.exit(0);
}
if (command === '/bin/launchctl' && commandArgs[0] === 'print') {
  process.stdout.write('  321      -  org.cirruslabs.tart-guest-rpc-example\\n');
  process.exit(0);
}
if (command === '/usr/bin/sudo' && commandArgs.includes('SIGTERM')) {
  writeFileSync(process.env.APPLE_SECURITY_TEST_STATE, 'recycled');
  process.exit(1);
}
if (command === '/bin/dd' && commandArgs.some((item) => item.startsWith('of=/tmp/.beale-tart-exec-'))) {
  process.stdin.resume();
  process.stdin.on('end', () => process.exit(0));
  return;
}
if (command === '/bin/chmod' && commandArgs.at(-1).startsWith('/tmp/.beale-tart-exec-')) process.exit(0);
if (command === '/bin/mv' && commandArgs.at(-1) === '/tmp/.beale-tart-exec-v3') process.exit(0);
if (command.startsWith('/tmp/.beale-tart-exec-') && commandArgs[0] === '--beale-probe') {
  process.stdout.write('6\\n');
  process.exit(0);
}
if (command.startsWith('/tmp/.beale-tart-exec-') && commandArgs[0] === '/usr/bin/printf') {
  process.stdout.write('recovered-ok');
  process.exit(0);
}
process.stderr.write('unexpected fake Tart exec');
process.exit(98);
`);
    chmodSync(fakeTart, 0o755);
    const messages = runServer([
      { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
      {
        jsonrpc: '2.0', id: 2, method: 'tools/call',
        params: { name: 'exec_tart_vm', arguments: { vmName: 'selected-vm', argv: ['/usr/bin/printf', 'recovered-ok'], timeoutSeconds: 10 } }
      }
    ], {
      APPLE_SECURITY_TEST_PLATFORM: 'darwin',
      APPLE_SECURITY_TART_COMMAND: fakeTart,
      APPLE_SECURITY_TEST_TART_EXEC_HELPER: fakeTart,
      APPLE_SECURITY_TEST_INVOCATION_LOG: invocationLog,
      APPLE_SECURITY_TEST_STATE: statePath
    });
    assert.equal(messages[1].result.isError, undefined, messages[1].result.content[0].text);
    const result = JSON.parse(messages[1].result.content[0].text);
    assert.equal(result.stdout, 'recovered-ok');
    const calls = readFileSync(invocationLog, 'utf8').trim().split('\n').map((line) => JSON.parse(line));
    const recycleIndex = calls.findIndex((call) => call.includes('SIGTERM'));
    const requestedCalls = calls.filter((call) => call.includes('/usr/bin/printf'));
    assert.ok(recycleIndex > 0);
    assert.equal(requestedCalls.length, 1);
    assert.ok(calls.indexOf(requestedCalls[0]) > recycleIndex);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('apple-security-devices stops on Guest Agent descriptor exhaustion without opening fallback transport', () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-fake-tart-fd-exhaustion-'));
  const fakeTart = join(directory, 'tart');
  const fakeSsh = join(directory, 'ssh');
  const fakeRunner = join(directory, 'runner');
  const invocationLog = join(directory, 'fallback.log');
  const tartInvocationLog = join(directory, 'tart.log');
  const identity = join(directory, 'identity');
  const knownHosts = join(directory, 'known-hosts');
  try {
    writeFileSync(fakeTart, `#!/usr/bin/env node
const { appendFileSync } = require('node:fs');
appendFileSync(process.env.APPLE_SECURITY_TEST_TART_INVOCATION_LOG, JSON.stringify(process.argv.slice(2)) + '\\n');
process.stderr.write('sudo: unable to create pipe: Too many open files');
process.exit(1);
`);
    const unexpectedFallback = `#!/usr/bin/env node
const { appendFileSync } = require('node:fs');
appendFileSync(process.env.APPLE_SECURITY_TEST_INVOCATION_LOG, 'fallback opened\\n');
process.exit(97);
`;
    writeFileSync(fakeSsh, unexpectedFallback);
    writeFileSync(fakeRunner, unexpectedFallback);
    writeFileSync(identity, 'test identity');
    writeFileSync(knownHosts, 'test known host');
    for (const executable of [fakeTart, fakeSsh, fakeRunner]) chmodSync(executable, 0o755);
    const messages = runServer([
      { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
      {
        jsonrpc: '2.0', id: 2, method: 'tools/call',
        params: { name: 'exec_tart_vm', arguments: { vmName: 'selected-vm', argv: ['/usr/bin/true'], timeoutSeconds: 10 } }
      },
      {
        jsonrpc: '2.0', id: 3, method: 'tools/call',
        params: { name: 'exec_tart_vm', arguments: { vmName: 'selected-vm', argv: ['/usr/bin/true'], timeoutSeconds: 10 } }
      }
    ], {
      APPLE_SECURITY_TEST_PLATFORM: 'darwin',
      APPLE_SECURITY_TART_COMMAND: fakeTart,
      APPLE_SECURITY_SSH_COMMAND: fakeSsh,
      APPLE_SECURITY_COMMAND_RUNNER: fakeRunner,
      APPLE_SECURITY_SSH_IDENTITY: identity,
      APPLE_SECURITY_SSH_KNOWN_HOSTS: knownHosts,
      APPLE_SECURITY_TEST_INVOCATION_LOG: invocationLog,
      APPLE_SECURITY_TEST_TART_INVOCATION_LOG: tartInvocationLog
    });
    assert.equal(messages[1].result.isError, true);
    assert.match(messages[1].result.content[0].text, /descriptor exhaustion was detected/u);
    assert.match(messages[1].result.content[0].text, /requested command was not replayed/u);
    assert.match(messages[1].result.content[0].text, /stop and restart/u);
    assert.equal(messages[2].result.isError, true);
    assert.match(messages[2].result.content[0].text, /latched off/u);
    assert.equal(readFileSync(tartInvocationLog, 'utf8').trim().split('\n').length, 1);
    assert.throws(() => readFileSync(invocationLog, 'utf8'), /ENOENT/u);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('apple-security-devices cancels an abandoned Tart request and releases the per-VM queue', () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-fake-tart-cancel-'));
  const fakeTart = join(directory, 'tart');
  try {
    writeFileSync(fakeTart, `#!/usr/bin/env node
const args = process.argv.slice(2);
if (args[0] !== 'exec') process.exit(99);
const attachInput = args[1] === '-i';
const commandIndex = attachInput ? 3 : 2;
const command = args[commandIndex];
const commandArgs = args.slice(commandIndex + 1);
if (command === '/bin/ls' && commandArgs.at(-1) === '/dev/fd') {
  process.stdout.write('0\\n1\\n2\\n3\\n4\\n5\\n');
  process.exit(0);
}
if (command === '/bin/dd') {
  process.stdin.resume();
  process.stdin.on('end', () => process.exit(0));
  return;
}
if (command === '/bin/chmod' || command === '/bin/mv') process.exit(0);
if (command.startsWith('/tmp/.beale-tart-exec-') && commandArgs[0] === '--beale-probe') {
  process.stdout.write('6\\n');
  process.exit(0);
}
if (command.startsWith('/tmp/.beale-tart-exec-') && commandArgs[0] === '/usr/bin/true') process.exit(0);
setInterval(() => {}, 1000);
`);
    chmodSync(fakeTart, 0o755);
    const messages = runServer([
      { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
      {
        jsonrpc: '2.0', id: 2, method: 'tools/call',
        params: { name: 'exec_tart_vm', arguments: { vmName: 'selected-vm', argv: ['/usr/bin/pending'], timeoutSeconds: 300 } }
      },
      { jsonrpc: '2.0', method: 'notifications/cancelled', params: { requestId: 2, reason: 'fixture timeout' } },
      {
        jsonrpc: '2.0', id: 3, method: 'tools/call',
        params: { name: 'exec_tart_vm', arguments: { vmName: 'selected-vm', argv: ['/usr/bin/true'], timeoutSeconds: 10 } }
      }
    ], {
      APPLE_SECURITY_TEST_PLATFORM: 'darwin',
      APPLE_SECURITY_TART_COMMAND: fakeTart,
      APPLE_SECURITY_TEST_TART_EXEC_HELPER: fakeTart
    });
    assert.equal(messages.some((message) => message.id === 2), false);
    const resumed = messages.find((message) => message.id === 3);
    assert.equal(resumed.result.isError, undefined, resumed.result.content[0].text);
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
let command = args[commandIndex];
let commandArgs = args.slice(commandIndex + 1);
if (command === '/bin/ls' && commandArgs.at(-1) === '/dev/fd') {
  process.stdout.write('0\\n1\\n2\\n3\\n4\\n5\\n');
  process.exit(0);
}
if (command === '/bin/chmod' && commandArgs.at(-1).startsWith('/tmp/.beale-tart-exec-')) process.exit(0);
if (command === '/bin/mv' && commandArgs.at(-1) === '/tmp/.beale-tart-exec-v3') process.exit(0);
if (command.startsWith('/tmp/.beale-tart-exec-') && commandArgs[0] === '--beale-probe') {
  process.stdout.write('6\\n');
  process.exit(0);
}
if (command.startsWith('/tmp/.beale-tart-exec-')) {
  command = commandArgs[0];
  commandArgs = commandArgs.slice(1);
}
if (command === '/bin/test' && commandArgs[0] === '-e') process.exit(existsSync(commandArgs[1]) ? 0 : 1);
if (command === '/bin/test' && commandArgs[0] === '-f') process.exit(existsSync(commandArgs[1]) && statSync(commandArgs[1]).isFile() ? 0 : 1);
if (command === '/bin/dd') {
  const output = commandArgs.find((item) => item.startsWith('of='))?.slice(3);
  if (output.startsWith('/tmp/.beale-tart-exec-')) {
    process.stdin.resume();
    process.stdin.on('end', () => process.exit(0));
    return;
  }
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
      APPLE_SECURITY_TART_COMMAND: fakeTart,
      APPLE_SECURITY_TEST_TART_EXEC_HELPER: fakeTart
    });
    const downloadMessages = runServer([
      { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
      {
        jsonrpc: '2.0', id: 3, method: 'tools/call',
        params: { name: 'copy_from_tart_vm', arguments: { vmName: 'selected-vm', guestPath: guest, localPath: downloaded, timeoutSeconds: 10 } }
      }
    ], {
      APPLE_SECURITY_TEST_PLATFORM: 'darwin',
      APPLE_SECURITY_TART_COMMAND: fakeTart,
      APPLE_SECURITY_TEST_TART_EXEC_HELPER: fakeTart
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
    return runServerWithPluginData(messages, pluginData, extraEnvironment);
  } finally {
    rmSync(pluginData, { recursive: true, force: true });
  }
}

function runServerWithPluginData(messages, pluginData, extraEnvironment = {}) {
  const result = spawnSync(process.execPath, [serverPath], {
    input: `${messages.map((message) => JSON.stringify(message)).join('\n')}\n`,
    encoding: 'utf8',
    env: { ...process.env, PLUGIN_ROOT: pluginRoot, PLUGIN_DATA: pluginData, ...extraEnvironment },
    timeout: 5000
  });
  assert.equal(result.status, 0, result.stderr);
  return result.stdout.trim().split('\n').filter(Boolean).map((line) => JSON.parse(line));
}

function waitUntil(predicate, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  const cell = new Int32Array(new SharedArrayBuffer(4));
  while (Date.now() < deadline) {
    if (predicate()) return true;
    Atomics.wait(cell, 0, 0, 20);
  }
  return predicate();
}

function processExists(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}
