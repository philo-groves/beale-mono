import assert from 'node:assert/strict';
import { afterEach, test } from 'node:test';
import { copyFileSync, mkdtempSync, mkdirSync, readFileSync, writeFileSync, existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import {
  initializeWorkspaceProject, checkpointWorkspace, listUnexpectedWorkspaceTopLevelEntries,
  listWorkspaceResearchEdits, publishWorkspaceFiles, preserveWorkspaceFile,
  workspaceResearchAuthority, WORKSPACE_DIRECTORIES, WORKSPACE_PROJECT_VERSION,
} from '../packages/research-agent/dist/workspace-project.js';

const roots = [];
afterEach(() => { for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true }); });
function workspace() {
  const root = mkdtempSync(join(tmpdir(), 'beale-project-example-'));
  roots.push(root);
  initializeWorkspaceProject(root, 'workspace-example');
  return root;
}

test('new research workspaces publish their index without creating a Git repository', () => {
  const root = workspace();
  const project = JSON.parse(readFileSync(join(root, 'workspace.json'), 'utf8'));
  assert.equal(project.schemaVersion, WORKSPACE_PROJECT_VERSION);
  assert.equal('checkpointIntervalMs' in project, false);
  assert.equal(workspaceResearchAuthority(root), 'files');
  assert.equal(existsSync(join(root, '.git')), false);
  assert.equal(existsSync(join(root, '.gitignore')), false);
  assert.ok(existsSync(join(root, '.beale', 'publication', 'publication.json')));
  assert.ok(existsSync(join(root, 'references', 'research-index.json')));
  for (const directory of WORKSPACE_DIRECTORIES) assert.ok(existsSync(join(root, directory)));
});

test('workspace publication does not stage, commit, or reject ordinary large working files', () => {
  const root = workspace();
  const candidate = join(root, 'investigations', 'example', 'candidate.bin');
  mkdirSync(join(root, 'investigations', 'example'), { recursive: true });
  writeFileSync(candidate, Buffer.alloc(5 * 1024 * 1024 + 1));
  assert.equal(checkpointWorkspace(root, 'Publish research').status, 'unchanged');
  assert.equal(existsSync(candidate), true);
  assert.equal(existsSync(join(root, '.git')), false);
});

test('existing workspace history remains intact and Beale-managed Git hooks are removed', () => {
  const root = workspace();
  const initialized = spawnSync('git', ['init'], { cwd: root, encoding: 'utf8' });
  assert.equal(initialized.status, 0, initialized.stderr);
  const hook = join(root, '.git', 'hooks', 'pre-commit');
  writeFileSync(hook, '#!/bin/sh\n# Beale managed research guard\n');
  initializeWorkspaceProject(root, 'workspace-example');
  assert.equal(existsSync(join(root, '.git')), true);
  assert.equal(existsSync(hook), false);
});

test('publication metadata preserves canonical bytes without Git', () => {
  const root = workspace();
  publishWorkspaceFiles(root, { 'references/example.json': '{"schemaVersion":1}\n' });
  assert.equal(readFileSync(join(root, 'references', 'example.json'), 'utf8'), '{"schemaVersion":1}\n');
  assert.deepEqual(listWorkspaceResearchEdits(root), []);
  preserveWorkspaceFile(root, join(root, 'references', 'example.json'));
  assert.ok(existsSync(join(root, '.beale', 'publication', 'recovery')));
});

test('a verified restage adopts the primary publication index on a retained guest', () => {
  const guest = workspace();
  const primary = workspace();
  publishWorkspaceFiles(guest, { 'references/example.json': '{"value":"old"}\n' });
  publishWorkspaceFiles(primary, { 'references/example.json': '{"value":"new"}\n' });
  copyFileSync(join(primary, 'references', 'example.json'), join(guest, 'references', 'example.json'));
  copyFileSync(join(primary, 'references', 'research-index.json'), join(guest, 'references', 'research-index.json'));
  initializeWorkspaceProject(guest, 'workspace-example', true);
  assert.deepEqual(listWorkspaceResearchEdits(guest), []);
  assert.equal(readFileSync(join(guest, '.beale', 'publication', 'publication.json'), 'utf8'),
    JSON.stringify(JSON.parse(readFileSync(join(primary, 'references', 'research-index.json'), 'utf8'))));
});

test('workspace layout still identifies unexpected top-level files', () => {
  const root = workspace();
  writeFileSync(join(root, 'unexpected.txt'), 'synthetic');
  assert.deepEqual(listUnexpectedWorkspaceTopLevelEntries(root).map((entry) => entry.name), ['unexpected.txt']);
});
