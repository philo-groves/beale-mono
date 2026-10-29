import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createRequire } from 'node:module';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { installIpswArchive, stageIpswBinary } from '../packages/research-agent/dist/ipsw-binary.js';

const require = createRequire(new URL('../packages/research-agent/package.json', import.meta.url));
const tar = require('tar');
const { zipSync } = require('fflate');

function sha256(bytes) { return createHash('sha256').update(bytes).digest('hex'); }

test('ipsw ZIP extraction checks the pinned digest and keeps only the executable and license', async () => {
  const root = await mkdtemp(join(tmpdir(), 'beale-ipsw-zip-'));
  const archivePath = join(root, 'release.zip');
  const destinationDirectory = join(root, 'installed');
  const archive = zipSync({
    'ipsw.exe': Buffer.from('synthetic executable'),
    LICENSE: Buffer.from('synthetic license'),
    'unrelated.txt': Buffer.from('not shipped'),
  });
  try {
    await writeFile(archivePath, archive);
    await assert.rejects(installIpswArchive({ archivePath, expectedSha256: '0'.repeat(64),
      destinationDirectory, executable: 'ipsw.exe' }), /checksum/u);
    const installed = await installIpswArchive({ archivePath, expectedSha256: sha256(archive),
      destinationDirectory, executable: 'ipsw.exe' });
    assert.equal(await readFile(installed, 'utf8'), 'synthetic executable');
    assert.equal(await readFile(join(destinationDirectory, 'LICENSE'), 'utf8'), 'synthetic license');
    await assert.rejects(readFile(join(destinationDirectory, 'unrelated.txt')), { code: 'ENOENT' });
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('ipsw tar extraction checks the pinned digest and keeps only the executable and license', async () => {
  const root = await mkdtemp(join(tmpdir(), 'beale-ipsw-tar-'));
  const source = join(root, 'source');
  const archivePath = join(root, 'release.tar.gz');
  const destinationDirectory = join(root, 'installed');
  try {
    await mkdir(source);
    await writeFile(join(source, 'ipsw'), 'synthetic executable');
    await writeFile(join(source, 'LICENSE'), 'synthetic license');
    await writeFile(join(source, 'unrelated.txt'), 'not shipped');
    await tar.c({ gzip: true, file: archivePath, cwd: source }, ['ipsw', 'LICENSE', 'unrelated.txt']);
    const digest = sha256(await readFile(archivePath));
    const installed = await installIpswArchive({ archivePath, expectedSha256: digest,
      destinationDirectory, executable: 'ipsw' });
    assert.equal(await readFile(installed, 'utf8'), 'synthetic executable');
    assert.equal(await readFile(join(destinationDirectory, 'LICENSE'), 'utf8'), 'synthetic license');
    await assert.rejects(readFile(join(destinationDirectory, 'unrelated.txt')), { code: 'ENOENT' });
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('ipsw staging selects the requested installer architecture instead of the build host', {
  skip: process.platform === 'win32',
}, async () => {
  const root = await mkdtemp(join(tmpdir(), 'beale-ipsw-stage-'));
  const previousHome = process.env.HOME;
  const target = { platform: process.platform, arch: process.arch === 'arm64' ? 'x64' : 'arm64' };
  const cached = join(root, '.beale', 'tools', 'ipsw', 'v3.1.739', `${target.platform}-${target.arch}`);
  try {
    await mkdir(cached, { recursive: true });
    await writeFile(join(cached, 'ipsw'), 'synthetic cross-architecture executable', { mode: 0o755 });
    await writeFile(join(cached, 'LICENSE'), 'synthetic license');
    process.env.HOME = root;
    const staged = await stageIpswBinary(join(root, 'staged'), target);
    assert.equal(staged, join(root, 'staged', `${target.platform}-${target.arch}`, 'ipsw'));
    assert.equal(await readFile(staged, 'utf8'), 'synthetic cross-architecture executable');
    assert.equal(await readFile(join(root, 'staged', `${target.platform}-${target.arch}`, 'LICENSE'), 'utf8'),
      'synthetic license');
  } finally {
    if (previousHome === undefined) delete process.env.HOME;
    else process.env.HOME = previousHome;
    await rm(root, { recursive: true, force: true });
  }
});
