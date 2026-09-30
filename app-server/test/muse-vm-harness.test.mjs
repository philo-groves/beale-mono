import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const harness = fileURLToPath(new URL('../resources/agent-plugins/meta-skills/skills/meta-bug-bounty-tools/scripts/muse-vm-harness.mjs', import.meta.url));

function run(...args) {
  return spawnSync(process.execPath, [harness, ...args], { encoding: 'utf8' });
}

test('Muse boundary harness creates an isolated run and assesses a synthetic capture', () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-muse-harness-'));
  try {
    const runDirectory = join(root, 'run');
    const init = run('init', '--out', runDirectory);
    assert.equal(init.status, 0, init.stderr);
    const manifest = JSON.parse(readFileSync(join(runDirectory, 'run.json'), 'utf8'));
    assert.match(manifest.runId, /^[a-f0-9]{32}$/);
    const probe = readFileSync(join(runDirectory, 'muse-runtime-probe.sh'), 'utf8');
    assert.match(probe, /\/proc\/self\/uid_map/);
    assert.match(probe, /END_BEALE_MUSE_BOUNDARY_V1/);
    assert.notEqual(run('init', '--out', runDirectory).status, 0);

    const capture = join(runDirectory, 'capture.txt');
    writeFileSync(capture, [
      `BEALE_MUSE_BOUNDARY_V1\t${manifest.runId}`,
      'uid\t0',
      'uid_map\t0 100000 65536',
      'cap_eff\t0000000000000000',
      'cap_bnd\t0000000000000000',
      `END_BEALE_MUSE_BOUNDARY_V1\t${manifest.runId}`,
      ''
    ].join('\n'));
    const assessed = run('assess', '--run', runDirectory, '--input', capture);
    assert.equal(assessed.status, 0, assessed.stderr);
    const report = JSON.parse(readFileSync(join(runDirectory, 'assessment.json'), 'utf8'));
    assert.equal(report.result, 'baseline-consistent');
    assert.ok(Object.values(report.checks).every(Boolean));
    assert.match(report.captureSha256, /^[a-f0-9]{64}$/);
    assert.notEqual(run('assess', '--run', runDirectory, '--input', capture).status, 0);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('Muse boundary harness flags capability deviations without claiming an escape', () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-muse-harness-'));
  try {
    const runDirectory = join(root, 'run');
    assert.equal(run('init', '--out', runDirectory).status, 0);
    const { runId } = JSON.parse(readFileSync(join(runDirectory, 'run.json'), 'utf8'));
    const capture = join(runDirectory, 'capture.txt');
    writeFileSync(capture, [
      `BEALE_MUSE_BOUNDARY_V1\t${runId}`,
      'uid\t0',
      'uid_map\t0 0 4294967295',
      'cap_eff\t0000000000081000',
      'cap_bnd\t0000000000081000',
      `END_BEALE_MUSE_BOUNDARY_V1\t${runId}`,
      ''
    ].join('\n'));
    assert.equal(run('assess', '--run', runDirectory, '--input', capture).status, 0);
    const report = JSON.parse(readFileSync(join(runDirectory, 'assessment.json'), 'utf8'));
    assert.equal(report.result, 'deviation-needs-review');
    assert.equal(report.checks.rootMappedAwayFromHostRoot, false);
    assert.equal(report.checks.effectiveNetAdminAbsent, false);
    assert.equal(report.checks.effectiveSysPtraceAbsent, false);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('Muse boundary harness rejects captures for another run', () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-muse-harness-'));
  try {
    const runDirectory = join(root, 'run');
    assert.equal(run('init', '--out', runDirectory).status, 0);
    const capture = join(runDirectory, 'capture.txt');
    writeFileSync(capture, 'BEALE_MUSE_BOUNDARY_V1\t00000000000000000000000000000000\n');
    const result = run('assess', '--run', runDirectory, '--input', capture);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /no matching run header/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
