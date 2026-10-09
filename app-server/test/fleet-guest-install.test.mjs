import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { installScript, normalizeArchitecture, startScript } from '../dist/fleetGuestInstall.js';
import { AppServerHostService } from '../dist/hostService.js';

test('Fleet guest installer recognizes the VM architecture before copying a host runtime', () => {
  assert.equal(normalizeArchitecture('arm64'), 'arm64');
  assert.equal(normalizeArchitecture('aarch64'), 'arm64');
  assert.equal(normalizeArchitecture('AMD64'), 'x64');
  assert.equal(normalizeArchitecture('x86_64'), 'x64');
  assert.equal(normalizeArchitecture('unknown'), null);
});

test('Tart install verifies its archive and starts only the session guest app-server', () => {
  const bundle = { archive: '/synthetic/archive.tar.gz', buildId: 'a'.repeat(24), sha256: 'b'.repeat(64) };
  const script = installScript('tart', bundle);
  const syntax = spawnSync('sh', ['-n'], { input: script, encoding: 'utf8' });
  assert.equal(syntax.status, 0, syntax.stderr);
  assert.match(script, /shasum -a 256/u);
  assert.match(script, /bin\/start\.mjs/u);
  assert.match(script, /restartSupervisorMain\.js/u);
  assert.match(script, /\$root\/version/u);
  assert.doesNotMatch(script, /sudo|providerToken|apiKey/u);
  assert.throws(() => startScript('tart', 'invalid; command'), /Invalid Fleet guest installation version/u);
});

test('Hyper-V install uses a user-scoped runtime and process launch', () => {
  const bundle = { archive: 'synthetic', buildId: 'a'.repeat(24), sha256: 'b'.repeat(64) };
  const script = installScript('hyper-v', bundle);
  assert.match(script, /Get-FileHash -Algorithm SHA256/u);
  assert.match(script, /Start-Process -FilePath/u);
  assert.match(script, /fleet-runtime/u);
  assert.match(script, /restartSupervisorMain\[\.\]js/u);
  assert.match(startScript('hyper-v', bundle.buildId), /bin\/start\.mjs/u);
  assert.doesNotMatch(script, /Restart-Computer|New-VM|providerToken|apiKey/u);
});

test('a VM-owning relay prepares a guest without requiring its own provider login', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-relay-install-example-'));
  try {
    const workspace = { workspaceId: 'workspace-example', workspacePath: directory,
      workspaceDirectories: [directory], name: 'Example workspace', researchKitId: 'general' };
    const service = new AppServerHostService({ registry: {
      registryDirectory: directory,
      resolveWorkspace: () => workspace,
      providerSettings: () => ({ authenticationPreferences: {}, riskAcknowledgements: [] }),
    } });
    let prepared = null;
    service.fleetRemote.prepare = async (...args) => {
      prepared = args;
      return { machineId: 'tart:session-clone', url: 'http://127.0.0.1:12345', operatorToken: 'synthetic-token' };
    };
    await service.executeOperation({ operation: 'fleet.prepare', input: {
      workspaceId: workspace.workspaceId, runId: 'run-example', machineId: 'tart:session-clone',
      ownerMachineId: 'machine-source',
    } });
    assert.equal(prepared?.[3], 'machine-source');
    assert.equal(prepared?.[4], undefined);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
