import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { AgentPluginRegistry } from '../packages/research-agent/dist/agent-plugin-registry.js';

const pluginRoot = resolve('managed-plugins/apple-security-devices');
const serverPath = join(pluginRoot, 'server.mjs');

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

    const runtime = registry.getHoneycrispRuntime();
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

test('apple-security-devices MCP surface is bounded and marks every mutation for confirmation', () => {
  const messages = runServer([
    { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
    { jsonrpc: '2.0', id: 2, method: 'tools/list', params: {} }
  ]);
  assert.equal(messages[0].result.serverInfo.name, 'apple-security-devices');
  const tools = messages[1].result.tools;
  const expectedReadTools = [
    'environment_status',
    'list_tart_vms',
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
    'install_physical_iphone_app',
    'launch_physical_iphone_app',
    'start_darwin_vm',
    'stop_darwin_vm',
    'run_darwin_vm_console_command'
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
      assert.equal(policy.confirmation, 'always');
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

function runServer(messages) {
  const pluginData = mkdtempSync(join(tmpdir(), 'beale-apple-security-plugin-data-'));
  try {
    const result = spawnSync(process.execPath, [serverPath], {
      input: `${messages.map((message) => JSON.stringify(message)).join('\n')}\n`,
      encoding: 'utf8',
      env: { ...process.env, PLUGIN_ROOT: pluginRoot, PLUGIN_DATA: pluginData },
      timeout: 5000
    });
    assert.equal(result.status, 0, result.stderr);
    return result.stdout.trim().split('\n').filter(Boolean).map((line) => JSON.parse(line));
  } finally {
    rmSync(pluginData, { recursive: true, force: true });
  }
}
