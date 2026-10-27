#!/usr/bin/env node
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { lstat, readFile, readdir } from 'node:fs/promises';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { parseDocument } from 'yaml';

const safeName = /^[A-Za-z0-9][A-Za-z0-9._-]*$/u;

async function digest(path) {
  const hash = createHash('sha512');
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest('base64');
}

async function checkFeed(directory, feedName, version, fileNames, primary) {
  const document = parseDocument(await readFile(join(directory, feedName), 'utf8'));
  if (document.errors.length) throw new Error(`${feedName}: ${document.errors[0].message}`);
  const feed = document.toJS();
  if (feed?.version !== version || !Array.isArray(feed.files) || feed.files.length !== fileNames.length) {
    throw new Error(`${feedName}: unexpected version or file list`);
  }
  if (feed.path !== primary || feed.sha512 !== feed.files.find((file) => file.url === primary)?.sha512) {
    throw new Error(`${feedName}: primary artifact does not match its file entry`);
  }
  const expected = new Set(fileNames);
  for (const file of feed.files) {
    if (!file || typeof file.url !== 'string' || !expected.delete(file.url)) {
      throw new Error(`${feedName}: unexpected or duplicate artifact URL`);
    }
    const path = join(directory, file.url);
    const info = await lstat(path);
    if (file.size !== info.size || file.sha512 !== await digest(path)) {
      throw new Error(`${feedName}: size or SHA-512 mismatch for ${file.url}`);
    }
  }
  if (expected.size) throw new Error(`${feedName}: missing artifact URL`);
}

export async function checkReleaseArtifacts(directory, version, mode) {
  if (!/^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?$/u.test(version)) {
    throw new Error(`Invalid release version: ${version}`);
  }
  if (!['macos', 'windows', 'both'].includes(mode)) throw new Error(`Invalid release mode: ${mode}`);
  const includeMac = mode !== 'windows';
  const includeWindows = mode !== 'macos';
  const dmg = `Beale-${version}-arm64.dmg`;
  const zip = `Beale-${version}-arm64-mac.zip`;
  const exe = `Beale-Setup-${version}.exe`;
  const expected = new Set([
    ...(includeMac ? [dmg, `${dmg}.blockmap`, zip, `${zip}.blockmap`, 'latest-mac.yml'] : []),
    ...(includeWindows ? [exe, `${exe}.blockmap`, 'latest.yml'] : []),
  ]);
  const actual = (await readdir(directory)).filter((name) => /\.(?:dmg|zip|exe|blockmap)$|^latest.*\.yml$/u.test(name));
  if (actual.length !== expected.size) throw new Error('Unexpected number of release artifacts');
  for (const name of actual) {
    if (!safeName.test(name) || !expected.delete(name) || !(await lstat(join(directory, name))).isFile()) {
      throw new Error(`Unexpected release artifact: ${name}`);
    }
  }
  if (expected.size) throw new Error(`Missing release artifacts: ${[...expected].join(', ')}`);
  if (includeMac) await checkFeed(directory, 'latest-mac.yml', version, [zip, dmg], zip);
  if (includeWindows) await checkFeed(directory, 'latest.yml', version, [exe], exe);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [directory, version, mode, extra] = process.argv.slice(2);
  if (!directory || !version || !['both', 'macos', 'windows'].includes(mode) || extra) {
    console.error('Usage: node scripts/check-release-artifacts.mjs DIRECTORY VERSION macos|windows|both');
    process.exitCode = 1;
  } else {
    checkReleaseArtifacts(directory, version, mode)
      .then(() => console.log('Release artifact names, feed URLs, sizes, and SHA-512 hashes match.'))
      .catch((error) => { console.error(error instanceof Error ? error.message : String(error)); process.exitCode = 1; });
  }
}
