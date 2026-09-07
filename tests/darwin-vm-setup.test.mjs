import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { AgentPluginRegistry } from '../packages/research-agent/dist/agent-plugin-registry.js';
import { inspectDarwinVmSetup, readDarwinVmSetup, saveDarwinVmSetup } from '../packages/research-agent/dist/darwin-vm-setup.js';

const pluginRoot = resolve('managed-plugins/apple-security-devices');

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), 'beale-darwin-setup-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const checkout = join(root, 'example-checkout');
  for (const file of ['qemu-sptm/build/qemu-system-aarch64', 'firmware/bootkc', 'firmware/dtree', 'firmware/ramdisk.tc', 'firmware/ramdisk.dmg']) {
    mkdirSync(dirname(join(checkout, file)), { recursive: true });
    writeFileSync(join(checkout, file), 'synthetic fixture; never execute');
  }
  return { root, checkout };
}

test('Darwin onboarding is gated by an enabled, valid plugin and retains opt-out across registry reloads', (t) => {
  const { root } = fixture(t);
  let registry = new AgentPluginRegistry(root);
  assert.equal(registry.getDarwinVmSetup().enabled, false);
  assert.throws(() => registry.updateDarwinVmSetup({ neverPrompt: true }), /Enable apple-security-devices/);
  const plugin = registry.addFromFilesystem(pluginRoot).plugins[0];
  assert.equal(registry.getDarwinVmSetup().enabled, true);
  registry.updateDarwinVmSetup({ neverPrompt: true });
  registry = new AgentPluginRegistry(root);
  assert.equal(registry.getDarwinVmSetup().neverPrompt, true);
  registry.setEnabled(plugin.id, false);
  assert.equal(registry.getDarwinVmSetup().enabled, false);
  registry.setEnabled(plugin.id, true);
  assert.equal(registry.getDarwinVmSetup().neverPrompt, true);
  registry.updateDarwinVmSetup({ neverPrompt: false });
  assert.equal(registry.getDarwinVmSetup().neverPrompt, false);
});

test('Darwin checkout validation rejects missing, empty and incomplete artifacts without overwriting configuration', (t) => {
  const { root, checkout } = fixture(t);
  assert.deepEqual(inspectDarwinVmSetup(checkout), []);
  saveDarwinVmSetup(root, { checkoutRoot: checkout });
  const saved = readDarwinVmSetup(root);
  assert.throws(() => saveDarwinVmSetup(root, { checkoutRoot: 'relative' }), /absolute/);
  assert.throws(() => saveDarwinVmSetup(root, { neverPrompt: 'true' }), /Invalid/);
  assert.deepEqual(readDarwinVmSetup(root), saved);
  writeFileSync(join(checkout, 'firmware/bootkc'), '');
  assert.match(inspectDarwinVmSetup(checkout).join(' '), /empty artifact/);
  writeFileSync(join(checkout, 'firmware/sptm'), 'synthetic');
  assert.match(inspectDarwinVmSetup(checkout).join(' '), /SPTM and TXM together/);
  rmSync(join(checkout, 'firmware/dtree'));
  assert.match(inspectDarwinVmSetup(checkout).join(' '), /Missing required artifact/);
  assert.throws(() => saveDarwinVmSetup(root, { checkoutRoot: checkout, neverPrompt: true }), /artifact/);
  assert.deepEqual(readDarwinVmSetup(root), saved);
});

test('saved Darwin checkout reaches the existing MCP inspection tool without new tools or a model-visible configuration file', (t) => {
  const { root, checkout } = fixture(t);
  const registry = new AgentPluginRegistry(root);
  registry.addFromFilesystem(pluginRoot);
  saveDarwinVmSetup(root, { checkoutRoot: checkout });
  const runtime = registry.getAppServerRuntime();
  const server = JSON.parse(readFileSync(runtime.mcpConfigPath, 'utf8')).servers['apple-security-devices.devices'];
  assert.equal(server.env.BEALE_DARWIN_VM_CHECKOUT, checkout);
  const messages = [
    { jsonrpc: '2.0', id: 1, method: 'tools/list', params: {} },
    { jsonrpc: '2.0', id: 2, method: 'tools/call', params: { name: 'inspect_darwin_vm', arguments: { hashArtifacts: true } } },
    { jsonrpc: '2.0', id: 3, method: 'tools/call', params: { name: 'inspect_darwin_vm', arguments: { checkoutRoot: join(root, 'missing') } } },
    { jsonrpc: '2.0', id: 4, method: 'tools/call', params: { name: 'environment_status', arguments: {} } }
  ];
  const result = spawnSync(process.execPath, [join(pluginRoot, 'server.mjs')], {
    env: { ...process.env, ...server.env }, encoding: 'utf8', timeout: 5000,
    input: `${messages.map((value) => JSON.stringify(value)).join('\n')}\n`
  });
  assert.equal(result.status, 0, result.stderr);
  const replies = result.stdout.trim().split('\n').map((line) => JSON.parse(line));
  const tools = replies.find((value) => value.id === 1).result.tools;
  assert.equal(tools.length, 19);
  for (const name of ['inspect_darwin_vm', 'start_darwin_vm']) {
    assert.equal(tools.find((tool) => tool.name === name).inputSchema.required?.includes('checkoutRoot') ?? false, false);
  }
  const inspection = JSON.parse(replies.find((value) => value.id === 2).result.content[0].text);
  assert.equal(inspection.ready, true);
  assert.equal(inspection.artifacts.find((artifact) => artifact.name === 'bootkc').sha256.length, 64);
  assert.equal(JSON.stringify(inspection).includes(checkout), false);
  assert.equal(replies.find((value) => value.id === 3).result.isError, true);
  const environment = JSON.parse(replies.find((value) => value.id === 4).result.content[0].text);
  assert.equal(environment.darwinVm.configured, true);
  assert.equal(environment.darwinVm.available, process.platform !== 'win32');
  assert.equal(JSON.stringify(environment.darwinVm).includes(checkout), false);
  rmSync(join(checkout, 'firmware/bootkc'));
  assert.equal(new AgentPluginRegistry(root).getDarwinVmSetup().prepared, false);
});

test('native Windows setup does not claim a runnable Darwin checkout', { skip: process.platform !== 'win32' }, (t) => {
  const { root, checkout } = fixture(t);
  const registry = new AgentPluginRegistry(root);
  registry.addFromFilesystem(pluginRoot);
  assert.throws(() => registry.updateDarwinVmSetup({ checkoutRoot: checkout }), /Windows native setup is not supported/);
  assert.equal(readDarwinVmSetup(root).checkoutRoot, null);
});
