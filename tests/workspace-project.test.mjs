import assert from 'node:assert/strict';
import { test, afterEach } from 'node:test';
import { mkdtempSync, readFileSync, writeFileSync, existsSync, mkdirSync, rmSync, readdirSync, copyFileSync, renameSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { initializeWorkspaceProject, checkpointWorkspace, listUnexpectedWorkspaceTopLevelEntries, listWorkspaceResearchEdits, publishWorkspaceFiles, workspaceContentHash, workspaceLayoutGuardMessage, preserveWorkspaceFile, quarantineWorkspaceDisposable, recoverWorkspacePublication, workspaceResearchAuthority, WORKSPACE_DIRECTORIES, WORKSPACE_PROJECT_VERSION } from '../packages/research-agent/dist/workspace-project.js';

const roots = [];
afterEach(() => { for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true }); });
function workspace() {
  const root = mkdtempSync(join(tmpdir(), 'beale-project-test-'));
  roots.push(root);
  initializeWorkspaceProject(root, 'workspace-example');
  return root;
}
function git(root, ...args) {
  return spawnSync('git', ['-c', 'user.name=Example Researcher', '-c', 'user.email=researcher@example.invalid', '-c', 'commit.gpgSign=false', ...args], { cwd: root, encoding: 'utf8', windowsHide: true });
}

test('creates a standalone local research repository with the explicit layout and no remote', () => {
  const root = workspace();
  const project = JSON.parse(readFileSync(join(root, 'workspace.json'), 'utf8'));
  assert.equal(project.schemaVersion, WORKSPACE_PROJECT_VERSION);
  assert.equal(project.researchAuthority, 'files');
  assert.equal(workspaceResearchAuthority(root), 'files');
  assert.ok(existsSync(join(root, 'references', 'research-index.json')));
  assert.equal(git(root, 'remote').stdout, '');
  assert.equal(git(root, 'log', '--format=%s').stdout.trim(), 'Initialize research workspace');
  assert.match(git(root, 'log', '-1', '--format=%B').stdout.trim(), /\n\nInvestigation-ID: none\nSession-ID: none$/u);
  for (const directory of WORKSPACE_DIRECTORIES) assert.ok(existsSync(join(root, directory)));
  assert.equal(git(root, 'status', '--porcelain').stdout, '');
  assert.equal(checkpointWorkspace(root, 'No changes').status, 'unchanged');
});

test('workspace initialization updates the managed ignore block without replacing operator rules', () => {
  const root = workspace();
  const ignorePath = join(root, '.gitignore');
  writeFileSync(ignorePath, `${readFileSync(ignorePath, 'utf8')}\n/operator-local.txt\n`);
  initializeWorkspaceProject(root, 'workspace-example');
  const once = readFileSync(ignorePath, 'utf8');
  initializeWorkspaceProject(root, 'workspace-example');
  const twice = readFileSync(ignorePath, 'utf8');
  assert.match(twice, /^\/operator-local\.txt$/mu);
  assert.equal(twice.match(/>>> Beale managed workspace layout >>>/gu)?.length, 1);
  assert.equal(twice, once, 'managed migration must be idempotent');
});

test('schema-v1 workspaces retain database-first compatibility authority', () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-project-v1-test-'));
  roots.push(root);
  writeFileSync(join(root, 'workspace.json'), JSON.stringify({
    schemaVersion: 1,
    workspaceId: 'workspace-legacy-example',
    directories: WORKSPACE_DIRECTORIES,
    checkpointIntervalMs: 600000,
  }));
  assert.equal(workspaceResearchAuthority(root), 'database');
});

test('file authority distinguishes revisioned edits from untyped record creation', () => {
  const root = workspace();
  publishWorkspaceFiles(root, { 'claims/claim-example.json': '{"revision":1}' });
  writeFileSync(join(root, 'claims', 'claim-example.json'), '{"revision":1,"summary":"edited"}');
  writeFileSync(join(root, 'claims', 'claim-new-example.json'), '{"revision":1}');
  assert.deepEqual(listWorkspaceResearchEdits(root), [
    { path: 'claims/claim-example.json', state: 'modified' },
    { path: 'claims/claim-new-example.json', state: 'created' },
  ]);
});

test('checkpoints eligible files but excludes disposable files and raw traces', () => {
  const root = workspace();
  writeFileSync(join(root, 'investigations', 'example.md'), '# Candidate\n');
  writeFileSync(join(root, 'scratch', 'temporary.txt'), 'temporary');
  mkdirSync(join(root, 'traces', 'session-example'));
  writeFileSync(join(root, 'traces', 'session-example', 'events.jsonl'), '{}\n');
  assert.equal(checkpointWorkspace(root, 'Research checkpoint').status, 'committed');
  assert.match(git(root, 'show', '--format=', '--name-only', 'HEAD').stdout, /investigations\/example.md/);
  assert.equal(git(root, 'ls-files', 'scratch', 'traces').stdout.trim(), 'traces/.gitkeep');
});

test('manual pre-commit guard validates staged content rather than the working file', () => {
  const root = workspace();
  writeFileSync(join(root, 'claims', 'secret.key'), 'example');
  assert.equal(git(root, 'add', '-f', 'claims/secret.key').status, 0);
  rmSync(join(root, 'claims', 'secret.key'));
  const commit = git(root, 'commit', '-m', 'Rejected credential file');
  assert.notEqual(commit.status, 0);
  assert.match(commit.stderr, /credential material/);
});

test('automatic commits preserve manual staging and all uncommitted work on failure', () => {
  const root = workspace();
  const path = join(root, 'investigations', 'example.md');
  writeFileSync(path, 'staged version');
  git(root, 'add', 'investigations/example.md');
  writeFileSync(path, 'working version');
  const result = checkpointWorkspace(root, 'Must not consume manual staging');
  assert.equal(result.status, 'failed');
  assert.match(result.error, /Manual staged changes/);
  assert.equal(git(root, 'show', ':investigations/example.md').stdout, 'staged version');
  assert.equal(readFileSync(path, 'utf8'), 'working version');
});

test('publishes complete deterministic snapshots and refuses to overwrite edited exports', () => {
  const root = workspace();
  const content = JSON.stringify({ schemaVersion: 1, id: 'claim-example', revision: 1 });
  const publish = () => publishWorkspaceFiles(root, { 'claims/claim-example.json': content });
  assert.equal(checkpointWorkspace(root, 'Claim revised', publish).status, 'committed');
  assert.equal(checkpointWorkspace(root, 'No canonical changes', publish).status, 'unchanged');
  writeFileSync(join(root, 'claims', 'claim-example.json'), 'operator edit');
  assert.equal(checkpointWorkspace(root, 'Preserve edit', publish).status, 'failed');
  assert.equal(readFileSync(join(root, 'claims', 'claim-example.json'), 'utf8'), 'operator edit');
});

test('cited evidence cannot be deleted or modified by a manual commit', () => {
  const root = workspace();
  const content = 'immutable supporting evidence\n';
  assert.equal(checkpointWorkspace(root, 'Accept evidence', () => publishWorkspaceFiles(root,
    { 'evidence/example.txt': content }, { 'evidence/example.txt': workspaceContentHash(content) })).status, 'committed');
  git(root, 'rm', 'evidence/example.txt');
  const result = git(root, 'commit', '-m', 'Remove evidence');
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /evidence\/example.txt/);
});

test('unexpected top-level entries are ignored by Git but repeatedly exposed by the filesystem guard', () => {
  const root = workspace();
  writeFileSync(join(root, 'loose-example.py'), 'print(1)');
  mkdirSync(join(root, 'loose-directory'));
  writeFileSync(join(root, 'loose-directory', 'notes.txt'), 'notes');
  writeFileSync(join(root, 'investigations', 'classified-example.md'), '# Candidate\n');

  assert.equal(git(root, 'check-ignore', 'loose-example.py').status, 0);
  assert.equal(git(root, 'check-ignore', 'loose-directory/notes.txt').status, 0);
  assert.notEqual(git(root, 'check-ignore', 'investigations/classified-example.md').status, 0);
  assert.deepEqual(listUnexpectedWorkspaceTopLevelEntries(root), [
    { name: 'loose-directory', kind: 'directory' },
    { name: 'loose-example.py', kind: 'file' },
  ]);
  assert.match(workspaceLayoutGuardMessage(root), /loose-example\.py/u);
  assert.equal(checkpointWorkspace(root, 'Checkpoint classified work').status, 'committed');
  assert.ok(existsSync(join(root, 'loose-example.py')), 'the guard must never remove misplaced research');

  renameSync(join(root, 'loose-example.py'), join(root, 'investigations', 'loose-example.py'));
  renameSync(join(root, 'loose-directory'), join(root, 'scratch', 'loose-directory'));
  assert.equal(workspaceLayoutGuardMessage(root), null);
});

test('workspace layout verification failures remain visible instead of appearing clean', () => {
  const root = workspace();
  writeFileSync(join(root, 'workspace.json'), '{not-json');
  const guard = workspaceLayoutGuardMessage(root);
  assert.match(guard, /Workspace layout verification failed/u);
  assert.match(guard, /repeats every turn/u);
});

test('forced staging cannot bypass the top-level workspace layout', () => {
  const root = workspace();
  writeFileSync(join(root, 'loose-example.py'), 'print(1)');
  assert.equal(git(root, 'add', '-f', 'loose-example.py').status, 0);
  const commit = git(root, 'commit', '-m', 'Misplaced file');
  assert.notEqual(commit.status, 0);
  assert.match(commit.stderr, /root files must be/u);
});

test('recovery copies retain bytes that have never been committed', () => {
  const root = workspace();
  const path = join(root, 'investigations', 'example.txt');
  writeFileSync(path, 'original');
  preserveWorkspaceFile(root, path);
  writeFileSync(path, 'replacement');
  assert.equal(readFileSync(join(root, '.git', 'beale', 'recovery', workspaceContentHash('original')), 'utf8'), 'original');
});

test('workspace identity is immutable in manual commits', () => {
  const root = workspace();
  const project = JSON.parse(readFileSync(join(root, 'workspace.json'), 'utf8'));
  writeFileSync(join(root, 'workspace.json'), JSON.stringify({ ...project, workspaceId: 'workspace-other-example' }));
  git(root, 'add', 'workspace.json');
  assert.match(git(root, 'commit', '-m', 'Change identity').stderr, /identity cannot be changed/);
});

test('quarantine preserves disposable files and only clears the selected session', () => {
  const root = workspace();
  mkdirSync(join(root, 'scratch', 'session-example', 'nested'), { recursive: true });
  writeFileSync(join(root, 'scratch', 'session-example', 'nested', 'attempt.txt'), 'incomplete experiment');
  writeFileSync(join(root, 'scratch', 'other.txt'), 'another session');
  assert.equal(quarantineWorkspaceDisposable(root, 'session-example'), 1);
  assert.deepEqual(readdirSync(join(root, 'scratch', 'session-example')), []);
  assert.equal(readFileSync(join(root, 'scratch', 'other.txt'), 'utf8'), 'another session');
  const quarantine = join(root, '.git', 'beale', 'quarantine');
  const journal = JSON.parse(readFileSync(join(quarantine, readdirSync(quarantine)[0], 'moves.json'), 'utf8'));
  assert.equal(readFileSync(join(root, journal[0].to, 'nested', 'attempt.txt'), 'utf8'), 'incomplete experiment');
});

test('publication recovery resumes after a removed export and preserves concurrent edits', () => {
  const root = workspace();
  publishWorkspaceFiles(root, { 'claims/old-example.json': '{}', 'claims/example.json': 'old' });
  const files = { 'claims/example.json': 'new' };
  const index = { schemaVersion: 1, workspaceId: 'workspace-example', files: { 'claims/example.json': workspaceContentHash('new') }, pins: {}, rawFiles: {} };
  writeFileSync(join(root, '.git', 'beale', 'pending-publication.json'), JSON.stringify({ files, index }));
  rmSync(join(root, 'claims', 'old-example.json'));
  writeFileSync(join(root, 'claims', 'example.json'), 'operator edit');
  assert.throws(() => recoverWorkspacePublication(root), /preserves the edit/);
  assert.equal(readFileSync(join(root, 'claims', 'example.json'), 'utf8'), 'operator edit');
  writeFileSync(join(root, 'claims', 'example.json'), 'new');
  recoverWorkspacePublication(root);
  assert.equal(checkpointWorkspace(root, 'Recovered publication').status, 'committed');
});

test('missing raw evidence blocks an otherwise unchanged checkpoint', () => {
  const root = workspace();
  const hash = workspaceContentHash('retained evidence');
  mkdirSync(join(root, 'evidence', 'raw'));
  const path = `evidence/raw/${hash}`;
  writeFileSync(join(root, path), 'retained evidence');
  assert.equal(checkpointWorkspace(root, 'Retain evidence', () => publishWorkspaceFiles(root, {}, {}, { [path]: hash })).status, 'committed');
  rmSync(join(root, path));
  const result = checkpointWorkspace(root, 'Verify unchanged research');
  assert.equal(result.status, 'failed');
  assert.match(result.error, /raw evidence is missing or changed/);
});

test('recovers the real index when a dead checkpoint owner committed before publishing it', () => {
  const root = workspace();
  const temporaryIndex = 'beale-index-00000000-0000-4000-8000-000000000001';
  const temporaryPath = join(root, '.git', temporaryIndex);
  copyFileSync(join(root, '.git', 'index'), temporaryPath);
  writeFileSync(join(root, 'investigations', 'recovered.txt'), 'preserved result');
  const alternateGit = (...args) => spawnSync('git', ['-c', 'user.name=Example', '-c', 'user.email=example@example.invalid', '-c', 'commit.gpgSign=false', ...args], { cwd: root, env: { ...process.env, GIT_INDEX_FILE: temporaryPath }, encoding: 'utf8', windowsHide: true });
  assert.equal(alternateGit('add', 'investigations/recovered.txt').status, 0);
  assert.equal(alternateGit('commit', '-m', 'Completed before crash').status, 0);
  const deadPid = spawnSync(process.execPath, ['-e', 'process.exit(0)'], { windowsHide: true }).pid;
  assert.ok(deadPid > 0);
  writeFileSync(join(root, '.git', 'beale-checkpoint.lock'), String(deadPid));
  writeFileSync(join(root, '.git', 'index.lock'), '');
  writeFileSync(join(root, '.git', 'beale', 'index-owner.json'), JSON.stringify({ pid: deadPid, temporaryIndex }));
  const result = checkpointWorkspace(root, 'Resume after crash');
  assert.equal(result.status, 'unchanged', result.error);
  assert.equal(git(root, 'status', '--porcelain').stdout, '');
  assert.equal(readFileSync(join(root, 'investigations', 'recovered.txt'), 'utf8'), 'preserved result');
});

test('creation resumes when the workspace marker exists before Git initialization', () => {
  const root = workspace();
  rmSync(join(root, '.git'), { recursive: true });
  initializeWorkspaceProject(root, 'workspace-example');
  assert.equal(git(root, 'log', '--format=%s').stdout.trim(), 'Initialize research workspace');
  assert.equal(git(root, 'remote').stdout, '');
});

test('checkpoint messages end with filterable investigation and session trailers', () => {
  const root = workspace();
  writeFileSync(join(root, 'investigations', 'example.md'), 'example research');
  const result = checkpointWorkspace(root, 'Research milestone', undefined, { investigationId: 'investigation-example', sessionId: 'session-example' });
  assert.equal(result.status, 'committed', result.error);
  assert.equal(git(root, 'log', '-1', '--format=%B').stdout.trim(), 'Research milestone\n\nInvestigation-ID: investigation-example\nSession-ID: session-example');
  assert.equal(git(root, 'log', '--format=%s', '--grep=^Session-ID: session-example$').stdout.trim(), 'Research milestone');
  writeFileSync(join(root, 'investigations', 'example.md'), 'new research');
  assert.equal(checkpointWorkspace(root, 'Invalid attribution', undefined, { sessionId: 'session-example\nInjected: value' }).status, 'failed');
});

test('manual commits receive trailers and retain explicitly supplied attribution without duplication', () => {
  const root = workspace();
  writeFileSync(join(root, 'investigations', 'example.md'), 'operator edit');
  git(root, 'add', 'investigations/example.md');
  assert.notEqual(git(root, 'commit', '--allow-empty-message', '-m', '').status, 0);
  assert.equal(git(root, 'commit', '-m', 'Operator edit').status, 0);
  assert.match(git(root, 'log', '-1', '--format=%B').stdout.trim(), /Investigation-ID: none\nSession-ID: none$/u);
  writeFileSync(join(root, 'investigations', 'example.md'), 'attributed edit');
  git(root, 'add', 'investigations/example.md');
  const message = 'Research edit\n\nInvestigation-ID: investigation-example\nSession-ID: session-example';
  assert.equal(git(root, 'commit', '-m', message).status, 0);
  assert.equal(git(root, 'log', '-1', '--format=%B').stdout.trim(), message);
});
