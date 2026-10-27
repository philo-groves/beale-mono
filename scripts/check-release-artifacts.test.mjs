import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { stringify } from 'yaml';
import { checkReleaseArtifacts } from './check-release-artifacts.mjs';

const version = '1.2.3';
const dmg = `Beale-${version}-arm64.dmg`;
const zip = `Beale-${version}-arm64-mac.zip`;
const exe = `Beale-Setup-${version}.exe`;

async function fixture(run) {
  const directory = await mkdtemp(join(tmpdir(), 'beale-release-check-'));
  try {
    const metadata = new Map();
    for (const name of [dmg, zip, exe]) {
      const bytes = Buffer.from(`synthetic ${name}`);
      await writeFile(join(directory, name), bytes);
      await writeFile(join(directory, `${name}.blockmap`), 'synthetic blockmap');
      metadata.set(name, { url: name, size: bytes.length, sha512: createHash('sha512').update(bytes).digest('base64') });
    }
    for (const [name, files, primary] of [
      ['latest-mac.yml', [zip, dmg], zip],
      ['latest.yml', [exe], exe],
    ]) {
      await writeFile(join(directory, name), stringify({ version, files: files.map((file) => metadata.get(file)), path: primary, sha512: metadata.get(primary).sha512 }));
    }
    await run(directory);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

test('accepts matching cross-platform release artifacts', async () => {
  await fixture((directory) => checkReleaseArtifacts(directory, version, 'both'));
});

test('accepts each packaging job output independently', async () => {
  await fixture(async (directory) => {
    for (const name of [exe, `${exe}.blockmap`, 'latest.yml']) await rm(join(directory, name));
    await checkReleaseArtifacts(directory, version, 'macos');
  });
  await fixture(async (directory) => {
    for (const name of [dmg, `${dmg}.blockmap`, zip, `${zip}.blockmap`, 'latest-mac.yml']) {
      await rm(join(directory, name));
    }
    await checkReleaseArtifacts(directory, version, 'windows');
  });
});

test('rejects a feed URL whose installer is absent', async () => {
  await fixture(async (directory) => {
    const path = join(directory, 'latest.yml');
    await writeFile(path, (await readFile(path, 'utf8')).replaceAll(exe, `Beale.Setup.${version}.exe`));
    await assert.rejects(checkReleaseArtifacts(directory, version, 'both'), /primary artifact|unexpected or duplicate artifact URL/u);
  });
});

test('rejects a GitHub-normalized installer filename', async () => {
  await fixture(async (directory) => {
    await rm(join(directory, exe));
    await writeFile(join(directory, `Beale.Setup.${version}.exe`), 'synthetic renamed installer');
    await assert.rejects(checkReleaseArtifacts(directory, version, 'both'), /Unexpected release artifact/u);
  });
});

test('rejects an extra update feed selected by the upload glob', async () => {
  await fixture(async (directory) => {
    await writeFile(join(directory, 'latest-other.yml'), 'version: 1.2.3');
    await assert.rejects(checkReleaseArtifacts(directory, version, 'both'), /Unexpected number of release artifacts/u);
  });
});

test('rejects an artifact changed after feed generation', async () => {
  await fixture(async (directory) => {
    await writeFile(join(directory, exe), 'different payload');
    await assert.rejects(checkReleaseArtifacts(directory, version, 'both'), /size or SHA-512 mismatch/u);
  });
});
