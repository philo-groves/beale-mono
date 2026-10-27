#!/usr/bin/env node
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const maxReleaseBodyLength = 125_000;

export function readReleaseNotes(version) {
  const curatedPath = join(root, '.github', 'release-notes', `${version}.md`);
  let notes;
  if (existsSync(curatedPath)) {
    notes = readFileSync(curatedPath, 'utf8');
  } else {
    const lines = readFileSync(join(root, 'CHANGELOG.md'), 'utf8').split(/\r?\n/u);
    const start = lines.indexOf(`## ${version}`);
    if (start < 0) throw new Error(`CHANGELOG.md has no ## ${version} section.`);
    const end = lines.findIndex((line, index) => index > start && line.startsWith('## '));
    notes = lines.slice(start + 1, end < 0 ? undefined : end).join('\n');
  }
  notes = `${notes.trim()}\n`;
  if (!notes.trim()) throw new Error(`Release notes for ${version} are empty.`);
  if (notes.length > maxReleaseBodyLength) {
    throw new Error(`Release notes for ${version} exceed GitHub's ${maxReleaseBodyLength}-character limit; add .github/release-notes/${version}.md.`);
  }
  return notes;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  try {
    const [version, output, extra] = process.argv.slice(2);
    if (!version || !output || extra) throw new Error('Usage: node scripts/release-notes.mjs VERSION OUTPUT');
    writeFileSync(output, readReleaseNotes(version));
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
