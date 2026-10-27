#!/usr/bin/env node
import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { readReleaseNotes } from './release-notes.mjs';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const manifests = [
  'package.json',
  'apps/desktop/package.json',
  'app-server/package.json',
  'packages/research-agent/package.json',
  'packages/app-server-runtime/package.json',
];
const sources = new Map([
  ['packages/app-server-runtime/src/protocol.ts', /APP_SERVER_RUNTIME_VERSION = "(?<version>[^"\n]+)"/gu],
  ['app-server/src/providerUsageLimits.ts', /clientInfo: \{ name: 'beale', title: 'Beale', version: '(?<version>[^'\n]+)'/gu],
  ['packages/research-agent/src/auth-routing.ts', /CLAUDE_AGENT_SDK_CLIENT_APP: "appServer\/(?<version>[^"\n]+)"/gu],
  ['packages/research-agent/src/mcp-client.ts', /version: "(?<version>[^"\n]+)"/gu],
  ['packages/research-agent/src/zcode-agent-executor.ts', /serverInfo: \{ name: "app-server", version: "(?<version>[^"\n]+)"/gu],
]);
const semver = /^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?$/u;

function readVersions() {
  const values = new Map();
  for (const path of manifests) {
    values.set(path, JSON.parse(readFileSync(join(root, path), 'utf8')).version);
  }
  for (const [path, pattern] of sources) {
    const matches = [...readFileSync(join(root, path), 'utf8').matchAll(pattern)];
    if (matches.length !== 1) throw new Error(`Expected one release version in ${path}; found ${matches.length}.`);
    values.set(path, matches[0].groups.version);
  }
  return values;
}

function check(tag) {
  const versions = readVersions();
  const version = versions.get('package.json');
  if (!semver.test(version)) throw new Error(`Invalid release version: ${version}`);
  const mismatches = [...versions].filter(([, value]) => value !== version);
  if (mismatches.length) {
    throw new Error(`Release version is ${version}; mismatches: ${mismatches.map(([path, value]) => `${path}=${value}`).join(', ')}`);
  }
  const changelogHeadings = readFileSync(join(root, 'CHANGELOG.md'), 'utf8')
    .split(/\r?\n/u).filter((line) => line.startsWith('## '));
  if (changelogHeadings[0] !== '## Unreleased' || !changelogHeadings.includes(`## ${version}`)) {
    throw new Error(`CHANGELOG.md must start with Unreleased and include a ## ${version} section.`);
  }
  readReleaseNotes(version);
  if (tag !== undefined) {
    if (tag !== `v${version}`) throw new Error(`Tag ${tag} does not match v${version}.`);
    const type = execFileSync('git', ['cat-file', '-t', `refs/tags/${tag}`], { cwd: root, encoding: 'utf8' }).trim();
    if (type !== 'tag') throw new Error(`Release tag ${tag} must be annotated.`);
    const taggedCommit = execFileSync('git', ['rev-parse', `refs/tags/${tag}^{commit}`], { cwd: root, encoding: 'utf8' }).trim();
    const head = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim();
    if (taggedCommit !== head) throw new Error(`Release tag ${tag} does not point to HEAD.`);
  }
  console.log(`Beale release version ${version} is consistent${tag ? ` with ${tag}` : ''}.`);
  return version;
}

function setVersion(next) {
  if (!semver.test(next)) throw new Error(`Invalid release version: ${next}`);
  const previous = check();
  if (next === previous) return;
  const changelog = readFileSync(join(root, 'CHANGELOG.md'), 'utf8');
  if (!changelog.split(/\r?\n/u).includes(`## ${next}`)) {
    throw new Error(`CHANGELOG.md must include a ## ${next} section before changing versions.`);
  }
  for (const path of manifests) {
    const file = join(root, path);
    const manifest = JSON.parse(readFileSync(file, 'utf8'));
    manifest.version = next;
    writeFileSync(file, `${JSON.stringify(manifest, null, 2)}\n`);
  }
  for (const [path, pattern] of sources) {
    const file = join(root, path);
    const content = readFileSync(file, 'utf8');
    writeFileSync(file, content.replace(pattern, (match) => match.replace(previous, next)));
  }
  check();
}

try {
  const [command, argument, extra] = process.argv.slice(2);
  if (command === 'check' && extra === undefined && (argument === undefined || argument.startsWith('v'))) check(argument);
  else if (command === 'set' && argument && extra === undefined) setVersion(argument);
  else throw new Error('Usage: node scripts/release-version.mjs check [vX.Y.Z] | set X.Y.Z');
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}
