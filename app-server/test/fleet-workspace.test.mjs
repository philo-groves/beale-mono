import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { FleetWorkspaceStore } from '../dist/fleetWorkspace.js';

test('Fleet stages checked file chunks and preserves concurrent primary edits as conflicts', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-files-'));
  const primary = join(root, 'primary');
  mkdirSync(primary);
  writeFileSync(join(primary, 'note.md'), 'original');
  const store = new FleetWorkspaceStore({ registryDirectory: join(root, 'guest') });
  const workspaceId = 'workspace-example';
  try {
    await store.guestStage({ action: 'begin', workspaceId });
    await assert.rejects(store.guestStage({ action: 'write', workspaceId, path: '../escape', offset: 0, data: '' }), /Invalid Fleet workspace file path/);
    const note = Buffer.from('guest result');
    await store.guestStage({ action: 'write', workspaceId, path: 'note.md', offset: 0, data: note.toString('base64') });
    await assert.rejects(store.guestStage({ action: 'seal', workspaceId, path: 'note.md', sha256: '0'.repeat(64) }), /integrity/);
    await store.guestStage({ action: 'seal', workspaceId, path: 'note.md', sha256: digest(note) });
    const listed = store.guestExport({ action: 'list', workspaceId });
    assert.equal(listed.files[0].sha256, digest(note));
    const fetched = store.guestExport({ action: 'read', workspaceId, path: 'note.md', offset: 0 });
    assert.equal(Buffer.from(fetched.data, 'base64').toString(), 'guest result');

    const baseline = { workspaceId, workspacePath: primary, machineId: 'tart:example-worker', runId: 'run-example', files: { 'note.md': digest(Buffer.from('original')) } };
    writeFileSync(join(primary, 'note.md'), 'primary changed');
    const outcome = await store.importGuestFile(baseline, listed.files[0], async () => fetched.data);
    assert.equal(outcome, 'conflict');
    assert.equal(readFileSync(join(primary, 'note.md'), 'utf8'), 'primary changed');
    assert.equal(readFileSync(join(primary, '.beale', 'fleet-conflicts', 'run-example', 'note.md'), 'utf8'), 'guest result');
    const claim = Buffer.from('{"id":"claim-example"}');
    const candidate = await store.importGuestFile(baseline,
      { path: 'claims/claim-example.json', size: claim.length, sha256: digest(claim) },
      async () => claim.toString('base64'));
    assert.equal(candidate, 'candidate');
    assert.equal(readFileSync(join(primary, '.beale', 'fleet-guest-records', 'run-example', 'claims', 'claim-example.json'), 'utf8'), claim.toString());
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

function digest(bytes) { return createHash('sha256').update(bytes).digest('hex'); }
