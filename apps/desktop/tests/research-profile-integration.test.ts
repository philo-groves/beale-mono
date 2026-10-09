import { createHash } from 'node:crypto';
import { preBealeHashDomain } from '@beale/research-agent/legacy-compatibility';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { DatabaseSync } from 'node:sqlite';
import { afterEach, describe, expect, it } from 'vitest';
import {
  decodeResearchProfileCatalogEnvelope,
  ResearchProfileService,
  resolveAppServerProfileInvocation
} from '../src/main/researchProfileService';
import { WorkspaceDatabase } from '../src/main/database';
import { ensureBealeAppServerRunning } from '../src/main/bealeAppServerClient';
import { AppServerRunEngine } from '../src/main/appServerRunEngine';
import { isResearchProfileMemoryStatusActive, WorkspaceService } from '../src/main/workspaceService';
import { migrateResearchProfile, serializeResearchProfile } from '../src/shared/researchProfile';
import type { ResearchProfile, ResolvedResearchProfile, StartRunInput } from '@shared/types';
import {
  resolvedTestResearchProfile,
  testResearchProfile,
  testResearchProfileCatalogEnvelope
} from './researchProfileFixture';
import { startRunForTest } from './workspaceTestSupport';

const directories: string[] = [];

afterEach(() => {
  stopFakeAppServer();
  delete process.env.BEALE_APP_SERVER_COMMAND;
  delete process.env.BEALE_APP_SERVER_ARGS_JSON;
  delete process.env.BEALE_APP_SERVER_STATE_FILE;
  delete process.env.BEALE_APP_SERVER_PARENT_PID;
  delete process.env.FAKE_APP_SERVER_CHILD_SCRIPT;
  delete process.env.FAKE_APP_SERVER_CHILD_ARGS_JSON;
  delete process.env.FAKE_APP_SERVER_SESSION_LAUNCH_MODULE;
  delete process.env.FAKE_APP_SERVER_REGISTRY_DIRECTORY;
  delete process.env.FAKE_APP_SERVER_DATABASE_PATH;
  delete process.env.FAKE_APP_SERVER_ARTIFACT_DIRECTORY;
  delete process.env.FAKE_RESEARCH_AGENT_MODULE;
  delete process.env.BEALE_APP_SERVER_COMMAND;
  delete process.env.BEALE_APP_SERVER_ARGS_JSON;
  delete process.env.BEALE_APP_SERVER_CWD;
  delete process.env.BEALE_APP_SERVER_ROOT;
  delete process.env.BEALE_APP_SERVER_NODE_COMMAND;
  delete process.env.BEALE_APP_SERVER_PNPM_COMMAND;
  delete process.env.BEALE_APP_SERVER_PROTOCOL_COMMAND;
  delete process.env.BEALE_APP_SERVER_PROTOCOL_ARGS_JSON;
  delete process.env.BEALE_APP_SERVER_PROTOCOL_CWD;
  delete process.env.BEALE_APP_SERVER_SESSION_OWNERSHIP;
  delete process.env.BEALE_APP_SERVER_PROFILE_COMMAND;
  delete process.env.BEALE_APP_SERVER_PROFILE_ARGS_JSON;
  delete process.env.BEALE_APP_SERVER_PROFILE_CWD;
  delete process.env.BEALE_APP_SERVER_PROFILE_ROOT;
  delete process.env.BEALE_APP_SERVER_PROFILE_NODE_COMMAND;
  delete process.env.BEALE_APP_SERVER_PROFILE_PNPM_COMMAND;
  delete process.env.BEALE_APP_SERVER_PROFILE_TOOL_FAMILY_CEILING_JSON;
  delete process.env.BEALE_APP_SERVER_PROFILE_SIDE_EFFECT_CEILING_JSON;
  delete process.env.BEALE_OPENAI_ACCESS_TOKEN;
  for (const directory of directories.splice(0)) rmSync(directory, { recursive: true, force: true });
});

describe('research profile host integration', () => {
  it('decodes an additive app-server catalog envelope and validates protocol, schema, and hash', () => {
    const envelope = { ...testResearchProfileCatalogEnvelope(), additiveField: { accepted: true } };
    const captured: { command?: string; args?: readonly string[] } = {};
    const service = new ResearchProfileService({
      resolveInvocation: () => ({
        command: 'app-server-test',
        prefixArgs: ['cli.js'],
        cwd: 'C:\\appServer',
        configuredBy: 'env_command',
        usesNodeRuntime: true
      }),
      runCommand: (command, args) => {
        captured.command = command;
        captured.args = args;
        return { status: 0, stdout: `runner banner\n${JSON.stringify(envelope)}`, stderr: '' };
      }
    });

    const resolved = service.resolve('C:\\workspace', 'security-research');
    expect(resolved.profile.name).toBe('Security');
    expect(captured).toEqual({
      command: 'app-server-test',
      args: ['cli.js', 'profile', 'resolve', '--workspace-root', 'C:\\workspace', '--profile-id', 'security-research', '--json']
    });

    expect(() => decodeResearchProfileCatalogEnvelope({ ...envelope, catalogProtocolVersion: 2 })).toThrow(/catalog protocol/);
    expect(() => decodeResearchProfileCatalogEnvelope({
      ...envelope,
      supportedResearchProfileSchemaVersions: [2]
    })).toThrow(/schema version 1 support/);
    expect(() => decodeResearchProfileCatalogEnvelope({ ...envelope, hash: '0'.repeat(64) })).toThrow(/hash mismatch/);
    expect(() => decodeResearchProfileCatalogEnvelope(testResearchProfileCatalogEnvelope({
      ...testResearchProfile(), id: 'mathematics'
    }))).toThrow(/Security research profiles only/);
  });

  it('migrates legacy local catalog profiles before validating the canonical hash', () => {
    const legacyProfile = testResearchProfile() as unknown as Record<string, unknown>;
    delete legacyProfile.schemaVersion;
    delete legacyProfile.modelJobs;
    const capabilities = legacyProfile.capabilities as Record<string, unknown>;
    delete capabilities.selectedSkillIds;
    delete capabilities.disabledSkillIds;
    delete capabilities.allowedMcpServerIds;
    const migratedProfile = migrateResearchProfile(legacyProfile).profile;
    const hash = createHash('sha256')
      .update(preBealeHashDomain('research-profile:v1\0'))
      .update(serializeResearchProfile(migratedProfile))
      .digest('hex');

    const decoded = decodeResearchProfileCatalogEnvelope({
      catalogProtocolVersion: 1,
      supportedResearchProfileSchemaVersions: [0],
      profile: legacyProfile,
      hash,
      source: 'explicit',
      path: 'C:\\workspace\\.beale\\profiles\\security.json'
    });

    expect(decoded.resolvedProfile).toMatchObject({
      hash,
      source: 'explicit',
      profile: {
        schemaVersion: 1,
        modelJobs: {},
        capabilities: {
          selectedSkillIds: [],
          disabledSkillIds: [],
          allowedMcpServerIds: []
        }
      }
    });
    expect(() => decodeResearchProfileCatalogEnvelope({
      catalogProtocolVersion: 1,
      supportedResearchProfileSchemaVersions: [0],
      profile: legacyProfile,
      hash: '0'.repeat(64),
      source: 'explicit'
    })).toThrow(/hash mismatch/);
  });

  it('resolves profile catalogs asynchronously in parallel and caches duplicate requests', async () => {
    const securityProfile = testResearchProfile();
    let calls = 0;
    let active = 0;
    let maxActive = 0;
    const service = new ResearchProfileService({
      resolveInvocation: () => ({
        command: 'app-server-test',
        prefixArgs: ['cli.js'],
        cwd: 'C:\\appServer',
        configuredBy: 'env_command',
        usesNodeRuntime: true
      }),
      runCommandAsync: async () => {
        calls += 1;
        active += 1;
        maxActive = Math.max(maxActive, active);
        await new Promise((resolveDelay) => setTimeout(resolveDelay, 10));
        active -= 1;
        return { status: 0, stdout: JSON.stringify(testResearchProfileCatalogEnvelope(securityProfile)), stderr: '' };
      }
    });

    const [security, secondWorkspace, duplicateSecurity] = await Promise.all([
      service.resolveAsync('C:\\workspace', 'security-research'),
      service.resolveAsync('C:\\other-workspace', 'security-research'),
      service.resolveAsync('C:\\workspace', 'security-research')
    ]);

    expect([security.profile.id, secondWorkspace.profile.id, duplicateSecurity.profile.id]).toEqual([
      'security-research',
      'security-research',
      'security-research'
    ]);
    expect(() => service.resolve('C:\\workspace', 'mathematics')).toThrow(/Security research profiles only/);
    expect(calls).toBe(2);
    expect(maxActive).toBe(2);
    await service.resolveAsync('C:\\workspace', 'security-research');
    expect(calls).toBe(2);
  });

  it('derives active recommendation memory from the profile status catalog', () => {
    const base = testResearchProfile();
    const profile: ResearchProfile = {
      ...base,
      memory: {
        ...base.memory,
        statuses: [
          { id: 'current', name: 'Current', description: 'Still useful.', order: 10, polarity: 'positive' },
          { id: 'complete', name: 'Complete', description: 'Finished.', order: 20, terminal: true, polarity: 'positive' },
          { id: 'archived', name: 'Archived', description: 'No longer active.', order: 30, terminal: true, polarity: 'neutral' },
          { id: 'discarded', name: 'Discarded', description: 'Invalidated.', order: 40, polarity: 'negative' }
        ],
        types: base.memory.types.map((type) => ({
          ...type,
          defaultStatus: 'current',
          allowedStatuses: ['current', 'complete', 'archived', 'discarded']
        }))
      }
    };

    expect(isResearchProfileMemoryStatusActive(profile, 'current')).toBe(true);
    expect(isResearchProfileMemoryStatusActive(profile, 'complete')).toBe(true);
    expect(isResearchProfileMemoryStatusActive(profile, 'archived')).toBe(false);
    expect(isResearchProfileMemoryStatusActive(profile, 'discarded')).toBe(false);
    expect(isResearchProfileMemoryStatusActive(profile, 'missing')).toBe(false);
  });

  it('keeps run-engine invocation overrides out of canonical profile resolution', () => {
    const unrelatedRoot = temporaryDirectory();
    process.env.BEALE_APP_SERVER_COMMAND = 'run-only-wrapper';
    process.env.BEALE_APP_SERVER_ARGS_JSON = JSON.stringify(['run-only.mjs']);
    process.env.BEALE_APP_SERVER_CWD = unrelatedRoot;
    process.env.BEALE_APP_SERVER_ROOT = unrelatedRoot;
    process.env.BEALE_APP_SERVER_NODE_COMMAND = 'run-only-node';
    process.env.BEALE_APP_SERVER_PNPM_COMMAND = 'run-only-pnpm';

    const workspaceRoot = resolve(process.cwd(), '..', '..');
    const workspaceCli = join(workspaceRoot, 'packages', 'app-server-runtime', 'dist', 'cli.js');
    expect(existsSync(workspaceCli)).toBe(true);

    const invocation = resolveAppServerProfileInvocation();
    expect(invocation).toMatchObject({
      prefixArgs: [workspaceCli],
      cwd: workspaceRoot,
      configuredBy: 'workspace_root',
      usesNodeRuntime: true
    });
    expect(invocation.command).not.toBe('run-only-wrapper');
    expect(invocation.command).not.toBe('run-only-node');
  });

  it('fails closed instead of using run-only invocation overrides when no canonical profile resolver is present', () => {
    const missingRoot = join(temporaryDirectory(), 'missing-appServer');
    process.env.BEALE_APP_SERVER_COMMAND = 'run-only-wrapper';
    process.env.BEALE_APP_SERVER_ARGS_JSON = JSON.stringify(['run-only.mjs']);
    process.env.BEALE_APP_SERVER_CWD = temporaryDirectory();

    expect(() => resolveAppServerProfileInvocation({ defaultRoot: missingRoot }))
      .toThrow(/Canonical app-server profile resolution is unavailable/);
  });

  it('uses an explicitly configured versioned profile resolver without run-only arguments', () => {
    const missingRoot = join(temporaryDirectory(), 'missing-appServer');
    const profileCwd = temporaryDirectory();
    process.env.BEALE_APP_SERVER_COMMAND = 'run-only-wrapper';
    process.env.BEALE_APP_SERVER_ARGS_JSON = JSON.stringify(['run-only.mjs']);
    process.env.BEALE_APP_SERVER_PROFILE_COMMAND = 'packaged-appServer';
    process.env.BEALE_APP_SERVER_PROFILE_ARGS_JSON = JSON.stringify(['--catalog-protocol', '1']);
    process.env.BEALE_APP_SERVER_PROFILE_CWD = profileCwd;

    expect(resolveAppServerProfileInvocation({ defaultRoot: missingRoot })).toMatchObject({
      command: 'packaged-appServer',
      prefixArgs: ['--catalog-protocol', '1'],
      cwd: profileCwd,
      configuredBy: 'env_command'
    });
  });

  it('uses a changed profile for new runs while retaining the original run snapshot', async () => {
    const root = temporaryDirectory();
    const workspace = join(root, 'workspace');
    const invocationLog = join(root, 'invocations.jsonl');
    const fakeAppServer = join(root, 'fake-appServer.mjs');
    mkdirSync(workspace, { recursive: true });
    writeFileSync(fakeAppServer, fakeAppServerSource());
    configureFakeAppServer(root, fakeAppServer, [invocationLog]);
    await ensureBealeAppServerRunning();

    const firstProfile = profileWithWorkflow('1.0.0', 'discovery');
    const secondProfileBase = profileWithWorkflow('2.0.0', 'analysis-pass');
    const secondProfile: ResearchProfile = {
      ...secondProfileBase,
      memory: {
        ...secondProfileBase.memory,
        types: secondProfileBase.memory.types.map((type) => ({
          ...type,
          description: 'A durable observation under the second catalog.'
        }))
      }
    };
    let currentProfile: ResolvedResearchProfile = resolvedTestResearchProfile(firstProfile);
    const service = new WorkspaceService(() => undefined, {
      workspaceRegistryDirectory: join(root, 'registry'),
      appServerDatabasePath: join(root, 'memory.sqlite'),
      appServerArtifactDirectory: join(root, 'artifacts'),
      researchProfileResolver: () => currentProfile
    });

    try {
      const opened = service.createWorkspace(workspace);
      expect(opened.researchProfile).toMatchObject({ profileVersion: '1.0.0', profileHash: currentProfile.hash });

      const firstStarted = service.startRun(runInput('discovery'));
      const firstRunId = firstStarted.runs[0]?.run.id ?? '';
      await waitForRun(service, firstRunId);
      const firstDetail = service.getRunDetail(firstRunId);
      const firstRun = firstDetail.run;
      const firstCatalogHash = firstDetail.appServerMemory?.activeCatalogHash;
      expect(firstRun.researchProfileSnapshotId).toBe(opened.researchProfile.id);
      expect(firstDetail.researchProfile?.profileVersion).toBe('1.0.0');
      expect(firstCatalogHash).toMatch(/^[a-f0-9]{64}$/u);

      currentProfile = resolvedTestResearchProfile(secondProfile, 'workspace-default', join(workspace, '.beale', 'profile.json'));
      const secondStarted = service.startRun(runInput('analysis-pass'));
      const secondRunId = secondStarted.runs.find((row) => row.run.id !== firstRunId)?.run.id ?? '';
      await waitForRun(service, secondRunId);
      const secondDetail = service.getRunDetail(secondRunId);
      const secondRun = secondDetail.run;
      expect(secondRun.researchProfileSnapshotId).not.toBe(firstRun.researchProfileSnapshotId);
      expect(secondDetail.researchProfile?.profileVersion).toBe('2.0.0');
      expect(secondDetail.appServerMemory?.activeCatalogHash).not.toBe(firstCatalogHash);
      const retainedFirstDetail = service.getRunDetail(firstRunId);
      expect(retainedFirstDetail.researchProfile?.profileVersion).toBe('1.0.0');
      expect(retainedFirstDetail.appServerMemory?.activeCatalogHash).toBe(firstCatalogHash);
      expect(service.getSnapshot()?.researchProfile).toMatchObject({
        profileVersion: '2.0.0',
        profileHash: currentProfile.hash
      });

      const invocations = readInvocations(invocationLog);
      expect(invocations.map((invocation) => invocation.profileVersion)).toEqual(['1.0.0', '2.0.0']);
      expect(invocations.map((invocation) => invocation.workflow)).toEqual(['discovery', 'analysis-pass']);
      expect(invocations[0]?.args).toEqual(expect.arrayContaining([
        '--research-profile-hash',
        '--workflow',
        'discovery'
      ]));
      expect(invocations[0]?.args).not.toContain('--resolved-research-profile');
      expect(invocations[0]?.args).toEqual(expect.arrayContaining([
        '--profile-tool-family-ceiling',
        'shell',
        '--profile-tool-family-ceiling',
        'repository-search',
        '--profile-tool-family-ceiling',
        'file-read',
        '--profile-side-effect-ceiling',
        'none',
        '--profile-side-effect-ceiling',
        'read',
        '--profile-side-effect-ceiling',
        'write',
        '--profile-side-effect-ceiling',
        'process'
      ]));
      expect(invocations[0]?.args).not.toContain('--tool-family');
      expect(invocations[0]?.args).toEqual(expect.arrayContaining(['--allowed-side-effect', 'network']));
      expect(invocations[0]?.args).not.toContain('--disable-tool-family');
      expect(invocations[0]?.args).not.toContain('--allow-mcp-server');
      expect(invocations[0]?.args).not.toContain('--skill');
      expect(invocations[0]?.args).not.toContain('--memory-type-descriptions');
      const detail = await service.getRunDetailForClient(firstRunId);
      const launchEvents = detail.traceEvents.filter((event) => event.summary.startsWith('app-server session requested from the Beale app-server'));
      expect(launchEvents).toHaveLength(1);
      const serializedLaunches = JSON.stringify(launchEvents);
      expect(serializedLaunches).not.toContain(resolvedTestResearchProfile(firstProfile).hash);
      expect(serializedLaunches).toContain('[profile-hash]');
      expect(serializedLaunches).toContain('[host-resolved-profile]');
    } finally {
      service.close();
    }
  }, 90_000);

  it('keeps memory-disabled recommendation jobs isolated from app-server memory storage and context', async () => {
    process.env.BEALE_OPENAI_ACCESS_TOKEN = 'memory-disabled-recommendation-test-token';
    const root = temporaryDirectory();
    const workspace = join(root, 'workspace');
    mkdirSync(workspace, { recursive: true });
    const baseProfile = testResearchProfile('memory-disabled');
    const profile: ResearchProfile = {
      ...baseProfile,
      workflows: baseProfile.workflows.map((workflow) => workflow.id === 'discovery'
        ? { ...workflow, goalSuggestionCount: 2 }
        : workflow),
      capabilities: {
        ...baseProfile.capabilities,
        memoryEnabled: false
      },
      modelJobs: {
        goalSuggestions: { provider: 'openai-codex', model: 'gpt-security-goals', effort: 'low' },
        promptGeneration: { provider: 'openai', model: 'gpt-security-prompts', effort: 'high' }
      }
    };
    const modelRequests: Record<string, unknown>[] = [];
    const databasePath = join(root, 'memory.sqlite');
    const service = new WorkspaceService(() => undefined, {
      workspaceRegistryDirectory: join(root, 'registry'),
      appServerDatabasePath: databasePath,
      appServerArtifactDirectory: join(root, 'artifacts'),
      researchProfileResolver: () => resolvedTestResearchProfile(profile),
      openAiFetch: async (_url, init) => {
        const request = JSON.parse(String(init.body ?? '{}')) as Record<string, unknown>;
        modelRequests.push(request);
        const task = (request.metadata as Record<string, unknown> | undefined)?.beale_task;
        return task === 'research_goal_suggestions'
          ? modelGoalSuggestionResponse(request, [
              'Review the recorded trust boundaries within the authorized scope.',
              'Investigate how input validation affects the scoped service.'
            ], 'resp_memory_disabled_goals')
          : modelJsonResponse({
              promptMarkdown: '# Security discovery\n\nInspect the authorized service, distinguish observations from inference, and preserve uncertainty.'
            }, 'resp_memory_disabled_prompt');
      }
    });

    try {
      service.createWorkspace(workspace);
      startRunForTest(service, runInput('discovery'));

      // Any accidental recommendation-path memory read now fails on the deliberately incompatible table.
      const memoryDatabase = new DatabaseSync(databasePath);
      try {
        memoryDatabase.exec('CREATE TABLE memory_nodes (broken TEXT)');
      } finally {
        memoryDatabase.close();
      }

      await expect(service.generateResearchGoalSuggestions({ phase: 'discovery' }))
        .resolves.toMatchObject({ phase: 'discovery' });
      await expect(service.generateResearchPrompt({
        operation: 'generate',
        researchPhase: 'discovery',
        mode: 'discovery',
        attemptStrategy: 'iterative_research',
        model: 'session-model',
        reasoningEffort: 'medium',
        sandboxProfile: 'host'
      })).resolves.toMatchObject({ promptMarkdown: expect.stringContaining('Security discovery') });

      expect(modelRequests).toHaveLength(2);
      for (const request of modelRequests) {
        expect(String(request.instructions)).not.toMatch(/app-server memory|recorded memories|active memory/i);
        const payload = modelRequestPayload(request);
        expect(JSON.stringify(payload)).not.toMatch(/activeMemoryNodes|recentMemoryEvidenceRefs|memoryNodeId/);
        const previousResearch = (payload.previousResearch ?? []) as Record<string, unknown>[];
        for (const previous of previousResearch) {
          expect(previous).not.toHaveProperty('memoryNodes');
          for (const contract of (previous.verifierContracts as Record<string, unknown>[])) {
            expect(contract).not.toHaveProperty('memoryNodeId');
          }
        }
      }
    } finally {
      service.close();
    }
  });

  it('rejects a mismatched profile at the Desktop/app-server capture boundary', async () => {
    const captureOptions: FakeAppServerCaptureOptions = {
      researchProfileOverride: {
        ...resolvedTestResearchProfile(profileWithWorkflow('9.0.0', 'discovery')),
        workflowId: 'discovery'
      }
    };
    const root = temporaryDirectory();
    const workspace = join(root, 'workspace');
    const invocationLog = join(root, 'invocations.jsonl');
    const fakeAppServer = join(root, 'fake-appServer.mjs');
    mkdirSync(workspace, { recursive: true });
    writeFileSync(fakeAppServer, fakeAppServerSource(captureOptions));
    configureFakeAppServer(root, fakeAppServer, [invocationLog]);
    await ensureBealeAppServerRunning();

    const service = new WorkspaceService(() => undefined, {
      workspaceRegistryDirectory: join(root, 'registry'),
      appServerDatabasePath: join(root, 'memory.sqlite'),
      appServerArtifactDirectory: join(root, 'artifacts'),
      researchProfileResolver: () => resolvedTestResearchProfile(testResearchProfile())
    });
    try {
      service.createWorkspace(workspace);
      const started = service.startRun(runInput('discovery'));
      const runId = started.runs[0]?.run.id ?? '';
      await waitForCondition(() => service.getRunDetail(runId).run.status === 'failed', 20_000);

      const detail = service.getRunDetail(runId);
      expect(detail.run.summary).toMatch(/does not match the profile and workflow pinned to session/);
      expect(detail.artifacts.some((artifact) => artifact.kind === 'app_server_flow_capture')).toBe(false);
      expect(detail.traceEvents.some((event) =>
        event.summary === 'app-server flow capture preserved as a Beale artifact.'
      )).toBe(false);
      expect(detail.transcriptMessages.some((message) => message.source === 'app-server')).toBe(false);
    } finally {
      service.close();
    }
  });

  it('blocks continuation of legacy runs without pinned research profile provenance', () => {
    const root = temporaryDirectory();
    const workspace = join(root, 'workspace');
    mkdirSync(workspace, { recursive: true });
    const database = new WorkspaceDatabase(join(root, 'memory.sqlite'), join(root, 'artifacts'), { workspacePath: workspace });
    database.initialize();
    const legacy = database.createRun({
      scopeVersionId: database.getActiveScope().id,
      title: 'Legacy research run',
      promptMarkdown: 'Legacy prompt.',
      shellSafetyMode: 'auto_review',
      mode: 'open_discovery',
      model: 'fixture-model',
      reasoningEffort: 'minimal',
      attemptStrategy: 'iterative_research',
      sandboxProfile: 'host',
      budget: { maxMinutes: 5, maxAttempts: 1, maxCostUsd: 0, runEngine: 'app-server' }
    });
    const engine = new AppServerRunEngine(database);
    try {
      expect(() => engine.extendRun(legacy.run.id, 'Continue.')).toThrow(/no pinned research profile snapshot/);
    } finally {
      engine.dispose();
      database.close();
    }
  });
});

function temporaryDirectory(): string {
  const directory = mkdtempSync(join(tmpdir(), 'beale-profile-integration-'));
  directories.push(directory);
  return directory;
}

function profileWithWorkflow(version: string, workflowId: string): ResearchProfile {
  const base = testResearchProfile(version, `Profile ${version}`);
  return {
    ...base,
    workflows: [{
      ...base.workflows[0]!,
      id: workflowId,
      name: workflowId === 'discovery' ? 'Discovery' : 'Analysis Pass',
      default: true
    }],
    capabilities: {
      ...base.capabilities,
      defaultToolFamilies: ['shell', 'code'],
      disabledToolFamilies: ['analysis'],
      allowedSideEffects: ['read', 'write', 'network'],
      selectedSkillIds: ['profile-skill'],
      allowedMcpServerIds: ['local']
    }
  };
}

function runInput(workflowId: string): StartRunInput {
  return {
    runEngine: 'app-server',
    provider: 'openai-codex',
    shellSafetyMode: 'auto_review',
    goalEnabled: false,
    goalObjective: null,
    promptMarkdown: `Research using ${workflowId}.`,
    workflowId,
    mode: 'dynamic_research',
    attemptStrategy: 'iterative_research',
    model: 'fixture-model',
    reasoningEffort: 'minimal',
    sandboxProfile: 'host',
    budget: { maxMinutes: 5, maxAttempts: 1, maxCostUsd: 0 }
  };
}

interface FakeAppServerCaptureOptions {
  researchProfileOverride?: ResolvedResearchProfile & { workflowId: string };
}

function fakeAppServerSource(options: FakeAppServerCaptureOptions = {}): string {
  const profileOverride = options.researchProfileOverride
    ? {
        schemaVersion: options.researchProfileOverride.profile.schemaVersion,
        id: options.researchProfileOverride.profile.id,
        version: options.researchProfileOverride.profile.version,
        hash: options.researchProfileOverride.hash,
        source: options.researchProfileOverride.source,
        ...(options.researchProfileOverride.path ? { path: options.researchProfileOverride.path } : {}),
        workflowId: options.researchProfileOverride.workflowId,
        snapshot: options.researchProfileOverride.profile
      }
    : null;
  return [
    "import { appendFileSync, mkdirSync, writeFileSync } from 'node:fs';",
    "import { dirname } from 'node:path';",
    "import { pathToFileURL } from 'node:url';",
    'const [logPath, ...args] = process.argv.slice(2);',
    "const value = (flag) => args[args.indexOf(flag) + 1];",
    "const values = (flag) => args.flatMap((arg, index) => arg === flag ? [args[index + 1]] : []);",
    "const capturePath = value('--capture');",
    "const profileHash = value('--research-profile-hash');",
    "const workflow = value('--workflow');",
    "const workspaceRoot = value('--workspace-root');",
    "const researchAgent = await import(pathToFileURL(process.env.FAKE_RESEARCH_AGENT_MODULE).href);",
    "const resolvedProfile = await researchAgent.resolveStoredResearchProfile({ workspaceRoot, researchProfileId: value('--research-profile-id'), researchProfileHash: profileHash });",
    "const profile = resolvedProfile.profile;",
    "const repositoryRoots = values('--repo-root');",
    "const binding = researchAgent.resolveStoredResearchWorkspaceBinding({ workspaceRoot, externalSessionId: value('--session-id'), workflowId: workflow, knownRepositoryRoots: repositoryRoots, researchProfileId: profile.id, researchProfileHash: resolvedProfile.hash });",
    "const workspaceContext = { schemaVersion: 1, workspaceRoot, memoryContext: binding.memoryContext, ...(binding.authorization ? { authorization: binding.authorization } : {}), knownRepositories: repositoryRoots.map((rootPath) => ({ rootPath, role: rootPath === workspaceRoot ? 'workspace' : 'known_repository', source: 'app-server' })), materializedSourcePaths: repositoryRoots, projectNotes: binding.projectNotes ?? [] };",
    `const profileOverride = ${JSON.stringify(profileOverride)};`,
    'appendFileSync(logPath, JSON.stringify({ args, profileHash, profileVersion: profile.version, workflow, workspaceContext }) + "\\n");',
    'mkdirSync(dirname(capturePath), { recursive: true });',
    'const now = new Date().toISOString();',
    "const captureResearchProfile = profileOverride ?? { schemaVersion: profile.schemaVersion, id: profile.id, version: profile.version, hash: profileHash, source: resolvedProfile.source, workflowId: workflow, snapshot: profile };",
    "const capture = { schemaVersion: 5, capturedAt: now, request: { prompt: value('-p') }, researchProfile: captureResearchProfile, agent: { id: 'agent_profile_fixture', status: 'complete', executorName: 'profile-fixture', startedAt: now, completedAt: now, outputText: 'Profile fixture complete.' }, eventTimeline: [] };",
    "writeFileSync(capturePath, JSON.stringify(capture) + '\\n');",
    "const sessionStore = new researchAgent.AppServerSessionStore();",
    "try { sessionStore.importCapture(value('--session-id'), { attemptId: value('--attempt-id'), capture }); } finally { sessionStore.close(); }"
  ].join('\n');
}

const FAKE_APP_SERVER_FIXTURE = fileURLToPath(new URL('./fixtures/fakeAppServer.mjs', import.meta.url));
const APP_SERVER_SESSION_LAUNCH_MODULE = fileURLToPath(new URL('../../../app-server/dist/index.js', import.meta.url));
const RESEARCH_AGENT_MODULE = fileURLToPath(new URL('../../../packages/research-agent/dist/index.js', import.meta.url));

/**
 * Routes run launches through the app-server client using the fixture host.
 * The scripted worker supplies deterministic capture data for Desktop tests.
 */
function configureFakeAppServer(root: string, childScript: string, childArgs: readonly string[]): void {
  delete process.env.BEALE_APP_SERVER_COMMAND;
  delete process.env.BEALE_APP_SERVER_ARGS_JSON;
  delete process.env.BEALE_APP_SERVER_CWD;
  const stateFile = join(root, 'app-server-state.json');
  process.env.BEALE_APP_SERVER_COMMAND = process.execPath;
  process.env.BEALE_APP_SERVER_ARGS_JSON = JSON.stringify([FAKE_APP_SERVER_FIXTURE]);
  process.env.BEALE_APP_SERVER_STATE_FILE = stateFile;
  process.env.BEALE_APP_SERVER_PARENT_PID = String(process.pid);
  process.env.FAKE_APP_SERVER_STATE_FILE = stateFile;
  process.env.FAKE_APP_SERVER_CHILD_SCRIPT = childScript;
  process.env.FAKE_APP_SERVER_CHILD_ARGS_JSON = JSON.stringify(childArgs);
  process.env.FAKE_APP_SERVER_SESSION_LAUNCH_MODULE = APP_SERVER_SESSION_LAUNCH_MODULE;
  process.env.FAKE_APP_SERVER_REGISTRY_DIRECTORY = join(root, 'registry');
  process.env.FAKE_APP_SERVER_DATABASE_PATH = join(root, 'memory.sqlite');
  process.env.FAKE_APP_SERVER_ARTIFACT_DIRECTORY = join(root, 'artifacts');
  process.env.FAKE_RESEARCH_AGENT_MODULE = RESEARCH_AGENT_MODULE;
  process.env.BEALE_APP_SERVER_SESSION_OWNERSHIP = 'app-server';
}

function stopFakeAppServer(): void {
  const stateFile = process.env.BEALE_APP_SERVER_STATE_FILE;
  if (!stateFile || !existsSync(stateFile)) return;
  try {
    const record = JSON.parse(readFileSync(stateFile, 'utf8')) as { pid?: unknown };
    if (typeof record.pid === 'number' && record.pid !== process.pid) {
      try { process.kill(record.pid); } catch { /* already gone */ }
    }
    rmSync(stateFile, { force: true });
  } catch {
    // Best-effort teardown.
  }
}

interface LoggedInvocation {
  args: string[];
  profileHash: string;
  profileVersion: string;
  workflow: string;
  workspaceContext: Record<string, unknown>;
}

function readInvocations(path: string): LoggedInvocation[] {
  if (!existsSync(path)) return [];
  return readFileSync(path, 'utf8')
    .trim()
    .split('\n')
    .filter(Boolean)
    .map((line) => JSON.parse(line) as LoggedInvocation);
}

function modelRequestPayload(request: Record<string, unknown>): Record<string, unknown> {
  const input = request.input as Array<{ content: Array<{ text: string }> }>;
  return JSON.parse(input[0]?.content[0]?.text ?? '{}') as Record<string, unknown>;
}

function modelJsonResponse(value: unknown, id: string): Response {
  const event = (name: string, data: Record<string, unknown>) =>
    `event: ${name}\ndata: ${JSON.stringify(data)}\n\n`;
  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(new TextEncoder().encode(
        event('response.output_text.done', { type: 'response.output_text.done', text: JSON.stringify(value) }) +
        event('response.completed', { type: 'response.completed', response: { id } })
      ));
      controller.close();
    }
  });
  return new Response(stream, { status: 200, headers: { 'content-type': 'text/event-stream' } });
}

function modelGoalSuggestionResponse(
  request: Record<string, unknown>,
  suggestions: readonly string[],
  id: string
): Response {
  const payload = modelRequestPayload(request);
  const candidateCount = Number(payload.candidateCount);
  return modelJsonResponse({
    candidates: Array.from({ length: candidateCount }, (_, index) => ({
      goal: suggestions[index % suggestions.length],
      groundingRefs: ['workspace:scope'],
      rationale: 'The recorded collection makes this a bounded and discriminating research direction.',
      noveltyAxis: `candidate-${index + 1}`
    }))
  }, id);
}

async function waitForRun(service: WorkspaceService, runId: string): Promise<void> {
  await waitForCondition(async () => {
    try {
      return (await service.getRunDetailForClient(runId)).run.status !== 'active';
    } catch (error) {
      if (error instanceof Error && error.message.includes('workspace research index is unavailable during another workspace operation')) return false;
      throw error;
    }
  }, 25_000);
  const detail = await service.getRunDetailForClient(runId);
  const run = detail.run;
  if (run.status !== 'completed') {
    throw new Error(
      `Expected run ${runId} to complete, but it ${run.status}: ${run.summary}; trace: ${JSON.stringify(detail.traceEvents.slice(-4))}`
    );
  }
}

async function waitForCondition(check: () => boolean | Promise<boolean>, timeoutMs = 5_000): Promise<void> {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    if (await check()) return;
    await new Promise<void>((resolve) => setTimeout(resolve, 25));
  }
  if (!await check()) throw new Error(`waitForCondition timed out after ${timeoutMs}ms`);
}
