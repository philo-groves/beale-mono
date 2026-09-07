import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { AgentPluginRegistry } from '../packages/research-agent/dist/agent-plugin-registry.js';
import { checkCanary } from '../managed-plugins/microsoft-security-devices/skills/microsoft-security-devices/scripts/check-canary.ts';

const pluginRoot = resolve('managed-plugins/microsoft-security-devices');
const scriptPath = join(pluginRoot, 'skills/microsoft-security-devices/scripts/check-canary.ts');
const now = Date.parse('2026-01-01T12:00:00.000Z');

function input() {
  return {
    sessionStartedAt: '2026-01-01T10:00:00.000Z',
    guest: {
      build: '99000.9', buildLabEx: '99000.1.amd64fre.example_branch.260101-0000',
      architecture: 'x64', channel: 'Canary', track: 'example-track', observedAt: '2026-01-01T10:01:00.000Z'
    },
    latest: {
      build: '99000.10', architecture: 'x64', channel: 'Canary', track: 'example-track',
      sourceUrl: 'https://learn.microsoft.com/en-us/windows-insider/flight-hub/',
      publishedAt: '2026-01-01T09:00:00.000Z', checkedAt: '2026-01-01T10:02:00.000Z'
    }
  };
}

test('Microsoft device guidance is an optional portable plugin with no model-facing tools', (t) => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-microsoft-plugin-test-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const registry = new AgentPluginRegistry(directory);
  assert.deepEqual(registry.getState().plugins, []);
  const plugin = registry.addFromFilesystem(pluginRoot).plugins[0];
  assert.equal(plugin.name, 'microsoft-security-devices');
  assert.equal(plugin.status, 'ready');
  assert.deepEqual(plugin.errors, []);
  assert.deepEqual(plugin.warnings, []);
  assert.deepEqual(plugin.skills.map((skill) => skill.id), ['microsoft-security-devices']);
  assert.deepEqual(plugin.mcpServers, []);
  const runtime = registry.getAppServerRuntime();
  assert.deepEqual(runtime.selectedSkillIds, ['microsoft-security-devices']);
  assert.deepEqual(runtime.allowedMcpServers, []);
  assert.equal(runtime.mcpConfigPath, null);
  registry.setEnabled(plugin.id, false);
  assert.deepEqual(new AgentPluginRegistry(directory).getAppServerRuntime().selectedSkillIds, []);
});

test('comparison distinguishes update revisions, newer builds, equality, and guests ahead of the reference', () => {
  const candidate = input();
  let result = checkCanary(candidate, now);
  assert.equal(result.status, 'behind');
  assert.match(result.message, /99000\.9.*behind.*99000\.10/);
  assert.equal(result.sourceUrl, candidate.latest.sourceUrl);
  assert.equal(result.sourceCheckedAt, candidate.latest.checkedAt);
  candidate.guest.build = '99000.10';
  assert.equal(checkCanary(candidate, now).status, 'matches-reference');
  candidate.guest.build = '99000.11';
  result = checkCanary(candidate, now);
  assert.equal(result.status, 'unknown');
  assert.match(result.warnings.join(' '), /ahead/);
  candidate.guest.build = '98999.9999';
  assert.equal(checkCanary(candidate, now).status, 'behind');
});

test('missing, malformed, and offline observations are unknown instead of assumed current', () => {
  for (const candidate of [null, [], {}, { ...input(), latest: null }]) {
    assert.equal(checkCanary(candidate, now).status, 'unknown');
  }
  for (const build of ['99000', 'Windows 11', '99000.NaN', '99000.-1', '99000.1.2', null]) {
    const candidate = input();
    candidate.guest.build = build;
    assert.equal(checkCanary(candidate, now).status, 'unknown');
  }
  const candidate = input();
  candidate.guest.buildLabEx = '';
  assert.equal(checkCanary(candidate, now).status, 'unknown');
});

test('channel, architecture, and track mismatches never use numerical build order to infer applicability', () => {
  for (const [key, value] of [['channel', 'Beta'], ['architecture', 'arm64'], ['track', 'other-example-track']]) {
    const candidate = input();
    candidate.latest[key] = value;
    const result = checkCanary(candidate, now);
    assert.equal(result.status, 'unknown');
    assert.match(result.warnings.join(' '), /Architecture, channel, or track/);
  }
  const candidate = input();
  candidate.guest.channel = candidate.latest.channel = 'Release Preview';
  assert.equal(checkCanary(candidate, now).status, 'unknown');
});

test('Experimental successor comparisons retain the Canary eligibility warning and do not alias channels', () => {
  const candidate = input();
  candidate.latest.channel = 'Experimental (Future Platforms)';
  assert.equal(checkCanary(candidate, now).status, 'unknown');
  candidate.guest.channel = candidate.latest.channel;
  let result = checkCanary(candidate, now);
  assert.equal(result.status, 'behind');
  assert.match(result.warnings.join(' '), /bounty guidance names Canary/);
  candidate.guest.build = candidate.latest.build;
  result = checkCanary(candidate, now);
  assert.equal(result.status, 'matches-reference');
  assert.match(result.warnings.join(' '), /does not resolve/);
});

test('prior-session, stale, future, and missing timestamps cannot establish freshness', () => {
  const changes = [
    ['guest', 'observedAt', '2026-01-01T09:59:00.000Z'],
    ['guest', 'observedAt', '2026-01-02T00:00:00.000Z'],
    ['latest', 'checkedAt', '2026-01-01T09:59:00.000Z'],
    ['latest', 'checkedAt', '2026-01-02T00:00:00.000Z'],
    ['latest', 'publishedAt', '2026-01-02T00:00:00.000Z'],
    ['latest', 'publishedAt', 'not-a-date']
  ];
  for (const [section, field, value] of changes) {
    const candidate = input();
    candidate[section][field] = value;
    assert.equal(checkCanary(candidate, now).status, 'unknown');
  }
  const stale = input();
  stale.sessionStartedAt = '2025-12-01T00:00:00.000Z';
  stale.guest.observedAt = '2025-12-31T09:00:00.000Z';
  assert.equal(checkCanary(stale, now).status, 'unknown');
  stale.guest.observedAt = input().guest.observedAt;
  stale.latest.checkedAt = stale.latest.publishedAt = '2025-12-31T09:00:00.000Z';
  assert.equal(checkCanary(stale, now).status, 'unknown');
});

test('the comparison only accepts official release-reference URL shapes', () => {
  for (const sourceUrl of [
    'https://example.test/latest', 'https://learn.microsoft.com.example.test/en-us/windows-insider/flight-hub/',
    'http://learn.microsoft.com/en-us/windows-insider/flight-hub/',
    'https://example@learn.microsoft.com/en-us/windows-insider/flight-hub/',
    'https://learn.microsoft.com/en-us/unrelated/',
    'https://blogs.windows.com/unrelated/'
  ]) {
    const candidate = input();
    candidate.latest.sourceUrl = sourceUrl;
    assert.equal(checkCanary(candidate, now).status, 'unknown');
  }
});

test('date-only source publication preserves available precision and rejects invalid calendar dates', () => {
  const candidate = input();
  candidate.latest.publishedAt = '2026-01-01';
  assert.equal(checkCanary(candidate, now).status, 'behind');
  candidate.latest.publishedAt = '2025-02-30';
  assert.equal(checkCanary(candidate, now).status, 'unknown');
});

test('standalone CLI runs from an unrelated directory and returns bounded JSON without altering its input', (t) => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-canary-cli-test-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const path = join(directory, 'example-input.json');
  const candidate = input();
  const recent = new Date(Date.now() - 60_000).toISOString();
  candidate.sessionStartedAt = candidate.guest.observedAt = candidate.latest.checkedAt = recent;
  const original = JSON.stringify(candidate);
  writeFileSync(path, original);
  const run = spawnSync(process.execPath, [scriptPath, path], { cwd: directory, encoding: 'utf8', timeout: 5000 });
  assert.equal(run.status, 0, run.stderr);
  assert.equal(JSON.parse(run.stdout).status, 'behind');
  assert.equal(readFileSync(path, 'utf8'), original);
  const missing = spawnSync(process.execPath, [scriptPath], { cwd: directory, encoding: 'utf8', timeout: 5000 });
  assert.equal(missing.status, 2);
  assert.equal(JSON.parse(missing.stdout).status, 'unknown');
  writeFileSync(path, ' '.repeat(65537));
  const oversized = spawnSync(process.execPath, [scriptPath, path], { cwd: directory, encoding: 'utf8', timeout: 5000 });
  assert.equal(oversized.status, 2);
  assert.equal(JSON.parse(oversized.stdout).status, 'unknown');
});
