import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createRequire } from 'node:module';
import { mkdtemp, mkdir, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { stagePortableBinary, unpackPortableArtifact } from '../packages/research-agent/dist/portable-binaries.js';

const require = createRequire(new URL('../packages/research-agent/package.json', import.meta.url));
const tar = require('tar');
const { zipSync } = require('fflate');
const digest = (bytes) => createHash('sha256').update(bytes).digest('hex');

test('jq direct artifact requires the pinned checksum', async () => {
  const root = await mkdtemp(join(tmpdir(), 'beale-jq-artifact-'));
  const artifactPath = join(root, 'jq-release');
  const destinationDirectory = join(root, 'installed');
  try {
    await writeFile(artifactPath, 'synthetic jq executable');
    await assert.rejects(unpackPortableArtifact({ artifactPath, expectedSha256: '0'.repeat(64),
      destinationDirectory, executable: 'jq', licenses: [] }), /checksum/u);
    await unpackPortableArtifact({ artifactPath, expectedSha256: digest(await readFile(artifactPath)),
      destinationDirectory, executable: 'jq', licenses: [] });
    assert.equal(await readFile(join(destinationDirectory, 'jq'), 'utf8'), 'synthetic jq executable');
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('fd tar artifact retains only its executable and licenses', async () => {
  const root = await mkdtemp(join(tmpdir(), 'beale-fd-tar-'));
  const source = join(root, 'source');
  const prefix = 'fd-v-example-aarch64-apple-darwin';
  const archivePath = join(root, 'release.tar.gz');
  const destinationDirectory = join(root, 'installed');
  try {
    await mkdir(join(source, prefix), { recursive: true });
    for (const [name, value] of [['fd', 'synthetic fd executable'], ['LICENSE-MIT', 'synthetic MIT license'],
      ['LICENSE-APACHE', 'synthetic Apache license'], ['unrelated.txt', 'not shipped']]) {
      await writeFile(join(source, prefix, name), value);
    }
    await tar.c({ gzip: true, file: archivePath, cwd: source }, [prefix]);
    await unpackPortableArtifact({ artifactPath: archivePath, expectedSha256: digest(await readFile(archivePath)),
      destinationDirectory, executable: 'fd', licenses: ['LICENSE-MIT', 'LICENSE-APACHE'], archivePrefix: prefix });
    assert.equal(await readFile(join(destinationDirectory, 'fd'), 'utf8'), 'synthetic fd executable');
    assert.equal(await readFile(join(destinationDirectory, 'LICENSE-MIT'), 'utf8'), 'synthetic MIT license');
    assert.equal(await readFile(join(destinationDirectory, 'LICENSE-APACHE'), 'utf8'), 'synthetic Apache license');
    await assert.rejects(readFile(join(destinationDirectory, 'unrelated.txt')), { code: 'ENOENT' });
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('fd ZIP artifact retains only its executable and licenses', async () => {
  const root = await mkdtemp(join(tmpdir(), 'beale-fd-zip-'));
  const prefix = 'fd-v-example-x86_64-pc-windows-msvc';
  const archivePath = join(root, 'release.zip');
  const destinationDirectory = join(root, 'installed');
  const archive = zipSync({
    [`${prefix}/fd.exe`]: Buffer.from('synthetic fd executable'),
    [`${prefix}/LICENSE-MIT`]: Buffer.from('synthetic MIT license'),
    [`${prefix}/LICENSE-APACHE`]: Buffer.from('synthetic Apache license'),
    [`${prefix}/unrelated.txt`]: Buffer.from('not shipped'),
  });
  try {
    await writeFile(archivePath, archive);
    await unpackPortableArtifact({ artifactPath: archivePath, expectedSha256: digest(archive),
      destinationDirectory, executable: 'fd.exe', licenses: ['LICENSE-MIT', 'LICENSE-APACHE'], archivePrefix: prefix });
    assert.equal(await readFile(join(destinationDirectory, 'fd.exe'), 'utf8'), 'synthetic fd executable');
    await assert.rejects(readFile(join(destinationDirectory, 'unrelated.txt')), { code: 'ENOENT' });
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('portable binary staging selects the installer architecture and copies licenses', {
  skip: process.platform === 'win32',
}, async () => {
  const root = await mkdtemp(join(tmpdir(), 'beale-portable-stage-'));
  const previousHome = process.env.HOME;
  const target = { platform: process.platform, arch: process.arch === 'arm64' ? 'x64' : 'arm64' };
  const key = `${target.platform}-${target.arch}`;
  try {
    for (const [name, version, licenses] of [
      ['jq', '1.8.2', ['COPYING']],
      ['fd', '10.5.0', ['LICENSE-MIT', 'LICENSE-APACHE']],
      ['opengrep', '1.33.0', ['LICENSE']],
    ]) {
      const cache = join(root, '.beale', 'tools', name, `v${version}`, key);
      await mkdir(cache, { recursive: true });
      await writeFile(join(cache, name), `synthetic ${name} executable`, { mode: 0o755 });
      for (const license of licenses) await writeFile(join(cache, license), `synthetic ${license}`);
    }
    process.env.HOME = root;
    for (const name of ['jq', 'fd', 'opengrep']) {
      const staged = await stagePortableBinary(name, join(root, 'staged', name), target);
      assert.equal(staged, join(root, 'staged', name, key, name));
      assert.equal(await readFile(staged, 'utf8'), `synthetic ${name} executable`);
    }
    assert.equal(await readFile(join(root, 'staged', 'jq', key, 'COPYING'), 'utf8'), 'synthetic COPYING');
    assert.equal((await stat(join(root, 'staged', 'jq', key, 'COPYING'))).mode & 0o777, 0o644);
    assert.equal(await readFile(join(root, 'staged', 'fd', key, 'LICENSE-MIT'), 'utf8'), 'synthetic LICENSE-MIT');
    assert.equal(await readFile(join(root, 'staged', 'opengrep', key, 'LICENSE'), 'utf8'), 'synthetic LICENSE');
  } finally {
    if (previousHome === undefined) delete process.env.HOME;
    else process.env.HOME = previousHome;
    await rm(root, { recursive: true, force: true });
  }
});
