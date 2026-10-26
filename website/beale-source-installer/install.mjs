#!/usr/bin/env node

import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readdirSync, renameSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join, resolve } from 'node:path';

const REPOSITORY = 'https://github.com/philo-groves/beale-mono.git';
const BRANCH = 'release';
const MIN_NODE = [22, 19, 0];
const REQUIRED_PNPM = '11.7.0';
const supported = process.platform === 'darwin' || process.platform === 'win32';

function run(command, args, cwd) {
  const windowsPnpm = process.platform === 'win32' && command === 'pnpm';
  const executable = windowsPnpm ? 'cmd.exe' : command;
  const commandArgs = windowsPnpm ? ['/d', '/c', ['pnpm', ...args].join(' ')] : args;
  const result = spawnSync(executable, commandArgs, {
    cwd,
    encoding: 'utf8',
    stdio: 'inherit',
    windowsHide: false
  });
  if (result.error) throw new Error(`${command} could not start: ${result.error.message}`);
  if (result.status !== 0) throw new Error(`${command} ${args.join(' ')} failed (${result.status ?? 'unknown'}).`);
}

function output(command, args) {
  const windowsPnpm = process.platform === 'win32' && command === 'pnpm';
  const executable = windowsPnpm ? 'cmd.exe' : command;
  const commandArgs = windowsPnpm ? ['/d', '/c', ['pnpm', ...args].join(' ')] : args;
  const result = spawnSync(executable, commandArgs, { encoding: 'utf8', windowsHide: true });
  if (result.error || result.status !== 0) {
    throw new Error(`${command} is required on PATH. ${result.error?.message ?? result.stderr?.trim() ?? ''}`.trim());
  }
  return result.stdout.trim();
}

function checkPrerequisites() {
  const node = process.versions.node.split('.').map(Number);
  const tooOld = MIN_NODE.findIndex((part, index) => node[index] !== part);
  if (tooOld >= 0 && node[tooOld] < MIN_NODE[tooOld]) {
    throw new Error('Beale requires Node.js 22.19.0 or newer.');
  }
  output('git', ['--version']);
  const pnpm = output('pnpm', ['--version']);
  if (pnpm !== REQUIRED_PNPM) {
    throw new Error(`Beale requires pnpm ${REQUIRED_PNPM}; found ${pnpm}. Install it with corepack or npm before retrying.`);
  }
}

function launcher(root) {
  if (process.platform === 'win32') {
    const path = join(root, 'Beale.cmd');
    writeFileSync(path, '@echo off\r\ncd /d "%~dp0source"\r\ncall pnpm start:desktop\r\n', 'utf8');
    return path;
  }
  const path = join(root, 'Beale.command');
  writeFileSync(path, '#!/bin/sh\ncd "$(dirname "$0")/source" || exit 1\nexec pnpm start:desktop\n', { mode: 0o755 });
  return path;
}

function main() {
  const args = process.argv.slice(2);
  if (args.filter((arg) => arg === '--dir').length > 1 || args.filter((arg) => arg === '--dry-run').length > 1) {
    throw new Error('Usage: node install-from-source.mjs [--dry-run] [--dir <install-directory>]');
  }
  const dryRun = args.includes('--dry-run');
  const directoryIndex = args.indexOf('--dir');
  if (args.some((arg, index) => arg !== '--dry-run' && arg !== '--dir' && index !== directoryIndex + 1)
    || (directoryIndex >= 0 && (!args[directoryIndex + 1] || args[directoryIndex + 1].startsWith('--')))) {
    throw new Error('Usage: node install-from-source.mjs [--dry-run] [--dir <install-directory>]');
  }
  if (!supported) throw new Error('This installer supports macOS and Windows.');
  const root = resolve(directoryIndex >= 0 ? args[directoryIndex + 1] : join(homedir(), 'Beale'));
  const source = join(root, 'source');
  if (existsSync(source)) throw new Error(`An installation already exists at ${source}. It was not modified.`);
  if (existsSync(root) && readdirSync(root).length > 0) {
    throw new Error(`The install directory ${root} is not empty. Choose an empty directory with --dir.`);
  }
  checkPrerequisites();
  const staging = join(root, `.source-install-${process.pid}`);
  if (existsSync(staging)) throw new Error(`A previous install attempt exists at ${staging}. Inspect it before retrying.`);
  process.stdout.write(`Beale source: ${REPOSITORY} (${BRANCH})\nInstall directory: ${root}\n`);
  if (dryRun) {
    process.stdout.write('Dry run: git clone, pnpm install --frozen-lockfile, pnpm build, pnpm build:desktop.\n');
    return;
  }
  mkdirSync(root, { recursive: true });
  try {
    run('git', ['clone', '--depth', '1', '--single-branch', '--branch', BRANCH, REPOSITORY, staging], root);
    run('pnpm', ['install', '--frozen-lockfile'], staging);
    run('pnpm', ['build'], staging);
    run('pnpm', ['build:desktop'], staging);
    renameSync(staging, source);
  } catch (error) {
    // A failed build is retained for inspection; an existing install is never replaced.
    process.stderr.write(`Install stopped. Partial source, if any, remains at ${staging}.\n`);
    throw error;
  }
  const entry = launcher(root);
  process.stdout.write(`Installed Beale from source. Start it with ${entry}\n`);
}

try {
  main();
} catch (error) {
  process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
  process.exitCode = 1;
}
