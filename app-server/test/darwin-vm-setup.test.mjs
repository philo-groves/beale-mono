import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { invokeAppServerProtocol } from '../dist/appServerProtocolClient.js';

test('canonical Darwin setup operations preserve opt-out and reject malformed updates', async (t) => {
  const registryDirectory = mkdtempSync(join(tmpdir(), 'beale-darwin-protocol-test-'));
  t.after(() => rmSync(registryDirectory, { recursive: true, force: true }));
  const input = {
    registryDirectory,
    builtinPlugins: [{ id: 'example-device-plugin', path: fileURLToPath(new URL('../../managed-plugins/apple-security-devices', import.meta.url)), installedAt: '2026-01-01T00:00:00.000Z' }]
  };
  const initial = await invokeAppServerProtocol('plugin.darwin_vm.get', { args: [], input });
  assert.equal(initial.enabled, true);
  assert.equal(initial.neverPrompt, false);
  await invokeAppServerProtocol('plugin.darwin_vm.update', { args: [], input: { ...input, update: { neverPrompt: true } } });
  const saved = await invokeAppServerProtocol('plugin.darwin_vm.get', { args: [], input });
  assert.equal(saved.neverPrompt, true);
  await assert.rejects(invokeAppServerProtocol('plugin.darwin_vm.update', { args: [], input: { ...input, update: { neverPrompt: 'yes' } } }), /Invalid Darwin VM setup update/);
});
