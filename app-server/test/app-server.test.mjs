import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { createServer } from "node:http";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { afterEach, beforeEach, test } from "node:test";
import { deserialize } from "node:v8";
import { fileURLToPath } from "node:url";
import {
  AppServerHostRegistry,
  AppServerHostService,
  acquireDiscoveryLock,
  createAppServerPairingPayload,
  createAppServerPairingQrCode,
  discoveryLockPath,
  appServerWorkerEnvironment,
  appServerSessionArgs,
  appServerSessionEnvironment,
  operatorTokenPath,
  nextAutomationRunAt,
  releaseDiscoveryLock,
  startAppServer,
} from "../dist/index.js";
import {
  APP_SERVER_SESSION_LAUNCH_VERSION,
  BEALE_APP_SERVER_CAPABILITIES,
  BEALE_APP_SERVER_CONTRACT_TIMESTAMP,
  BEALE_APP_SERVER_CONTROL_VERSION,
  MANAGED_TOOL_PLUGIN_IDS,
} from "@beale/app-server-runtime/protocol";
import { AppServerSessionStore, ResearchTrackStore, WORKSPACE_DIRECTORIES } from "../../packages/research-agent/dist/index.js";
import {
  AppServerWorkerDatabaseBroker,
  AppServerWorkerDatabaseCoordinator,
} from "../dist/workerDatabaseBroker.js";
import { createWorkerResearchDatabaseFactory } from "../dist/workerDatabaseClient.js";
import { scheduleCompletedWorkerExit } from "../dist/appServerSession.js";
import { researchTrackStopCause } from "../dist/appServer.js";
import {
  callAppServerResearchTool,
  listAppServerResearchTools,
} from "../dist/researchToolBridge.js";
import { WorkspaceDatabase } from "../dist/workspaceDatabase.js";
import { initializeWorkspaceProjectAsync } from "../dist/workspaceCheckpoints.js";
import { WorkspaceRegistry } from "../dist/workspaceRegistryStore.js";
import { FleetIncompleteResponseError } from "../dist/fleet.js";

const requireFromHere = createRequire(import.meta.url);
const WebSocket = requireFromHere("ws");

const servers = [];
const temporaryDirectories = [];
const originalMockMode = process.env.BEALE_APP_SERVER_MOCK;

test('host-default auxiliary completion uses Beale provider and authentication settings', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-bore-provider-'));
  temporaryDirectories.push(directory);
  let invocation;
  const registry = {
    ...hostRegistryFixture(directory),
    providerSettings: () => ({
      defaultProviderId: 'openrouter',
      modelDefaults: { openrouter: { leadModel: 'example/model', reasoningEffort: 'high' } },
      authenticationPreferences: { openrouter: 'api_key' }, riskAcknowledgements: [],
    }),
  };
  const host = new AppServerHostService({ registry, invokeProtocol: async (operation, options) => {
    invocation = { operation, options };
    return { text: '{"probe_id":"own-container-round-trip"}', usage: {} };
  } });
  const result = await host.executeOperation({ operation: 'provider.complete', input: {
    useHostDefaults: true, provider: 'attacker-choice', model: 'attacker-model',
    systemPrompt: 'Choose a compiled probe.', prompt: '{"probe_ids":["own-container-round-trip"]}',
    authenticationPreferences: { openrouter: 'subscription' },
  } });
  assert.equal(result.text, '{"probe_id":"own-container-round-trip"}');
  assert.equal(invocation.operation, 'provider.complete');
  assert.equal(invocation.options.input.provider, 'openrouter');
  assert.equal(invocation.options.input.model, 'example/model');
  assert.deepEqual(invocation.options.input.authenticationPreferences, { openrouter: 'api_key' });
  assert.equal(invocation.options.input.useHostDefaults, undefined);
});

test("released research-index state cannot block core workspace state", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-workspace-state-isolation-"));
  temporaryDirectories.push(directory);
  mkdirSync(join(directory, ".beale"), { recursive: true });
  writeFileSync(join(directory, "workspace.json"), JSON.stringify({
    schemaVersion: 2,
    workspaceId: "workspace-test",
    directories: WORKSPACE_DIRECTORIES,
    checkpointIntervalMs: 600000,
    researchAuthority: "files",
  }));
  writeFileSync(join(directory, ".beale", "research-cache.json"), JSON.stringify({
    schemaVersion: 1,
    workspaceId: "workspace-test",
    publicationHash: "a".repeat(64),
    state: "released",
    updatedAt: "2026-09-01T00:00:00.000Z",
  }));
  let invoked = false;
  const host = new AppServerHostService({
    registry: hostRegistryFixture(directory),
    invokeProtocol: async (operation) => {
      assert.equal(operation, "workspace.state");
      invoked = true;
      return { opened: true };
    },
  });
  const result = await host.executeOperation({
    operation: "workspace.state",
    input: {
      workspacePath: directory,
      workspaceId: "workspace-test",
      artifactRoot: join(directory, ".beale", "artifacts"),
      databasePath: join(directory, "memory.sqlite"),
      artifactDirectoryPath: join(directory, "artifacts"),
      action: "initialize",
    },
  });
  assert.equal(invoked, true);
  assert.deepEqual(result, { opened: true });
});

test("persists the OpenAI context size provider setting", () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-provider-context-size-"));
  temporaryDirectories.push(directory);
  const registry = new WorkspaceRegistry(directory);
  assert.equal(registry.getProviderSettings().contextSizes, undefined);
  assert.deepEqual(registry.setProviderContextSize("openai-codex", "large").contextSizes, {
    "openai-codex": "large",
  });
  registry.close();

  const reopened = new WorkspaceRegistry(directory);
  assert.equal(reopened.getProviderSettings().contextSizes?.["openai-codex"], "large");
  reopened.close();
});

test("persists one OpenAI Daybreak mode and exposes it to the session host", () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-daybreak-mode-"));
  temporaryDirectories.push(directory);
  const registry = new WorkspaceRegistry(directory);
  registry.setProviderModelDefaults("openai-codex", {
    largeModel: "gpt-daybreak-blue-latest", smallModel: "gpt-5.6-luna", reasoningEffort: "high",
  });
  registry.setProviderOptionalModelEnabled("openai-codex", "gpt-daybreak-red-latest", true);
  assert.deepEqual(registry.getProviderSettings().enabledOptionalModels?.["openai-codex"], ["gpt-daybreak-red-latest"]);
  assert.deepEqual(registry.getProviderSettings().disabledOptionalModels?.["openai-codex"], ["gpt-daybreak-blue-latest"]);
  assert.equal(registry.getProviderSettings().modelDefaults["openai-codex"], undefined);
  assert.equal(new AppServerHostRegistry({ registryDirectory: directory }).providerSettings().daybreakMode, "red");
  registry.setProviderOptionalModelEnabled("openai-codex", "gpt-daybreak-blue-latest", true);
  assert.equal(registry.getProviderSettings().enabledOptionalModels?.["openai-codex"], undefined);
  assert.equal(registry.getProviderSettings().disabledOptionalModels?.["openai-codex"], undefined);
  assert.equal(new AppServerHostRegistry({ registryDirectory: directory }).providerSettings().daybreakMode, "blue");
  registry.close();
});

test('research starts without a workspace Git checkpoint', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-no-checkpoint-example-'));
  temporaryDirectories.push(directory);
  const upstream = await createFakeAppServerSessionHost();
  const hostService = testHostService(directory);
  hostService.checkpointSession = async () => { throw new Error('Git checkpoint must not run.'); };
  const server = await startAppServer({ host: '127.0.0.1', port: 0, hostService, spawnSession: upstream.spawnSession });
  servers.push(server);
  await server.startSession(sessionLaunchRequest(directory, { sessionId: 'session-no-checkpoint-example' }));
  assert.equal(server.listSessions()[0]?.state, 'running');
  upstream.complete();
});

test('the app-server reserves a safe research-track epoch and marks a failed next launch as paused', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-track-host-example-'));
  temporaryDirectories.push(directory);
  const registry = hostRegistryFixture(directory);
  registry.listWorkspaces = () => [{ workspaceId: 'workspace-test' }];
  const appended = [];
  const hostService = new AppServerHostService({ registry, invokeProtocol: async (operation, options) => {
    if (operation !== 'session.append_event') throw new Error('Unexpected protocol operation.');
    appended.push(options.input);
    return null;
  } });
  const details = async () => ({
    scope: { id: 'scope-example', expiresAt: null },
    targets: [{ id: 'local', ready: true }],
    surfaceMap: { surfaces: [{ id: 'surface-example', parentId: null, name: 'Example service',
      coverageState: { covered: false, unmetRequirements: ['surface_map', 'sinks'], unblockedFindingIds: [] },
      progressBlocker: null }] },
  });
  hostService.workspaceDetails = details;
  hostService.workspaceResearchTrackContext = details;
  const collaboration = { mode: 'adaptive', subagentMode: 'advanced', intensity: 'normal',
    providers: [{ provider: 'openai-codex', model: 'example-model', enabled: true }] };
  const started = await hostService.workspaceResearchTrack('workspace-test', 'start', {
    provider: { id: 'openai-codex', model: 'example-model', reasoningEffort: 'high' },
    targetIds: ['local'], collaboration
  });
  assert.equal(started.status, 'running');
  const due = await hostService.dueResearchTracks();
  assert.equal(due.length, 1);
  assert.equal(due[0].request.launch.mode, 'research-track');
  assert.equal(due[0].request.launch.shellSafetyMode, 'auto_review');
  assert.equal(due[0].request.launch.guardianEnabled, true);
  assert.deepEqual(due[0].request.launch.targetIds, ['local']);
  assert.deepEqual(due[0].request.launch.collaboration, collaboration);
  assert.match(due[0].request.launch.promptMarkdown, /Assigned component: Example service/u);
  assert.match(due[0].request.launch.promptMarkdown, /pre-auth boundaries, sensitive memory/u);
  assert.match(due[0].request.launch.promptMarkdown, /Compare alternative explanations/u);
  assert.match(due[0].request.launch.promptMarkdown, /Dedicated track workers handle the existing finding/u);
  assert.match(due[0].request.launch.promptMarkdown, /call track\.checkpoint/u);
  assert.equal((await hostService.workspaceResearchTrack('workspace-test', 'status')).activeSessionId,
    due[0].request.sessionId);
  await hostService.recordResearchTrackEpochLaunchFailure('workspace-test', due[0].request.sessionId,
    'SSH host key verification failed. Synthetic example failure.');
  assert.equal((await hostService.workspaceResearchTrack('workspace-test', 'status')).status, 'paused');
  assert.equal(appended.length, 1);
  assert.deepEqual(appended[0].payload, {
    type: 'track_epoch_paused', cause: 'ssh_host_key', agentPath: '/root'
  });
});

test('retiring the assigned surface ends its epoch and schedules remaining track work', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-track-retired-example-'));
  temporaryDirectories.push(directory);
  const registry = hostRegistryFixture(directory);
  registry.listWorkspaces = () => [{ workspaceId: 'workspace-test' }];
  const hostService = new AppServerHostService({ registry });
  const available = [
    { id: 'surface-example-a', parentId: null, name: 'Example service A',
      coverageState: { covered: false, unmetRequirements: ['surface_map'], unblockedFindingIds: [] }, progressBlocker: null },
    { id: 'surface-example-b', parentId: null, name: 'Example service B',
      coverageState: { covered: false, unmetRequirements: ['surface_map'], unblockedFindingIds: [] }, progressBlocker: null }
  ];
  let surfaces = available;
  hostService.workspaceResearchTrackContext = async () => ({
    scope: { id: 'scope-example', expiresAt: null }, targets: [{ id: 'local', ready: true }],
    surfaceMap: { surfaces }
  });
  const track = await hostService.workspaceResearchTrack('workspace-test', 'start');
  const [first] = await hostService.dueResearchTracks();
  assert.equal(first.request.sessionId, track.id);
  const assigned = (await hostService.workspaceResearchTrack('workspace-test', 'status')).activeSurfaceId;
  surfaces = available.filter((surface) => surface.id !== assigned);

  const retired = await hostService.workspaceResearchTrack('workspace-test', 'status');
  assert.equal(retired.status, 'running');
  assert.equal(retired.activeSessionId, null);
  assert.equal(retired.lastError, null);
  assert.equal(retired.surfaces.find((surface) => surface.surfaceId === assigned).status, 'excluded');
  const [next] = await hostService.dueResearchTracks();
  assert.equal(next.request.sessionId, track.id);
  assert.equal((await hostService.workspaceResearchTrack('workspace-test', 'status')).activeSurfaceId, surfaces[0].id);
});

test('the scheduler retires a missing assigned surface without pausing the track', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-track-scheduler-example-'));
  temporaryDirectories.push(directory);
  const registry = hostRegistryFixture(directory);
  registry.listWorkspaces = () => [{ workspaceId: 'workspace-test' }];
  const hostService = new AppServerHostService({ registry });
  const available = [
    { id: 'surface-example-a', parentId: null, name: 'Example service A',
      coverageState: { covered: false, unmetRequirements: ['surface_map'], unblockedFindingIds: [] }, progressBlocker: null },
    { id: 'surface-example-b', parentId: null, name: 'Example service B',
      coverageState: { covered: false, unmetRequirements: ['surface_map'], unblockedFindingIds: [] }, progressBlocker: null }
  ];
  let surfaces = available;
  hostService.workspaceResearchTrackContext = async () => ({
    scope: { id: 'scope-example', expiresAt: null }, targets: [{ id: 'local', ready: true }],
    surfaceMap: { surfaces }
  });
  const track = await hostService.workspaceResearchTrack('workspace-test', 'start');
  await hostService.dueResearchTracks();
  const assigned = (await hostService.workspaceResearchTrack('workspace-test', 'status')).activeSurfaceId;
  surfaces = available.filter((surface) => surface.id !== assigned);

  assert.deepEqual(await hostService.dueResearchTracks(), []);
  const retired = await hostService.workspaceResearchTrack('workspace-test', 'status');
  assert.equal(retired.status, 'running');
  assert.equal(retired.activeSessionId, null);
  assert.equal(retired.lastError, null);
  assert.equal(retired.surfaces.find((surface) => surface.surfaceId === assigned).status, 'excluded');
  assert.equal((await hostService.dueResearchTracks()).length, 1);
  assert.equal((await hostService.workspaceResearchTrack('workspace-test', 'status')).activeSurfaceId, surfaces[0].id);
});

test('research-track scheduler context reads authorization and surfaces without loading workspace memory', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-track-context-example-'));
  temporaryDirectories.push(directory);
  const registry = hostRegistryFixture(directory);
  const operations = [];
  const hostService = new AppServerHostService({ registry, invokeProtocol: async (operation) => {
    operations.push(operation);
    if (operation !== 'workspace.state') throw new Error('Full workspace memory must not be loaded.');
    return { activeScope: { id: 'scope-example', expiresAt: null } };
  } });
  hostService.fleet.state = async () => ({ enabled: true,
    workspaceTargets: { 'workspace-test': ['local'] }, browsers: [], machines: [], sandboxes: [] });
  hostService.workspaceSurfaceMap = async () => ({ surfaces: [{ id: 'surface-example' }] });

  const context = await hostService.workspaceResearchTrackContext(registry.resolveWorkspace('workspace-test'));
  assert.deepEqual(context.scope, { id: 'scope-example', expiresAt: null });
  assert.deepEqual(context.targets, [{ id: 'local', name: 'Local', ready: true }]);
  assert.deepEqual(context.surfaceMap.surfaces, [{ id: 'surface-example' }]);
  assert.deepEqual(operations, ['workspace.state']);
});

test('research-track target readiness accepts an allowed VM recovery base', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-track-vm-recovery-example-'));
  temporaryDirectories.push(directory);
  const registry = hostRegistryFixture(directory);
  const hostService = new AppServerHostService({ registry, invokeProtocol: async () => ({
    activeScope: { id: 'scope-example', expiresAt: null }
  }) });
  const runTarget = { id: 'tart:example-vm', name: 'example-vm', canRun: true, canClone: false,
    state: 'stopped', sshConfigured: false, guestPlatform: 'darwin', owner: null };
  const cloneBase = { id: 'tart:example-base', name: 'example-base', canRun: false, canClone: true,
    state: 'stopped', sshConfigured: true, guestPlatform: 'darwin', owner: null };
  hostService.fleet.state = async () => ({ enabled: true,
    workspaceTargets: { 'workspace-test': ['local', 'vm:run:tart:example-vm', 'vm:clone:tart:example-base'] },
    browsers: [], machines: [runTarget, cloneBase], sandboxes: [] });
  hostService.workspaceSurfaceMap = async () => ({ surfaces: [] });
  const workspace = registry.resolveWorkspace('workspace-test');
  const recoverable = await hostService.workspaceResearchTrackContext(workspace);
  assert.equal(recoverable.targets.find((target) => target.id === 'vm:run:tart:example-vm')?.ready, true);
  hostService.fleet.state = async () => ({ enabled: true,
    workspaceTargets: { 'workspace-test': ['local', 'vm:run:tart:example-vm'] },
    browsers: [], machines: [runTarget, cloneBase], sandboxes: [] });
  const unavailable = await hostService.workspaceResearchTrackContext(workspace);
  assert.equal(unavailable.targets.find((target) => target.id === 'vm:run:tart:example-vm')?.ready, false);
});

test('stopping a track session continues research while explicit pause still pauses it', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-track-stop-example-'));
  temporaryDirectories.push(directory);
  const registry = hostRegistryFixture(directory);
  registry.listWorkspaces = () => [{ workspaceId: 'workspace-test' }];
  const hostService = new AppServerHostService({ registry,
    invokeProtocol: async (operation) => operation === 'session.get' ? { status: 'stopped' } : null });
  const details = async () => ({
    scope: { id: 'scope-example', expiresAt: null }, targets: [{ id: 'local', ready: true }],
    surfaceMap: { surfaces: [{ id: 'surface-example', parentId: null, name: 'Example service',
      coverageState: { covered: false, unmetRequirements: ['surface_map'], unblockedFindingIds: [] },
      progressBlocker: null }] },
  });
  hostService.workspaceDetails = details;
  hostService.workspaceResearchTrackContext = details;
  await hostService.workspaceResearchTrack('workspace-test', 'start');
  const first = (await hostService.dueResearchTracks())[0].request;
  assert.equal(first.launch.mode, 'research-track');
  assert.equal((await hostService.workspaceResearchTrack('workspace-test', 'status')).activeSessionId, first.sessionId);
  await hostService.recordSessionTerminalState({ request: first, sessionId: first.sessionId,
    attemptId: 'attempt-example-one', state: 'stopped' });
  assert.equal((await hostService.workspaceResearchTrack('workspace-test', 'status')).status, 'running');
  const second = (await hostService.dueResearchTracks())[0].request;
  await hostService.recordSessionTerminalState({ request: second, sessionId: second.sessionId,
    attemptId: 'attempt-example-two', state: 'stopped', trackRotation: true });
  assert.equal((await hostService.workspaceResearchTrack('workspace-test', 'status')).status, 'running');
  assert.equal((await hostService.workspaceResearchTrack('workspace-test', 'pause')).status, 'paused');
  assert.equal((await hostService.workspaceResearchTrack('workspace-test', 'resume')).status, 'running');
  registry.resolveWorkspace('workspace-test').memoryBackend = 'disabled';
  assert.equal((await hostService.workspaceResearchTrack('workspace-test', 'status')).status, 'paused');
  assert.equal((await hostService.workspaceResearchTrack('workspace-test', 'stop')).status, 'stopped');
});

test('a new track pass waits for the stopped worker and starts a fresh session', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-track-restart-example-'));
  temporaryDirectories.push(directory);
  const registry = hostRegistryFixture(directory);
  registry.listWorkspaces = () => [{ workspaceId: 'workspace-test' }];
  const hostService = new AppServerHostService({ registry });
  hostService.workspaceResearchTrackContext = async () => ({
    scope: { id: 'scope-example', expiresAt: null },
    targets: [{ id: 'local', ready: true }],
    surfaceMap: { surfaces: [{ id: 'surface-example', parentId: null, name: 'Example service',
      coverageState: { covered: false, unmetRequirements: ['surface_map'], unblockedFindingIds: [] },
      progressBlocker: null }] },
  });
  const first = await hostService.workspaceResearchTrack('workspace-test', 'start');
  const [firstEpoch] = await hostService.dueResearchTracks();
  assert.equal(firstEpoch.request.sessionId, first.id);
  const stopped = await hostService.workspaceResearchTrack('workspace-test', 'stop');
  assert.equal(stopped.activeSessionId, null);
  const second = await hostService.workspaceResearchTrack('workspace-test', 'start');
  assert.notEqual(second.id, first.id);
  assert.deepEqual(await hostService.dueResearchTracks(new Date(), new Set([first.id]),
    new Set(['workspace-test'])), []);
  const [secondEpoch] = await hostService.dueResearchTracks();
  assert.equal(secondEpoch.request.sessionId, second.id);
  assert.equal(secondEpoch.request.launch.continuation, undefined);
});

test('a blocked track checkpoint advances to another surface in the same session', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-track-blocked-example-'));
  temporaryDirectories.push(directory);
  const registry = hostRegistryFixture(directory);
  registry.listWorkspaces = () => [{ workspaceId: 'workspace-test' }];
  const hostService = new AppServerHostService({ registry,
    invokeProtocol: async (operation) => operation === 'session.get' ? { status: 'blocked' } : null });
  const surfaces = ['surface-alpha', 'surface-beta'].map((id) => ({
    id, parentId: null, name: id,
    coverageState: { covered: false, unmetRequirements: ['sinks'], unblockedFindingIds: [] },
    progressBlocker: null
  }));
  const details = async () => ({ scope: { id: 'scope-example', expiresAt: null },
    targets: [{ id: 'local', ready: true }], surfaceMap: { surfaces } });
  hostService.workspaceDetails = details;
  hostService.workspaceResearchTrackContext = details;
  await hostService.workspaceResearchTrack('workspace-test', 'start');
  const first = (await hostService.dueResearchTracks())[0].request;
  assert.match(first.launch.promptMarkdown, /surface-alpha/u);

  surfaces[0].progressBlocker = { reason: 'External access required.' };
  const store = new ResearchTrackStore({ databasePath: registry.storageForProfile('security-research').databasePath,
    workspaceId: 'workspace-test', sessionId: first.sessionId });
  try {
    store.checkpoint({ surfaceId: 'surface-alpha', outcome: 'blocked', impact: 'high',
      theory: 'Cross-boundary access requires a separate authorization.', nextAction: 'Request authorized access.',
      evidenceRefs: [] }, surfaces[0]);
  } finally { store.close(); }
  await hostService.recordSessionTerminalState({ request: first, sessionId: first.sessionId,
    attemptId: 'attempt-example-one', state: 'completed' });

  const next = (await hostService.dueResearchTracks())[0].request;
  assert.equal(next.sessionId, first.sessionId);
  assert.match(next.launch.promptMarkdown, /surface-beta/u);
  assert.equal((await hostService.workspaceResearchTrack('workspace-test', 'status')).epochCount, 2);
});

test('an authorization change pauses a reserved research epoch before the next scheduler action', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-track-scope-example-'));
  temporaryDirectories.push(directory);
  const registry = hostRegistryFixture(directory);
  registry.listWorkspaces = () => [{ workspaceId: 'workspace-test' }];
  const hostService = new AppServerHostService({ registry });
  let scopeId = 'scope-example-one';
  const details = async () => ({
    scope: { id: scopeId, expiresAt: null }, targets: [{ id: 'local', ready: true }],
    surfaceMap: { surfaces: [{ id: 'surface-example', parentId: null, name: 'Example service',
      coverageState: { covered: false, unmetRequirements: ['surface_map'], unblockedFindingIds: [] },
      progressBlocker: null }] },
  });
  hostService.workspaceDetails = details;
  hostService.workspaceResearchTrackContext = details;
  await hostService.workspaceResearchTrack('workspace-test', 'start');
  assert.equal((await hostService.dueResearchTracks()).length, 1);
  scopeId = 'scope-example-two';
  assert.equal((await hostService.dueResearchTracks()).length, 0);
  assert.equal((await hostService.workspaceResearchTrack('workspace-test', 'status')).status, 'paused');
});

test('a connected primary forwards research-track control to its owning app-server', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-track-remote-example-'));
  temporaryDirectories.push(directory);
  const hostService = new AppServerHostService({ registry: hostRegistryFixture(directory) });
  let forwarded;
  hostService.fleet.callRemote = async (serverId, operation, input) => {
    forwarded = { serverId, operation, input };
    return { id: 'track-example', status: 'running' };
  };
  const result = await hostService.executeOperation({ operation: 'fleet.remote_research_track',
    input: { serverId: 'server-example', workspaceId: 'workspace-test', action: 'start',
      configuration: { provider: { id: 'openai-codex', model: 'example-model', reasoningEffort: 'high' },
        targetIds: ['local'], collaboration: { subagentMode: 'advanced' } } } });
  assert.equal(result.status, 'running');
  assert.deepEqual(forwarded, { serverId: 'server-example', operation: 'workspace.research_track',
    input: { workspaceId: 'workspace-test', action: 'start',
      configuration: { provider: { id: 'openai-codex', model: 'example-model', reasoningEffort: 'high' },
        targetIds: ['local'], collaboration: { subagentMode: 'advanced' } } } });
});

test('remote research-track control checks committed state after an incomplete reply', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-track-remote-recovery-example-'));
  temporaryDirectories.push(directory);
  const hostService = new AppServerHostService({ registry: hostRegistryFixture(directory) });
  const actions = [];
  hostService.fleet.callRemote = async (_serverId, _operation, input) => {
    actions.push(input.action);
    if (input.action === 'start') throw new FleetIncompleteResponseError('workspace.research_track');
    return { id: 'track-example', status: 'running' };
  };
  const result = await hostService.executeOperation({ operation: 'fleet.remote_research_track',
    input: { serverId: 'server-example', workspaceId: 'workspace-test', action: 'start' } });
  assert.equal(result.status, 'running');
  assert.deepEqual(actions, ['start', 'status']);
});

test('keeps a hosted session starting until the runtime readiness handshake completes', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-runtime-ready-example-'));
  temporaryDirectories.push(directory);
  const upstream = await createFakeAppServerSessionHost();
  let releaseReady;
  const ready = new Promise((resolve) => { releaseReady = resolve; });
  const server = await startAppServer({
    hostService: testHostService(directory),
    spawnSession: async (options) => ({
      ...await upstream.spawnSession(options),
      waitReady: () => ready,
    }),
  });
  servers.push(server);

  const started = await server.startSession(sessionLaunchRequest(directory, { sessionId: 'session-runtime-ready-example' }));
  assert.equal(started.session.state, 'starting');
  assert.equal(server.listSessions()[0]?.state, 'starting');

  releaseReady();
  await waitFor(() => server.listSessions()[0]?.state === 'running');
  upstream.complete();
});

test('a connected primary session gives its worker a session-scoped browser endpoint', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-remote-browser-example-'));
  temporaryDirectories.push(directory);
  const upstream = await createFakeAppServerSessionHost();
  let browserEndpoint;
  const server = await startAppServer({ hostService: testHostService(directory),
    spawnSession: async (options) => {
      browserEndpoint = options.env.BEALE_FLEET_BROWSER_ENDPOINT;
      return upstream.spawnSession(options);
    } });
  servers.push(server);
  const request = sessionLaunchRequest(directory, { sessionId: 'session-remote-browser-example' });
  request.launch.machineId = 'local';
  request.launch.browserRelay = true;
  await server.startSession(request);
  assert.match(browserEndpoint, /^http:\/\/127\.0\.0\.1:[0-9]+\/v1\/fleet-browser\/session-remote-browser-example\?token=/u);
  upstream.complete();
});

test('fails stalled runtime initialization with a visible bounded diagnostic', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-runtime-startup-timeout-example-'));
  temporaryDirectories.push(directory);
  const upstream = await createFakeAppServerSessionHost();
  const hostService = testHostService(directory);
  const launchFailures = [];
  const terminalStates = [];
  hostService.recordSessionLaunchFailure = async (input) => { launchFailures.push(input); };
  hostService.recordSessionTerminalState = async (input) => { terminalStates.push(input); };
  const server = await startAppServer({
    hostService,
    sessionStartupTimeoutMs: 100,
    longSessionRecovery: false,
    spawnSession: async (options) => ({
      ...await upstream.spawnSession(options),
      waitReady: () => new Promise(() => undefined),
    }),
  });
  servers.push(server);

  const started = await server.startSession(sessionLaunchRequest(directory, { sessionId: 'session-runtime-timeout-example' }));
  assert.equal(started.session.state, 'starting');
  const messages = [];
  const socket = await connect(webSocketUrl(server.url, started.transport.path), started.transport.token, messages);
  socket.send(JSON.stringify(clientHello('session-runtime-timeout-example', 'startup-timeout-test')));
  await waitFor(() => server.listSessions()[0]?.state === 'failed');
  const entry = server.listSessions()[0];
  assert.match(entry?.diagnostic ?? '', /did not become ready within 1 seconds/u);
  assert.match(entry?.diagnostic ?? '', /No model request was sent/u);
  assert.equal(upstream.stopCalls(), 1);
  assert.equal(launchFailures.length, 1);
  assert.equal(launchFailures[0].sessionId, 'session-runtime-timeout-example');
  await waitFor(() => terminalStates.length === 1);
  assert.equal(terminalStates[0].sessionId, 'session-runtime-timeout-example');
  assert.equal(terminalStates[0].state, 'failed');
  await waitFor(() => messages.some((message) => (
    message.type === 'session.event'
      && message.event?.kind === 'model.output'
      && /No model request was sent/u.test(message.event.payload?.text ?? '')
  )));
  socket.close();
});

test('session finalization persists early worker exits without overwriting terminal sessions', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-session-finalization-example-'));
  temporaryDirectories.push(directory);
  let status = 'active';
  const transitions = [];
  const service = new AppServerHostService({
    registry: hostRegistryFixture(directory),
    invokeProtocol: async (operation, options) => {
      if (operation === 'session.get') return { status };
      if (operation === 'session.transition') {
        transitions.push(options.input);
        status = options.input.status;
        return { status };
      }
      throw new Error(`Unexpected operation: ${operation}`);
    },
  });
  const input = { request: sessionLaunchRequest(directory, { sessionId: 'session-finalization-example' }),
    sessionId: 'session-finalization-example', attemptId: 'attempt-example', state: 'failed' };
  await service.recordSessionTerminalState(input);
  assert.equal(status, 'failed');
  assert.equal(transitions[0].attemptId, 'attempt-example');
  assert.equal(transitions.length, 1);
  await service.recordSessionTerminalState({ ...input, state: 'stopped' });
  assert.equal(status, 'failed');
  assert.equal(transitions.length, 1);
});

test("creates a versioned app-server pairing payload without altering credentials", () => {
  const payload = new URL(createAppServerPairingPayload(
    "https://beale.example.ts.net",
    "operator_token-123"
  ));
  assert.equal(payload.protocol, "beale:");
  assert.equal(payload.hostname, "connect");
  assert.equal(payload.searchParams.get("v"), "1");
  assert.equal(payload.searchParams.get("url"), "https://beale.example.ts.net");
  assert.equal(payload.searchParams.get("token"), "operator_token-123");
  assert.throws(
    () => createAppServerPairingPayload("https://beale.example.ts.net/path", "operator_token-123"),
    /HTTP or HTTPS origin/,
  );
});

test("creates a PNG QR code for a synthetic app-server pairing endpoint", async () => {
  const result = await createAppServerPairingQrCode(
    "https://server.example.test:47174", "synthetic-operator-token"
  );
  assert.match(result.dataUrl, /^data:image\/png;base64,[A-Za-z0-9+/]+={0,2}$/u);
  assert.ok(Buffer.from(result.dataUrl.slice("data:image/png;base64,".length), "base64").length > 500);
  await assert.rejects(
    createAppServerPairingQrCode("https://server.example.test/path", "synthetic-operator-token"),
    /HTTP or HTTPS origin/u
  );
});

test("Fleet pairing QR operation uses a saved app-server credential", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-fleet-pairing-example-"));
  temporaryDirectories.push(directory);
  writeFileSync(join(directory, "fleet.json"), JSON.stringify({
    version: 3,
    role: "primary",
    enabled: true,
    appServers: {
      "server-example": {
        name: "Example Server",
        url: "https://server.example.ts.net:47174",
        operatorToken: "synthetic-operator-token"
      }
    }
  }));
  const host = new AppServerHostService({ registry: hostRegistryFixture(directory) });
  const result = await host.executeOperation({
    operation: "fleet.pairing_qr_code", input: { serverId: "server-example" }
  });
  assert.match(result.dataUrl, /^data:image\/png;base64,/u);
  await assert.rejects(
    host.executeOperation({ operation: "fleet.pairing_qr_code", input: { serverId: "missing" } }),
    /Unknown Fleet app server/u
  );
});

test("Fleet pairing QR operation uses only the current local app-server discovery", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-local-pairing-example-"));
  temporaryDirectories.push(directory);
  const discoveryPath = join(directory, "app-server.json");
  const priorStateFile = process.env.BEALE_APP_SERVER_STATE_FILE;
  process.env.BEALE_APP_SERVER_STATE_FILE = discoveryPath;
  const record = {
    version: 1, contractTimestamp: BEALE_APP_SERVER_CONTRACT_TIMESTAMP,
    pid: process.pid, host: "127.0.0.1", port: 47173,
    localUrl: "http://127.0.0.1:47173", url: "https://server.example.ts.net:47174",
    operatorToken: "synthetic-operator-token", startedAt: "2030-01-01T00:00:00.000Z"
  };
  const host = new AppServerHostService({ registry: hostRegistryFixture(directory) });
  try {
    writeFileSync(discoveryPath, JSON.stringify(record));
    const result = await host.executeOperation({
      operation: "fleet.pairing_qr_code", input: { serverId: "local" }
    });
    assert.match(result.dataUrl, /^data:image\/png;base64,/u);
    writeFileSync(discoveryPath, JSON.stringify({ ...record, pid: process.pid + 1 }));
    await assert.rejects(
      host.executeOperation({ operation: "fleet.pairing_qr_code", input: { serverId: "local" } }),
      /local app server is unavailable/u
    );
  } finally {
    if (priorStateFile === undefined) delete process.env.BEALE_APP_SERVER_STATE_FILE;
    else process.env.BEALE_APP_SERVER_STATE_FILE = priorStateFile;
  }
});

beforeEach(() => {
  process.env.BEALE_APP_SERVER_MOCK = "1";
});

afterEach(async () => {
  while (servers.length) {
    await servers.pop().close();
  }
  for (const directory of temporaryDirectories.splice(0)) {
    rmSync(directory, { recursive: true, force: true });
  }
  if (originalMockMode === undefined) delete process.env.BEALE_APP_SERVER_MOCK;
  else process.env.BEALE_APP_SERVER_MOCK = originalMockMode;
});

test("health endpoint responds ok without an operator token", async () => {
  const server = await startAppServer();
  servers.push(server);
  const response = await fetch(`${server.url}/health`);
  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), {
    ok: true,
    controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
    contractTimestamp: BEALE_APP_SERVER_CONTRACT_TIMESTAMP,
    capabilities: BEALE_APP_SERVER_CAPABILITIES,
  });
});

test("control plane requires the operator bearer token", async () => {
  const server = await startAppServer({ operatorToken: "operator-secret" });
  servers.push(server);

  const unauthedGet = await fetch(`${server.url}/v1/sessions`);
  assert.equal(unauthedGet.status, 401);

  const unauthedPost = await fetch(`${server.url}/v1/sessions`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({}),
  });
  assert.equal(unauthedPost.status, 401);

  const wrongTokenGet = await fetch(`${server.url}/v1/sessions`, {
    headers: { authorization: "Bearer wrong" },
  });
  assert.equal(wrongTokenGet.status, 401);
  assert.deepEqual(await wrongTokenGet.json(), {
    controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
    error: {
      code: "unauthorized",
      message: "An operator bearer token is required for this operation.",
      retryable: false,
    },
  });

  const authorizedGet = await fetch(`${server.url}/v1/sessions`, {
    headers: { authorization: `Bearer ${server.operatorToken}` },
  });
  assert.equal(authorizedGet.status, 200);
  assert.deepEqual(await authorizedGet.json(), {
    controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
    sessions: [],
  });

  const descriptor = await fetch(`${server.url}/v1/server`, {
    headers: { authorization: `Bearer ${server.operatorToken}` },
  });
  assert.equal(descriptor.status, 200);
  assert.deepEqual((await descriptor.json()).endpoints, {
    sessions: "/v1/sessions",
    workspaces: "/v1/workspaces",
    providers: "/v1/providers",
    operations: "/v1/operations",
    shutdown: "/v1/server/shutdown",
  });
});

test('Bore checkpoint route requires operator auth and returns path-free hash', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-bore-route-'));
  temporaryDirectories.push(directory);
  const hostService = new AppServerHostService({ registry: hostRegistryFixture(directory) });
  const server = await startAppServer({ hostService, operatorToken: 'operator-secret' });
  servers.push(server);
  const body = { workspaceId: 'workspace-test', session: {
    schemaVersion: 1, id: '11111111-2222-4333-8444-555555555555',
    prompt: 'Test a synthetic app boundary.', startedAt: '2026-01-01T00:00:00Z',
    updatedAt: '2026-01-01T00:01:00Z', status: 'paused', planner: 'Beale: example/model',
    environment: { platform: 'iOS', osVersion: '19.0', osBuild: 'example', hardwareModel: 'iPhone-example',
      bundleID: 'org.example.bore', appVersion: '1', entitlementProfile: 'baseline', executionContext: 'foreground app' },
    observations: [],
  } };
  const unauthorized = await fetch(`${server.url}/v1/bore/checkpoints`, {
    method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body),
  });
  assert.equal(unauthorized.status, 401);
  const capabilities = await fetch(`${server.url}/v1/bore/checkpoints`, {
    headers: { authorization: 'Bearer operator-secret' },
  });
  assert.deepEqual(await capabilities.json(), { schemaVersion: 1, checkpointVersion: 1, hostReceiptVersion: 1 });
  const authorized = await fetch(`${server.url}/v1/bore/checkpoints`, {
    method: 'POST', headers: { 'content-type': 'application/json', authorization: 'Bearer operator-secret' },
    body: JSON.stringify(body),
  });
  assert.equal(authorized.status, 200);
  const result = await authorized.json();
  assert.equal(result.sessionId, body.session.id);
  assert.match(result.contentHash, /^[0-9a-f]{64}$/u);
  assert.equal(JSON.stringify(result).includes(directory), false);
  const jobId = 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee';
  const receipt = await fetch(`${server.url}/v1/bore/receipts`, {
    method: 'POST', headers: { 'content-type': 'application/json', authorization: 'Bearer operator-secret' },
    body: JSON.stringify({ workspaceId: 'workspace-test', receipt: {
      schemaVersion: 1, jobId, sessionId: body.session.id, kind: 'proof-cycle', status: 'completed',
      completedAt: '2026-01-01T00:02:00Z', steps: [{ name: 'build-proof', status: 'completed' }],
    } }),
  });
  assert.equal(receipt.status, 200);
  const lookup = await fetch(`${server.url}/v1/bore/receipts/${jobId}?workspaceId=workspace-test&sessionId=${body.session.id}`, {
    headers: { authorization: 'Bearer operator-secret' },
  });
  assert.equal(lookup.status, 200);
  assert.deepEqual((await lookup.json()).steps, [{ name: 'build-proof', status: 'completed' }]);
});

test("a primary proxies a remote-owned VM session through its guest transport", async () => {
  const guestRequests = [];
  let guestLaunch = null;
  const guestSockets = new WebSocket.Server({ noServer: true });
  const browserInputs = [];
  const guest = createServer((request, response) => {
    guestRequests.push({ method: request.method, path: request.url });
    response.setHeader("content-type", "application/json");
    if (request.method === "POST" && request.url === "/v1/sessions") {
      const chunks = [];
      request.on("data", (chunk) => chunks.push(chunk));
      request.on("end", () => {
        guestLaunch = JSON.parse(Buffer.concat(chunks).toString("utf8"));
        response.statusCode = 201;
        response.end(JSON.stringify({ controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
          session: { sessionId: "session-relay", state: "running" }, attemptId: "attempt-relay",
          transport: { path: "/v1/sessions/session-relay/transport", protocolVersion: 1,
            authentication: "bearer", token: "guest-client-token", reconnect: "replay" } }));
      });
      return;
    }
    if (request.url === "/v1/sessions/session-relay") {
      response.end(JSON.stringify({ controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
        session: { sessionId: "session-relay", state: "running" } }));
      return;
    }
    if (request.url === "/v1/sessions/session-relay/attachments") {
      response.statusCode = 201;
      response.end(JSON.stringify({ controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
        transport: { path: "/v1/sessions/session-relay/transport", protocolVersion: 1,
          authentication: "bearer", token: "attached-client-token", reconnect: "replay" } }));
      return;
    }
    response.statusCode = 404;
    response.end("{}");
  });
  guest.on("upgrade", (request, socket, head) => {
    if (request.url !== "/v1/sessions/session-relay/browser"
      || !["Bearer guest-client-token", "Bearer attached-client-token"].includes(request.headers.authorization)) {
      socket.destroy(); return;
    }
    guestSockets.handleUpgrade(request, socket, head, (client) => {
      client.send(JSON.stringify({ type: "browser.ready", url: "https://example.test/", width: 1280, height: 800 }));
      client.on("message", (raw) => browserInputs.push(JSON.parse(String(raw))));
    });
  });
  await new Promise((resolve) => guest.listen(0, "127.0.0.1", resolve));
  try {
    const guestUrl = `http://127.0.0.1:${guest.address().port}`;
    const operations = [];
    const server = await startAppServer({ hostService: {
      async executeOperation(request) {
        operations.push(request);
        if (request.operation === "fleet.state") return { role: "primary" };
        if (request.operation === "fleet.prepare" || request.operation === "fleet.connect") {
          return { machineId: "tart:session-clone", url: guestUrl, operatorToken: "guest-operator-token" };
        }
        throw new Error(`Unexpected operation ${request.operation}`);
      },
    } });
    servers.push(server);
    const auth = { authorization: `Bearer ${server.operatorToken}`, "content-type": "application/json" };
    const request = sessionLaunchRequest(tmpdir(), { sessionId: "session-relay" });
    request.launch.machineId = "tart:worker";
    request.launch.fleetOwnerMachineId = "machine-source";
    const started = await fetch(`${server.url}/v1/sessions`, { method: "POST", headers: auth, body: JSON.stringify(request) });
    assert.equal(started.status, 201);
    assert.equal((await started.json()).transport.token, "guest-client-token");
    assert.equal(guestLaunch.launch.machineId, "tart:session-clone");
    const read = await fetch(`${server.url}/v1/sessions/session-relay`, { headers: auth });
    assert.equal(read.status, 200);
    assert.equal((await read.json()).session.state, "running");
    const attached = await fetch(`${server.url}/v1/sessions/session-relay/attachments`, { method: "POST", headers: auth });
    assert.equal(attached.status, 201);
    assert.equal((await attached.json()).transport.token, "attached-client-token");
    const connected = await fetch(`${server.url}/v1/operations`, { method: "POST", headers: auth,
      body: JSON.stringify({ operation: "fleet.connect", input: { runId: "session-relay", machineId: "tart:worker", proxy: true } }) });
    assert.equal(connected.status, 200);
    const browser = new WebSocket(`${server.url.replace(/^http/u, "ws")}/v1/sessions/session-relay/browser`, {
      headers: { authorization: "Bearer guest-client-token" },
    });
    try {
      const ready = await new Promise((resolve, reject) => {
        const timeout = setTimeout(() => reject(new Error("Fleet browser relay did not respond.")), 3000);
        browser.once("message", (raw) => { clearTimeout(timeout); resolve(JSON.parse(String(raw))); });
        browser.once("error", reject);
      });
      assert.equal(ready.url, "https://example.test/");
      browser.send(JSON.stringify({ type: "browser.mouse", id: 1, eventType: "mouseMoved", x: 10, y: 10 }));
      await new Promise((resolve) => setTimeout(resolve, 20));
      assert.equal(browserInputs[0].type, "browser.mouse");
    } finally { browser.close(); }
    assert.deepEqual(operations.find((operation) => operation.operation === "fleet.prepare").input,
      { workspaceId: "workspace-test", runId: "session-relay", machineId: "tart:worker", ownerMachineId: "machine-source" });
    assert.ok(guestRequests.some((entry) => entry.method === "GET" && entry.path === "/v1/sessions/session-relay"));
  } finally { guestSockets.close(); await new Promise((resolve) => guest.close(resolve)); }
});

test("continues a terminal session through the authenticated control plane", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-continuation-route-"));
  temporaryDirectories.push(directory);
  const upstream = await createFakeAppServerSessionHost();
  const hostService = testHostService(directory);
  const continuationCalls = [];
  hostService.createSessionContinuationRequest = async (workspaceId, sessionId, instruction) => {
    continuationCalls.push({ workspaceId, sessionId, instruction });
    const request = sessionLaunchRequest(directory, { sessionId, promptMarkdown: instruction });
    request.launch.attemptId = "attempt-continuation";
    request.launch.generateTitle = false;
    request.launch.continuation = {
      resumeAttemptId: "attempt-original",
      resumeFromInitialAttempt: true,
      fallbackPrompt: "Retained canonical session history.",
    };
    return request;
  };
  hostService.recordSessionContinuationInstruction = async (request) => {
    continuationCalls.push({ recordedAttemptId: request.launch.attemptId });
  };
  const server = await startAppServer({
    hostService,
    spawnSession: async (...args) => {
      continuationCalls.push({ spawned: true });
      return await upstream.spawnSession(...args);
    },
    operatorToken: "operator-secret",
  });
  servers.push(server);

  const response = await fetch(`${server.url}/v1/sessions/session-existing/continuations`, {
    method: "POST",
    headers: {
      authorization: "Bearer operator-secret",
      "content-type": "application/json",
    },
    body: JSON.stringify({
      workspaceId: "workspace-test",
      instruction: "Continue from the prior evidence.",
    }),
  });
  assert.equal(response.status, 201);
  const started = await response.json();
  assert.equal(started.session.sessionId, "session-existing");
  assert.deepEqual(continuationCalls, [
    {
      workspaceId: "workspace-test",
      sessionId: "session-existing",
      instruction: "Continue from the prior evidence.",
    },
    { recordedAttemptId: "attempt-continuation" },
    { spawned: true },
  ]);
  upstream.complete();
});

test("steers an active session through authenticated HTTP control", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-http-control-"));
  temporaryDirectories.push(directory);
  const upstream = await createFakeAppServerSessionHost();
  const server = await startAppServer({
    hostService: testHostService(directory),
    spawnSession: upstream.spawnSession,
    operatorToken: "operator-secret",
  });
  servers.push(server);
  await server.startSession(sessionLaunchRequest(directory, { sessionId: "session-http-control" }));

  const response = await fetch(`${server.url}/v1/sessions/session-http-control/control`, {
    method: "POST",
    headers: {
      authorization: "Bearer operator-secret",
      "content-type": "application/json",
    },
    body: JSON.stringify({ type: "steer", instruction: "Check the alternate parser path." }),
  });

  assert.equal(response.status, 202);
  const result = await response.json();
  assert.equal(result.accepted, true);
  assert.equal(result.sessionId, "session-http-control");
  assert.equal(result.type, "steer");
  assert.match(result.requestId, /^[0-9a-f-]{36}$/u);
  assert.deepEqual(upstream.controls, [{
    schemaVersion: 1,
    requestId: result.requestId,
    type: "steer",
    instruction: "Check the alternate parser path.",
  }]);
  upstream.complete();
});

test("HTTP stop control terminates the hosted worker without waiting for cooperative completion", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-http-stop-"));
  temporaryDirectories.push(directory);
  const upstream = await createFakeAppServerSessionHost();
  const server = await startAppServer({
    hostService: testHostService(directory),
    spawnSession: upstream.spawnSession,
    operatorToken: "operator-secret",
  });
  servers.push(server);
  await server.startSession(sessionLaunchRequest(directory, { sessionId: "session-http-stop" }));

  const response = await fetch(`${server.url}/v1/sessions/session-http-stop/control`, {
    method: "POST",
    headers: {
      authorization: "Bearer operator-secret",
      "content-type": "application/json",
    },
    body: JSON.stringify({ type: "stop" }),
  });

  assert.equal(response.status, 202);
  const result = await response.json();
  assert.equal(result.accepted, true);
  assert.equal(result.type, "stop");
  await waitFor(() => upstream.stopCalls() === 1);
  await waitFor(() => server.listSessions()[0]?.state === "stopped");
  assert.deepEqual(upstream.controls, [{
    schemaVersion: 1,
    requestId: result.requestId,
    type: "stop",
  }]);
});

test("an accepted worker stop arms termination and cannot be undone by a late resume acknowledgement", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-stop-ack-example-"));
  temporaryDirectories.push(directory);
  const upstream = await createFakeAppServerSessionHost();
  const hostService = testHostService(directory);
  const states = [];
  hostService.recordSessionControlState = async (input) => { states.push(input.state); };
  const server = await startAppServer({ hostService, spawnSession: upstream.spawnSession });
  servers.push(server);
  await server.startSession(sessionLaunchRequest(directory, { sessionId: "session-stop-ack-example" }));
  const acknowledge = (type) => upstream.sendEvent({
    schemaVersion: 1, kind: "agent.event", timestamp: new Date().toISOString(),
    payload: { eventType: "control.received", type, accepted: true, requestId: `request-${type}-example` },
  });
  acknowledge("stop");
  acknowledge("resume");
  assert.equal(upstream.stopCalls(), 1);
  assert.deepEqual(states, ["stopped"]);
  await waitFor(() => server.listSessions()[0]?.state === "stopped");
});

test("publishes a path-free model catalog for connected providers with host defaults", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-providers-"));
  temporaryDirectories.push(directory);
  const registry = hostRegistryFixture(directory);
  registry.providerSettings = () => ({
    defaultProviderId: "xai",
    modelDefaults: {
      xai: { leadModel: "grok-4.6", smallModel: "grok-4.3", reasoningEffort: "high" },
    },
    authenticationPreferences: { xai: "api_key" },
    riskAcknowledgements: ["xai"],
  });
  const server = await startAppServer({ hostService: new AppServerHostService({ registry }) });
  servers.push(server);

  const response = await fetch(`${server.url}/v1/providers`, {
    headers: { authorization: `Bearer ${server.operatorToken}` },
  });
  assert.equal(response.status, 200);
  const payload = await response.json();
  assert.equal(payload.controlVersion, BEALE_APP_SERVER_CONTROL_VERSION);
  assert.equal(payload.defaultProviderId, "xai");
  assert.deepEqual(payload.providers.map((provider) => provider.providerId), ["xai"]);
  assert.equal(payload.providers[0].defaultLeadModel, "grok-4.6");
  assert.equal(payload.providers[0].defaultSubagentModel, "grok-4.3");
  assert.equal(payload.providers[0].defaultReasoningEffort, "high");
  assert.equal(payload.providers[0].riskAcknowledged, true);
  assert.ok(payload.providers[0].models.some((model) => model.id === "grok-4.6"));
  assert.ok(payload.providers[0].models.some((model) => model.id === "grok-4.7"));
  assert.doesNotMatch(JSON.stringify(payload), /credential|token|auth|workspacePath|databasePath/u);
});

test("serves sanitized provider limits to authenticated clients and relays a saved app server", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-limits-"));
  temporaryDirectories.push(directory);
  const host = new AppServerHostService({ registry: hostRegistryFixture(directory) });
  const requests = [];
  host.providerUsageLimits = async (providerId, refresh, serverId) => {
    requests.push({ providerId, refresh, serverId });
    return {
      providerId,
      checkedAt: "2033-01-01T00:00:00.000Z",
      status: "available",
      detail: null,
      windows: [{ id: "session", label: "Session Limit", usedPercent: refresh ? 20 : 10,
        resetsAt: "2033-01-01T04:00:00.000Z", durationMinutes: 240 }],
    };
  };
  const server = await startAppServer({ hostService: host });
  servers.push(server);
  const path = "/v1/providers/limits?provider=openai-codex&refresh=1";
  assert.equal((await fetch(`${server.url}${path}`)).status, 401);
  const response = await fetch(`${server.url}${path}`, {
    headers: { authorization: `Bearer ${server.operatorToken}` },
  });
  assert.equal(response.status, 200);
  const payload = await response.json();
  assert.equal(payload.windows[0].label, "Session Limit");
  assert.equal(payload.windows[0].usedPercent, 20);
  assert.doesNotMatch(JSON.stringify(payload), /credential|token|authFile|accessToken/u);
  const remote = await fetch(`${server.url}${path}&serverId=server-example`, {
    headers: { authorization: `Bearer ${server.operatorToken}` },
  });
  assert.equal(remote.status, 200);
  assert.deepEqual(requests, [
    { providerId: 'openai-codex', refresh: true, serverId: null },
    { providerId: 'openai-codex', refresh: true, serverId: 'server-example' },
  ]);
  const invalid = await fetch(`${server.url}/v1/providers/limits?provider=invalid`, {
    headers: { authorization: `Bearer ${server.operatorToken}` },
  });
  assert.equal(invalid.status, 400);
  const invalidServer = await fetch(`${server.url}/v1/providers/limits?provider=openai-codex&serverId=`, {
    headers: { authorization: `Bearer ${server.operatorToken}` },
  });
  assert.equal(invalidServer.status, 400);
});

test("executes canonical operations inside the app-server host", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-operation-"));
  temporaryDirectories.push(directory);
  const server = await startAppServer({
    hostService: new AppServerHostService({ registry: hostRegistryFixture(directory) }),
  });
  servers.push(server);
  const response = await fetch(`${server.url}/v1/operations`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${server.operatorToken}`,
      "content-type": "application/json",
    },
    body: JSON.stringify({ operation: "provider.describe", input: {} }),
  });
  assert.equal(response.status, 200);
  const payload = await response.json();
  assert.equal(payload.controlVersion, BEALE_APP_SERVER_CONTROL_VERSION);
  assert.equal(payload.result.sessionTitleEffort, "medium");
});

test("owns Desktop workspace and registry persistence behind allowlisted operations", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-persistence-"));
  temporaryDirectories.push(directory);
  const workspacePath = join(directory, "workspace");
  const registryDirectory = join(directory, "registry");
  const databasePath = join(directory, "memory.sqlite");
  const artifactDirectoryPath = join(directory, "artifacts");
  mkdirSync(workspacePath, { recursive: true });
  const server = await startAppServer({
    hostService: new AppServerHostService({ registry: hostRegistryFixture(directory) }),
  });
  servers.push(server);

  const invoke = async (operation, input) => {
    const response = await fetch(`${server.url}/v1/operations`, {
      method: "POST",
      headers: {
        authorization: `Bearer ${server.operatorToken}`,
        "content-type": "application/json",
      },
      body: JSON.stringify({ operation, input }),
    });
    assert.equal(response.status, 200);
    return (await response.json()).result;
  };

  const registry = await invoke("registry.state", {
    registryDirectory,
    action: "initialize",
  });
  assert.deepEqual(registry.state.workspaces, []);
  assert.equal(existsSync(join(registryDirectory, "workspace-registry.sqlite")), true);
  assert.deepEqual(await invoke("registry.state", {
    registryDirectory,
    action: "setTracesEnabled",
    args: [true],
  }), { tracesEnabled: true });

  const workspace = await invoke("workspace.state", {
    workspacePath,
    artifactRoot: join(workspacePath, ".beale", "artifacts"),
    databasePath,
    artifactDirectoryPath,
    researchKitId: "general",
    action: "initialize",
  });
  assert.equal(typeof workspace.workspaceId, "string");
  assert.equal(workspace.researchKitId, "general");
  assert.equal(existsSync(databasePath), true);
  assert.equal(await invoke("workspace.state", {
    workspacePath,
    workspaceId: workspace.workspaceId,
    artifactRoot: join(workspacePath, ".beale", "artifacts"),
    databasePath,
    artifactDirectoryPath,
    researchKitId: "general",
    action: "addWorkspaceRule",
    args: ["Keep the persistence boundary explicit."],
  }).then((rule) => rule.text), "Keep the persistence boundary explicit.");

  const sessions = new AppServerSessionStore({ databasePath });
  try {
    sessions.create({
      id: "session-registry-sync",
      workspaceId: workspace.workspaceId,
      attemptId: "attempt-registry-sync",
      title: "Canonical registry session",
      prompt: "Verify canonical sessions reach the workspace registry.",
      provider: "openai-codex",
      model: "gpt-5.6-sol",
      reasoningEffort: "high",
      metadata: {
        bealeRun: {
          id: "session-registry-sync",
          mode: "open_discovery",
          attemptStrategy: "iterative_research",
          sandboxProfile: "host",
          budget: { runEngine: "app-server" },
        },
      },
    });
  } finally {
    sessions.close();
  }

  const synchronized = await invoke("registry.state", {
    registryDirectory,
    action: "syncWorkspaceFromStorage",
    args: [{
      workspacePath,
      workspaceDirectories: [workspacePath],
      databasePath,
      artifactRoot: join(workspacePath, ".beale", "artifacts"),
      researchProfileId: "security-research",
      rememberLast: true,
    }],
  });
  const synchronizedRegistry = synchronized.state;
  assert.equal(synchronizedRegistry.workspaces.length, 1);
  assert.equal(synchronizedRegistry.workspaces[0].workspaceId, workspace.workspaceId);
  assert.equal(synchronizedRegistry.workspaces[0].workspaceName, "Untitled Workspace");
  assert.equal(synchronizedRegistry.workspaces[0].runCount, 1);
  assert.deepEqual(synchronizedRegistry.researchSessions.map((session) => ({
    runId: session.runId,
    title: session.title,
    status: session.status,
  })), [{
    runId: "session-registry-sync",
    title: "Canonical registry session",
    status: "active",
  }]);
  assert.equal(synchronized.lastKnownWorkspace.workspaceId, workspace.workspaceId);
});

test("routes Codex research tools through canonical workspace identity and storage", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-codex-tools-"));
  temporaryDirectories.push(directory);
  const calls = [];
  const service = new AppServerHostService({
    registry: hostRegistryFixture(directory),
    invokeProtocol: async (operation, options) => {
      calls.push({ operation, options });
      return { status: "complete" };
    },
  });

  await service.executeOperation({
    operation: "research.tools.mutate",
    input: {
      workspaceId: "workspace-test",
      workspaceRoot: "/client-controlled",
      researchProfileId: "client-profile",
      memoryBackend: "disabled",
      sessionId: "session-codex",
      toolName: "lead.create",
      toolInput: { title: "Candidate" },
      modelAuthor: { provider: "openai-codex", model: "gpt-6-astra" },
    },
  });

  assert.equal(calls[0].operation, "research.tools.mutate");
  assert.deepEqual(calls[0].options.input, {
    workspaceId: "workspace-test",
    workspaceName: "Test workspace",
    workspaceRoot: directory,
    researchProfileId: "security-research",
    memoryBackend: "app-server",
    sessionId: "session-codex",
    toolName: "lead.create",
    toolInput: { title: "Candidate" },
    modelAuthor: { provider: "openai-codex", model: "gpt-6-astra" },
  });
  assert.equal(calls[0].options.storage.databasePath, join(directory, "memory.sqlite"));
  await assert.rejects(
    service.executeOperation({
      operation: "research.tools.list",
      profileId: "security-research",
      input: { workspaceId: "unregistered", workspaceRoot: "/client-controlled" },
    }),
    /registered Beale workspace/,
  );
});

test("exposes the in-Beale durable tool registry to Codex with enforced effect routing", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-codex-tool-bridge-"));
  temporaryDirectories.push(directory);
  const storage = {
    databasePath: join(directory, "memory.sqlite"),
    artifactDirectoryPath: join(directory, "artifacts"),
  };
  const database = new WorkspaceDatabase(storage.databasePath, storage.artifactDirectoryPath, {
    workspacePath: directory,
    workspaceId: "workspace-test",
  });
  database.initialize();
  database.close();
  const context = {
    workspaceId: "workspace-test",
    workspaceName: "Test workspace",
    workspaceRoot: directory,
    researchProfileId: "security-research",
    memoryBackend: "app-server",
    sessionId: "session-codex",
    modelAuthor: { provider: "openai-codex", model: "gpt-6-astra" },
    workspaceReferences: [{
      workspaceId: "workspace-prior-example",
      workspaceName: "Prior example",
    }],
  };

  const catalog = await listAppServerResearchTools(context, storage);
  const descriptors = new Map(catalog.tools.map((tool) => [tool.name, tool]));
  assert.equal(descriptors.get("history.search").sideEffects, "read");
  assert.deepEqual(descriptors.get("history.search").inputSchema.properties.workspaceId.enum, [
    "workspace-test",
    "workspace-prior-example",
  ]);
  assert.match(descriptors.get("history.search").inputSchema.properties.workspaceId.description, /Prior example/u);
  assert.deepEqual(descriptors.get("claim.get").inputSchema.properties.workspaceId.enum, [
    "workspace-test",
    "workspace-prior-example",
  ]);
  assert.deepEqual(descriptors.get("runbook.get").inputSchema.properties.workspaceId.enum, [
    "workspace-test",
    "workspace-prior-example",
  ]);
  assert.equal(descriptors.get("resource.catalog").sideEffects, "write");
  assert.equal(descriptors.get("runbook.run").sideEffects, "process");

  await assert.rejects(
    callAppServerResearchTool({
      ...context,
      toolName: "resource.catalog",
      toolInput: { operation: "discover", kind: "service", name: "Example service",
        locator: "service.example.test", rationale: "Potential example campaign component." },
    }, storage, "read"),
    /mutating research-tool operation/,
  );

  const created = await callAppServerResearchTool({
    ...context,
    toolName: "resource.catalog",
    toolInput: { operation: "discover", kind: "service", name: "Example service",
      locator: "service.example.test", rationale: "Potential example campaign component." },
  }, storage, "mutating");
  assert.equal(created.result.status, "complete", JSON.stringify(created.result));

  const listed = await callAppServerResearchTool({
    ...context,
    toolName: "resource.catalog",
    toolInput: { operation: "list" },
  }, storage, "mutating");
  assert.equal(listed.result.status, "complete");
  assert.equal(listed.result.output.resources[0].name, "Example service");

  const touch = await callAppServerResearchTool({ ...context, toolName: "resource.catalog",
    toolInput: { operation: "touch", resourceId: created.result.output.resource.id,
      purpose: "Inspect the example service." } }, storage, "mutating");
  assert.equal(touch.result.status, "blocked");
  assert.match(touch.result.summary, /recorded scope inclusion or rule/);
});

test("research Subject rebinding migrates workspace-owned research identity", () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-subject-rebind-"));
  temporaryDirectories.push(directory);
  const databasePath = join(directory, "memory.sqlite");
  const workspaceId = "workspace-example";
  const database = new WorkspaceDatabase(databasePath, join(directory, "artifacts"), {
    workspacePath: directory,
    workspaceId,
  });
  database.initialize();
  const previous = database.setResearchSubject({ name: "Example Legacy Subject" });
  const raw = new DatabaseSync(databasePath);
  raw.exec(`
    CREATE TABLE memory_nodes (
      id TEXT PRIMARY KEY,
      subject_id TEXT NOT NULL,
      subject_name TEXT NOT NULL
    );
    CREATE TABLE memory_node_workspaces (
      node_id TEXT NOT NULL,
      workspace_id TEXT NOT NULL,
      workspace_name TEXT NOT NULL,
      PRIMARY KEY(node_id, workspace_id)
    );
    CREATE TABLE synthetic_subject_records (
      id TEXT PRIMARY KEY,
      workspace_id TEXT NOT NULL,
      subject_id TEXT NOT NULL,
      subject_name TEXT NOT NULL
    );
  `);
  raw.prepare("INSERT INTO memory_nodes VALUES (?, ?, ?)").run("memory-example", previous.id, previous.name);
  raw.prepare("INSERT INTO memory_node_workspaces VALUES (?, ?, ?)").run("memory-example", workspaceId, "Example Workspace");
  raw.prepare("INSERT INTO synthetic_subject_records VALUES (?, ?, ?, ?)")
    .run("record-example", workspaceId, previous.id, previous.name);
  raw.close();

  const next = database.setResearchSubject({ id: "subject-example-shared", name: "Example Shared Subject" });
  assert.deepEqual({ id: next.id, name: next.name }, {
    id: "subject-example-shared",
    name: "Example Shared Subject",
  });
  database.close();

  const verified = new DatabaseSync(databasePath, { readOnly: true });
  assert.deepEqual({ ...verified.prepare("SELECT subject_id, subject_name FROM memory_nodes WHERE id = ?").get("memory-example") }, {
    subject_id: "subject-example-shared",
    subject_name: "Example Shared Subject",
  });
  assert.deepEqual({ ...verified.prepare("SELECT subject_id, subject_name FROM synthetic_subject_records WHERE id = ?").get("record-example") }, {
    subject_id: "subject-example-shared",
    subject_name: "Example Shared Subject",
  });
  verified.close();
});

test("owns research goal suggestion storage and provider routing at the app-server boundary", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-suggestions-"));
  temporaryDirectories.push(directory);
  const calls = [];
  const registry = hostRegistryFixture(directory, { memoryBackend: "disabled" });
  registry.providerSettings = () => ({
    defaultProviderId: "xai",
    modelDefaults: { xai: { smallModel: "grok-host", reasoningEffort: "low" } },
    authenticationPreferences: { xai: "api_key" },
    riskAcknowledgements: [],
  });
  const service = new AppServerHostService({
    registry,
    invokeProtocol: async (operation, options) => {
      calls.push({ operation, options });
      return { phase: "discovery", suggestions: ["Inspect the canonical boundary."] };
    },
  });

  const result = await service.executeOperation({
    operation: "suggestion.generate",
    input: {
      workspaceId: "workspace-test",
      phase: "discovery",
      databasePath: "client-controlled.sqlite",
      provider: { id: "client-provider" },
    },
  });

  assert.deepEqual(result, { phase: "discovery", suggestions: ["Inspect the canonical boundary."] });
  assert.equal(calls[0].operation, "suggestion.generate");
  assert.deepEqual(calls[0].options.input, {
    workspaceId: "workspace-test",
    phase: "discovery",
    databasePath: join(directory, "memory.sqlite"),
    provider: {
      id: "xai",
      smallModel: "grok-host",
      reasoningEffort: "low",
      authenticationPreferences: { xai: "api_key" },
    },
    workspaceRoot: directory,
    artifactDirectoryPath: join(directory, "artifacts"),
    researchProfileId: "security-research",
    memoryEnabled: false,
  });
});

test("routes the host Codex OAuth bridge into OpenAI suggestion generation", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-codex-suggestions-"));
  temporaryDirectories.push(directory);
  const codexAuthFile = join(directory, "codex-auth.json");
  writeFileSync(codexAuthFile, "{}", { mode: 0o600 });
  const previousAppServerPath = process.env.APP_SERVER_CODEX_AUTH_FILE;
  const previousBealePath = process.env.BEALE_OPENAI_CODEX_AUTH_FILE;
  delete process.env.APP_SERVER_CODEX_AUTH_FILE;
  process.env.BEALE_OPENAI_CODEX_AUTH_FILE = codexAuthFile;
  try {
    const calls = [];
    const registry = hostRegistryFixture(directory, { memoryBackend: "disabled" });
    registry.providerSettings = () => ({
      defaultProviderId: "openai-codex",
      modelDefaults: { "openai-codex": { smallModel: "gpt-small", reasoningEffort: "medium" } },
      authenticationPreferences: { "openai-codex": "subscription" },
      riskAcknowledgements: ["openai-codex"],
    });
    const service = new AppServerHostService({
      registry,
      invokeProtocol: async (operation, options) => {
        calls.push({ operation, options });
        return { phase: "discovery", suggestions: ["Inspect the canonical boundary."] };
      },
    });

    await service.executeOperation({
      operation: "suggestion.generate",
      input: { workspaceId: "workspace-test", phase: "discovery" },
    });

    assert.equal(calls[0].options.input.provider.codexAuthFile, codexAuthFile);
  } finally {
    if (previousAppServerPath === undefined) delete process.env.APP_SERVER_CODEX_AUTH_FILE;
    else process.env.APP_SERVER_CODEX_AUTH_FILE = previousAppServerPath;
    if (previousBealePath === undefined) delete process.env.BEALE_OPENAI_CODEX_AUTH_FILE;
    else process.env.BEALE_OPENAI_CODEX_AUTH_FILE = previousBealePath;
  }
});

test("expands research prompts with host-owned workspace storage and connected model policy", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-prompt-expansion-"));
  temporaryDirectories.push(directory);
  const calls = [];
  const registry = hostRegistryFixture(directory, { memoryBackend: "disabled" });
  registry.providerSettings = () => ({
    defaultProviderId: "xai",
    modelDefaults: { xai: { leadModel: "grok-4.6", reasoningEffort: "high" } },
    authenticationPreferences: { xai: "api_key" },
    riskAcknowledgements: [],
  });
  const service = new AppServerHostService({
    registry,
    invokeProtocol: async (operation, options) => {
      calls.push({ operation, options });
      return { phase: "discovery", promptMarkdown: "Expanded canonical request." };
    },
  });

  const result = await service.executeOperation({
    operation: "prompt.expand",
    input: {
      workspaceId: "workspace-test",
      promptMarkdown: "Inspect parsing.",
      workspaceRoot: "/client/path",
      databasePath: "client-controlled.sqlite",
      provider: { id: "xai", model: "grok-4.6" },
    },
  });

  assert.deepEqual(result, { phase: "discovery", promptMarkdown: "Expanded canonical request." });
  assert.equal(calls[0].operation, "prompt.expand");
  assert.deepEqual(calls[0].options.input, {
    workspaceId: "workspace-test",
    promptMarkdown: "Inspect parsing.",
    workspaceRoot: directory,
    databasePath: join(directory, "memory.sqlite"),
    artifactDirectoryPath: join(directory, "artifacts"),
    provider: {
      id: "xai",
      model: "grok-4.6",
      reasoningEffort: "high",
      authenticationPreferences: { xai: "api_key" },
    },
    researchProfileId: "security-research",
    memoryEnabled: false,
  });
});

test("rejects prompt expansion through a provider that is not connected", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-prompt-provider-"));
  temporaryDirectories.push(directory);
  const registry = hostRegistryFixture(directory, { memoryBackend: "disabled" });
  registry.providerSettings = () => ({
    defaultProviderId: "xai",
    modelDefaults: { xai: { leadModel: "grok-4.6" } },
    authenticationPreferences: { xai: "api_key" },
    riskAcknowledgements: [],
  });
  const service = new AppServerHostService({ registry, invokeProtocol: async () => ({}) });

  await assert.rejects(
    service.executeOperation({
      operation: "prompt.expand",
      input: {
        workspaceId: "workspace-test",
        promptMarkdown: "Inspect parsing.",
        provider: { id: "anthropic", model: "claude-opus-5" },
      },
    }),
    /not connected/,
  );
});

test("maps transient app-server failures to typed retryable HTTP errors", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-errors-"));
  temporaryDirectories.push(directory);
  const hostService = testHostService(directory);
  hostService.workspaceSessions = async () => {
    throw new Error("app-server protocol operation timed out after 30000ms.");
  };
  const server = await startAppServer({ hostService });
  servers.push(server);
  const response = await fetch(`${server.url}/v1/workspaces/workspace-test/sessions`, {
    headers: { authorization: `Bearer ${server.operatorToken}` },
  });

  assert.equal(response.status, 504);
  assert.deepEqual(await response.json(), {
    controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
    error: {
      code: "temporarily_unavailable",
      message: "app-server protocol operation timed out after 30000ms.",
      retryable: true,
    },
  });
});

test("runs app-server workers in Node mode when hosted by Electron", () => {
  const environment = appServerWorkerEnvironment({
    BEALE_TEST_CHILD_ENVIRONMENT: "preserved",
    ELECTRON_RUN_AS_NODE: "0",
  }, "41.3.0");

  assert.equal(environment.BEALE_TEST_CHILD_ENVIRONMENT, "preserved");
  assert.equal(environment.ELECTRON_RUN_AS_NODE, "1");
});

test("mediates runtime-worker SQLite operations through host-owned connections", () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-worker-database-"));
  temporaryDirectories.push(directory);
  const databasePath = join(directory, "memory.sqlite");
  const broker = new AppServerWorkerDatabaseBroker(databasePath);
  try {
    const opened = brokerRequest(broker, {
      operation: "open",
      databasePath,
    });
    assert.equal(opened.ok, true);
    assert.equal(typeof opened.value, "number");
    const connectionId = opened.value;

    assert.equal(brokerRequest(broker, {
      operation: "exec",
      connectionId,
      sql: "CREATE TABLE mediated_records (id TEXT PRIMARY KEY, value TEXT NOT NULL)",
    }).ok, true);
    assert.equal(brokerRequest(broker, {
      operation: "run",
      connectionId,
      sql: "INSERT INTO mediated_records (id, value) VALUES (?, ?)",
      parameters: ["record-1", "host-owned"],
    }).ok, true);
    assert.deepEqual(brokerRequest(broker, {
      operation: "get",
      connectionId,
      sql: "SELECT id, value FROM mediated_records WHERE id = ?",
      parameters: ["record-1"],
    }), {
      ok: true,
      value: { id: "record-1", value: "host-owned" },
    });

    const denied = brokerRequest(broker, {
      operation: "open",
      databasePath: join(directory, "other.sqlite"),
    });
    assert.equal(denied.ok, false);
    assert.match(denied.error.message, /outside its app-server-owned database/);
  } finally {
    broker.close();
  }

  const inspection = new DatabaseSync(databasePath, { readOnly: true });
  try {
    assert.equal(
      inspection.prepare("SELECT value FROM mediated_records WHERE id = ?").get("record-1").value,
      "host-owned",
    );
  } finally {
    inspection.close();
  }
});

test("retries oversized read-only database responses with a larger bounded buffer", () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-worker-database-growth-"));
  temporaryDirectories.push(directory);
  const databasePath = join(directory, "memory.sqlite");
  const broker = new AppServerWorkerDatabaseBroker(databasePath);
  const createDatabase = createWorkerResearchDatabaseFactory(
    (message) => broker.handle(message),
    {
      initialResponseBufferBytes: 1024,
      maximumResponseBufferBytes: 64 * 1024,
    },
  );
  const database = createDatabase(databasePath);
  const value = "x".repeat(16 * 1024);
  try {
    database.exec("CREATE TABLE large_records (id TEXT PRIMARY KEY, value TEXT NOT NULL)");
    database.prepare("INSERT INTO large_records (id, value) VALUES (?, ?)").run("record-1", value);

    assert.deepEqual(
      database.prepare("SELECT id, value FROM large_records WHERE id = ?").get("record-1"),
      { id: "record-1", value },
    );
  } finally {
    database.close();
    broker.close();
  }
});

test("serializes cross-worker transactions without blocking the app-server event loop", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-worker-database-lease-"));
  temporaryDirectories.push(directory);
  const databasePath = join(directory, "memory.sqlite");
  const coordinator = new AppServerWorkerDatabaseCoordinator();
  const firstBroker = new AppServerWorkerDatabaseBroker(databasePath, coordinator);
  const secondBroker = new AppServerWorkerDatabaseBroker(databasePath, coordinator);
  try {
    const firstConnectionId = brokerRequest(firstBroker, {
      operation: "open",
      databasePath,
    }).value;
    const secondConnectionId = brokerRequest(secondBroker, {
      operation: "open",
      databasePath,
    }).value;
    assert.equal(brokerRequest(firstBroker, {
      operation: "exec",
      connectionId: firstConnectionId,
      sql: "CREATE TABLE runbook_updates (id TEXT PRIMARY KEY, value TEXT NOT NULL)",
    }).ok, true);
    assert.equal(brokerRequest(firstBroker, {
      operation: "exec",
      connectionId: firstConnectionId,
      sql: "BEGIN IMMEDIATE",
    }).ok, true);

    const queuedInsert = brokerRequestMessage({
      operation: "run",
      connectionId: secondConnectionId,
      sql: "INSERT INTO runbook_updates (id, value) VALUES (?, ?)",
      parameters: ["second", "queued"],
    });
    secondBroker.handle(queuedInsert.message);
    assert.equal(Atomics.load(new Int32Array(queuedInsert.responseBuffer, 0, 2), 0), 0);

    let hostReadStarted = false;
    const hostRead = coordinator.runWhenAvailable(databasePath, () => {
      hostReadStarted = true;
      const database = new DatabaseSync(databasePath, { readOnly: true });
      try {
        return database.prepare("SELECT COUNT(*) AS count FROM runbook_updates").get().count;
      } finally {
        database.close();
      }
    });
    assert.equal(hostReadStarted, false);

    assert.equal(brokerRequest(firstBroker, {
      operation: "run",
      connectionId: firstConnectionId,
      sql: "INSERT INTO runbook_updates (id, value) VALUES (?, ?)",
      parameters: ["first", "transaction owner"],
    }).ok, true);
    assert.equal(brokerRequest(firstBroker, {
      operation: "exec",
      connectionId: firstConnectionId,
      sql: "COMMIT",
    }).ok, true);

    assert.equal(brokerResponse(queuedInsert.responseBuffer).ok, true);
    assert.equal(await hostRead, 2);
    assert.deepEqual(brokerRequest(secondBroker, {
      operation: "all",
      connectionId: secondConnectionId,
      sql: "SELECT id, value FROM runbook_updates ORDER BY id",
    }).value, [
      { id: "first", value: "transaction owner" },
      { id: "second", value: "queued" },
    ]);
  } finally {
    firstBroker.close();
    secondBroker.close();
  }
});

test("keeps SQLite construction out of the app-server runtime worker", () => {
  const workerSource = readFileSync(new URL("../dist/runtimeWorker.js", import.meta.url), "utf8");
  const clientSource = readFileSync(new URL("../dist/workerDatabaseClient.js", import.meta.url), "utf8");
  assert.match(workerSource, /installResearchDatabaseFactory/);
  assert.match(workerSource, /createWorkerResearchDatabaseFactory/);
  assert.doesNotMatch(workerSource, /node:sqlite|new DatabaseSync/);
  assert.doesNotMatch(clientSource, /node:sqlite|new DatabaseSync/);

  const researchSourceRoot = new URL("../../packages/research-agent/src/", import.meta.url);
  for (const file of [
    "topics.ts",
    "findings.ts",
    "goal-suggestions.ts",
    "knowledge-artifacts.ts",
    "memory-graph.ts",
    "memory-summary.ts",
    "reports.ts",
    "research-resources.ts",
    "runbooks.ts",
    "session-store.ts",
    "workspace-binding.ts",
  ]) {
    assert.doesNotMatch(readFileSync(new URL(file, researchSourceRoot), "utf8"), /new DatabaseSync/);
  }
});

test("serves host workspaces and canonical app-server reads from one authenticated control plane", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-control-"));
  temporaryDirectories.push(directory);
  const server = await startAppServer({ hostService: testHostService(directory) });
  servers.push(server);
  const headers = { authorization: `Bearer ${server.operatorToken}` };

  const workspaceResponse = await fetch(`${server.url}/v1/workspaces`, { headers });
  assert.equal(workspaceResponse.status, 200);
  assert.equal((await workspaceResponse.json()).workspaces[0].workspaceId, "workspace-test");

  const workspaceDetailsResponse = await fetch(`${server.url}/v1/workspaces/workspace-test/details`, { headers });
  assert.equal(workspaceDetailsResponse.status, 200);
  assert.equal((await fetch(`${server.url}/v1/workspaces/workspace-test/details`)).status, 401);
  const workspaceDetails = await workspaceDetailsResponse.json();
  assert.equal(workspaceDetails.schemaVersion, 1);
  assert.equal(workspaceDetails.workspace.workspaceId, "workspace-test");
  assert.equal(workspaceDetails.scope.name, "Test workspace");
  assert.equal(workspaceDetails.targets[0].name, "Local");

  const quickChatsResponse = await fetch(`${server.url}/v1/quick-chats`, { headers });
  assert.equal(quickChatsResponse.status, 200);
  assert.deepEqual(await quickChatsResponse.json(), { schemaVersion: 1, chats: [] });
  assert.equal((await fetch(`${server.url}/v1/quick-chats`)).status, 401);
  const invalidQuickChat = await fetch(`${server.url}/v1/quick-chats`, {
    method: "POST", headers: { ...headers, "content-type": "application/json" }, body: JSON.stringify({ promptMarkdown: "" })
  });
  assert.equal(invalidQuickChat.status, 400);

  const reads = [
    ["memory", "memory"],
    ["memory-notifications", "memory-notifications"],
    ["sessions", "sessions"],
    ["sessions/session-test/update", "update"],
    ["sessions/session-test/events?stream=trace&tail=true", "events"],
    ["sessions/session-test/collaboration", "collaboration"],
    ["sessions/session-test/captures", "captures"],
  ];
  for (const [path, kind] of reads) {
    const response = await fetch(`${server.url}/v1/workspaces/workspace-test/${path}`, { headers });
    assert.equal(response.status, 200);
    assert.equal((await response.json()).result.kind, kind);
  }

  const details = await fetch(
    `${server.url}/v1/workspaces/workspace-test/sessions/session-test/event-details`,
    {
      method: "POST",
      headers: { ...headers, "content-type": "application/json" },
      body: JSON.stringify({ eventIds: ["event-1"] }),
    },
  );
  assert.equal(details.status, 200);
  assert.equal((await details.json()).result.kind, "event-details");

  const retiredTopicEndpoint = await fetch(`${server.url}/v1/workspaces/workspace-test/topics`, { headers });
  assert.equal(retiredTopicEndpoint.status, 404);

});

test("starts a Quick Chat through the authenticated client-neutral endpoint", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-quick-chat-http-example-"));
  temporaryDirectories.push(directory);
  const upstream = await createFakeAppServerSessionHost();
  const hostService = testHostService(directory);
  hostService.quickChatLaunchRequest = (input) => ({
    launchVersion: APP_SERVER_SESSION_LAUNCH_VERSION,
    launch: { workspaceId: "quick-chat-workspace-example", mode: "quick-chat", promptMarkdown: input.promptMarkdown.trim(), machineId: "local" }
  });
  const server = await startAppServer({ hostService, spawnSession: upstream.spawnSession });
  servers.push(server);
  const response = await fetch(`${server.url}/v1/quick-chats`, {
    method: "POST",
    headers: { authorization: `Bearer ${server.operatorToken}`, "content-type": "application/json" },
    body: JSON.stringify({ promptMarkdown: "Explain example.test" })
  });
  assert.equal(response.status, 201);
  const result = await response.json();
  assert.equal(result.schemaVersion, 1);
  assert.equal(result.workspaceId, "quick-chat-workspace-example");
  assert.equal(typeof result.sessionId, "string");
  upstream.complete();
});

test("workspace details use the owning app-server state and configured targets", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-workspace-details-example-"));
  temporaryDirectories.push(directory);
  const calls = [];
  const host = new AppServerHostService({
    registry: hostRegistryFixture(directory),
    invokeProtocol: async (operation, options) => {
      calls.push({ operation, options });
      if (operation === "workspace.state") return {
        activeScope: { id: "scope-example", workspaceName: "ExampleCo Research", scopeOwner: "ExampleCo", descriptionMarkdown: "Research example.test.", assets: [{ id: "asset-example", kind: "domain", direction: "in_scope", value: "example.test", sensitivity: "public" }] },
        researchSubject: { name: "ExampleCo" },
        workspaceRules: [{ id: "rule-example", text: "Stay in scope." }],
      };
      if (operation === "memory.summary") return { status: "ready", nodeCount: 1,
        nodes: [{ id: "memory-example", type: "invariant", title: "Example memory", surfaceId: "surface-example" }],
        leads: [{ id: "claim-example", projection: "lead", classification: "security.vulnerability",
          title: "Example lead", surfaceId: "surface-example" }], findings: [],
        runbooks: [{ id: "runbook-example", title: "Example runbook", purpose: "Synthetic check", updatedAt: "2026-01-01T00:00:00.000Z" }], reports: [] };
      throw new Error(`Unexpected operation ${operation}`);
    },
  });
  host.fleet.state = async () => ({ workspaceTargets: { "workspace-test": ["local"] }, browsers: [], machines: [], sandboxes: [] });
  const details = await host.workspaceDetails("workspace-test");
  assert.equal(details.scope.resources[0].value, "example.test");
  assert.equal(details.rules[0].text, "Stay in scope.");
  assert.equal(details.runbooks[0].title, "Example runbook");
  assert.equal(details.memory.nodes[0].surfaceId, "surface-example");
  assert.equal(details.memory.leads[0].surfaceId, "surface-example");
  assert.deepEqual(details.targets, [{ id: "local", name: "Local", ready: true }]);
  assert.equal(calls[0].operation, "workspace.state");
  assert.equal(calls[0].options.input.workspacePath, directory);
  assert.equal(JSON.stringify(details).includes(directory), false);
});

test('Fleet state operations use workspace-scoped discovery when requested', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-fleet-state-example-'));
  temporaryDirectories.push(directory);
  const host = new AppServerHostService({ registry: hostRegistryFixture(directory) });
  const calls = [];
  const scoped = { workspaceTargets: { 'workspace-test': ['local'] } };
  const global = { workspaceTargets: {} };
  host.fleet.stateForWorkspace = async (workspaceId) => { calls.push(workspaceId); return scoped; };
  host.fleet.state = async () => global;
  assert.equal(await host.executeOperation({ operation: 'fleet.state', input: { workspaceId: 'workspace-test' } }), scoped);
  assert.equal(await host.executeOperation({ operation: 'fleet.state' }), global);
  assert.deepEqual(calls, ['workspace-test']);
});

test("resolves workspace identity and host policy from the shared Beale registry", () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-registry-"));
  temporaryDirectories.push(directory);
  const workspacePath = join(directory, "workspace");
  const database = new DatabaseSync(join(directory, "workspace-registry.sqlite"));
  try {
    database.exec(`
      CREATE TABLE registry_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at TEXT NOT NULL);
      CREATE TABLE workspaces (
        id TEXT PRIMARY KEY, workspace_path TEXT NOT NULL, workspace_id TEXT NOT NULL,
        workspace_name TEXT NOT NULL, research_profile_id TEXT NOT NULL,
        research_kit_id TEXT NOT NULL, workspace_directories_json TEXT NOT NULL,
        memory_backend TEXT NOT NULL,
        updated_at TEXT NOT NULL
      );
      CREATE TABLE research_sessions (
        id TEXT PRIMARY KEY, registry_workspace_id TEXT NOT NULL, updated_at TEXT NOT NULL
      );
    `);
    database.prepare(`INSERT INTO workspaces VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`).run(
      "registry-workspace-test",
      workspacePath,
      "workspace-test",
      "Test workspace",
      "security-research",
      "general",
      JSON.stringify([workspacePath, join(directory, "second-repository")]),
      "disabled",
      "2026-08-21T00:00:00.000Z",
    );
    database.prepare(`INSERT INTO workspaces VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`).run(
      "registry-quick-chats",
      join(directory, "internal-workspaces", "quick-chats"),
      "workspace-quick-chats",
      "Quick Chats",
      "security-research",
      "general",
      "[]",
      "app-server",
      "2026-08-21T00:02:00.000Z",
    );
    database.prepare(`INSERT INTO research_sessions VALUES (?, ?, ?)`).run(
      "session-test", "registry-workspace-test", "2026-08-21T00:01:00.000Z",
    );
    for (const [key, value] of [
      ["default_provider_id", "openai-codex"],
      ["provider_model_defaults_json", JSON.stringify({
        "openai-codex": { largeModel: "gpt-lead", smallModel: "gpt-small", reasoningEffort: "high" },
      })],
      ["provider_preferred_authentication_methods_json", JSON.stringify({ "openai-codex": "subscription" })],
      ["provider_context_sizes_json", JSON.stringify({ "openai-codex": "large" })],
      ["openai_trusted_access_cyber_risk_acknowledged", "1"],
    ]) {
      database.prepare(`INSERT INTO registry_meta VALUES (?, ?, ?)`).run(
        key, value, "2026-08-21T00:00:00.000Z",
      );
    }
  } finally {
    database.close();
  }

  const registry = new AppServerHostRegistry({ registryDirectory: directory });
  assert.equal(registry.listWorkspaces().length, 1);
  assert.equal(registry.listWorkspaces()[0].runCount, 1);
  assert.equal(registry.listHostWorkspaces()[0].workspacePath, workspacePath);
  assert.equal(registry.resolveWorkspace("workspace-quick-chats").name, "Quick Chats");
  assert.equal(registry.resolveWorkspace("workspace-test").workspacePath, workspacePath);
  assert.equal(registry.resolveWorkspace("workspace-test").memoryBackend, "disabled");
  assert.equal(registry.resolveWorkspace("registry-workspace-test").workspaceDirectories.length, 1);
  assert.deepEqual(registry.providerSettings(), {
    defaultProviderId: "openai-codex",
    customProviders: [],
    modelDefaults: {
      "openai-codex": { leadModel: "gpt-lead", smallModel: "gpt-small", reasoningEffort: "high" },
    },
    authenticationPreferences: { "openai-codex": "subscription" },
    contextSizes: { "openai-codex": "large" },
    daybreakMode: "blue",
    riskAcknowledgements: ["openai-codex"],
  });
});

test("lists older workspace registries that do not yet have session catalog metadata", () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-old-registry-"));
  temporaryDirectories.push(directory);
  const database = new DatabaseSync(join(directory, "workspace-registry.sqlite"));
  try {
    database.exec(`
      CREATE TABLE workspaces (
        id TEXT PRIMARY KEY, workspace_path TEXT NOT NULL, workspace_id TEXT NOT NULL,
        workspace_name TEXT NOT NULL, research_profile_id TEXT NOT NULL,
        research_kit_id TEXT NOT NULL, workspace_directories_json TEXT NOT NULL,
        updated_at TEXT NOT NULL
      );
    `);
    database.prepare(`INSERT INTO workspaces VALUES (?, ?, ?, ?, ?, ?, ?, ?)`).run(
      "registry-workspace-old",
      join(directory, "workspace"),
      "workspace-old",
      "Old workspace",
      "security-research",
      "general",
      "[]",
      "2026-08-21T00:00:00.000Z",
    );
  } finally {
    database.close();
  }

  const registry = new AppServerHostRegistry({ registryDirectory: directory });
  assert.deepEqual(registry.listWorkspaces()[0], {
    id: "registry-workspace-old",
    workspaceId: "workspace-old",
    name: "Old workspace",
    researchProfileId: "security-research",
    researchKitId: "general",
    runCount: 0,
    lastRunAt: null,
    updatedAt: "2026-08-21T00:00:00.000Z",
  });
});

test("hides legacy non-security workspaces without deleting their registry records", () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-hidden-workspace-"));
  temporaryDirectories.push(directory);
  const database = new DatabaseSync(join(directory, "workspace-registry.sqlite"));
  try {
    database.exec(`
      CREATE TABLE workspaces (
        id TEXT PRIMARY KEY, workspace_path TEXT NOT NULL, workspace_id TEXT NOT NULL,
        workspace_name TEXT NOT NULL, research_profile_id TEXT NOT NULL,
        research_kit_id TEXT NOT NULL, workspace_directories_json TEXT NOT NULL,
        updated_at TEXT NOT NULL
      );
    `);
    database.prepare(`INSERT INTO workspaces VALUES (?, ?, ?, ?, ?, ?, ?, ?)`).run(
      "registry-legacy-example", join(directory, "legacy-workspace"), "workspace-legacy-example",
      "Legacy example", "mathematics", "general", "[]", "2026-08-21T00:00:00.000Z",
    );
  } finally {
    database.close();
  }

  const registry = new AppServerHostRegistry({ registryDirectory: directory });
  assert.deepEqual(registry.listWorkspaces(), []);
  assert.equal(registry.resolveWorkspace("workspace-legacy-example"), null);
  const databaseAfter = new DatabaseSync(join(directory, "workspace-registry.sqlite"), { readOnly: true });
  try {
    assert.equal(databaseAfter.prepare("SELECT COUNT(*) AS count FROM workspaces").get().count, 1);
  } finally {
    databaseAfter.close();
  }
});

test("enforces workspace ownership for canonical session reads", async () => {
  const calls = [];
  const service = new AppServerHostService({
    registry: hostRegistryFixture(tmpdir()),
    invokeProtocol: async (operation) => {
      calls.push(operation);
      if (operation === "session.get_update") {
        return { session: { id: "session-other", workspaceId: "workspace-other", attempts: [] } };
      }
      return { unexpected: true };
    },
  });

  await assert.rejects(
    service.sessionCaptures("workspace-test", "session-other"),
    /does not belong to workspace workspace-test/,
  );
  assert.deepEqual(calls, ["session.get_update"]);
});

test("validates session update ownership without a redundant summary read", async () => {
  const calls = [];
  const service = new AppServerHostService({
    registry: hostRegistryFixture(tmpdir()),
    invokeProtocol: async (operation, options) => {
      calls.push({ operation, options });
      return {
        session: { id: "session-test", workspaceId: "workspace-test", attempts: [] },
        events: [],
      };
    },
  });

  const response = await service.sessionUpdate("workspace-test", "session-test", {
    tail: true,
    limit: 25,
    maxBytes: 4096,
  });

  assert.equal(response.result.session.id, "session-test");
  assert.equal(calls.length, 1);
  assert.equal(calls[0].operation, "session.get_update");
  assert.deepEqual(calls[0].options.args, [
    "session", "get-update", "--session-id", "session-test",
    "--tail", "--limit", "25", "--max-bytes", "4096",
  ]);
});

test("continues an inactive canonical session with its original prompt and transcript history", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-continuation-"));
  temporaryDirectories.push(directory);
  const calls = [];
  const originalPrompt = "Inspect the example parser without restarting the investigation.";
  const session = {
    id: "session-continue",
    workspaceId: "workspace-test",
    status: "completed",
    prompt: originalPrompt,
    attempts: [{ id: "attempt-original", parentAttemptId: null }],
    metadata: {
      appServerRestartLaunch: {
        schemaVersion: 1,
        eligible: true,
        launch: {
          workspaceId: "workspace-test",
          promptMarkdown: originalPrompt,
          provider: { id: "openai-codex" },
          shellSafetyMode: "auto_review",
          researchProfileId: "security-research",
        },
      },
    },
  };
  const service = new AppServerHostService({
    registry: hostRegistryFixture(directory),
    invokeProtocol: async (operation, options) => {
      calls.push({ operation, options });
      if (operation === "session.get_update") {
        return {
          session,
          events: [{
            payload: {
              record: {
                role: "assistant",
                contentMarkdown: "The parser shares mutable state across requests.",
                metadata: { agentPath: "/root" },
              },
            },
          }],
        };
      }
      if (operation === "provider.describe") {
        return { defaultSmallModels: {}, sessionTitleEffort: "medium", shellReviewEffort: "medium" };
      }
      if (operation === "plugin.runtime") {
        return { skillDirs: [], selectedSkillIds: [], allowedMcpServers: [] };
      }
      if (operation === "session.get") return session;
      if (["session.begin_attempt", "session.transition", "session.append_event"].includes(operation)) {
        return { revision: 2 };
      }
      throw new Error(`Unexpected operation: ${operation}`);
    },
  });

  const request = await service.createSessionContinuationRequest(
    "workspace-test",
    "session-continue",
    "Verify whether the race is exploitable.",
  );
  assert.equal(request.sessionId, "session-continue");
  assert.equal(request.launch.promptMarkdown, "Verify whether the race is exploitable.");
  assert.equal(request.launch.generateTitle, false);
  assert.equal(request.launch.continuation.resumeAttemptId, "attempt-original");
  assert.match(request.launch.continuation.fallbackPrompt, /Inspect the example parser/);
  assert.match(request.launch.continuation.fallbackPrompt, /shares mutable state/);
  assert.match(request.launch.continuation.fallbackPrompt, /Verify whether the race is exploitable/);

  await service.prepareSession(request, "unused-generated-session");
  await service.recordSessionContinuationInstruction(request);
  const transition = calls.find((call) => call.operation === "session.transition");
  assert.equal(Object.hasOwn(transition.options.input.configuration, "prompt"), false);
  const appended = calls.find((call) => call.operation === "session.append_event");
  assert.equal(appended.options.input.id, `session_continuation_${request.launch.attemptId}`);
  assert.equal(appended.options.input.payload.record.contentMarkdown, "Verify whether the race is exploitable.");
});

test("projects heat-bearing memory through the canonical notification feed", async () => {
  const calls = [];
  const service = new AppServerHostService({
    registry: hostRegistryFixture(tmpdir()),
    invokeProtocol: async (operation, options) => {
      calls.push({ operation, options });
      return {
        schemaVersion: 3,
        workspaceId: "workspace-test",
        profile: { id: "security-research", version: "1", hash: "a".repeat(64) },
        nodes: [],
      };
    },
  });

  const response = await service.workspaceMemoryNotifications("workspace-test", "session-test");

  assert.equal(response.result.schemaVersion, 3);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].operation, "memory.notification_feed");
  assert.deepEqual(calls[0].options.args, ["knowledge", "notification-feed"]);
  assert.deepEqual(calls[0].options.input, {
    workspaceId: "workspace-test",
    workspaceRoot: tmpdir(),
    researchProfileId: "security-research",
    sessionId: "session-test",
  });

  const disabledService = new AppServerHostService({
    registry: hostRegistryFixture(tmpdir(), { memoryBackend: "disabled" }),
    invokeProtocol: async () => ({
      schemaVersion: 3,
      workspaceId: "workspace-test",
      nodes: [{ id: "memory-hidden" }],
    }),
  });
  const disabledResponse = await disabledService.workspaceMemoryNotifications("workspace-test", "session-test");
  assert.deepEqual(disabledResponse.result.nodes, []);
});

test("projects workspace memory into a path-free mobile catalog", async () => {
  const service = new AppServerHostService({
    registry: hostRegistryFixture(tmpdir()),
    invokeProtocol: async (operation, options) => {
      assert.equal(operation, "memory.summary");
      assert.deepEqual(options.input, {
        workspaceId: "workspace-test",
        workspaceRoot: tmpdir(),
        researchProfileId: "security-research",
      });
      return {
        status: "ready",
        nodeCount: 1,
        nodeTypeCounts: { finding: 1, ignored: -1 },
        databasePath: "/private/memory.sqlite",
        nodes: [{
          id: "memory-1",
          sessionIds: ["session-1"],
          type: "finding",
          title: "Authentication boundary",
          summary: "A privileged boundary needs further verification.",
          bodyMarkdown: "private body",
          status: "candidate",
          confidence: 0.75,
          tags: ["auth"],
          evidence: [{ path: "/private/source.c" }],
          attrs: { workspacePath: "/private/workspace" },
          createdAt: "2026-08-22T00:01:00.000Z",
          updatedAt: "2026-08-22T00:02:00.000Z",
          revision: 2,
        }],
        findings: [{
          id: "claim-1", originSessionId: "session-1", projection: "finding", maturity: "verified",
          status: "verified", classification: "security.vulnerability",
          title: "Parser race", summary: "A shared parser crosses requests.",
          impact: "Integrity loss.", stepsToReproduce: "Run the example parser with two requests and inspect the second response.",
          rating: "high", confidence: 0.9, evidence: [],
          duplicateClaims: [{
            id: "claim-duplicate", projection: "lead", maturity: "observed", rating: "high",
            classification: "security.vulnerability", title: "Duplicate parser race", status: "observed",
            revision: 2, markedAt: "2026-08-22T00:03:00.000Z",
          }],
          securityTracking: {
            reachability: { state: "reachable", conditions: "Public listener.", assessedAt: "2026-08-22T00:03:00.000Z" },
            cvssAssessments: [{ version: "4.0", vector: "CVSS:4.0/AV:N", score: 8.7, nomenclature: "CVSS-B", assessedAt: "2026-08-22T00:03:00.000Z" }],
            affectedAssetIds: ["asset-parser"],
            affectedVersions: [{ assetId: "asset-parser", range: "1.x", fixedVersion: "2.0" }],
            externalReferences: [{ kind: "cwe", identifier: "CWE-362", url: "https://cwe.mitre.org/data/definitions/362.html" }],
            privateDecision: { actorId: "operator-private", rationale: "private" },
          },
          createdAt: "2026-08-22T00:01:00.000Z", updatedAt: "2026-08-22T00:03:00.000Z", revision: 3,
        }],
      };
    },
  });

  const response = await service.workspaceMemory("workspace-test");

  assert.deepEqual(response.result, {
    schemaVersion: 4,
    workspaceId: "workspace-test",
    status: "ready",
    nodeCount: 1,
    nodeTypeCounts: { finding: 1 },
    nodes: [{
      id: "memory-1",
      sessionIds: ["session-1"],
      type: "finding",
      title: "Authentication boundary",
      summary: "A privileged boundary needs further verification.",
      status: "candidate",
      confidence: 0.75,
      tags: ["auth"],
      createdAt: "2026-08-22T00:01:00.000Z",
      updatedAt: "2026-08-22T00:02:00.000Z",
      revision: 2,
      duplicateMemories: [],
    }],
    leads: [],
    findings: [{
      id: "claim-1",
      sessionIds: ["session-1"],
      projection: "finding",
      maturity: "verified",
      status: "verified",
      staleFromStatus: null,
      classification: "security.vulnerability",
      componentClaimIds: [],
      duplicateClaims: [{
        id: "claim-duplicate",
        projection: "lead",
        maturity: "observed",
        rating: "high",
        classification: "security.vulnerability",
        title: "Duplicate parser race",
        status: "observed",
        revision: 2,
        markedAt: "2026-08-22T00:03:00.000Z",
      }],
      title: "Parser race",
      summary: "A shared parser crosses requests.",
      impact: "Integrity loss.",
      stepsToReproduce: "Run the example parser with two requests and inspect the second response.",
      rating: "high",
      securityTracking: {
        reachability: { state: "reachable", conditions: "Public listener.", assessedAt: "2026-08-22T00:03:00.000Z" },
        cvssAssessments: [{ version: "4.0", vector: "CVSS:4.0/AV:N", score: 8.7, nomenclature: "CVSS-B", assessedAt: "2026-08-22T00:03:00.000Z" }],
        affectedAssetIds: ["asset-parser"],
        surfaceIds: [],
        progressBlocker: null,
        reportability: null,
        triage: { blocker: null, packet: null },
        externalReferences: [{ kind: "cwe", identifier: "CWE-362", url: "https://cwe.mitre.org/data/definitions/362.html" }],
      },
      confidence: 0.9,
      evidenceCount: 0,
      createdAt: "2026-08-22T00:01:00.000Z",
      updatedAt: "2026-08-22T00:03:00.000Z",
      revision: 3,
    }],
  });
  const serialized = JSON.stringify(response);
  assert.doesNotMatch(serialized, /databasePath|workspacePath|bodyMarkdown|attrs|privateDecision|operator-private|\/private\//u);
});

test("rejects a client profile that differs from the registered workspace profile", async () => {
  const service = new AppServerHostService({ registry: hostRegistryFixture(tmpdir()) });
  await assert.rejects(
    service.prepareSession({
      ...sessionLaunchRequest(tmpdir()),
      launch: {
        ...sessionLaunchRequest(tmpdir()).launch,
        researchProfileId: "different-profile",
      },
    }, "generated-session"),
    /uses research profile security-research, not different-profile/,
  );
});

test('app-server rejects session targets outside the workspace allowlist', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-target-allowlist-'));
  temporaryDirectories.push(directory);
  const service = new AppServerHostService({ registry: hostRegistryFixture(directory) });
  const request = sessionLaunchRequest(directory, { sessionId: 'session-target-allowlist' });
  await assert.rejects(service.prepareSession({ ...request, launch: {
    ...request.launch, machineId: 'vm:run:tart:example-vm',
  } }, 'session-target-allowlist'), /not enabled for the workspace/);
});

test('app-server prepares and releases every VM in one selected-target session', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-multi-target-session-'));
  temporaryDirectories.push(directory);
  const service = new AppServerHostService({
    registry: hostRegistryFixture(directory),
    invokeProtocol: async (operation) => {
      if (operation === 'provider.describe') return { defaultSmallModels: {}, sessionTitleEffort: 'medium', shellReviewEffort: 'medium' };
      if (operation === 'plugin.runtime') return { skillDirs: [], selectedSkillIds: [], allowedMcpServers: [] };
      if (operation === 'session.get') throw new Error('Session not found: session-multi-target');
      if (operation === 'session.create') return { revision: 1 };
      throw new Error(`Unexpected operation: ${operation}`);
    },
  });
  const vmTargets = ['vm:run:tart:example-one', 'vm:run:tart:example-two'];
  service.fleet.stateForWorkspace = async () => ({ enabled: true,
    workspaceTargets: { 'workspace-test': ['local', ...vmTargets] }, browsers: [], machines: [] });
  const preparedIds = [];
  const releasedIds = [];
  service.fleet.prepareSessionTarget = async (targetId) => {
    preparedIds.push(targetId);
    return { machineId: targetId, sshArgs: ['example@192.0.2.10'], guestPlatform: 'linux', guestCwd: '/home/example' };
  };
  service.fleet.releaseSessionTarget = async (machineId) => { releasedIds.push(machineId); };
  const request = sessionLaunchRequest(directory, { sessionId: 'session-multi-target' });
  request.launch.machineId = 'local';
  request.launch.targetIds = ['local', ...vmTargets];
  const prepared = await service.prepareSession(request, 'unused-generated-session');
  assert.deepEqual(preparedIds, vmTargets);
  assert.deepEqual(prepared.launch.selectedTargets.ids, ['local', ...vmTargets]);
  assert.deepEqual(Object.keys(prepared.launch.selectedTargets.remoteShells), vmTargets);
  assert.deepEqual(JSON.parse(appServerSessionEnvironment(prepared.launch, {}).BEALE_FLEET_SELECTED_TARGETS).ids,
    ['local', ...vmTargets]);
  await service.releaseSessionTarget('session-multi-target');
  assert.deepEqual(releasedIds, vmTargets);
  preparedIds.length = 0;
  releasedIds.length = 0;
  service.fleet.prepareSessionTarget = async (targetId) => {
    if (targetId === vmTargets[1]) throw new Error('Synthetic VM preparation failure.');
    preparedIds.push(targetId);
    return { machineId: targetId, sshArgs: ['example@192.0.2.10'], guestPlatform: 'linux', guestCwd: '/home/example' };
  };
  await assert.rejects(service.prepareSession({ ...request, sessionId: 'session-multi-target-failed' },
    'unused-generated-session'), /Synthetic VM preparation failure/u);
  assert.deepEqual(preparedIds, [vmTargets[0]]);
  assert.deepEqual(releasedIds, [vmTargets[0]]);
});

test('app-server launches a session on an enabled sandbox target and releases it', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-sandbox-target-session-'));
  temporaryDirectories.push(directory);
  const service = new AppServerHostService({
    registry: hostRegistryFixture(directory),
    invokeProtocol: async (operation) => {
      if (operation === 'provider.describe') return { defaultSmallModels: {}, sessionTitleEffort: 'medium', shellReviewEffort: 'medium' };
      if (operation === 'plugin.runtime') return { skillDirs: [], selectedSkillIds: [], allowedMcpServers: [] };
      if (operation === 'session.get') throw new Error('Session not found');
      if (operation === 'session.create') return { revision: 1 };
      throw new Error(`Unexpected operation: ${operation}`);
    },
  });
  const sandbox = (enabled) => ({ provider: 'local', name: 'Docker', enabled, credentialsConfigured: true });
  service.fleet.stateForWorkspace = async () => ({ enabled: true,
    workspaceTargets: { 'workspace-test': ['local', 'sandbox:local'] }, browsers: [], machines: [], sandboxes: [sandbox(true)] });
  const released = [];
  service.fleet.prepareSessionTarget = async (targetId) => ({ machineId: `${targetId}:sandbox-example`,
    sandbox: { provider: 'local', sandboxId: 'sandbox-example' }, guestPlatform: 'linux', guestCwd: '/workspace' });
  service.fleet.releaseSessionTarget = async (machineId) => { released.push(machineId); };
  const request = sessionLaunchRequest(directory, { sessionId: 'session-sandbox-target' });
  request.launch.machineId = 'sandbox:local';
  request.launch.targetIds = ['sandbox:local'];
  const prepared = await service.prepareSession(request, 'unused-generated-session');
  assert.deepEqual(JSON.parse(appServerSessionEnvironment(prepared.launch, {}).BEALE_FLEET_REMOTE_SHELL),
    { sandbox: { provider: 'local', sandboxId: 'sandbox-example' }, guestPlatform: 'linux', guestCwd: '/workspace' });
  await service.releaseSessionTarget('session-sandbox-target');
  assert.deepEqual(released, ['sandbox:local:sandbox-example']);

  service.fleet.stateForWorkspace = async () => ({ enabled: true,
    workspaceTargets: { 'workspace-test': ['local', 'sandbox:local'] }, browsers: [], machines: [], sandboxes: [sandbox(false)] });
  await assert.rejects(service.prepareSession({ ...request, sessionId: 'session-sandbox-disabled' }, 'unused-generated-session'),
    /sandbox provider is unavailable/u);
});

test("research track launch retains collaboration settings and enabled browser tools", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-track-launch-"));
  temporaryDirectories.push(directory);
  const trackStore = new ResearchTrackStore({ databasePath: join(directory, "memory.sqlite"), workspaceId: "workspace-test" });
  const surface = { id: "surface-example", parentId: null, name: "Example service",
    coverageState: { covered: false, unmetRequirements: ["surface_map"] }, progressBlocker: null };
  trackStore.start({ scopeId: "scope-example", provider: { id: "openai-codex" }, targetIds: ["local"], surfaces: [surface] });
  trackStore.reserveNext("track-example", [surface]);
  trackStore.close();
  const calls = [];
  let canonicalSession = null;
  const service = new AppServerHostService({
    registry: hostRegistryFixture(directory),
    invokeProtocol: async (operation, options) => {
      calls.push({ operation, options });
      if (operation === "provider.describe") return { defaultSmallModels: { 'openai-codex': 'example-small-model' }, sessionTitleEffort: "medium", shellReviewEffort: "low" };
      if (operation === "plugin.runtime") return {
        managedPluginIds: MANAGED_TOOL_PLUGIN_IDS, skillDirs: [], selectedSkillIds: [], allowedMcpServers: []
      };
      if (operation === "session.get") {
        if (!canonicalSession) throw new Error("Session not found: track-example");
        return canonicalSession;
      }
      if (operation === "session.create") {
        canonicalSession = { ...options.input, revision: 1 };
        return canonicalSession;
      }
      if (operation === "session.append_event") return { revision: 2 };
      throw new Error(`Unexpected operation: ${operation}`);
    },
  });
  const collaboration = {
    mode: "adaptive", subagentMode: "advanced", intensity: "normal",
    providers: [{ provider: "openai-codex", model: "example-model", enabled: true }]
  };
  const request = sessionLaunchRequest(directory, { sessionId: "track-example" });
  request.launch.mode = "research-track";
  request.launch.collaboration = collaboration;
  request.launch.shellSafetyMode = "auto_review";
  const prepared = await service.prepareSession(request, "unused-generated-session");
  const createCall = calls.find((call) => call.operation === "session.create");

  assert.deepEqual(createCall.options.input.metadata.bealeRun.budget.collaboration, collaboration);
  assert.equal(createCall.options.input.title, 'Inspect the component and choose a');
  assert.equal(prepared.launch.provider.title.model, 'example-small-model');
  assert.equal(prepared.launch.provider.title.style, 'research-track');
  const launchArgs = appServerSessionArgs(prepared.launch, {});
  assert.equal(launchArgs[launchArgs.indexOf('--title-style') + 1], 'research-track');
  assert.ok(launchArgs.includes('--track-epoch'));
  const titleCall = calls.find((call) => call.operation === "session.append_event");
  assert.equal(titleCall, undefined);
  assert.deepEqual(createCall.options.input.metadata.appServerRestartLaunch.launch.collaboration, collaboration);
  assert.deepEqual(JSON.parse(readFileSync(prepared.launch.collaborationConfigPath, "utf8")), collaboration);
  assert.ok(prepared.launch.pluginRuntime.managedPluginIds.includes("beale-browser"));
  assert.ok(appServerSessionArgs(prepared.launch, {}).includes("--managed-plugins"));
  assert.ok(appServerSessionArgs(prepared.launch, {}).includes(MANAGED_TOOL_PLUGIN_IDS.join(",")));
});

test("app-server preserves launch options through restart metadata and runtime arguments", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-fast-mode-"));
  temporaryDirectories.push(directory);
  const calls = [];
  const registry = hostRegistryFixture(directory);
  const providerSettings = registry.providerSettings();
  registry.providerSettings = () => ({
    ...providerSettings,
    contextSizes: { "openai-codex": "large" },
    daybreakMode: "red",
  });
  const service = new AppServerHostService({
    registry,
    invokeProtocol: async (operation, options) => {
      calls.push({ operation, options });
      if (operation === "provider.describe") {
        return {
          defaultSmallModels: { "openai-codex": "gpt-5.6-luna" },
          sessionTitleEffort: "medium",
          shellReviewEffort: "medium",
        };
      }
      if (operation === "plugin.runtime") {
        return { skillDirs: [], selectedSkillIds: [], allowedMcpServers: [] };
      }
      if (operation === "session.get") throw new Error("Session not found: session-fast-mode");
      if (operation === "session.create") return { revision: 1 };
      throw new Error(`Unexpected operation: ${operation}`);
    },
  });
  const request = sessionLaunchRequest(directory, { sessionId: "session-fast-mode" });
  request.launch.provider = {
    id: "openai-codex",
    model: "gpt-5.6-sol",
    reasoningEffort: "high",
    fastMode: true,
    daybreakBlue: false,
  };
  request.launch.devHarnessFeedback = true;
  request.launch.guardianEnabled = false;

  const prepared = await service.prepareSession(request, "generated-session");

  assert.equal(prepared.launch.provider.fastMode, true);
  assert.equal(prepared.launch.provider.daybreakBlue, undefined);
  assert.equal(prepared.launch.provider.daybreakMode, "red");
  assert.equal(prepared.launch.provider.contextSize, "large");
  assert.equal(appServerSessionEnvironment(prepared.launch, {}).APP_SERVER_OPENAI_CONTEXT_SIZE, "large");
  assert.ok(appServerSessionArgs(prepared.launch, {}).includes("--fast-mode"));
  assert.ok(appServerSessionArgs(prepared.launch, {}).includes("--daybreak-mode"));
  assert.ok(appServerSessionArgs(prepared.launch, {}).includes("red"));
  assert.ok(appServerSessionArgs(prepared.launch, {}).includes("--dev-harness-feedback"));
  assert.ok(appServerSessionArgs(prepared.launch, {}).includes("--no-guardian"));
  const createCall = calls.find((call) => call.operation === "session.create");
  assert.equal(
    createCall.options.input.metadata.appServerRestartLaunch.launch.provider.fastMode,
    true,
  );
  assert.equal(createCall.options.input.metadata.appServerRestartLaunch.launch.guardianEnabled, false);
  assert.equal(
    createCall.options.input.metadata.appServerRestartLaunch.launch.provider.daybreakBlue,
    undefined,
  );
  assert.equal(createCall.options.input.metadata.appServerRestartLaunch.launch.devHarnessFeedback, true);

  await assert.rejects(
    service.prepareSession({
      ...request,
      sessionId: "session-invalid-fast-mode",
      launch: {
        ...request.launch,
        provider: { id: "xai", model: "grok-4.6", fastMode: true },
      },
    }, "generated-invalid-session"),
    /Fast mode is available only when OpenAI is the Lead provider/,
  );
  await assert.rejects(
    service.prepareSession({
      ...request,
      sessionId: "session-invalid-daybreak",
      launch: {
        ...request.launch,
        provider: { id: "xai", model: "grok-4.7", daybreakBlue: true },
      },
    }, "generated-invalid-session"),
    /Daybreak Blue is available only when OpenAI is the Lead provider/,
  );
});

test("session launch exposes same-Subject workspace references without retained research rows", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-subject-reference-"));
  const priorDirectory = join(directory, "prior-workspace");
  const unrelatedDirectory = join(directory, "unrelated-workspace");
  temporaryDirectories.push(directory);
  mkdirSync(priorDirectory);
  mkdirSync(unrelatedDirectory);
  const databasePath = join(directory, "memory.sqlite");
  const database = new DatabaseSync(databasePath);
  try {
    database.exec(`
      CREATE TABLE workspaces (id TEXT PRIMARY KEY, workspace_path TEXT NOT NULL);
      CREATE TABLE workspace_research_subjects (
        workspace_id TEXT PRIMARY KEY, subject_id TEXT NOT NULL, display_name TEXT NOT NULL
      );
      CREATE TABLE scope_versions (
        workspace_id TEXT NOT NULL, status TEXT NOT NULL, workspace_name TEXT NOT NULL
      );
    `);
    assert.equal(database.prepare("SELECT COUNT(*) AS count FROM sqlite_master WHERE name='memory_nodes'").get().count, 0,
      "reference discovery must not depend on retained derived research tables");
    const insertWorkspace = database.prepare("INSERT INTO workspaces VALUES (?, ?)");
    const insertSubject = database.prepare("INSERT INTO workspace_research_subjects VALUES (?, ?, ?)");
    const insertScope = database.prepare("INSERT INTO scope_versions VALUES (?, 'active', ?)");
    for (const [workspaceId, workspacePath, subjectId] of [
      ["workspace-test", directory, "subject-example"],
      ["workspace-prior", priorDirectory, "subject-example"],
      ["workspace-unrelated", unrelatedDirectory, "subject-other"],
    ]) {
      insertWorkspace.run(workspaceId, workspacePath);
      insertSubject.run(workspaceId, subjectId, subjectId === "subject-example" ? "Example Subject" : "Other Subject");
      insertScope.run(workspaceId, workspaceId === "workspace-prior" ? "Canonical prior workspace" : "Canonical current workspace");
    }
  } finally {
    database.close();
  }
  const hostWorkspace = (workspaceId, name, workspacePath) => ({
    id: `registry-${workspaceId}`,
    workspaceId,
    name,
    researchProfileId: "security-research",
    researchKitId: "general",
    runCount: 0,
    lastRunAt: null,
    updatedAt: "2026-08-21T00:00:00.000Z",
    workspacePath,
    workspaceDirectories: [workspacePath],
    memoryBackend: "app-server",
  });
  const registry = hostRegistryFixture(directory, {
    hostWorkspaces: [
      hostWorkspace("workspace-test", "Current", directory),
      hostWorkspace("workspace-prior", "Registered prior label", priorDirectory),
      hostWorkspace("workspace-unrelated", "Unrelated", unrelatedDirectory),
    ],
  });
  const researchToolCalls = [];
  const service = new AppServerHostService({
    registry,
    invokeProtocol: async (operation, options) => {
      if (operation === "provider.describe") return {
        defaultSmallModels: { "openai-codex": "gpt-5.6-luna" },
        sessionTitleEffort: "medium",
        shellReviewEffort: "medium",
      };
      if (operation === "plugin.runtime") return { skillDirs: [], selectedSkillIds: [], allowedMcpServers: [] };
      if (operation === "session.get") throw new Error("Session not found: session-subject-reference");
      if (operation === "session.create") return { revision: 1 };
      if (operation === "research.tools.list") {
        researchToolCalls.push(options);
        return { tools: [] };
      }
      throw new Error(`Unexpected operation: ${operation}`);
    },
  });

  const prepared = await service.prepareSession(
    sessionLaunchRequest(directory, { sessionId: "session-subject-reference" }),
    "generated-session",
  );
  assert.deepEqual(prepared.launch.workspaceReferences, [{
    workspaceId: "workspace-prior",
    workspaceName: "Registered prior label",
    workspaceRoot: priorDirectory,
    subjectId: "subject-example",
  }]);
  assert.equal(appServerSessionArgs(prepared.launch, {}).filter((argument) => argument === "--workspace-reference").length, 1);

  await service.executeOperation({
    operation: "research.tools.list",
    input: { workspaceId: "workspace-test" },
  });
  assert.deepEqual(researchToolCalls[0].input.workspaceReferences, [{
    workspaceId: "workspace-prior",
    workspaceName: "Registered prior label",
  }]);
});

test("app-server owns built-in plugins and pins canonical session profile identity", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-host-policy-"));
  temporaryDirectories.push(directory);
  const metaSkillDirectory = fileURLToPath(new URL('../../managed-plugins/meta-skills/skills', import.meta.url));
  const pluginCatalogPath = join(directory, "plugin-catalog.json");
  writeFileSync(pluginCatalogPath, JSON.stringify([
    { id: "meta-skills-builtin", name: "meta-skills", skills: [{ id: "meta-bug-bounty-tools", name: "Meta tools", useWhen: "Meta kit research", path: join(metaSkillDirectory, "meta-bug-bounty-tools", "SKILL.md") }], mcpServers: [] },
    { id: "beale-introspection-builtin", name: "beale-introspection", skills: [], mcpServers: ["beale-introspection.beale"] },
    { id: "example-plugin", name: "example-plugin", skills: [], mcpServers: [] },
  ]));
  const calls = [];
  let managedPluginIds = [...MANAGED_TOOL_PLUGIN_IDS];
  let pluginReadFails = false;
  const registry = hostRegistryFixture(directory, { memoryBackend: "disabled" });
  registry.providerSettings = () => ({
    defaultProviderId: "xai",
    modelDefaults: {
      xai: { leadModel: "grok-4.6", smallModel: "grok-4.3", reasoningEffort: "high" },
    },
    authenticationPreferences: { xai: "api_key" },
    riskAcknowledgements: ["openai-codex", "anthropic", "xai"],
  });
  const service = new AppServerHostService({
    registry,
    invokeProtocol: async (operation, options) => {
      calls.push({ operation, options });
      if (operation === "provider.describe") {
        return {
          defaultSmallModels: { "openai-codex": "gpt-small", xai: "grok-4.3" },
          sessionTitleEffort: "medium",
          shellReviewEffort: "medium",
        };
      }
      if (operation === "plugin.runtime") {
        if (pluginReadFails) throw new Error("Synthetic plugin settings read failure.");
        return {
          managedPluginIds,
          pluginCatalogPath,
          skillDirs: [metaSkillDirectory],
          selectedSkillIds: ["meta-bug-bounty-tools"],
          allowedMcpServers: ["beale-introspection.beale", "example.tools"],
        };
      }
      if (operation === "plugin.list") return { plugins: [{ id: "meta-skills-builtin", installed: false }] };
      if (operation === "plugin.set_enabled") return { enabled: true };
      if (operation === "session.get") throw new Error("Session not found: session-policy");
      if (operation === "session.create") return { revision: 1 };
      throw new Error(`Unexpected operation: ${operation}`);
    },
  });
  const hash = "a".repeat(64);

  const prepared = await service.prepareSession({
    ...sessionLaunchRequest(directory),
    sessionId: "session-policy",
    launch: {
      ...sessionLaunchRequest(directory).launch,
      provider: {},
      generateTitle: true,
      researchProfileId: "security-research",
      researchProfileHash: hash,
      repeatSchedule: { type: "hourly", interval: 2 },
    },
  }, "generated-session");

  assert.equal(prepared.launch.researchProfileId, "security-research");
  assert.equal(prepared.launch.researchProfileHash, hash);
  assert.equal(prepared.launch.memoryBackend, "disabled");
  assert.deepEqual(prepared.launch.pluginRuntime.allowedMcpServers, ["example.tools"]);
  assert.deepEqual(prepared.launch.pluginRuntime.skillDirectories, []);
  assert.deepEqual(prepared.launch.pluginRuntime.selectedSkillIds, []);
  assert.deepEqual(JSON.parse(readFileSync(prepared.launch.pluginRuntime.pluginCatalogPath, "utf8")).map((plugin) => plugin.id), ["example-plugin"]);
  assert.deepEqual(prepared.launch.provider, {
    id: "xai",
    model: "grok-4.6",
    reasoningEffort: "high",
    contextSize: "default",
    riskAcknowledgements: ["openai-codex", "anthropic", "xai"],
    authenticationPreferences: { xai: "api_key" },
    title: { model: "grok-4.3", effort: "medium" },
    shellReview: {
      models: { "openai-codex": "gpt-small", xai: "grok-4.3" },
      effort: "medium",
    },
  });
  const pluginCall = calls.find((call) => call.operation === "plugin.runtime");
  assert.equal(pluginCall.options.input.workspaceId, "workspace-test");
  assert.equal(pluginCall.options.input.researchKitId, "general");
  assert.ok(pluginCall.options.input.builtinPlugins.length >= MANAGED_TOOL_PLUGIN_IDS.length + 5);
  assert.equal(pluginCall.options.input.builtinPlugins.some((plugin) => plugin.id === "beale-browser-use-builtin"), false);
  assert.equal(pluginCall.options.input.builtinPlugins.some((plugin) => plugin.id === "beale-terminator-builtin"), false);
  assert.ok(pluginCall.options.input.builtinPlugins.some((plugin) =>
    plugin.id === "beale-goals-builtin" && plugin.path.endsWith(join("harness-features", "beale-goals"))
  ));
  assert.ok(pluginCall.options.input.builtinPlugins.some((plugin) =>
    plugin.id === "meta-skills-builtin" && plugin.enabledByDefault === false
      && plugin.path.endsWith(join("managed-plugins", "meta-skills"))
  ));
  assert.ok(pluginCall.options.input.builtinPlugins.some((plugin) =>
    plugin.id === "microsoft-security-devices-builtin" && plugin.enabledByDefault === false
      && plugin.path.endsWith(join("managed-plugins", "microsoft-security-devices"))
  ));
  assert.deepEqual(prepared.launch.pluginRuntime.managedPluginIds, MANAGED_TOOL_PLUGIN_IDS);
  assert.ok(appServerSessionArgs(prepared.launch, {}).includes(MANAGED_TOOL_PLUGIN_IDS.join(',')));
  assert.ok(pluginCall.options.input.builtinPlugins.every((plugin) =>
    (plugin.path.includes(join("app-server", "resources", "harness-features"))
      || plugin.path.includes(join("managed-plugins"))) && existsSync(plugin.path)
  ));
  const createCall = calls.find((call) => call.operation === "session.create");
  assert.deepEqual(createCall.options.input.profile, { id: "security-research", hash });
  assert.deepEqual(createCall.options.input.metadata.bealeRun.budget.repeatSchedule, { type: "hourly", interval: 2 });
  assert.deepEqual(createCall.options.input.metadata.bealeRun.budget.targetIds, ["local"]);
  assert.deepEqual(createCall.options.input.metadata.appServerRestartLaunch, {
    schemaVersion: 1,
    eligible: true,
    launch: {
      workspaceId: "workspace-test",
      promptMarkdown: "Test the typed app-server launch contract.",
      provider: { id: "xai", model: "grok-4.6", reasoningEffort: "high" },
      shellSafetyMode: "manual_approval",
      guardianEnabled: true,
      researchProfileId: "security-research",
      researchProfileHash: hash,
    },
  });
  managedPluginIds = [];
  registry.resolveWorkspace("workspace-test").researchKitId = "meta-bug-bounty";
  const next = await service.prepareSession({ ...sessionLaunchRequest(directory), sessionId: "session-plugin-disabled" }, "generated-disabled-session");
  assert.ok(calls.some((call) => call.operation === "plugin.set_enabled"
    && call.options.input.pluginId === "meta-skills-builtin"));
  assert.deepEqual(next.launch.pluginRuntime.managedPluginIds, []);
  assert.deepEqual(next.launch.pluginRuntime.skillDirectories, [metaSkillDirectory]);
  assert.deepEqual(next.launch.pluginRuntime.selectedSkillIds, ["meta-bug-bounty-tools"]);
  assert.notEqual(next.launch.pluginRuntime.pluginCatalogPath, pluginCatalogPath);
  assert.deepEqual(JSON.parse(readFileSync(next.launch.pluginRuntime.pluginCatalogPath, "utf8")).map((plugin) => plugin.id), ["meta-skills-builtin", "example-plugin"]);
  const nextArgs = appServerSessionArgs(next.launch, {});
  assert.equal(nextArgs[nextArgs.indexOf('--managed-plugins') + 1], 'none');
  pluginReadFails = true;
  await assert.rejects(service.prepareSession({ ...sessionLaunchRequest(directory), sessionId: "session-plugin-unavailable" }, "generated-unavailable-session"), /Managed plugin settings could not be loaded/);
});

test("app-server snapshots edited feature prompts into the enabled session catalog", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-feature-catalog-example-"));
  temporaryDirectories.push(directory);
  const sourceCatalogPath = join(directory, "plugin-catalog.json");
  writeFileSync(sourceCatalogPath, JSON.stringify([
    { id: "beale-claims", name: "Claims", description: "Original claim description.", skills: [], mcpServers: [] },
    { id: "example-plugin", name: "Example Plugin", description: "External plugin.", skills: [], mcpServers: [] },
  ]));
  const registry = hostRegistryFixture(directory);
  registry.featurePromptOverrides = () => ({ "beale-claims": {
    description: "Synthetic claim guidance.", promptExtension: "Use the synthetic claim workflow."
  } });
  const service = new AppServerHostService({ registry, invokeProtocol: async (operation) => {
    if (operation === "provider.describe") return { defaultSmallModels: {}, sessionTitleEffort: "medium", shellReviewEffort: "low" };
    if (operation === "plugin.runtime") return { managedPluginIds: ["beale-claims"], pluginCatalogPath: sourceCatalogPath,
      skillDirs: [], selectedSkillIds: [], allowedMcpServers: [] };
    if (operation === "session.get") throw new Error("Session not found: session-feature-prompt");
    if (operation === "session.create") return { revision: 1 };
    throw new Error(`Unexpected operation: ${operation}`);
  } });
  const prepared = await service.prepareSession(sessionLaunchRequest(directory, { sessionId: "session-feature-prompt" }), "Example session");
  assert.notEqual(prepared.launch.pluginRuntime.pluginCatalogPath, sourceCatalogPath);
  const snapshot = JSON.parse(readFileSync(prepared.launch.pluginRuntime.pluginCatalogPath, "utf8"));
  assert.deepEqual(snapshot[0], { id: "beale-claims", name: "Claims", description: "Synthetic claim guidance.",
    promptExtension: "Use the synthetic claim workflow.", skills: [], mcpServers: [] });
  assert.equal(snapshot[1].promptExtension, undefined);
  assert.equal(JSON.parse(readFileSync(sourceCatalogPath, "utf8"))[0].description, "Original claim description.");
});

test("prepares automatic recovery as a child attempt and pauses an interrupted active attempt", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-recovery-policy-"));
  temporaryDirectories.push(directory);
  const calls = [];
  let sessionGetCount = 0;
  const service = new AppServerHostService({
    registry: hostRegistryFixture(directory),
    invokeProtocol: async (operation, options) => {
      calls.push({ operation, options });
      if (operation === "provider.describe") {
        return { defaultSmallModels: {}, sessionTitleEffort: "medium", shellReviewEffort: "medium" };
      }
      if (operation === "plugin.runtime") {
        return { skillDirs: [], selectedSkillIds: [], allowedMcpServers: [] };
      }
      if (operation === "session.get") {
        sessionGetCount += 1;
        return {
          id: "session-recovery-policy",
          workspaceId: "workspace-test",
          status: sessionGetCount === 1 ? "active" : "paused",
          attempts: [{ id: "attempt-initial", status: sessionGetCount === 1 ? "active" : "paused" }],
        };
      }
      if (operation === "session.transition" || operation === "session.begin_attempt") return { revision: 2 };
      throw new Error(`Unexpected operation: ${operation}`);
    },
  });

  const request = sessionLaunchRequest(directory, {
    sessionId: "session-recovery-policy",
    promptMarkdown: "Continue the parser investigation.",
  });
  request.launch.attemptId = "attempt-initial";
  const prepared = await service.prepareSessionRecovery({
    request,
    sessionId: "session-recovery-policy",
    previousAttemptId: "attempt-initial",
    previousAttemptWasInitial: true,
    fallbackPrompt: "Recover from durable state.",
  });

  assert.notEqual(prepared.attemptId, "attempt-initial");
  assert.equal(prepared.launch.resumeCapturePath, join(
    directory,
    ".beale",
    "app-server-runs",
    "session-recovery-policy.capture.json",
  ));
  const transitionCall = calls.find((call) => call.operation === "session.transition");
  assert.equal(transitionCall.options.input.status, "paused");
  assert.equal(transitionCall.options.input.attemptId, "attempt-initial");
  const beginCall = calls.find((call) => call.operation === "session.begin_attempt");
  assert.equal(beginCall.options.input.parentAttemptId, "attempt-initial");
  assert.match(beginCall.options.input.summary, /Continuing/u);
});

test("startup recovery continues only interrupted sessions with a sanitized restart launch", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-startup-recovery-policy-"));
  temporaryDirectories.push(directory);
  const calls = [];
  let sessionGetCount = 0;
  const restartLaunch = {
    schemaVersion: 1,
    eligible: true,
    launch: {
      workspaceId: "workspace-test",
      promptMarkdown: "Resume the interrupted parser investigation.",
      provider: { id: "openai-codex", model: "gpt-5.6-sol", reasoningEffort: "high" },
      shellSafetyMode: "auto_review",
      researchProfileId: "security-research",
    },
  };
  const registry = hostRegistryFixture(directory);
  registry.listWorkspaces = () => [{
    id: "registry-workspace-test",
    workspaceId: "workspace-test",
    name: "Test workspace",
    researchProfileId: "security-research",
    researchKitId: "general",
    runCount: 1,
    lastRunAt: null,
    updatedAt: "2026-08-28T00:00:00.000Z",
    workspacePath: directory,
    workspaceDirectories: [directory],
    memoryBackend: "app-server",
  }];
  const service = new AppServerHostService({
    registry,
    invokeProtocol: async (operation, options) => {
      calls.push({ operation, options });
      if (operation === "session.recover_interrupted") {
        return {
          interruptedSessions: 1,
          interruptedAttempts: 1,
          sessionIds: ["session-startup-recovery"],
        };
      }
      if (operation === "session.get") {
        sessionGetCount += 1;
        return {
          id: "session-startup-recovery",
          workspaceId: "workspace-test",
          status: "paused",
          metadata: { appServerRestartLaunch: restartLaunch },
          attempts: [{
            id: "attempt-interrupted",
            parentAttemptId: null,
            status: "paused",
            metadata: { interruptedByRecovery: true },
          }],
        };
      }
      if (operation === "provider.describe") {
        return { defaultSmallModels: {}, sessionTitleEffort: "medium", shellReviewEffort: "medium" };
      }
      if (operation === "plugin.runtime") {
        return { skillDirs: [], selectedSkillIds: [], allowedMcpServers: [] };
      }
      if (operation === "session.begin_attempt" || operation === "session.transition") return { revision: 3 };
      throw new Error(`Unexpected operation: ${operation}`);
    },
  });

  const recovery = await service.recoverInterruptedSessions();

  assert.equal(recovery.interruptedSessions, 1);
  assert.equal(recovery.recovered.length, 1);
  assert.equal(recovery.skippedSessions, 0);
  assert.equal(recovery.errors.length, 0);
  assert.equal(recovery.recovered[0].request.launch.introspection, undefined);
  assert.equal(recovery.recovered[0].prepared.launch.resumeCapturePath, join(
    directory,
    ".beale",
    "app-server-runs",
    "session-startup-recovery.capture.json",
  ));
  const beginCall = calls.find((call) => call.operation === "session.begin_attempt");
  assert.equal(beginCall.options.input.parentAttemptId, "attempt-interrupted");
  assert.ok(sessionGetCount >= 2);
});

test("discovers overdue automations from canonical session metadata and rebuilds their launch", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-automation-due-"));
  temporaryDirectories.push(directory);
  const registry = hostRegistryFixture(directory);
  registry.listWorkspaces = () => [registry.resolveWorkspace("workspace-test")];
  const service = new AppServerHostService({
    registry,
    invokeProtocol: async (operation) => {
      assert.equal(operation, "session.list_summaries");
      return [{
        id: "session-automation",
        workspaceId: "workspace-test",
        status: "completed",
        prompt: "Recheck the parser boundary.",
        provider: "openai-codex",
        model: "gpt-5.6-sol",
        reasoningEffort: "high",
        workflowId: "discovery",
        profile: { id: "security-research", hash: "profile-hash" },
        metadata: {
          bealeRun: {
            shellSafetyMode: "auto_review",
            budget: {
              repeatSchedule: { type: "hourly", interval: 2 },
              targetIds: ["local", "browser:example"],
              fastMode: true,
              goalEnabled: true,
              goalObjective: "Keep the parser audit moving.",
              collaboration: { mode: "always", providers: [] },
            },
          },
        },
        attempts: [{ startedAt: "2026-08-28T16:00:00.000Z" }],
        createdAt: "2026-08-28T16:00:00.000Z",
      }];
    },
  });

  const due = await service.dueAutomations(new Date("2026-08-28T18:00:00.000Z"));

  assert.equal(due.length, 1);
  assert.equal(due[0].dueAt, "2026-08-28T18:00:00.000Z");
  assert.deepEqual(due[0].request, {
    launchVersion: 3,
    sessionId: "session-automation",
    launch: {
      workspaceId: "workspace-test",
      machineId: "local",
      targetIds: ["local", "browser:example"],
      browserRelay: true,
      promptMarkdown: "Recheck the parser boundary.",
      goal: { objective: "Keep the parser audit moving." },
      provider: { id: "openai-codex", model: "gpt-5.6-sol", reasoningEffort: "high", fastMode: true, daybreakBlue: true },
      shellSafetyMode: "auto_review",
      workflowId: "discovery",
      researchProfileId: "security-research",
      researchProfileHash: "profile-hash",
      collaboration: { mode: "always", providers: [] },
      generateTitle: false,
    },
  });
  assert.equal(
    nextAutomationRunAt({ type: "monthly", interval: 1 }, "2026-08-15T12:00:00.000Z")?.toISOString(),
    "2026-09-15T12:00:00.000Z",
  );
});

test('classifies an explicit track pause before stopping its active session', () => {
  const sessionId = 'session-example-track';
  assert.equal(researchTrackStopCause({ status: 'running', activeSessionId: sessionId, lastError: null }, sessionId), null);
  assert.equal(researchTrackStopCause({ status: 'paused', activeSessionId: sessionId, lastError: null }, sessionId), 'track_control');
  assert.equal(researchTrackStopCause({ status: 'paused', activeSessionId: sessionId,
    lastError: 'Synthetic scope change.' }, sessionId), 'track_state');
  assert.equal(researchTrackStopCause(null, sessionId), 'track_state');
});

test('automatically launches a reserved research-track epoch while the app-server is resident', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-track-scheduler-example-'));
  temporaryDirectories.push(directory);
  const upstream = await createFakeAppServerSessionHost();
  const hostService = testHostService(directory);
  let scanned = false;
  hostService.dueResearchTracks = async () => {
    if (scanned) return [];
    scanned = true;
    const request = sessionLaunchRequest(directory, { sessionId: 'session-track-example' });
    request.launch.mode = 'research-track';
    request.launch.targetIds = ['local'];
    request.launch.machineId = 'local';
    request.launch.shellSafetyMode = 'auto_review';
    request.launch.guardianEnabled = true;
    return [{ request }];
  };
  hostService.workspaceResearchTrack = async () => ({ status: 'running', activeSessionId: 'session-track-example' });
  const server = await startAppServer({ hostService, spawnSession: upstream.spawnSession });
  servers.push(server);
  await waitFor(() => server.listSessions().some((session) => session.sessionId === 'session-track-example'));
  assert.equal(server.listSessions().find((session) => session.sessionId === 'session-track-example')?.state, 'running');
  const attachedResponse = await fetch(`${server.url}/v1/sessions/session-track-example/attachments`, {
    method: 'POST', headers: { authorization: `Bearer ${server.operatorToken}` }
  });
  assert.equal(attachedResponse.status, 201);
  const attached = await attachedResponse.json();
  const messages = [];
  const socket = await connect(webSocketUrl(server.url, attached.transport.path), attached.transport.token, messages);
  socket.send(JSON.stringify(clientHello('session-track-example', 'beale-desktop')));
  upstream.sendEvent({ schemaVersion: 1, kind: 'session.title', timestamp: new Date().toISOString(),
    payload: { status: 'generated', title: 'Parser Boundary Verification', provider: 'openai-codex', model: 'example-small-model' } });
  await waitFor(() => messages.some((message) => message.type === 'session.event'
    && message.event.kind === 'session.title'
    && message.event.payload.title === 'Parser Boundary Verification'));
  upstream.complete();
  await waitForSocketClose(socket);
});

test('pauses a track when its terminal session state cannot be persisted', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-track-finalization-example-'));
  temporaryDirectories.push(directory);
  const upstream = await createFakeAppServerSessionHost();
  const hostService = testHostService(directory);
  const failures = [];
  hostService.recordSessionTerminalState = async () => { throw new Error('Synthetic terminal persistence failure.'); };
  hostService.recordResearchTrackEpochLaunchFailure = (workspaceId, sessionId, reason) => {
    failures.push({ workspaceId, sessionId, reason });
  };
  const server = await startAppServer({ hostService, spawnSession: upstream.spawnSession, longSessionRecovery: false });
  servers.push(server);
  const request = sessionLaunchRequest(directory, { sessionId: 'session-track-finalization-example' });
  request.launch.mode = 'research-track';
  await server.startSession(request);
  upstream.complete();
  await waitFor(() => failures.length === 1);
  assert.equal(failures[0].workspaceId, request.launch.workspaceId);
  assert.equal(failures[0].sessionId, request.sessionId);
  assert.match(failures[0].reason, /Could not persist completed session state/);
});

test('a hanging terminal write stops a track from showing indefinite progress', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-track-terminal-timeout-example-'));
  temporaryDirectories.push(directory);
  const upstream = await createFakeAppServerSessionHost();
  const hostService = testHostService(directory);
  const failures = [];
  hostService.recordSessionTerminalState = async () => new Promise(() => undefined);
  hostService.recordResearchTrackEpochLaunchFailure = (workspaceId, sessionId, reason) => {
    failures.push({ workspaceId, sessionId, reason });
  };
  const server = await startAppServer({ hostService, spawnSession: upstream.spawnSession,
    longSessionRecovery: false, sessionTerminalPersistenceTimeoutMs: 25 });
  servers.push(server);
  const request = sessionLaunchRequest(directory, { sessionId: 'session-track-terminal-timeout-example' });
  request.launch.mode = 'research-track';
  await server.startSession(request);
  upstream.complete();
  await waitFor(() => failures.length === 1);
  assert.match(failures[0].reason, /Terminal session persistence timed out/);
});

test('a completed worker that retains an open handle is terminated after the exit grace period', async () => {
  let terminateCalls = 0;
  const worker = { terminate: async () => { terminateCalls += 1; return 1; } };
  scheduleCompletedWorkerExit(worker, 10);
  await waitFor(() => terminateCalls === 1);
  const cancel = scheduleCompletedWorkerExit(worker, 10);
  cancel();
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(terminateCalls, 1);
});

test("automatically launches due automations while the app-server is resident", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-automation-loop-"));
  temporaryDirectories.push(directory);
  const upstream = await createFakeAppServerSessionHost();
  const hostService = testHostService(directory);
  const prepareSession = hostService.prepareSession.bind(hostService);
  let automationRequest;
  hostService.prepareSession = async (request, generatedSessionId) => {
    if ((request.sessionId ?? generatedSessionId) === "session-automation-loop") automationRequest = request;
    return await prepareSession(request, generatedSessionId);
  };
  let scanned = false;
  hostService.dueAutomations = async () => {
    if (scanned) return [];
    scanned = true;
    return [{ request: sessionLaunchRequest(directory, { sessionId: "session-automation-loop" }) }];
  };
  const server = await startAppServer({
    hostService,
    spawnSession: upstream.spawnSession,
    automationScheduler: { scanIntervalMs: 10 },
  });
  servers.push(server);

  await waitFor(() => server.listSessions().some((session) => session.sessionId === "session-automation-loop"));
  assert.equal(server.listSessions().find((session) => session.sessionId === "session-automation-loop")?.state, "running");
  assert.equal(automationRequest.launch.introspection.runtimeMode, "standard");
  assert.equal(automationRequest.launch.introspection.url, `${server.url}/v1/introspection`);

  const listResponse = await fetch(`${automationRequest.launch.introspection.url}/tool`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${automationRequest.launch.introspection.token}`,
      "content-type": "application/json",
    },
    body: JSON.stringify({ tool: "list_sessions", args: {} }),
  });
  assert.equal(listResponse.status, 200);
  assert.deepEqual(await listResponse.json(), { ok: true, result: { sessions: [] } });

  const launchResponse = await fetch(`${automationRequest.launch.introspection.url}/tool`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${automationRequest.launch.introspection.token}`,
      "content-type": "application/json",
    },
    body: JSON.stringify({
      tool: "launch_session",
      args: { promptMarkdown: "Investigate the next parser vulnerability." },
    }),
  });
  assert.equal(launchResponse.status, 200);
  const launched = await launchResponse.json();
  assert.equal(launched.ok, true);
  assert.ok(server.listSessions().some((session) => session.sessionId === launched.result.runId));

  const stopResponse = await fetch(`${automationRequest.launch.introspection.url}/tool`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${automationRequest.launch.introspection.token}`,
      "content-type": "application/json",
    },
    body: JSON.stringify({ tool: "stop_session", args: { runId: launched.result.runId } }),
  });
  assert.equal(stopResponse.status, 200);
  assert.deepEqual(await stopResponse.json(), {
    ok: true,
    result: { runId: launched.result.runId, stopped: true },
  });
  upstream.complete();
});

test("startup recovery leaves manually paused and stopped sessions untouched", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-manual-lifecycle-"));
  temporaryDirectories.push(directory);
  const calls = [];
  const states = new Map([
    ["session-paused", "active"],
    ["session-stopped", "active"],
  ]);
  const registry = hostRegistryFixture(directory);
  registry.listWorkspaces = () => [{
    id: "registry-workspace-test",
    workspaceId: "workspace-test",
    name: "Test workspace",
    researchProfileId: "security-research",
    researchKitId: "general",
    runCount: 2,
    lastRunAt: null,
    updatedAt: "2026-08-28T00:00:00.000Z",
    workspacePath: directory,
    workspaceDirectories: [directory],
    memoryBackend: "app-server",
  }];
  const service = new AppServerHostService({
    registry,
    invokeProtocol: async (operation, options) => {
      calls.push({ operation, options });
      if (operation === "session.recover_interrupted") {
        const sessionIds = [...states].flatMap(([sessionId, state]) => state === "active" ? [sessionId] : []);
        return {
          interruptedSessions: sessionIds.length,
          interruptedAttempts: sessionIds.length,
          sessionIds,
        };
      }
      if (operation === "session.transition") {
        const sessionId = options.args[options.args.indexOf("--session-id") + 1];
        states.set(sessionId, options.input.status);
        return { revision: 2 };
      }
      throw new Error(`Unexpected operation: ${operation}`);
    },
  });
  const pausedRequest = sessionLaunchRequest(directory, { sessionId: "session-paused" });
  const stoppedRequest = sessionLaunchRequest(directory, { sessionId: "session-stopped" });

  await service.recordSessionControlState({
    request: pausedRequest,
    sessionId: "session-paused",
    attemptId: "attempt-paused",
    state: "paused",
  });
  await service.recordSessionControlState({
    request: stoppedRequest,
    sessionId: "session-stopped",
    attemptId: "attempt-stopped",
    state: "stopped",
  });
  const recovery = await service.recoverInterruptedSessions();

  assert.equal(recovery.interruptedSessions, 0);
  assert.equal(recovery.recovered.length, 0);
  const transitions = calls.filter((call) => call.operation === "session.transition");
  assert.deepEqual(transitions.map((call) => call.options.input.status), ["paused", "stopped"]);
});

test("quick-chat launches receive an isolated authenticated Beale introspection runtime", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-quick-chat-"));
  temporaryDirectories.push(directory);
  const calls = [];
  const service = new AppServerHostService({
    registry: hostRegistryFixture(directory),
    invokeProtocol: async (operation, options) => {
      calls.push({ operation, options });
      if (operation === "provider.describe") {
        return { defaultSmallModels: {}, sessionTitleEffort: "medium", shellReviewEffort: "medium" };
      }
      if (operation === "plugin.runtime") {
        if (options.input.registryDirectory === directory) return { managedPluginIds: [], allowedMcpServers: ["beale-introspection.beale"] };
        return {
          skillDirs: [],
          selectedSkillIds: [],
          mcpConfigPath: join(directory, "quick-chat-mcp.json"),
          allowedMcpServers: ["beale-introspection.beale"],
        };
      }
      if (operation === "session.get") throw new Error("Session not found: session-quick-chat");
      if (operation === "session.create") return { revision: 1 };
      throw new Error(`Unexpected operation: ${operation}`);
    },
  });

  const request = sessionLaunchRequest(directory, { sessionId: "session-quick-chat" });
  request.launch.introspection = {
    url: "http://127.0.0.1:42123",
    token: "quick-chat-token",
  };
  const prepared = await service.prepareSession(request, "session-quick-chat");
  const pluginCall = calls.find((call) => call.operation === "plugin.runtime" && call.options.input.registryDirectory !== directory);

  assert.equal(prepared.launch.pluginRuntime.allowedMcpServers[0], "beale-introspection.beale");
  assert.deepEqual(prepared.launch.pluginRuntime.managedPluginIds, []);
  assert.deepEqual(prepared.launch.introspection, {
    url: "http://127.0.0.1:42123",
    token: "quick-chat-token",
  });
  assert.equal(pluginCall.options.input.registryDirectory, join(directory, "quick-chat-plugin-runtime"));
  assert.equal(pluginCall.options.input.runtimeEnvironment, undefined);
  assert.equal(pluginCall.options.input.builtinPlugins[0].path.endsWith(join("harness-features", "beale-introspection")), true);
  assert.equal(existsSync(join(pluginCall.options.input.builtinPlugins[0].path, "server.mjs")), true);
});

test("quick-chat respects disabled Introspection feature setting", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-quick-chat-disabled-"));
  temporaryDirectories.push(directory);
  const service = new AppServerHostService({
    registry: hostRegistryFixture(directory),
    invokeProtocol: async (operation) => {
      if (operation === "provider.describe") return { defaultSmallModels: {}, sessionTitleEffort: "medium", shellReviewEffort: "medium" };
      if (operation === "plugin.runtime") return { managedPluginIds: [], allowedMcpServers: [] };
      throw new Error(`Unexpected operation: ${operation}`);
    },
  });
  const request = sessionLaunchRequest(directory, { sessionId: "session-quick-chat-disabled" });
  request.launch.introspection = { url: "http://127.0.0.1:42123", token: "example-token" };
  await assert.rejects(service.prepareSession(request, "session-quick-chat-disabled"), /Enable Introspection in Agent Settings > Optional Features/);
});

test("workspace launches retain the standard plugin runtime while authenticating introspection", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-workspace-introspection-"));
  temporaryDirectories.push(directory);
  const calls = [];
  const service = new AppServerHostService({
    registry: hostRegistryFixture(directory),
    invokeProtocol: async (operation, options) => {
      calls.push({ operation, options });
      if (operation === "provider.describe") {
        return { defaultSmallModels: {}, sessionTitleEffort: "medium", shellReviewEffort: "medium" };
      }
      if (operation === "plugin.runtime") {
        return {
          skillDirs: [join(directory, "standard-skill")],
          selectedSkillIds: ["standard-skill"],
          mcpConfigPath: join(directory, "standard-mcp.json"),
          allowedMcpServers: ["beale-introspection.beale", "example.tools"],
        };
      }
      if (operation === "session.get") throw new Error("Session not found: session-workspace-introspection");
      if (operation === "session.create") return { revision: 1 };
      throw new Error(`Unexpected operation: ${operation}`);
    },
  });

  const request = sessionLaunchRequest(directory, { sessionId: "session-workspace-introspection" });
  request.launch.introspection = {
    url: "http://127.0.0.1:42124",
    token: "workspace-introspection-token",
    runtimeMode: "standard",
  };
  const prepared = await service.prepareSession(request, "session-workspace-introspection");
  const pluginCall = calls.find((call) => call.operation === "plugin.runtime");

  assert.equal(pluginCall.options.input.registryDirectory, directory);
  for (const name of ["beale-introspection"]) {
    const plugin = pluginCall.options.input.builtinPlugins.find((candidate) => candidate.id === `${name}-builtin`);
    assert.equal(plugin?.path.endsWith(join("harness-features", name)), true);
    assert.equal(existsSync(join(plugin.path, "server.mjs")), true);
  }
  assert.deepEqual(prepared.launch.pluginRuntime.allowedMcpServers, [
    "beale-introspection.beale",
    "example.tools",
  ]);
  assert.deepEqual(prepared.launch.introspection, {
    url: "http://127.0.0.1:42124",
    token: "workspace-introspection-token",
  });
});

test("session requests validate their input before spawning app-server", async () => {
  const server = await startAppServer();
  servers.push(server);
  const auth = { authorization: `Bearer ${server.operatorToken}`, "content-type": "application/json" };

  const legacyArgs = await fetch(`${server.url}/v1/sessions`, {
    method: "POST",
    headers: auth,
    body: JSON.stringify({ sessionId: "session-validate", args: [42] }),
  });
  assert.equal(legacyArgs.status, 400);

  const invalidVersion = await fetch(`${server.url}/v1/sessions`, {
    method: "POST",
    headers: auth,
    body: JSON.stringify({ ...sessionLaunchRequest(tmpdir()), launchVersion: 1 }),
  });
  assert.equal(invalidVersion.status, 400);

  const invalidProvider = await fetch(`${server.url}/v1/sessions`, {
    method: "POST",
    headers: auth,
    body: JSON.stringify({
      ...sessionLaunchRequest(tmpdir()),
      launch: { ...sessionLaunchRequest(tmpdir()).launch, provider: [] },
    }),
  });
  assert.equal(invalidProvider.status, 400);

  const invalidIntrospectionMode = await fetch(`${server.url}/v1/sessions`, {
    method: "POST",
    headers: auth,
    body: JSON.stringify({
      ...sessionLaunchRequest(tmpdir()),
      launch: {
        ...sessionLaunchRequest(tmpdir()).launch,
        introspection: {
          url: "http://127.0.0.1:42126",
          token: "test-token",
          runtimeMode: "shared",
        },
      },
    }),
  });
  assert.equal(invalidIntrospectionMode.status, 400);

  const malformed = await fetch(`${server.url}/v1/sessions`, {
    method: "POST",
    headers: auth,
    body: "{not json",
  });
  assert.equal(malformed.status, 400);

  const missing = await fetch(`${server.url}/v1/sessions/session-missing`, {
    method: "DELETE",
    headers: { authorization: `Bearer ${server.operatorToken}` },
  });
  assert.equal(missing.status, 404);
});

test("authenticated shutdown requests are delegated to the process host", async () => {
  let requested = false;
  const server = await startAppServer({
    onShutdownRequested: () => { requested = true; },
  });
  servers.push(server);
  const response = await fetch(`${server.url}/v1/server/shutdown`, {
    method: "POST",
    headers: { authorization: `Bearer ${server.operatorToken}` },
  });
  assert.equal(response.status, 202);
  assert.deepEqual(await response.json(), {
    controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
    shuttingDown: true,
  });
  await waitFor(() => requested);
});

test("control-plane shutdown cannot interrupt an active research session", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-shutdown-guard-"));
  temporaryDirectories.push(directory);
  const upstream = await createFakeAppServerSessionHost();
  servers.push(upstream);
  let requested = false;
  const server = await startAppServer({
    hostService: testHostService(directory),
    spawnSession: upstream.spawnSession,
    onShutdownRequested: () => { requested = true; },
  });
  servers.push(server);
  const auth = {
    authorization: `Bearer ${server.operatorToken}`,
    "content-type": "application/json",
  };
  const startResponse = await fetch(`${server.url}/v1/sessions`, {
    method: "POST",
    headers: auth,
    body: JSON.stringify(sessionLaunchRequest(directory, { sessionId: "session-active-shutdown" })),
  });
  assert.equal(startResponse.status, 201);

  const refused = await fetch(`${server.url}/v1/server/shutdown`, {
    method: "POST",
    headers: auth,
  });
  assert.equal(refused.status, 409);
  assert.deepEqual(await refused.json(), {
    controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
    error: {
      code: "sessions_active",
      message: "The Beale app-server cannot restart while 1 research session is active.",
      retryable: true,
    },
  });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(requested, false);

  upstream.complete();
  await waitForCondition(async () => {
    const response = await fetch(`${server.url}/v1/sessions/session-active-shutdown`, {
      headers: { authorization: `Bearer ${server.operatorToken}` },
    });
    return response.ok && (await response.json()).session.state === "completed";
  });
  const accepted = await fetch(`${server.url}/v1/server/shutdown`, {
    method: "POST",
    headers: auth,
  });
  assert.equal(accepted.status, 202);
  await waitFor(() => requested);
});

test("expands typed session intent into app-server-owned runtime policy", () => {
  const request = sessionLaunchRequest("C:\\workspace", {
    sessionId: "session-compose",
    researchProfile: {
      id: "security-research",
      hash: "a".repeat(64),
      workflowId: "discovery",
    },
  });
  const launch = resolvedSessionLaunch("C:\\workspace", {
    capturePath: "C:\\workspace\\capture.json",
    workspaceContextPath: "C:\\workspace\\workspace-context.json",
    researchProfileHash: request.launch.researchProfileHash,
    workflowId: request.launch.workflowId,
  });
  const args = appServerSessionArgs({
    ...launch,
    workspaceReferences: [{
      workspaceId: "workspace-prior-example",
      workspaceName: "Prior example",
      workspaceRoot: "C:\\prior-workspace",
      subjectId: "subject-example",
    }],
    provider: { ...launch.provider, fastMode: true },
  }, {
    BEALE_APP_SERVER_PROFILE_TOOL_FAMILY_CEILING_JSON: JSON.stringify(["repository-search", "file-read"]),
    BEALE_APP_SERVER_PROFILE_SIDE_EFFECT_CEILING_JSON: JSON.stringify(["none", "read"]),
    BEALE_APP_SERVER_TOOL_MAX_BYTES: "123456",
  });

  assert.equal("args" in request, false);
  assert.equal("env" in request, false);
  assert.deepEqual(args.slice(0, 6), [
    "--workspace-root", "C:\\workspace",
    "--capture", "C:\\workspace\\capture.json",
    "--executor", "agent",
  ]);
  assert.equal(args[args.indexOf("--workspace-context") + 1], "C:\\workspace\\workspace-context.json");
  assert.deepEqual(JSON.parse(args[args.indexOf("--workspace-reference") + 1]), {
    workspaceId: "workspace-prior-example",
    workspaceName: "Prior example",
    workspaceRoot: "C:\\prior-workspace",
    subjectId: "subject-example",
  });
  assert.equal(args[args.indexOf("--attempt-id") + 1], "attempt-test");
  assert.equal(args.includes("--investigation-id"), false);
  assert.equal(args[args.indexOf("--memory-backend") + 1], "app-server");
  assert.ok(args.includes("--no-default-tool-config"));
  assert.ok(args.includes("--fast-mode"));
  assert.ok(appServerSessionArgs({ ...launch, guardianEnabled: false }, {}).includes("--no-guardian"));
  assert.ok(args.includes("--profile-tool-family-ceiling"));
  assert.ok(args.includes("--profile-side-effect-ceiling"));
  assert.equal(args.includes("--disable-tool-family"), false);
  assert.equal(args.includes("--dev-harness-feedback"), false);
  assert.equal(args.includes("--track-epoch"), false);
  assert.equal(args[args.indexOf("--tool-max-bytes") + 1], "123456");
  assert.throws(() => appServerSessionArgs(launch, {
    BEALE_APP_SERVER_PROFILE_TOOL_FAMILY_CEILING_JSON: JSON.stringify(["unknown-family"]),
  }), /unsupported capability: unknown-family/);

  const introspectionEnvironment = appServerSessionEnvironment({
    ...launch,
    introspection: {
      url: "http://127.0.0.1:42125",
      token: "session-introspection-token",
    },
  }, {});
  assert.equal(introspectionEnvironment.BEALE_INTROSPECTION_URL, "http://127.0.0.1:42125");
  assert.equal(introspectionEnvironment.BEALE_INTROSPECTION_TOKEN, "session-introspection-token");
  assert.equal(introspectionEnvironment.APP_SERVER_OPENAI_CONTEXT_SIZE, "default");
});

test("proxies a real mock run over the versioned session transport and retains its terminal state", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-"));
  temporaryDirectories.push(directory);
  const capturePath = join(directory, "capture.json");
  writeFileSync(join(directory, "workspace-context.json"), JSON.stringify({
    schemaVersion: 1,
    workspaceRoot: directory,
    authorization: {
      recorded: true,
      source: "beale",
      scopeId: "scope_app_server_test",
      scopeName: "App-server test scope",
    },
  }));
  const server = await startAppServer({
    hostService: testHostService(directory, { capturePath, memoryBackend: "disabled" }),
  });
  servers.push(server);
  const auth = { authorization: `Bearer ${server.operatorToken}`, "content-type": "application/json" };

  const startResponse = await fetch(`${server.url}/v1/sessions`, {
    method: "POST",
    headers: auth,
    body: JSON.stringify(sessionLaunchRequest(directory, {
      sessionId: "session-facade",
      promptMarkdown: "Exercise the app-server facade.",
    })),
  });
  assert.equal(startResponse.status, 201);
  const started = await startResponse.json();
  assert.equal(started.controlVersion, BEALE_APP_SERVER_CONTROL_VERSION);
  assert.equal(started.session.sessionId, "session-facade");
  assert.equal(started.attemptId, "attempt-test");
  assert.equal(started.transport.path, "/v1/sessions/session-facade/transport");
  assert.equal(started.transport.reconnect, "replay");
  assert.ok(started.transport.token.length >= 16);

  const duplicate = await fetch(`${server.url}/v1/sessions`, {
    method: "POST",
    headers: auth,
    body: JSON.stringify(sessionLaunchRequest(directory, { sessionId: "session-facade" })),
  });
  assert.equal(duplicate.status, 409);

  await assert.rejects(
    connect(webSocketUrl(server.url, started.transport.path), "wrong-token"),
    (error) => error.statusCode === 401,
  );

  const socket = await connect(webSocketUrl(server.url, started.transport.path), started.transport.token);
  const messages = [];
  socket.on("message", (data) => messages.push(JSON.parse(data.toString("utf8"))));
  socket.send(JSON.stringify({
    protocolVersion: 1,
    type: "client.hello",
    sessionId: "session-facade",
    client: { name: "app-server-test", version: "0.1.0" },
  }));

  await waitFor(() => messages.some((message) => message.type === "server.hello"));
  assert.equal(messages.find((message) => message.type === "server.hello").protocolVersion, 1);
  await waitFor(() => messages.some((message) => message.type === "session.event"), 5_000).catch(async (error) => {
    const diagnostic = await (await fetch(`${server.url}/v1/sessions/session-facade`, {
      headers: { authorization: `Bearer ${server.operatorToken}` },
    })).json();
    throw new Error(`${error.message} Frames: ${JSON.stringify(messages)} Session: ${JSON.stringify(diagnostic)}`);
  });
  await waitForSocketClose(socket);
  const deliveredEvents = messages.filter((message) => message.type === "session.event");
  assert.ok(deliveredEvents.length > 0);
  assert.equal(deliveredEvents.every((message) => (
    typeof message.event?.payload?.eventId === "string" && message.event.payload.eventId.length > 0
  )), true);
  assert.equal(existsSync(capturePath), true);
  const capture = JSON.parse(readFileSync(capturePath, "utf8"));
  assert.equal(capture.runtimeConfig.memoryBackend, "disabled");
  assert.equal(capture.runtimeConfig.tools.some((tool) => tool.name.startsWith("memory.")), false);
  assert.equal(capture.runtimeConfig.tools.some((tool) => tool.name.startsWith("finding.")), false);

  const sessionAuth = { authorization: `Bearer ${server.operatorToken}` };
  await waitForCondition(async () => {
    const catalog = await (await fetch(`${server.url}/v1/sessions`, { headers: sessionAuth })).json();
    const candidate = catalog.sessions.find((item) => item.sessionId === "session-facade");
    return Boolean(candidate && ["completed", "failed", "stopped"].includes(candidate.state));
  });
  const catalog = await (await fetch(`${server.url}/v1/sessions`, { headers: sessionAuth })).json();
  const entry = catalog.sessions.find((candidate) => candidate.sessionId === "session-facade");
  assert.equal(entry.state, "completed");
  assert.equal(entry.clientConnected, false);
  assert.equal(entry.exitCode, 0);
  assert.equal(entry.diagnostic, null);
  assert.equal(entry.replay.droppedFrames, 0);

  const persistedStore = new AppServerSessionStore({ databasePath: join(directory, "memory.sqlite") });
  try {
    const persistedIds = new Set(persistedStore.get("session-facade").events.map((event) => event.id));
    assert.equal(deliveredEvents.some((message) => persistedIds.has(message.event.payload.eventId)), true);
  } finally {
    persistedStore.close();
  }

  const removed = await fetch(`${server.url}/v1/sessions/session-facade`, {
    method: "DELETE",
    headers: sessionAuth,
  });
  assert.equal(removed.status, 200);
  assert.deepEqual(await removed.json(), {
    controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
    stopped: false,
    sessionId: "session-facade",
  });

  const gone = await fetch(`${server.url}/v1/sessions/session-facade`, {
    method: "DELETE",
    headers: { authorization: `Bearer ${server.operatorToken}` },
  });
  assert.equal(gone.status, 404);

  const reused = await fetch(`${server.url}/v1/sessions`, {
    method: "POST",
    headers: auth,
    body: JSON.stringify(sessionLaunchRequest(directory, {
      sessionId: "session-facade",
      promptMarkdown: "Reuse the session id.",
    })),
  });
  assert.equal(reused.status, 201);
});

test("runs database-backed research tools through the app-server worker mediator", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-mediated-worker-"));
  temporaryDirectories.push(directory);
  const capturePath = join(directory, "capture.json");
  writeFileSync(join(directory, "workspace-context.json"), JSON.stringify({
    schemaVersion: 1,
    workspaceRoot: directory,
    authorization: {
      recorded: true,
      source: "beale",
      scopeId: "scope_mediated_worker_test",
      scopeName: "Mediated worker test scope",
    },
  }));
  const server = await startAppServer({
    hostService: testHostService(directory, { capturePath, memoryBackend: "app-server" }),
  });
  servers.push(server);
  const authorization = `Bearer ${server.operatorToken}`;
  const startResponse = await fetch(`${server.url}/v1/sessions`, {
    method: "POST",
    headers: { authorization, "content-type": "application/json" },
    body: JSON.stringify(sessionLaunchRequest(directory, {
      sessionId: "session-mediated-database",
      promptMarkdown: "Exercise host-mediated research storage.",
    })),
  });
  assert.equal(startResponse.status, 201);

  await waitForCondition(async () => {
    const catalog = await (await fetch(`${server.url}/v1/sessions`, {
      headers: { authorization },
    })).json();
    const session = catalog.sessions.find((candidate) => candidate.sessionId === "session-mediated-database");
    return Boolean(session && ["completed", "failed", "stopped"].includes(session.state));
  });
  const catalog = await (await fetch(`${server.url}/v1/sessions`, {
    headers: { authorization },
  })).json();
  const session = catalog.sessions.find((candidate) => candidate.sessionId === "session-mediated-database");
  assert.equal(session.state, "completed", session.diagnostic ?? undefined);

  const capture = JSON.parse(readFileSync(capturePath, "utf8"));
  assert.equal(capture.runtimeConfig.memoryBackend, "app-server");
  assert.equal(capture.runtimeConfig.tools.some((tool) => tool.name.startsWith("memory.")), true);
  assert.equal(capture.runtimeConfig.tools.some((tool) => tool.name.startsWith("finding.")), true);
  const database = new DatabaseSync(join(directory, "memory.sqlite"), { readOnly: true });
  try {
    assert.equal(
      database.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'memory_nodes'").get()?.name,
      "memory_nodes",
    );
    assert.equal(
      database.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'app_server_research_claims'").get()?.name,
      "app_server_research_claims",
    );
  } finally {
    database.close();
  }
});

test("replays hosted events after a mobile client disconnects and reconnects", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-replay-"));
  temporaryDirectories.push(directory);
  const upstream = await createFakeAppServerSessionHost();
  servers.push(upstream);
  const server = await startAppServer({
    hostService: testHostService(directory),
    spawnSession: upstream.spawnSession,
  });
  servers.push(server);

  const startResponse = await fetch(`${server.url}/v1/sessions`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${server.operatorToken}`,
      "content-type": "application/json",
    },
    body: JSON.stringify(sessionLaunchRequest(directory, { sessionId: "session-replay" })),
  });
  assert.equal(startResponse.status, 201);
  const started = await startResponse.json();

  const firstMessages = [];
  const firstSocket = await connect(
    webSocketUrl(server.url, started.transport.path),
    started.transport.token,
    firstMessages,
  );
  firstSocket.send(JSON.stringify({
    protocolVersion: 1,
    type: "client.hello",
    sessionId: "session-replay",
    client: { name: "app-server-test", version: "0.1.0" },
  }));
  await waitFor(() => firstMessages.some((message) => message.type === "server.hello"));
  firstSocket.close(1000);
  await waitForSocketClose(firstSocket);
  await waitForCondition(async () => {
    const response = await fetch(`${server.url}/v1/sessions/session-replay`, {
      headers: { authorization: `Bearer ${server.operatorToken}` },
    });
    return response.ok && (await response.json()).session.clientConnected === false;
  });

  upstream.sendEvent({ schemaVersion: 1, kind: "fixture.replayed" });
  await waitForCondition(async () => {
    const response = await fetch(`${server.url}/v1/sessions/session-replay`, {
      headers: { authorization: `Bearer ${server.operatorToken}` },
    });
    return response.ok && (await response.json()).session.replay.bufferedFrames === 1;
  });

  const resumedMessages = [];
  const resumedSocket = await connect(
    webSocketUrl(server.url, started.transport.path),
    started.transport.token,
    resumedMessages,
  );
  resumedSocket.send(JSON.stringify(clientHello("session-replay", "beale-ios")));
  await waitFor(() => resumedMessages.some((message) => message.type === "server.hello"));
  await waitFor(() => resumedMessages.some(
    (message) => message.type === "session.event" && message.event.kind === "fixture.replayed"
  ));

  upstream.complete();
  await waitForSocketClose(resumedSocket);
});

test("fans one app-server session out to independently authenticated desktop and mobile clients", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-multi-client-"));
  temporaryDirectories.push(directory);
  const upstream = await createFakeAppServerSessionHost();
  servers.push(upstream);
  const server = await startAppServer({
    hostService: testHostService(directory),
    spawnSession: upstream.spawnSession,
  });
  servers.push(server);

  const operatorHeaders = {
    authorization: `Bearer ${server.operatorToken}`,
    "content-type": "application/json",
  };
  const startedResponse = await fetch(`${server.url}/v1/sessions`, {
    method: "POST",
    headers: operatorHeaders,
    body: JSON.stringify(sessionLaunchRequest(directory, { sessionId: "session-shared" })),
  });
  assert.equal(startedResponse.status, 201);
  const started = await startedResponse.json();

  const desktopMessages = [];
  const desktop = await connect(
    webSocketUrl(server.url, started.transport.path),
    started.transport.token,
    desktopMessages,
  );
  desktop.send(JSON.stringify(clientHello("session-shared", "beale-desktop")));
  await waitFor(() => desktopMessages.some((message) => message.type === "server.hello"));

  const attachmentResponse = await fetch(`${server.url}/v1/sessions/session-shared/attachments`, {
    method: "POST",
    headers: operatorHeaders,
  });
  assert.equal(attachmentResponse.status, 201);
  const attachment = await attachmentResponse.json();
  assert.notEqual(attachment.transport.token, started.transport.token);
  assert.equal(attachment.transport.path, started.transport.path);

  const mobileMessages = [];
  const mobile = await connect(
    webSocketUrl(server.url, attachment.transport.path),
    attachment.transport.token,
    mobileMessages,
  );
  mobile.send(JSON.stringify(clientHello("session-shared", "beale-ios")));
  await waitFor(() => mobileMessages.some((message) => message.type === "server.hello"));

  const desktopControl = sessionControl("session-shared", "desktop-control", "Desktop steering");
  const mobileControl = sessionControl("session-shared", "mobile-control", "Mobile steering");
  desktop.send(JSON.stringify(desktopControl));
  mobile.send(JSON.stringify(mobileControl));
  await waitFor(() => upstream.controls.length === 2);
  assert.deepEqual(
    upstream.controls.map((control) => control.requestId).sort(),
    ["desktop-control", "mobile-control"],
  );
  await waitFor(() => desktopMessages.filter(controlAcknowledgement).length === 2);
  await waitFor(() => mobileMessages.filter(controlAcknowledgement).length === 2);

  upstream.sendEvent({ schemaVersion: 1, kind: "fixture.shared" });
  await waitFor(() => desktopMessages.some((message) => message.event?.kind === "fixture.shared"));
  await waitFor(() => mobileMessages.some((message) => message.event?.kind === "fixture.shared"));

  mobile.close(1000);
  await waitForSocketClose(mobile);
  upstream.sendEvent({ schemaVersion: 1, kind: "fixture.desktop-remains" });
  await waitFor(() => desktopMessages.some((message) => message.event?.kind === "fixture.desktop-remains"));
  const catalog = await (await fetch(`${server.url}/v1/sessions/session-shared`, {
    headers: { authorization: `Bearer ${server.operatorToken}` },
  })).json();
  assert.equal(catalog.session.clientConnected, true);

  upstream.complete();
  await waitForSocketClose(desktop);
});

test("stops a launched session that has no connected client", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-stop-"));
  temporaryDirectories.push(directory);
  const server = await startAppServer({ hostService: testHostService(directory) });
  servers.push(server);
  const auth = { authorization: `Bearer ${server.operatorToken}`, "content-type": "application/json" };

  const startResponse = await fetch(`${server.url}/v1/sessions`, {
    method: "POST",
    headers: auth,
    body: JSON.stringify(sessionLaunchRequest(directory, {
      sessionId: "session-stop",
      promptMarkdown: "Waiting for a client.",
    })),
  });
  assert.equal(startResponse.status, 201);

  const stopResponse = await fetch(`${server.url}/v1/sessions/session-stop`, {
    method: "DELETE",
    headers: { authorization: `Bearer ${server.operatorToken}` },
  });
  assert.equal(stopResponse.status, 202);
  assert.deepEqual(await stopResponse.json(), {
    controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
    stopped: true,
    sessionId: "session-stop",
  });

  await waitForCondition(async () => {
    const response = await fetch(`${server.url}/v1/sessions`, {
      headers: { authorization: `Bearer ${server.operatorToken}` },
    });
    const catalog = await response.json();
    const entry = catalog.sessions.find((candidate) => candidate.sessionId === "session-stop");
    return Boolean(entry && entry.state === "stopped");
  });

  const removed = await fetch(`${server.url}/v1/sessions/session-stop`, {
    method: "DELETE",
    headers: { authorization: `Bearer ${server.operatorToken}` },
  });
  assert.equal(removed.status, 200);
});

test("writes and clears the discovery record", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-discovery-"));
  temporaryDirectories.push(directory);
  const stateFile = join(directory, "app-server.json");
  const server = await startAppServer({ discoveryFile: stateFile, hostMode: "tray" });
  servers.push(server);

  assert.equal(existsSync(stateFile), true);
  const record = JSON.parse(readFileSync(stateFile, "utf8"));
  assert.equal(record.version, 1);
  assert.equal(record.contractTimestamp, BEALE_APP_SERVER_CONTRACT_TIMESTAMP);
  assert.equal(record.hostMode, "tray");
  assert.equal(record.pid, process.pid);
  assert.equal(record.port, server.port);
  assert.equal(record.localUrl, `http://127.0.0.1:${server.port}`);
  assert.equal(record.url, server.url);
  assert.equal(record.operatorToken, server.operatorToken);
  assert.equal(existsSync(operatorTokenPath(stateFile)), true);
  assert.match(record.startedAt, /^\d{4}-\d{2}-\d{2}T/);

  const persistentToken = server.operatorToken;

  await server.close();
  servers.pop();
  assert.equal(existsSync(stateFile), false);

  const restarted = await startAppServer({ discoveryFile: stateFile });
  servers.push(restarted);
  assert.equal(restarted.operatorToken, persistentToken);
});

test("serializes app-server launches with a recoverable discovery lock", () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-lock-"));
  temporaryDirectories.push(directory);
  const stateFile = join(directory, "app-server.json");
  const lockFile = discoveryLockPath(stateFile);

  assert.equal(acquireDiscoveryLock(stateFile, process.pid), true);
  assert.equal(existsSync(lockFile), true);
  assert.equal(acquireDiscoveryLock(stateFile, process.pid), true);
  assert.equal(releaseDiscoveryLock(stateFile, process.pid + 1), false);
  assert.equal(releaseDiscoveryLock(stateFile, process.pid), true);

  assert.notEqual(process.ppid, process.pid);
  writeFileSync(lockFile, `${JSON.stringify({ pid: process.ppid })}\n`, "utf8");
  assert.equal(acquireDiscoveryLock(stateFile, process.pid), false);
  rmSync(lockFile, { force: true });

  writeFileSync(lockFile, `${JSON.stringify({ pid: 2_147_483_647 })}\n`, "utf8");
  assert.equal(acquireDiscoveryLock(stateFile, process.pid), true);
  assert.equal(releaseDiscoveryLock(stateFile, process.pid), true);
});

test("recovers a failed long session without replacing its client transport", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-recovery-"));
  temporaryDirectories.push(directory);
  const spawned = [];
  const recoveryInputs = [];
  let releaseRecoveryPreparation;
  const recoveryPreparation = new Promise((resolve) => { releaseRecoveryPreparation = resolve; });
  const hostService = testHostService(directory);
  hostService.prepareSessionRecovery = async (input) => {
    recoveryInputs.push(input);
    await recoveryPreparation;
    return {
      sessionId: input.sessionId,
      attemptId: "attempt-recovery",
      launch: {
        ...resolvedSessionLaunch(directory, {
          capturePath: join(directory, "session-recovery.attempt-recovery.capture.json"),
          promptMarkdown: input.request.launch.promptMarkdown,
        }),
        attemptId: "attempt-recovery",
        resumeCapturePath: join(directory, "session-recovery.capture.json"),
        resumeFallbackPromptPath: join(directory, "session-recovery.resume-fallback.md"),
      },
    };
  };
  const server = await startAppServer({
    hostService,
    longSessionRecovery: { maxAttempts: 2, delayMs: () => 0 },
    spawnSession: async ({ sessionId, args }) => {
      let resolveExit;
      const listeners = new Set();
      const controls = [];
      const exit = new Promise((resolve) => { resolveExit = resolve; });
      const worker = {
        args,
        controls,
        finish(code, stderr = "") { resolveExit({ code, stderr }); },
      };
      spawned.push(worker);
      return {
        sessionId,
        onEvent(listener) {
          listeners.add(listener);
          return () => listeners.delete(listener);
        },
        sendControl(control) { controls.push(control); },
        stderrTail: () => "",
        waitExit: () => exit,
        stop: () => resolveExit({ code: null, stderr: "" }),
      };
    },
  });
  servers.push(server);
  const response = await fetch(`${server.url}/v1/sessions`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${server.operatorToken}`,
      "content-type": "application/json",
    },
    body: JSON.stringify(sessionLaunchRequest(directory, {
      sessionId: "session-recovery",
      promptMarkdown: "Continue the long-running parser investigation.",
    })),
  });
  assert.equal(response.status, 201);
  const started = await response.json();
  const messages = [];
  const socket = await connect(webSocketUrl(server.url, started.transport.path), started.transport.token, messages);
  socket.send(JSON.stringify(clientHello("session-recovery", "recovery-test")));
  await waitFor(() => messages.some((message) => message.type === "server.hello"));

  spawned[0].finish(1, "WebSocket disconnected after the provider retry limit was reached.");
  await waitFor(() => messages.some((message) => (
    message.type === "session.event"
      && message.event?.kind === "model.output"
      && message.event?.payload?.messagePhase === "commentary"
      && /continuing automatically/u.test(message.event.payload.text)
  )));
  socket.send(JSON.stringify(sessionControl(
    "session-recovery",
    "control-during-recovery",
    "Also inspect the adjacent parser variant."
  )));
  releaseRecoveryPreparation();

  await waitFor(() => spawned.length === 2);
  await waitFor(() => spawned[1].controls.some((control) => control.requestId === "control-during-recovery"));
  assert.equal(socket.readyState, WebSocket.OPEN);
  assert.equal(recoveryInputs.length, 1);
  assert.equal(recoveryInputs[0].previousAttemptId, "attempt-test");
  assert.equal(recoveryInputs[0].previousAttemptWasInitial, true);
  assert.match(recoveryInputs[0].fallbackPrompt, /Continue the long-running parser investigation/u);
  assert.ok(spawned[1].args.includes("--resume-capture"));

  spawned[1].finish(0);
  await waitForSocketClose(socket);
  const catalog = await (await fetch(`${server.url}/v1/sessions`, {
    headers: { authorization: `Bearer ${server.operatorToken}` },
  })).json();
  const entry = catalog.sessions.find((candidate) => candidate.sessionId === "session-recovery");
  assert.equal(entry.state, "completed");
  assert.equal(entry.exitCode, 0);
  assert.equal(entry.diagnostic, null);
});

test("app-server startup relaunches interrupted sessions before clients attach", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-startup-recovery-"));
  temporaryDirectories.push(directory);
  const fakeHost = await createFakeAppServerSessionHost();
  const request = sessionLaunchRequest(directory, {
    sessionId: "session-startup-recovery",
    promptMarkdown: "Resume after the app-server restart.",
  });
  const hostService = testHostService(directory);
  let startupRecoveryCalls = 0;
  hostService.recoverInterruptedSessions = async () => {
    startupRecoveryCalls += 1;
    return {
      interruptedSessions: 1,
      skippedSessions: 0,
      errors: [],
      recovered: [{
        request,
        prepared: {
          sessionId: "session-startup-recovery",
          attemptId: "attempt-startup-recovery",
          launch: {
            ...resolvedSessionLaunch(directory, {
              capturePath: join(directory, "session-startup-recovery.attempt-startup-recovery.capture.json"),
              promptMarkdown: request.launch.promptMarkdown,
            }),
            attemptId: "attempt-startup-recovery",
            resumeCapturePath: join(directory, "session-startup-recovery.capture.json"),
            resumeFallbackPromptPath: join(directory, "session-startup-recovery.resume-fallback.md"),
          },
        },
      }],
    };
  };
  const server = await startAppServer({
    hostService,
    spawnSession: fakeHost.spawnSession,
    recoverInterruptedOnStart: true,
  });
  servers.push(server);

  assert.equal(startupRecoveryCalls, 1);
  assert.equal(server.listSessions()[0].state, "running");

  const attachment = await fetch(`${server.url}/v1/sessions/session-startup-recovery/attachments`, {
    method: "POST",
    headers: { authorization: `Bearer ${server.operatorToken}` },
  });
  assert.equal(attachment.status, 201);
  const attached = await attachment.json();
  const messages = [];
  const socket = await connect(webSocketUrl(server.url, attached.transport.path), attached.transport.token, messages);
  socket.send(JSON.stringify(clientHello("session-startup-recovery", "startup-recovery-test")));
  await waitFor(() => messages.some((message) => (
    message.type === "session.event"
      && message.event?.kind === "model.output"
      && /previous app-server session ended unexpectedly/u.test(message.event?.payload?.text)
  )));
  assert.equal(socket.readyState, WebSocket.OPEN);

  fakeHost.complete();
  await waitForSocketClose(socket);
  assert.equal(server.listSessions()[0].state, "completed");
  await fakeHost.close();
});

test("accepted pause and stop controls are persisted as intentional session state", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-control-state-"));
  temporaryDirectories.push(directory);
  const upstream = await createFakeAppServerSessionHost();
  const hostService = testHostService(directory);
  const states = [];
  hostService.recordSessionControlState = async (input) => { states.push(input.state); };
  const server = await startAppServer({ hostService, spawnSession: upstream.spawnSession });
  servers.push(server);
  const response = await fetch(`${server.url}/v1/sessions`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${server.operatorToken}`,
      "content-type": "application/json",
    },
    body: JSON.stringify(sessionLaunchRequest(directory, { sessionId: "session-manual-controls" })),
  });
  const started = await response.json();
  const messages = [];
  const socket = await connect(webSocketUrl(server.url, started.transport.path), started.transport.token, messages);
  socket.send(JSON.stringify(clientHello("session-manual-controls", "manual-control-test")));
  await waitFor(() => messages.some((message) => message.type === "server.hello"));

  socket.send(JSON.stringify({
    protocolVersion: 1,
    type: "session.control",
    sessionId: "session-manual-controls",
    requestId: "pause-request",
    control: { schemaVersion: 1, type: "pause", requestId: "pause-request" },
  }));
  await waitFor(() => states.includes("paused"));
  socket.send(JSON.stringify({
    protocolVersion: 1,
    type: "session.control",
    sessionId: "session-manual-controls",
    requestId: "stop-request",
    control: { schemaVersion: 1, type: "stop", requestId: "stop-request" },
  }));
  await waitFor(() => states.includes("stopped"));
  await waitFor(() => upstream.stopCalls() === 1);
  await waitFor(() => server.listSessions()[0]?.state === "stopped");

  assert.equal(states.includes("paused"), true);
  assert.equal(states.includes("stopped"), true);
  await waitForSocketClose(socket);
  await upstream.close();
});

test("graceful app-server shutdown leaves active canonical sessions eligible for startup recovery", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-graceful-restart-"));
  temporaryDirectories.push(directory);
  const upstream = await createFakeAppServerSessionHost();
  const hostService = testHostService(directory);
  const states = [];
  hostService.recordSessionControlState = async (input) => { states.push(input.state); };
  const server = await startAppServer({ hostService, spawnSession: upstream.spawnSession });
  const response = await fetch(`${server.url}/v1/sessions`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${server.operatorToken}`,
      "content-type": "application/json",
    },
    body: JSON.stringify(sessionLaunchRequest(directory, { sessionId: "session-graceful-restart" })),
  });
  assert.equal(response.status, 201);

  await server.close();

  assert.deepEqual(states, []);
  await upstream.close();
});

test("advertises an HTTPS public origin without embedding it in session transport paths", async () => {
  const server = await startAppServer({ publicUrl: "https://beale.example.ts.net" });
  servers.push(server);
  assert.equal(server.url, "https://beale.example.ts.net");
  assert.equal(server.host, "127.0.0.1");
});

function connect(url, token, messages = null) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(url, {
      headers: token ? { authorization: `Bearer ${token}` } : {},
    });
    if (messages) {
      socket.on("message", (data) => messages.push(JSON.parse(data.toString("utf8"))));
    }
    socket.once("open", () => resolve(socket));
    socket.once("unexpected-response", (_request, response) => {
      const error = new Error(`Unexpected response: ${response.statusCode}`);
      error.statusCode = response.statusCode;
      response.destroy();
      reject(error);
    });
    socket.once("error", reject);
  });
}

function webSocketUrl(baseUrl, path) {
  return `${baseUrl.replace(/^http/u, "ws")}${path}`;
}

async function createFakeAppServerSessionHost() {
  const listeners = new Set();
  let sessionId = "";
  let exitResolved = false;
  let resolveExit;
  let stopCalls = 0;
  const controls = [];
  const exit = new Promise((resolve) => { resolveExit = resolve; });
  const emit = (event) => {
    for (const listener of listeners) listener(event);
  };

  const finish = (code) => {
    if (exitResolved) return;
    exitResolved = true;
    resolveExit({ code, stderr: "" });
  };

  return {
    spawnSession: async (options) => {
      sessionId = options.sessionId;
      return {
        sessionId,
        onEvent: (listener) => {
          listeners.add(listener);
          return () => listeners.delete(listener);
        },
        sendControl: (control) => {
          controls.push(control);
          emit({
            schemaVersion: 1,
            kind: "agent.event",
            timestamp: new Date().toISOString(),
            payload: {
              eventType: "control.received",
              type: control.type,
              accepted: true,
              requestId: control.requestId,
            },
          });
        },
        stderrTail: () => "",
        waitExit: () => exit,
        stop: () => {
          stopCalls += 1;
          finish(null);
        },
      };
    },
    sendEvent: emit,
    controls,
    stopCalls: () => stopCalls,
    complete: () => finish(0),
    close: async () => {
      finish(null);
      listeners.clear();
    },
  };
}

function clientHello(sessionId, name) {
  return {
    protocolVersion: 1,
    type: "client.hello",
    sessionId,
    client: { name, version: "0.1.0" },
  };
}

function sessionControl(sessionId, requestId, instruction) {
  return {
    protocolVersion: 1,
    type: "session.control",
    sessionId,
    requestId,
    control: { schemaVersion: 1, type: "steer", requestId, instruction },
  };
}

function controlAcknowledgement(message) {
  return message.type === "session.event"
    && message.event?.kind === "agent.event"
    && message.event?.payload?.eventType === "control.received";
}

function sessionLaunchRequest(directory, options = {}) {
  return {
    launchVersion: 3,
    sessionId: options.sessionId ?? "session-test",
    launch: {
      workspaceId: "workspace-test",
      promptMarkdown: options.promptMarkdown ?? "Test the typed app-server launch contract.",
      provider: {
        id: "openai-codex",
      },
      shellSafetyMode: "manual_approval",
      ...(options.researchProfile?.id ? { researchProfileId: options.researchProfile.id } : {}),
      ...(options.researchProfile?.hash ? { researchProfileHash: options.researchProfile.hash } : {}),
      ...(options.researchProfile?.workflowId ? { workflowId: options.researchProfile.workflowId } : {}),
    },
  };
}

function resolvedSessionLaunch(directory, options = {}) {
  return {
    workspaceRoot: directory,
    workspaceDirectories: [directory],
    capturePath: options.capturePath ?? join(directory, "capture.json"),
    ...(options.workspaceContextPath ? { workspaceContextPath: options.workspaceContextPath } : {}),
    attemptId: "attempt-test",
    promptMarkdown: options.promptMarkdown ?? "Test the typed app-server launch contract.",
    provider: {
      id: "openai-codex",
      riskAcknowledgements: ["openai-codex"],
      authenticationPreferences: { "openai-codex": "subscription" },
      title: { model: "gpt-5.6-luna", effort: "medium" },
      shellReview: {
        models: { "openai-codex": "gpt-5.6-luna" },
        effort: "medium",
      },
    },
    shellSafetyMode: "manual_approval",
    ...(options.researchProfileHash ? { researchProfileHash: options.researchProfileHash } : {}),
    ...(options.workflowId ? { workflowId: options.workflowId } : {}),
    profileAware: true,
    memoryBackend: options.memoryBackend ?? "app-server",
    storage: {
      databasePath: join(directory, "memory.sqlite"),
      artifactDirectoryPath: join(directory, "artifacts"),
    },
  };
}

function testHostService(directory, options = {}) {
  return {
    browserHomePage() { return 'https://example.test/'; },
    listWorkspaces() {
      return {
        controlVersion: 1,
        workspaces: [{
          id: "registry-workspace-test",
          workspaceId: "workspace-test",
          name: "Test workspace",
          researchProfileId: "security-research",
          updatedAt: "2026-08-21T00:00:00.000Z",
        }],
      };
    },
    async prepareSession(request) {
      const workspaceContextPath = join(directory, "workspace-context.json");
      const databasePath = join(directory, "memory.sqlite");
      const store = new AppServerSessionStore({ databasePath });
      try {
        if (!store.getSummary(request.sessionId ?? "session-generated")) {
          store.create({
            id: request.sessionId ?? "session-generated",
            workspaceId: request.launch.workspaceId,
            attemptId: "attempt-test",
            title: "App-server test session",
            prompt: request.launch.promptMarkdown,
            provider: request.launch.provider?.id ?? "openai-codex",
            model: request.launch.provider?.model ?? "mock",
            reasoningEffort: request.launch.provider?.reasoningEffort ?? "medium",
          });
        }
      } finally {
        store.close();
      }
      return {
        sessionId: request.sessionId ?? "session-generated",
        attemptId: "attempt-test",
        launch: resolvedSessionLaunch(directory, {
          capturePath: options.capturePath,
          workspaceContextPath: existsSync(workspaceContextPath) ? workspaceContextPath : undefined,
          promptMarkdown: request.launch.promptMarkdown,
                researchProfileHash: request.launch.researchProfileHash,
          workflowId: request.launch.workflowId,
          memoryBackend: options.memoryBackend,
        }),
      };
    },
    async workspaceMemory() { return canonicalFixture("memory"); },
    async quickChats() { return { schemaVersion: 1, chats: [] }; },
    async workspaceDetails() { return {
      schemaVersion: 1,
      workspace: { id: "registry-workspace-test", workspaceId: "workspace-test", name: "Test workspace", researchProfileId: "security-research", researchKitId: "general", runCount: 0, lastRunAt: null, updatedAt: "2026-08-21T00:00:00.000Z" },
      scope: { id: "scope-test", name: "Test workspace", owner: "ExampleCo", descriptionMarkdown: "Research example.test.", expiresAt: null, resources: [] },
      researchSubject: "ExampleCo", rules: [],
      memory: { schemaVersion: 4, workspaceId: "workspace-test", status: "empty", nodeCount: 0, nodeTypeCounts: {}, nodes: [], leads: [], findings: [] },
      runbooks: [], reports: [],
      targets: [{ id: "local", name: "Local", ready: true }],
    }; },
    async workspaceMemoryNotifications() { return canonicalFixture("memory-notifications"); },
    async workspaceSessions() { return canonicalFixture("sessions"); },
    async sessionUpdate() { return canonicalFixture("update"); },
    async sessionEvents() { return canonicalFixture("events"); },
    async sessionEventDetails() { return canonicalFixture("event-details"); },
    async sessionCollaboration() { return canonicalFixture("collaboration"); },
    async sessionCaptures() { return canonicalFixture("captures"); },
  };
}

function hostRegistryFixture(directory, options = {}) {
  const currentWorkspace = {
    id: "registry-workspace-test",
    workspaceId: "workspace-test",
    name: "Test workspace",
    researchProfileId: "security-research",
    researchKitId: "general",
    runCount: 0,
    lastRunAt: null,
    updatedAt: "2026-08-21T00:00:00.000Z",
    workspacePath: directory,
    workspaceDirectories: [directory],
    memoryBackend: options.memoryBackend ?? "app-server",
  };
  const hostWorkspaces = options.hostWorkspaces ?? [currentWorkspace];
  return {
    registryDirectory: directory,
    registryPath: join(directory, "workspace-registry.sqlite"),
    shellOptionsPath: join(directory, "shell-options.json"),
    listWorkspaces: () => [],
    listHostWorkspaces: () => hostWorkspaces,
    resolveWorkspace: (identifier) => hostWorkspaces.find((workspace) => (
      workspace.workspaceId === identifier || workspace.id === identifier
    )) ?? null,
    providerSettings: () => ({
      defaultProviderId: "openai-codex",
      modelDefaults: {},
      authenticationPreferences: {},
      riskAcknowledgements: [],
    }),
    promptTemplateOverride: () => null,
    featurePromptOverrides: () => ({}),
    memoryTypeDescriptions: () => null,
    storageForProfile: () => ({
      databasePath: join(directory, "memory.sqlite"),
      artifactDirectoryPath: join(directory, "artifacts"),
    }),
  };
}

function canonicalFixture(kind) {
  return {
    controlVersion: 1,
    workspace: {
      id: "registry-workspace-test",
      workspaceId: "workspace-test",
      name: "Test workspace",
      researchProfileId: "security-research",
      updatedAt: "2026-08-21T00:00:00.000Z",
    },
    result: { kind },
  };
}

function brokerRequest(broker, request) {
  const pending = brokerRequestMessage(request);
  broker.handle(pending.message);
  return brokerResponse(pending.responseBuffer);
}

function brokerRequestMessage(request) {
  const responseBuffer = new SharedArrayBuffer(1024 * 1024);
  return {
    message: { type: "database.request", request, responseBuffer },
    responseBuffer,
  };
}

function brokerResponse(responseBuffer) {
  const header = new Int32Array(responseBuffer, 0, 2);
  assert.equal(Atomics.load(header, 0), 1);
  const length = Atomics.load(header, 1);
  return deserialize(Buffer.from(new Uint8Array(responseBuffer, 8, length)));
}

function waitFor(predicate, timeoutMs = 30_000) {
  return new Promise((resolve, reject) => {
    const deadline = Date.now() + timeoutMs;
    const tick = () => {
      if (predicate()) {
        resolve();
        return;
      }
      if (Date.now() > deadline) {
        reject(new Error("Timed out waiting for condition."));
        return;
      }
      setTimeout(tick, 50);
    };
    tick();
  });
}

async function waitForCondition(predicate, timeoutMs = 15_000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    if (await predicate()) return;
    if (Date.now() > deadline) throw new Error("Timed out waiting for condition.");
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
}

function waitForSocketClose(socket, timeoutMs = 15_000) {
  if (socket.readyState === WebSocket.CLOSED) return Promise.resolve();
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("Timed out waiting for socket close.")), timeoutMs);
    socket.once("close", () => {
      clearTimeout(timer);
      resolve();
    });
  });
}
