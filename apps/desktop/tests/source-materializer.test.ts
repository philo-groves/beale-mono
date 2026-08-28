import { createHash } from 'node:crypto';
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, utimesSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import type { WorkspaceScopeVersion, ScopeAsset } from '@shared/types';
import {
  materializeGitRepository,
  materializeGitRepositoryAsync,
  normalizeSourceRepositoryUrl,
  selectSourceRepository,
  sourceRepositoryCandidates
} from '../src/main/sourceMaterializer';

const createdDirs: string[] = [];
const appServerStateFiles: string[] = [];
const previousEnvironment = new Map<string, string | undefined>();

afterEach(() => {
  for (const stateFile of appServerStateFiles.splice(0)) stopTestAppServer(stateFile);
  for (const [name, value] of previousEnvironment) {
    if (value === undefined) delete process.env[name];
    else process.env[name] = value;
  }
  previousEnvironment.clear();
  for (const dir of createdDirs.splice(0)) {
    rmSync(dir, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
  }
});

describe('source materializer', () => {
  it('recognizes GitLab source repository scope entries', () => {
    const clonedDirectory = join(tempDir(), 'repositories', 'gitlab');
    const scope = scopeWithAssets([
      {
        ...sourceAsset('repo_gitlab', 'https://gitlab.com/gitlab-org/gitlab'),
        attributes: { clonedDirectory }
      },
      sourceAsset('repo_opstrace', 'https://gitlab.com/gitlab-org/opstrace/opstrace')
    ]);

    expect(sourceRepositoryCandidates(scope).map((candidate) => candidate.url)).toEqual([
      'https://gitlab.com/gitlab-org/gitlab',
      'https://gitlab.com/gitlab-org/opstrace/opstrace'
    ]);
    expect(selectSourceRepository(scope, 'https://gitlab.com/gitlab-org/gitlab').reason).toBe('matched');
    expect(sourceRepositoryCandidates(scope)[0]?.clonedDirectory).toBe(clonedDirectory);
    expect(normalizeSourceRepositoryUrl('git@gitlab.com:gitlab-org/gitlab.git')).toBe('https://gitlab.com/gitlab-org/gitlab');
    expect(normalizeSourceRepositoryUrl('https://fuchsia.googlesource.com/fuchsia')).toBe('https://fuchsia.googlesource.com/fuchsia');
    expect(normalizeSourceRepositoryUrl('https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/src/starnix/')).toBe('https://fuchsia.googlesource.com/fuchsia');
  });

  it('enables long-path support for Git for Windows clone checkouts', async () => {
    const workspace = tempDir();
    const repositoryStore = join(workspace, 'repositories');
    const argumentsFile = join(workspace, 'git-arguments.jsonl');
    const fakeGit = join(workspace, 'fake-git-longpaths.mjs');
    writeFileSync(
      fakeGit,
      [
        '#!/usr/bin/env node',
        "import { appendFileSync, mkdirSync } from 'node:fs';",
        'const args = process.argv.slice(2);',
        `appendFileSync(${JSON.stringify(argumentsFile)}, JSON.stringify(args) + "\\n");`,
        "if (args.includes('clone')) { mkdirSync(`${args.at(-1)}/.git`, { recursive: true }); process.exit(0); }",
        "if (args.includes('rev-parse')) { process.stdout.write('0123456789abcdef0123456789abcdef01234567\\n'); process.exit(0); }",
        'process.exit(0);'
      ].join('\n')
    );
    chmodSync(fakeGit, 0o700);
    process.env.BEALE_GIT_COMMAND = fakeGit;
    configureIsolatedAppServer(workspace);
    const candidate = sourceRepositoryCandidates(scopeWithAssets([
      sourceAsset('repo_gitlab', 'https://gitlab.com/gitlab-org/gitlab')
    ]))[0];

    const materialized = await materializeGitRepositoryAsync(candidate, '', { repositoryStoreDirectory: repositoryStore });

    const invocations = readFileSync(argumentsFile, 'utf8')
      .trim()
      .split(/\r?\n/u)
      .map((line) => JSON.parse(line) as string[]);
    const cloneArgs = invocations.find((args) => args.includes('clone'));
    expect(cloneArgs).toBeTruthy();
    expect(cloneArgs).not.toContain('--depth');
    expect(cloneArgs).not.toContain('--filter=blob:none');
    expect(materialized.cloneMode).toBe('deep');
    if (process.platform === 'win32') {
      expect(cloneArgs).toContain('core.longpaths=true');
    } else {
      expect(cloneArgs).not.toContain('core.longpaths=true');
    }
  });

  it('uses a depth-one clone only when shallow mode is requested', async () => {
    const workspace = tempDir();
    const repositoryStore = join(workspace, 'repositories');
    const argumentsFile = join(workspace, 'git-arguments.jsonl');
    const fakeGit = join(workspace, 'fake-git-shallow.mjs');
    writeFileSync(fakeGit, [
      '#!/usr/bin/env node',
      "import { appendFileSync, mkdirSync } from 'node:fs';",
      'const args = process.argv.slice(2);',
      `appendFileSync(${JSON.stringify(argumentsFile)}, JSON.stringify(args) + "\\n");`,
      "if (args.includes('clone')) { mkdirSync(`${args.at(-1)}/.git`, { recursive: true }); process.exit(0); }",
      "if (args.includes('--is-shallow-repository')) { process.stdout.write('true\\n'); process.exit(0); }",
      "if (args.includes('rev-parse')) { process.stdout.write('0123456789abcdef0123456789abcdef01234567\\n'); process.exit(0); }",
      'process.exit(0);'
    ].join('\n'));
    chmodSync(fakeGit, 0o700);
    process.env.BEALE_GIT_COMMAND = fakeGit;
    configureIsolatedAppServer(workspace);
    const candidate = sourceRepositoryCandidates(scopeWithAssets([
      sourceAsset('repo_gitlab', 'https://gitlab.com/gitlab-org/gitlab')
    ]))[0];

    const materialized = await materializeGitRepositoryAsync(candidate, '', {
      cloneMode: 'shallow',
      repositoryStoreDirectory: repositoryStore
    });

    const cloneArgs = readFileSync(argumentsFile, 'utf8')
      .trim()
      .split(/\r?\n/u)
      .map((line) => JSON.parse(line) as string[])
      .find((args) => args.includes('clone'));
    expect(cloneArgs).toEqual(expect.arrayContaining(['--depth', '1']));
    expect(cloneArgs).toContain('--filter=blob:none');
    expect(materialized.cloneMode).toBe('shallow');
  });

  it('deepens an existing shallow checkout when deep mode is requested', () => {
    const workspace = tempDir();
    const repositoryStore = join(workspace, 'repositories');
    const checkout = join(repositoryStore, 'github.com_Netflix_zuul', 'default');
    mkdirSync(join(checkout, '.git'), { recursive: true });
    const stateFile = join(workspace, 'clone-state.txt');
    const argumentsFile = join(workspace, 'git-arguments.jsonl');
    writeFileSync(stateFile, 'shallow');
    const fakeGit = join(workspace, 'fake-git-deepen.mjs');
    writeFileSync(fakeGit, [
      '#!/usr/bin/env node',
      "import { appendFileSync, readFileSync, writeFileSync } from 'node:fs';",
      'const args = process.argv.slice(2);',
      `appendFileSync(${JSON.stringify(argumentsFile)}, JSON.stringify(args) + "\\n");`,
      `const stateFile = ${JSON.stringify(stateFile)};`,
      "if (args.includes('--is-shallow-repository')) { process.stdout.write(readFileSync(stateFile, 'utf8') === 'shallow' ? 'true\\n' : 'false\\n'); process.exit(0); }",
      "if (args.includes('--unshallow')) { writeFileSync(stateFile, 'deep'); process.exit(0); }",
      "if (args.includes('rev-parse')) { process.stdout.write('0123456789abcdef0123456789abcdef01234567\\n'); process.exit(0); }",
      'process.exit(0);'
    ].join('\n'));
    chmodSync(fakeGit, 0o700);
    process.env.BEALE_GIT_COMMAND = fakeGit;
    configureIsolatedAppServer(workspace);
    const candidate = sourceRepositoryCandidates(scopeWithAssets([
      sourceAsset('repo_zuul', 'https://github.com/Netflix/zuul')
    ]))[0];

    const materialized = materializeGitRepository(candidate, '', {
      cloneMode: 'deep',
      repositoryStoreDirectory: repositoryStore
    });

    expect(materialized.cloned).toBe(false);
    expect(materialized.cloneMode).toBe('deep');
    expect(readFileSync(argumentsFile, 'utf8')).toContain('--unshallow');
  });

  it('preserves the terminal Git checkout error after cleaning a partial clone', async () => {
    const workspace = tempDir();
    const repositoryStore = join(workspace, 'repositories');
    const fakeGit = join(workspace, 'fake-git-checkout-failure.mjs');
    writeFileSync(
      fakeGit,
      [
        '#!/usr/bin/env node',
        "import { mkdirSync } from 'node:fs';",
        'const args = process.argv.slice(2);',
        "if (args.includes('clone')) {",
        '  mkdirSync(`${args.at(-1)}/partial/deep/tree`, { recursive: true });',
        '  process.stderr.write(`${"checkout warning\\n".repeat(200)}fatal: unable to checkout working tree: Filename too long\\n`);',
        '  process.exit(1);',
        '}',
        'process.exit(0);'
      ].join('\n')
    );
    chmodSync(fakeGit, 0o700);
    process.env.BEALE_GIT_COMMAND = fakeGit;
    configureIsolatedAppServer(workspace);
    const candidate = sourceRepositoryCandidates(scopeWithAssets([
      sourceAsset('repo_gitlab', 'https://gitlab.com/gitlab-org/gitlab')
    ]))[0];

    await expect(materializeGitRepositoryAsync(candidate, '', {
      repositoryStoreDirectory: repositoryStore
    })).rejects.toThrow(/Filename too long/u);
    expect(existsSync(join(repositoryStore, 'gitlab.com_gitlab-org_gitlab', 'default'))).toBe(false);
  });

  it('reuses the checkout that wins concurrent repository preparation', async () => {
    const workspace = tempDir();
    const repositoryStore = join(workspace, 'repositories');
    const fakeGit = join(workspace, 'fake-git-concurrent.mjs');
    writeFileSync(
      fakeGit,
      [
        '#!/usr/bin/env node',
        "import { mkdirSync } from 'node:fs';",
        'const args = process.argv.slice(2);',
        "if (args.includes('clone')) { setTimeout(() => { mkdirSync(`${args.at(-1)}/.git`, { recursive: true }); process.exit(0); }, 50); }",
        "else if (args.includes('rev-parse')) { process.stdout.write('0123456789abcdef0123456789abcdef01234567\\n'); }",
        'else process.exit(0);'
      ].join('\n')
    );
    chmodSync(fakeGit, 0o700);
    process.env.BEALE_GIT_COMMAND = fakeGit;
    configureIsolatedAppServer(workspace);
    const candidate = sourceRepositoryCandidates(scopeWithAssets([
      sourceAsset('repo_gitlab', 'https://gitlab.com/gitlab-org/gitlab')
    ]))[0];

    const results = await Promise.all([
      materializeGitRepositoryAsync(candidate, '', { repositoryStoreDirectory: repositoryStore }),
      materializeGitRepositoryAsync(candidate, '', { repositoryStoreDirectory: repositoryStore })
    ]);

    expect(results.map((result) => result.cloned).sort()).toEqual([false, true]);
    expect(results[0].localPath).toBe(results[1].localPath);
    expect(existsSync(join(results[0].localPath, '.git'))).toBe(true);
  });

  it('reclaims an abandoned partial checkout before cloning again', async () => {
    const workspace = tempDir();
    const repositoryStore = join(workspace, 'repositories');
    const repositoryDirectory = join(repositoryStore, 'gitlab.com_gitlab-org_gitlab');
    const staleCheckout = join(repositoryDirectory, '.default.tmp-1234-1000');
    mkdirSync(join(staleCheckout, 'partial', 'tree'), { recursive: true });
    const staleTime = new Date(Date.now() - 8 * 24 * 60 * 60_000);
    utimesSync(staleCheckout, staleTime, staleTime);
    const fakeGit = join(workspace, 'fake-git-stale-cleanup.mjs');
    writeFileSync(
      fakeGit,
      [
        '#!/usr/bin/env node',
        "import { mkdirSync } from 'node:fs';",
        'const args = process.argv.slice(2);',
        "if (args.includes('clone')) { mkdirSync(`${args.at(-1)}/.git`, { recursive: true }); process.exit(0); }",
        "if (args.includes('rev-parse')) { process.stdout.write('0123456789abcdef0123456789abcdef01234567\\n'); process.exit(0); }",
        'process.exit(0);'
      ].join('\n')
    );
    chmodSync(fakeGit, 0o700);
    process.env.BEALE_GIT_COMMAND = fakeGit;
    configureIsolatedAppServer(workspace);
    const candidate = sourceRepositoryCandidates(scopeWithAssets([
      sourceAsset('repo_gitlab', 'https://gitlab.com/gitlab-org/gitlab')
    ]))[0];

    await materializeGitRepositoryAsync(candidate, '', { repositoryStoreDirectory: repositoryStore });

    expect(existsSync(staleCheckout)).toBe(false);
  });

  it('runs clone materialization without blocking the event loop', async () => {
    const workspace = tempDir();
    mkdirSync(join(workspace, '.beale'), { recursive: true });
    const fakeGit = join(workspace, 'fake-git.mjs');
    writeFileSync(
      fakeGit,
      [
        '#!/usr/bin/env node',
        "import { mkdirSync } from 'node:fs';",
        'const args = process.argv.slice(2);',
        "if (args.includes('clone')) {",
        '  const target = args.at(-1);',
        "  setTimeout(() => { mkdirSync(`${target}/.git`, { recursive: true }); process.exit(0); }, 120);",
        '} else if (args.includes("rev-parse")) {',
        '  process.stdout.write("0123456789abcdef0123456789abcdef01234567\\n");',
        '} else {',
        '  process.exit(0);',
        '}'
      ].join('\n')
    );
    chmodSync(fakeGit, 0o700);
    process.env.BEALE_GIT_COMMAND = fakeGit;
    configureIsolatedAppServer(workspace);
    const scope = scopeWithAssets([sourceAsset('repo_gitlab', 'https://gitlab.com/gitlab-org/gitlab')]);
    const candidate = sourceRepositoryCandidates(scope)[0];
    let timerFired = false;

    const materializedPromise = materializeGitRepositoryAsync(candidate, '', {
      repositoryStoreDirectory: join(workspace, 'beale-home', 'repositories')
    });
    await new Promise((resolve) => setTimeout(resolve, 20));
    timerFired = true;
    const materialized = await materializedPromise;

    expect(timerFired).toBe(true);
    expect(materialized.cloned).toBe(true);
    expect(materialized.head).toBe('0123456789abcdef0123456789abcdef01234567');
  });

  it('fetches and checks out requested refs in managed existing checkouts', () => {
    const workspace = tempDir();
    mkdirSync(join(workspace, '.beale'), { recursive: true });
    const repositoryStore = join(workspace, 'beale-home', 'repositories');
    const refDigest = createHash('sha256').update('feature-ref').digest('hex').slice(0, 12);
    const managedCheckout = join(repositoryStore, 'github.com_Netflix_zuul', `feature-ref-${refDigest}`);
    mkdirSync(join(managedCheckout, '.git'), { recursive: true });
    const stateFile = join(workspace, 'git-head.txt');
    writeFileSync(stateFile, 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa');
    const fakeGit = join(workspace, 'fake-git-ref.mjs');
    writeFileSync(
      fakeGit,
      [
        '#!/usr/bin/env node',
        "import { readFileSync, writeFileSync } from 'node:fs';",
        'const args = process.argv.slice(2);',
        `const stateFile = ${JSON.stringify(stateFile)};`,
        'const command = args.find((arg) => ["rev-parse", "fetch", "checkout"].includes(arg));',
        'if (command === "rev-parse" && args.at(-1) === "HEAD") { process.stdout.write(`${readFileSync(stateFile, "utf8").trim()}\\n`); process.exit(0); }',
        'if (command === "rev-parse" && args.at(-1) === "feature-ref^{commit}") { process.stdout.write("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\\n"); process.exit(0); }',
        'if (command === "fetch") process.exit(0);',
        'if (command === "checkout") { writeFileSync(stateFile, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"); process.exit(0); }',
        'process.exit(1);'
      ].join('\n')
    );
    chmodSync(fakeGit, 0o700);
    process.env.BEALE_GIT_COMMAND = fakeGit;
    configureIsolatedAppServer(workspace);
    const scope = scopeWithAssets([sourceAsset('repo_zuul', 'https://github.com/Netflix/zuul')]);
    const candidate = sourceRepositoryCandidates(scope)[0];

    const materialized = materializeGitRepository(candidate, 'feature-ref', {
      repositoryStoreDirectory: repositoryStore
    });

    expect(materialized.localPath).toBe(managedCheckout);
    expect(materialized.requestedRefHead).toBe('bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb');
    expect(materialized.requestedRefMatchesHead).toBe(true);
  });

  it('reuses one user-global checkout across materializations', async () => {
    const repositoryStore = join(tempDir(), 'repositories');
    const fakeGit = join(tempDir(), 'fake-git-global.mjs');
    writeFileSync(
      fakeGit,
      [
        '#!/usr/bin/env node',
        "import { mkdirSync } from 'node:fs';",
        'const args = process.argv.slice(2);',
        "if (args.includes('clone')) { mkdirSync(`${args.at(-1)}/.git`, { recursive: true }); process.exit(0); }",
        "if (args.includes('rev-parse') && args.at(-1) === 'HEAD') { process.stdout.write('0123456789abcdef0123456789abcdef01234567\\n'); process.exit(0); }",
        'process.exit(1);'
      ].join('\n')
    );
    chmodSync(fakeGit, 0o700);
    process.env.BEALE_GIT_COMMAND = fakeGit;
    configureIsolatedAppServer(join(repositoryStore, '..'));
    const candidate = sourceRepositoryCandidates(scopeWithAssets([sourceAsset('repo_zuul', 'https://github.com/Netflix/zuul')]))[0];

    const first = await materializeGitRepositoryAsync(candidate, '', {
      repositoryStoreDirectory: repositoryStore
    });
    const second = await materializeGitRepositoryAsync(candidate, '', {
      repositoryStoreDirectory: repositoryStore
    });

    expect(first.cloned).toBe(true);
    expect(second.cloned).toBe(false);
    expect(second.localPath).toBe(first.localPath);
    expect(first.localPath).toBe(join(repositoryStore, 'github.com_Netflix_zuul', 'default'));
  });
});

function tempDir(): string {
  const dir = mkdtempSync(join(tmpdir(), 'beale-source-test-'));
  createdDirs.push(dir);
  return dir;
}

function configureIsolatedAppServer(directory: string): void {
  const stateFile = join(directory, 'app-server.json');
  appServerStateFiles.push(stateFile);
  setEnvironment('BEALE_APP_SERVER_STATE_FILE', stateFile);
  setEnvironment('BEALE_APP_SERVER_PARENT_PID', String(process.pid));
  setEnvironment('BEALE_APP_SERVER_PORT', '0');
}

function stopTestAppServer(stateFile: string): void {
  if (!existsSync(stateFile)) return;
  try {
    const record = JSON.parse(readFileSync(stateFile, 'utf8')) as { pid?: unknown };
    if (typeof record.pid === 'number' && record.pid > 0 && record.pid !== process.pid) {
      try { process.kill(record.pid); } catch { /* already stopped */ }
    }
  } catch {
    // Best-effort teardown for the detached test app-server.
  }
}

function setEnvironment(name: string, value: string): void {
  if (!previousEnvironment.has(name)) previousEnvironment.set(name, process.env[name]);
  process.env[name] = value;
}

function sourceAsset(id: string, value: string): ScopeAsset {
  return {
    id,
    scopeVersionId: 'scope_1',
    direction: 'in_scope',
    kind: 'repo',
    value,
    attributes: {},
    sensitivity: 'public',
    createdAt: '2026-01-01T00:00:00.000Z'
  };
}

function scopeWithAssets(assets: ScopeAsset[]): WorkspaceScopeVersion {
  return {
    id: 'scope_1',
    version: 1,
    status: 'active',
    workspaceName: 'GitLab',
    scopeOwner: 'GitLab',
    descriptionMarkdown: '',
    rulesMarkdown: '',
    activeFrom: '2026-01-01T00:00:00.000Z',
    expiresAt: null,
    createdAt: '2026-01-01T00:00:00.000Z',
    createdBy: 'test',
    assets
  };
}
