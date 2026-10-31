import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { APP_SERVER_PROTOCOL_VERSION, APP_SERVER_SESSION_LAUNCH_VERSION, BEALE_APP_SERVER_CAPABILITIES, BEALE_APP_SERVER_CONTRACT_TIMESTAMP, BEALE_APP_SERVER_CONTROL_VERSION } from '@beale/app-server-runtime/protocol';
import { FleetIncompleteResponseError, FleetService, assertCompatibleRemoteAppServerDescriptor, probeFleetAppServerStatus } from '../dist/fleet.js';

test('remote research-track status retries an incomplete JSON reply without replaying control', async () => {
  const fleet = new FleetService();
  fleet.connectedRemoteAppServer = async () => ({ url: 'https://remote.example.test', operatorToken: 'synthetic-token' });
  const originalFetch = globalThis.fetch;
  const actions = [];
  globalThis.fetch = async (_url, options) => {
    actions.push(JSON.parse(options.body).input.action);
    return actions.length === 1 || actions.at(-1) === 'start' ? new Response('', { status: 200 })
      : Response.json({ controlVersion: BEALE_APP_SERVER_CONTROL_VERSION, result: null });
  };
  try {
    assert.equal(await fleet.callRemote('server-example', 'workspace.research_track',
      { workspaceId: 'workspace-example', action: 'status' }), null);
    assert.deepEqual(actions, ['status', 'status']);
    await assert.rejects(fleet.callRemote('server-example', 'workspace.research_track',
      { workspaceId: 'workspace-example', action: 'start' }), FleetIncompleteResponseError);
    assert.deepEqual(actions, ['status', 'status', 'start']);
  } finally { globalThis.fetch = originalFetch; }
});

test('workspace-scoped Fleet state skips VM discovery for Local-only targets', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-targets-example-'));
  const path = join(root, 'fleet.json');
  const commands = [];
  const fleet = new FleetService(path, 'darwin', {
    async run(command, args) { commands.push([command, ...args]); return '[]'; },
    launch() {},
  });
  try {
    const local = await fleet.stateForWorkspace('workspace-example');
    assert.deepEqual(local.workspaceTargets, {});
    assert.deepEqual(local.machines, []);
    assert.deepEqual(commands, []);
    await fleet.configure({ action: 'select-machine', workspaceId: 'workspace-example', machineId: 'local' });
    assert.deepEqual(commands, []);

    const config = JSON.parse(readFileSync(path, 'utf8'));
    config.workspaceTargets['workspace-example'] = ['vm:run:tart:example-vm'];
    writeFileSync(path, JSON.stringify(config));
    await fleet.stateForWorkspace('workspace-example');
    assert.deepEqual(commands, [['tart', 'list', '--source', 'local', '--format', 'json']]);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('Fleet saves a validated browser home page and restores the default for older configurations', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-browser-settings-'));
  const path = join(root, 'fleet.json');
  const fleet = new FleetService(path, 'linux');
  try {
    assert.equal((await fleet.state()).browserHomePage, 'https://duckduckgo.com/');
    assert.equal((await fleet.configure({ action: 'set-browser-home-page', url: 'https://example.test/start' })).browserHomePage,
      'https://example.test/start');
    assert.equal(new FleetService(path, 'linux').browserHomePage(), 'https://example.test/start');
    await assert.rejects(fleet.configure({ action: 'set-browser-home-page', url: 'file:///example.txt' }), /HTTP or HTTPS/u);
    await assert.rejects(fleet.configure({ action: 'set-browser-home-page', url: 'https://user:pass@example.test/' }), /HTTP or HTTPS/u);
    assert.equal(fleet.browserHomePage(), 'https://example.test/start');
    const legacy = JSON.parse(readFileSync(path, 'utf8'));
    delete legacy.browserHomePage;
    writeFileSync(path, JSON.stringify(legacy));
    assert.equal(new FleetService(path, 'linux').browserHomePage(), 'https://duckduckgo.com/');
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('remote primary compatibility identifies stale and newer runtime builds', () => {
  assert.doesNotThrow(() => assertCompatibleRemoteAppServerDescriptor({
    ok: true, controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
    contractTimestamp: BEALE_APP_SERVER_CONTRACT_TIMESTAMP,
  }));
  assert.throws(() => assertCompatibleRemoteAppServerDescriptor({
    ok: true, controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
    contractTimestamp: '2000-01-01T00:00:00.000Z',
  }), /older control contract.*Run pnpm build on that machine/u);
  assert.throws(() => assertCompatibleRemoteAppServerDescriptor({
    ok: true, controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
    contractTimestamp: '2099-01-01T00:00:00.000Z',
  }), /newer control contract.*Update and restart this Beale installation/u);
});

test('app-server status distinguishes reachable compatible, reachable troubled, and unreachable peers', async () => {
  const descriptor = {
    ok: true, controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
    contractTimestamp: BEALE_APP_SERVER_CONTRACT_TIMESTAMP, capabilities: BEALE_APP_SERVER_CAPABILITIES,
    sessionLaunchVersion: APP_SERVER_SESSION_LAUNCH_VERSION, appServerProtocolVersion: APP_SERVER_PROTOCOL_VERSION,
    endpoints: { sessions: '/v1/sessions', workspaces: '/v1/workspaces', providers: '/v1/providers', operations: '/v1/operations', shutdown: '/v1/server/shutdown' },
    limits: { requestBodyBytes: 1024, frameBytes: 1024, replayBytes: 4_194_304, replayFrames: 256 }
  };
  const verify = async (url) => { assert.equal(url, 'https://worker.example.test'); };
  const get = async (url, options) => {
    assert.equal(url, 'https://worker.example.test/v1/server');
    assert.equal(options.headers.authorization, 'Bearer synthetic-operator-token');
    return Response.json(descriptor);
  };
  const probe = (verifyPeer, fetcher) => probeFleetAppServerStatus(
    'server-example', 'https://worker.example.test', 'synthetic-operator-token', verifyPeer, fetcher
  );
  assert.equal((await probe(verify, get)).state, 'online');
  assert.equal((await probe(async () => { throw new Error('peer offline'); }, async () => { throw new Error('Should not fetch'); })).state, 'offline');
  assert.equal((await probe(verify, async () => { throw new Error('Connection refused'); })).state, 'offline');
  assert.equal((await probe(verify, async () => new Response(null, { status: 401 }))).state, 'issue');
  assert.equal((await probe(verify, async () => new Response(null, { status: 503 }))).state, 'issue');
  assert.equal((await probe(verify, async () => Response.json({ ...descriptor, contractTimestamp: '2000-01-01T00:00:00.000Z' }))).state, 'issue');
  assert.equal((await probe(verify, async () => Response.json({ ok: true }))).state, 'issue');
});

test('connected primary sessions request one remote browser and attach with a session token', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-browser-primary-'));
  const requests = [];
  const fleet = new FleetService(join(root, 'fleet.json'), 'darwin', { async run() { return '[]'; }, launch() {} });
  fleet.connectedRemoteAppServer = async () => ({ id: 'server-example', name: 'Example',
    url: 'https://worker.example.test', operatorToken: 'synthetic-operator-token' });
  fleet.callRemote = async () => ({ enabled: true, role: 'primary', requiredWorkspaceIds: [], optionalWorkspaceIds: [],
    workspaceTargets: { 'workspace-example': ['local', 'vm:run:tart:example-vm'] },
    browsers: [{ id: 'in-agent', enabled: true }],
    machines: [{ id: 'tart:example-vm', canRun: true, canClone: false, state: 'running' }] });
  const previousFetch = globalThis.fetch;
  globalThis.fetch = async (url, init) => {
    requests.push({ url: String(url), body: JSON.parse(init.body) });
    if (String(url).endsWith('/attachments')) return Response.json({ transport: { token: 'synthetic-session-token' } });
    return Response.json({ session: { sessionId: 'session-example' } });
  };
  try {
    assert.deepEqual(await fleet.remoteLaunch('server-example', 'workspace-example', { promptMarkdown: 'Research the example scope.', targetIds: ['local'], provider: { id: 'openai-codex', model: 'gpt-example', reasoningEffort: 'high' }, goal: { objective: 'Assess example.test.' }, repeatSchedule: { type: 'hourly', interval: 2 } }),
      { sessionId: 'session-example' });
    assert.equal(requests[0].body.launch.browserRelay, true);
    assert.equal(requests[0].body.launch.machineId, 'local');
    assert.equal(requests[0].body.launch.provider.model, 'gpt-example');
    assert.equal(requests[0].body.launch.goal.objective, 'Assess example.test.');
    assert.deepEqual(requests[0].body.launch.repeatSchedule, { type: 'hourly', interval: 2 });
    assert.deepEqual(await fleet.remoteBrowserAttach('server-example', 'session-example'), {
      url: 'wss://worker.example.test/v1/sessions/session-example/browser', token: 'synthetic-session-token',
    });
    assert.equal(requests[1].url, 'https://worker.example.test/v1/sessions/session-example/attachments');
    await fleet.remoteLaunch('server-example', 'workspace-example', { promptMarkdown: 'Research the example scope.', targetIds: ['vm:run:tart:example-vm'] });
    assert.equal(requests[2].body.launch.machineId, 'vm:run:tart:example-vm');
    assert.equal('fleetOwnerMachineId' in requests[2].body.launch, false);
    await fleet.remoteLaunch('server-example', 'workspace-example', { promptMarkdown: 'Research the example scope.', targetIds: ['local', 'vm:run:tart:example-vm'] });
    assert.equal(requests.length, 4);
    assert.equal(requests[3].body.launch.machineId, 'local');
    assert.deepEqual(requests[3].body.launch.targetIds, ['local', 'vm:run:tart:example-vm']);
    await assert.rejects(fleet.remoteLaunch('server-example', 'workspace-example', { promptMarkdown: 'Research the example scope.',
      targetIds: ['vm:run:tart:example-vm'], repeatSchedule: { type: 'hourly', interval: 2 } }), /Repeating sessions/);
  } finally { globalThis.fetch = previousFetch; rmSync(root, { recursive: true, force: true }); }
});

test('remote catalog includes the selected app-server provider and target settings', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-catalog-'));
  const fleet = new FleetService(join(root, 'fleet.json'), 'linux');
  fleet.connectedRemoteAppServer = async () => ({ id: 'server-example', name: 'Example',
    url: 'https://worker.example.test', operatorToken: 'synthetic-operator-token' });
  fleet.callRemote = async () => ({ machines: [], browsers: [], workspaceTargets: { 'workspace-example': ['local'] },
    requiredWorkspaceIds: [], optionalWorkspaceIds: [] });
  const paths = [];
  fleet.remoteGet = async (_server, path) => {
    paths.push(path);
    if (path === '/v1/providers') return { controlVersion: 1, defaultProviderId: 'openai-codex', providers: [
      { providerId: 'openai-codex', providerName: 'OpenAI', riskAcknowledged: true, defaultLeadModel: 'gpt-example',
        defaultSubagentModel: null, defaultReasoningEffort: 'high', models: [
          { id: 'gpt-example', name: 'Example model', reasoning: true, effortLevels: ['high'] }] }] };
    if (path === '/v1/workspaces') return { workspaces: [
      { id: 'registry-example', workspaceId: 'workspace-example', name: 'ExampleCo Research', runCount: 0 }] };
    return { result: [] };
  };
  try {
    const catalog = await fleet.remoteCatalog('server-example');
    assert.equal(catalog.providerCatalog.defaultProviderId, 'openai-codex');
    assert.deepEqual(catalog.workspaceTargets['workspace-example'], ['local']);
    assert.ok(paths.includes('/v1/providers'));
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('remote provider limits use the selected app server and validate its response', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-limits-'));
  const fleet = new FleetService(join(root, 'fleet.json'), 'linux');
  const paths = [];
  fleet.connectedRemoteAppServer = async (serverId) => {
    assert.equal(serverId, 'server-example');
    return { id: serverId, name: 'Example', url: 'https://worker.example.test', operatorToken: 'synthetic-operator-token' };
  };
  fleet.remoteGet = async (_server, path) => {
    paths.push(path);
    return { providerId: 'openai-codex', checkedAt: '2033-01-01T00:00:00.000Z', status: 'available', detail: null,
      windows: [{ id: 'session', label: 'Session Limit', usedPercent: 20, resetsAt: null, durationMinutes: 300 }] };
  };
  try {
    const limits = await fleet.remoteProviderUsageLimits('server-example', 'openai-codex', true);
    assert.equal(limits.windows[0].usedPercent, 20);
    assert.deepEqual(paths, ['/v1/providers/limits?provider=openai-codex&refresh=1']);
    await assert.rejects(fleet.remoteProviderUsageLimits('server-example', 'xai', false), /different provider/u);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('remote Add Context runs on the workspace-owning app server', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-expand-'));
  const fleet = new FleetService(join(root, 'fleet.json'), 'linux');
  fleet.callRemote = async (serverId, operation, input) => {
    assert.equal(serverId, 'server-example');
    assert.equal(operation, 'prompt.expand');
    assert.deepEqual(input, { workspaceId: 'workspace-example', promptMarkdown: 'Inspect example.test.',
      phase: 'discovery', provider: { id: 'openai-codex', model: 'gpt-example' } });
    return { phase: 'discovery', promptMarkdown: 'Inspect example.test with workspace context.' };
  };
  try {
    const result = await fleet.remoteExpandPrompt('server-example', 'workspace-example', {
      promptMarkdown: 'Inspect example.test.', phase: 'discovery',
      provider: { id: 'openai-codex', model: 'gpt-example' }
    });
    assert.equal(result.promptMarkdown, 'Inspect example.test with workspace context.');
    await assert.rejects(fleet.remoteExpandPrompt('server-example', '../outside', {
      promptMarkdown: 'Inspect example.test.'
    }), /Invalid workspace ID/u);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('remote New Research suggestions come from the workspace-owning app server', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-suggestions-'));
  const fleet = new FleetService(join(root, 'fleet.json'), 'linux');
  fleet.callRemote = async (serverId, operation, input) => {
    assert.equal(serverId, 'server-example');
    assert.equal(operation, 'suggestion.generate');
    assert.deepEqual(input, { workspaceId: 'workspace-example', phase: 'discovery', refresh: true, incremental: true });
    return { phase: 'discovery', suggestions: ['Inspect example.test.'] };
  };
  try {
    assert.deepEqual(await fleet.remoteGoalSuggestions('server-example', 'workspace-example', 'discovery', true, true),
      { phase: 'discovery', suggestions: ['Inspect example.test.'] });
    await assert.rejects(fleet.remoteGoalSuggestions('server-example', '../outside', 'discovery', false), /Invalid remote research suggestion request/u);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('remote session history requests canonical pages before the current event', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-history-'));
  const fleet = new FleetService(join(root, 'fleet.json'), 'darwin', { async run() { return '[]'; }, launch() {} });
  fleet.connectedRemoteAppServer = async () => ({ id: 'server-example', name: 'Example',
    url: 'https://worker.example.test', operatorToken: 'synthetic-operator-token' });
  const paths = [];
  fleet.remoteGet = async (_server, path) => { paths.push(path); return { result: { events: [] } }; };
  try {
    await fleet.remoteSession('server-example', 'workspace-example', 'session-example');
    await fleet.remoteSession('server-example', 'workspace-example', 'session-example', 'event-example');
    assert.deepEqual(paths, [
      '/v1/workspaces/workspace-example/sessions/session-example/update?tail=true&limit=200&maxBytes=1000000',
      '/v1/workspaces/workspace-example/sessions/session-example/events?beforeEventId=event-example&limit=200&maxBytes=1000000'
    ]);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('remote workspace details use the shared workspace endpoint and reject malformed responses', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-details-'));
  const fleet = new FleetService(join(root, 'fleet.json'), 'darwin', { async run() { return '[]'; }, launch() {} });
  fleet.connectedRemoteAppServer = async () => ({ id: 'server-example', name: 'Example',
    url: 'https://worker.example.test', operatorToken: 'synthetic-operator-token' });
  const paths = [];
  fleet.remoteGet = async (_server, path) => {
    paths.push(path);
    return {
      schemaVersion: 1,
      workspace: { id: 'registry-example', workspaceId: 'workspace-example', name: 'ExampleCo Research', researchProfileId: 'security-research', researchKitId: 'general', runCount: 0 },
      scope: null, researchSubject: '', rules: [],
      memory: { schemaVersion: 4, workspaceId: 'workspace-example', nodes: [], leads: [], findings: [] },
      surfaceMap: { schemaVersion: 1, workspaceId: 'workspace-example', surfaces: [], agents: [] },
      runbooks: [], reports: [],
      targets: []
    };
  };
  try {
    const details = await fleet.remoteWorkspaceDetails('server-example', 'workspace-example');
    assert.equal(details.workspace.name, 'ExampleCo Research');
    assert.deepEqual(paths, ['/v1/workspaces/workspace-example/details']);
    fleet.remoteGet = async () => ({ schemaVersion: 1 });
    await assert.rejects(fleet.remoteWorkspaceDetails('server-example', 'workspace-example'), /invalid workspace details/);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('remote workspace documents use read-only operations and omit runbook host paths', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-documents-'));
  const fleet = new FleetService(join(root, 'fleet.json'), 'darwin', { async run() { return '[]'; }, launch() {} });
  const calls = [];
  fleet.callRemote = async (_serverId, operation, input) => {
    calls.push({ operation, input });
    return operation === 'runbook.get'
      ? { runbookId: 'runbook-example', nbformat: 4, nbformatMinor: 0, language: null, revision: 1, enabledFeatures: [], latestRun: null,
        cells: [{ id: 'cell-example', type: 'code', source: 'echo example', features: [], active: true,
          language: 'sh', executionCount: null, outputs: [], latestRun: null,
          executor: { kind: 'tart-vm', vmName: 'example-vm', timeoutSeconds: 30, argv: [], workspacePath: '/example-host/workspace' } }] }
      : { reportId: 'report-example', content: 'Synthetic report content.' };
  };
  try {
    const runbook = await fleet.remoteWorkspaceDocument('server-example', 'workspace-example', 'runbook', 'runbook-example');
    assert.equal(runbook.document.cells[0].executor.workspacePath, null);
    const report = await fleet.remoteWorkspaceDocument('server-example', 'workspace-example', 'report', 'report-example');
    assert.equal(report.document.content, 'Synthetic report content.');
    assert.deepEqual(calls, [
      { operation: 'runbook.get', input: { workspaceId: 'workspace-example', runbookId: 'runbook-example' } },
      { operation: 'report.get', input: { workspaceId: 'workspace-example', reportId: 'report-example' } }
    ]);
    await assert.rejects(fleet.remoteWorkspaceDocument('server-example', 'workspace-example', 'report', ''), /Invalid remote workspace document request/);
    fleet.callRemote = async () => ({ reportId: 'other-report', content: 'Wrong document.' });
    await assert.rejects(fleet.remoteWorkspaceDocument('server-example', 'workspace-example', 'report', 'report-example'), /invalid workspace document/);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('remote Quick Chats use the versioned host endpoints', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-quick-chat-'));
  const fleet = new FleetService(join(root, 'fleet.json'), 'darwin', { async run() { return '[]'; }, launch() {} });
  fleet.connectedRemoteAppServer = async () => ({ id: 'server-example', name: 'Example',
    url: 'https://worker.example.test', operatorToken: 'synthetic-operator-token' });
  const paths = [];
  fleet.remoteGet = async (_server, path) => {
    paths.push(path);
    return { schemaVersion: 1, chats: [{ sessionId: 'session-example', workspaceId: 'quick-chat-example', title: 'Example chat', promptMarkdown: 'Hello', status: 'completed', updatedAt: '2026-01-01T00:00:00.000Z' }] };
  };
  fleet.remotePost = async (_server, path, body) => {
    paths.push(path);
    if (path === '/v1/quick-chats') {
      assert.equal(body.promptMarkdown, 'Hello');
      return { schemaVersion: 1, sessionId: 'session-new', workspaceId: 'quick-chat-example' };
    }
    assert.deepEqual(body, { workspaceId: 'quick-chat-example', instruction: 'Follow up' });
    return { session: { sessionId: 'session-new' } };
  };
  try {
    const list = await fleet.remoteQuickChats('server-example');
    assert.equal(list.chats[0].title, 'Example chat');
    const started = await fleet.remoteStartQuickChat('server-example', { promptMarkdown: 'Hello' });
    assert.equal(started.sessionId, 'session-new');
    assert.equal((await fleet.remoteContinueQuickChat('server-example', 'quick-chat-example', 'session-new', 'Follow up')).sessionId, 'session-new');
    assert.deepEqual(paths, ['/v1/quick-chats', '/v1/quick-chats', '/v1/sessions/session-new/continuations']);
    await assert.rejects(fleet.remoteStartQuickChat('server-example', { promptMarkdown: '' }), /promptMarkdown/);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('Fleet lets an operator enable run and clone independently for a VM', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-test-'));
  const machines = new Map([['example-base', false]]);
  const launched = [];
  const runner = {
    async run(command, args) {
      assert.equal(command, 'tart');
      if (args[0] === 'list') return JSON.stringify([...machines].map(([Name, Running]) => ({ Name, Running })));
      if (args[0] === 'clone') {
        machines.set(args[2], false);
        return '';
      }
      if (args[0] === 'stop') {
        machines.set(args[1], false);
        return '';
      }
      throw new Error('Unexpected test command.');
    },
    launch(command, args) { launched.push({ command, args }); machines.set(args[1], true); },
  };
  try {
    const path = join(root, 'fleet.json');
    const fleet = new FleetService(path, 'darwin', runner);
    assert.equal((await fleet.state()).machines[0]?.base, false);
    await assert.rejects(fleet.clone('tart:example-base', 'example-copy'), /Can Clone/);
    await fleet.configure({ action: 'set-machine', machineId: 'tart:example-base', base: true,
      canRun: false, canClone: true, privilege: 'elevated', sshUser: 'example' });
    await assert.rejects(fleet.start('tart:example-base'), /Can Run/);
    const cloned = await fleet.clone('tart:example-base', 'example-copy');
    assert.equal(cloned.machines.find((machine) => machine.name === 'example-copy')?.base, false);
    assert.equal(cloned.machines.find((machine) => machine.name === 'example-copy')?.privilege, 'elevated');
    await fleet.start('tart:example-copy');
    assert.deepEqual(launched, [{ command: 'tart', args: ['run', 'example-copy', '--no-graphics'] }]);
    await fleet.configure({ action: 'set-machine', machineId: 'tart:example-copy', sshUser: 'example' });
    await fleet.configure({ action: 'set-machine', machineId: 'tart:example-base', canRun: true, canClone: true, base: false });
    await fleet.configure({ action: 'set-workspace-targets', workspaceId: 'workspace-example',
      targetIds: ['vm:run:tart:example-base', 'vm:clone:tart:example-base'] });
    await fleet.configure({ action: 'select-machine', workspaceId: 'workspace-example', machineId: 'vm:run:tart:example-base' });
    assert.equal((await fleet.state()).lastMachineByWorkspace['workspace-example'], 'vm:run:tart:example-base');
    await assert.rejects(fleet.configure({ action: 'select-machine', workspaceId: 'workspace-example', machineId: 'local' }), /runnable/);
    const owner = { machineId: 'machine-example', sessionId: 'session-example' };
    const [first, again] = await Promise.all([
      fleet.cloneForSession('tart:example-base', owner), fleet.cloneForSession('tart:example-base', owner),
    ]);
    assert.equal(first.id, again.id);
    assert.match(first.name, /^beale-example-base-[a-f0-9]{16}$/);
    assert.deepEqual((await fleet.state()).machines.find((machine) => machine.id === first.id)?.owner, owner);
    assert.equal(new FleetService(path, 'darwin', runner).isSessionClone(first.id), true);
    assert.equal((await fleet.state()).machines.find((machine) => machine.id === 'tart:example-base')?.state, 'stopped');
    await fleet.release(first.id, owner);
    const next = await fleet.cloneForSession('tart:example-base', owner);
    assert.equal(next.name, first.name);
    assert.equal(next.id, first.id);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('Fleet prepares direct VM shell access from the primary and releases its reservation', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-shell-test-'));
  let running = false;
  const calls = [];
  const runner = {
    async run(command, args) {
      calls.push({ command, args });
      if (command === 'tart' && args[0] === 'list') return JSON.stringify([{ Name: 'example-vm', Running: running }]);
      if (command === 'tart' && args[0] === 'ip') return '192.0.2.10\n';
      if (command === 'ssh' && args.at(-1) === 'pwd') return '/Users/example\n';
      throw new Error('Unexpected Fleet command.');
    },
    launch(command, args) { assert.equal(command, 'tart'); running = true; },
  };
  try {
    const fleet = new FleetService(join(root, 'fleet.json'), 'darwin', runner);
    await fleet.configure({ action: 'set-machine', machineId: 'tart:example-vm', canRun: true,
      canClone: false, guestPlatform: 'darwin', sshUser: 'example' });
    await fleet.configure({ action: 'set-workspace-targets', workspaceId: 'workspace-example',
      targetIds: ['vm:run:tart:example-vm'] });
    const prepared = await fleet.prepareSessionTarget('vm:run:tart:example-vm', 'workspace-example', 'session-example');
    assert.equal(prepared.machineId, 'tart:example-vm');
    assert.equal(prepared.guestCwd, '/Users/example');
    assert.equal(prepared.sshArgs.at(-1), 'example@192.0.2.10');
    assert.equal(prepared.sshArgs.some((arg) => arg.startsWith('HostKeyAlias=')), false);
    assert.equal(calls.some(({ command }) => command === 'ssh'), true);
    await fleet.releaseSessionTarget(prepared.machineId, 'session-example');
    assert.equal((await fleet.state()).machines[0].owner, null);
    assert.equal(running, true);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('Fleet resumes a clone target by replacing an unbootable session VM', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-recovery-example-'));
  const machines = new Map([['example-base', false]]);
  const clones = [];
  const runner = {
    async run(command, args) {
      if (command === 'tart' && args[0] === 'list') return JSON.stringify([...machines].map(([Name, Running]) => ({ Name, Running })));
      if (command === 'tart' && args[0] === 'clone') {
        clones.push(args[2]);
        machines.set(args[2], false);
        return '';
      }
      if (command === 'tart' && args[0] === 'stop') { machines.set(args[1], false); return ''; }
      if (command === 'tart' && args[0] === 'ip') return '192.0.2.10\n';
      if (command === 'ssh' && args.at(-1) === 'pwd') return '/Users/example\n';
      throw new Error('Unexpected Fleet command.');
    },
    launch(command, args) {
      assert.equal(command, 'tart');
      if (args[1] === clones[0]) throw new Error('Synthetic VM launch failure.');
      machines.set(args[1], true);
    },
  };
  try {
    const fleet = new FleetService(join(root, 'fleet.json'), 'darwin', runner);
    await fleet.configure({ action: 'set-machine', machineId: 'tart:example-base', base: true,
      canRun: false, canClone: true, sshUser: 'example' });
    await fleet.configure({ action: 'set-workspace-targets', workspaceId: 'workspace-example',
      targetIds: ['vm:clone:tart:example-base'] });
    const prepared = await fleet.prepareSessionTarget('vm:clone:tart:example-base', 'workspace-example', 'session-example');
    assert.equal(clones.length, 2);
    assert.equal(prepared.machineId, `tart:${clones[1]}`);
    assert.equal(prepared.guestCwd, '/Users/example');
    const alias = prepared.sshArgs.find((arg) => arg.startsWith('HostKeyAlias='));
    assert.match(alias, /^HostKeyAlias=beale-vm-[a-f0-9]{24}$/);
    assert.ok(prepared.sshArgs.includes('CheckHostIP=no'));
    assert.ok(prepared.sshArgs.includes('StrictHostKeyChecking=accept-new'));
    assert.ok(prepared.sshArgs.includes(`UserKnownHostsFile=${join(homedir(), '.ssh', 'tart_known_hosts')}`));
    const state = await fleet.state();
    assert.equal(state.machines.find((machine) => machine.id === `tart:${clones[0]}`)?.canRun, false);
    assert.equal(state.machines.find((machine) => machine.id === `tart:${clones[0]}`)?.owner, null);
    assert.equal(state.machines.find((machine) => machine.id === prepared.machineId)?.owner?.sessionId, 'session-example');
    await fleet.releaseSessionTarget(prepared.machineId, 'session-example');
    const resumed = await fleet.prepareSessionTarget('vm:clone:tart:example-base', 'workspace-example', 'session-example');
    assert.equal(resumed.machineId, prepared.machineId);
    assert.ok(resumed.sshArgs.includes(alias));
    assert.equal(clones.length, 2);
    await fleet.releaseSessionTarget(resumed.machineId, 'session-example');
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('Fleet stops retrying a clone when SSH reports a changed host key', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-host-key-example-'));
  const machines = new Map([['example-base', false]]);
  let sshAttempts = 0;
  const runner = {
    async run(command, args) {
      if (command === 'tart' && args[0] === 'list') return JSON.stringify([...machines].map(([Name, Running]) => ({ Name, Running })));
      if (command === 'tart' && args[0] === 'clone') { machines.set(args[2], false); return ''; }
      if (command === 'tart' && args[0] === 'ip') return '192.0.2.10\n';
      if (command === 'ssh' && args.at(-1) === 'pwd') {
        sshAttempts += 1;
        throw Object.assign(new Error('SSH rejected the host'), { stderr: 'WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! Host key verification failed.' });
      }
      throw new Error('Unexpected Fleet command.');
    },
    launch(_command, args) { machines.set(args[1], true); },
  };
  try {
    const fleet = new FleetService(join(root, 'fleet.json'), 'darwin', runner);
    await fleet.configure({ action: 'set-machine', machineId: 'tart:example-base', base: true,
      canRun: false, canClone: true, sshUser: 'example' });
    await fleet.configure({ action: 'set-workspace-targets', workspaceId: 'workspace-example',
      targetIds: ['vm:clone:tart:example-base'] });
    await assert.rejects(fleet.prepareSessionTarget('vm:clone:tart:example-base', 'workspace-example', 'session-example'),
      /SSH host key verification failed/);
    assert.equal(sshAttempts, 3);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('Fleet restores inherited SSH settings when reusing a session clone', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-clone-ssh-example-'));
  const machines = new Map([['example-base', false]]);
  const runner = {
    async run(command, args) {
      if (command === 'tart' && args[0] === 'list') return JSON.stringify([...machines].map(([Name, Running]) => ({ Name, Running })));
      if (command === 'tart' && args[0] === 'clone') { machines.set(args[2], false); return ''; }
      if (command === 'tart' && args[0] === 'ip') return '192.0.2.10\n';
      if (command === 'tart' && args[0] === 'stop') { machines.set(args[1], false); return ''; }
      if (command === 'ssh' && args.at(-1) === 'pwd') return '/Users/example\n';
      throw new Error('Unexpected Fleet command.');
    },
    launch(_command, args) { machines.set(args[1], true); },
  };
  try {
    const fleet = new FleetService(join(root, 'fleet.json'), 'darwin', runner);
    await fleet.configure({ action: 'set-machine', machineId: 'tart:example-base', base: true,
      canRun: false, canClone: true, sshUser: 'example' });
    await fleet.configure({ action: 'set-workspace-targets', workspaceId: 'workspace-example',
      targetIds: ['vm:clone:tart:example-base'] });
    const first = await fleet.prepareSessionTarget('vm:clone:tart:example-base', 'workspace-example', 'session-example');
    await fleet.releaseSessionTarget(first.machineId, 'session-example');
    await fleet.configure({ action: 'set-machine', machineId: first.machineId, sshUser: '' });
    const resumed = await fleet.prepareSessionTarget('vm:clone:tart:example-base', 'workspace-example', 'session-example');
    assert.equal(resumed.machineId, first.machineId);
    assert.equal((await fleet.state()).machines.find((machine) => machine.id === first.machineId)?.sshUser, 'example');
    await fleet.releaseSessionTarget(resumed.machineId, 'session-example');
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('Fleet replaces a stopped run target with an allowed clone target', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-run-recovery-example-'));
  const machines = new Map([['example-vm', false], ['example-base', false]]);
  let cloneName = null;
  const runner = {
    async run(command, args) {
      if (command === 'tart' && args[0] === 'list') return JSON.stringify([...machines].map(([Name, Running]) => ({ Name, Running })));
      if (command === 'tart' && args[0] === 'clone') { cloneName = args[2]; machines.set(cloneName, false); return ''; }
      if (command === 'tart' && args[0] === 'ip') return '192.0.2.10\n';
      if (command === 'tart' && args[0] === 'stop') { machines.set(args[1], false); return ''; }
      if (command === 'ssh' && args.at(-1) === 'pwd') return '/Users/example\n';
      throw new Error('Unexpected Fleet command.');
    },
    launch(_command, args) {
      if (args[1] === 'example-vm') throw new Error('Synthetic VM launch failure.');
      machines.set(args[1], true);
    },
  };
  try {
    const fleet = new FleetService(join(root, 'fleet.json'), 'darwin', runner);
    await fleet.configure({ action: 'set-machine', machineId: 'tart:example-vm', canRun: true,
      canClone: false, sshUser: 'example' });
    await fleet.configure({ action: 'set-machine', machineId: 'tart:example-base', base: true,
      canRun: false, canClone: true, sshUser: 'example' });
    await fleet.configure({ action: 'set-workspace-targets', workspaceId: 'workspace-example',
      targetIds: ['vm:run:tart:example-vm', 'vm:clone:tart:example-base'] });
    const prepared = await fleet.prepareSessionTarget('vm:run:tart:example-vm', 'workspace-example', 'session-example');
    assert.equal(prepared.machineId, `tart:${cloneName}`);
    assert.equal(prepared.guestCwd, '/Users/example');
    assert.equal((await fleet.state()).machines.find((machine) => machine.id === 'tart:example-vm')?.owner, null);
    await fleet.releaseSessionTarget(prepared.machineId, 'session-example');
    await fleet.configure({ action: 'set-machine', machineId: 'tart:example-vm', sshUser: '' });
    const recovered = await fleet.prepareSessionTarget('vm:run:tart:example-vm', 'workspace-example', 'session-ssh-example');
    assert.notEqual(recovered.machineId, prepared.machineId);
    assert.equal(recovered.guestCwd, '/Users/example');
    await fleet.releaseSessionTarget(recovered.machineId, 'session-ssh-example');
    await fleet.configure({ action: 'set-workspace-targets', workspaceId: 'workspace-example',
      targetIds: ['vm:run:tart:example-vm'] });
    await assert.rejects(fleet.prepareSessionTarget('vm:run:tart:example-vm', 'workspace-example', 'session-other'),
      /SSH user is not configured/);
    assert.equal((await fleet.state()).machines.filter((machine) => machine.name.startsWith('beale-')).length, 2);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('Fleet tests draft SSH settings without saving them or starting a base', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-ssh-test-'));
  const machines = new Map([['example-base', false], ['example-worker', true]]);
  const calls = [];
  let sshError = null;
  let rejectAgent = false;
  const runner = {
    async run(command, args) {
      calls.push({ command, args });
      if (command === 'tart' && args[0] === 'list') return JSON.stringify([...machines].map(([Name, Running]) => ({ Name, Running })));
      if (command === 'tart' && args[0] === 'ip') {
        if (rejectAgent && args.includes('agent')) throw new Error('guest agent unavailable');
        return '192.0.2.10\n';
      }
      if (command === 'ssh') {
        if (sshError) throw sshError;
        return '';
      }
      throw new Error('Unexpected test command.');
    },
    launch() { throw new Error('SSH test must not start a VM.'); },
  };
  try {
    const path = join(root, 'fleet.json');
    const fleet = new FleetService(path, 'darwin', runner);
    await fleet.configure({ action: 'set-machine', machineId: 'tart:example-base', base: true, sshUser: 'saved' });
    assert.match((await fleet.testSsh({ machineId: 'tart:example-base', sshHost: '', sshUser: 'draft' })).message, /Clone and start a worker/);
    const before = readFileSync(path, 'utf8');
    const result = await fleet.testSsh({ machineId: 'tart:example-worker', sshHost: '', sshUser: 'example', sshIdentityFile: '~/example-key', sshKnownHostsFile: join(root, 'known_hosts') });
    assert.equal(result.success, true);
    const ssh = calls.find((call) => call.command === 'ssh');
    assert.ok(ssh);
    assert.ok(ssh.args.includes('example@192.0.2.10'));
    assert.ok(calls.some((call) => call.command === 'tart' && call.args.includes('agent')));
    assert.ok(ssh.args.includes(`UserKnownHostsFile=${join(root, 'known_hosts')}`));
    assert.ok(ssh.args.includes('StrictHostKeyChecking=accept-new'));
    assert.ok(ssh.args.some((arg) => arg.endsWith('example-key')));
    rejectAgent = true;
    assert.equal((await fleet.testSsh({ machineId: 'tart:example-worker', sshHost: '', sshUser: 'example' })).success, true);
    assert.ok(calls.some((call) => call.command === 'tart' && call.args.includes('--wait') && !call.args.includes('agent')));
    sshError = new Error('synthetic SSH failure');
    assert.deepEqual(await fleet.testSsh({ machineId: 'tart:example-worker', sshHost: '192.0.2.10', sshUser: 'example' }), {
      success: false,
      message: 'SSH failed. Check the guest user, identity file, known-hosts file, and Remote Login. These settings have not been saved. Address checked: 192.0.2.10.',
    });
    sshError = Object.assign(new Error('synthetic SSH failure'), { stderr: 'Host key verification failed.' });
    assert.match((await fleet.testSsh({ machineId: 'tart:example-worker', sshHost: '192.0.2.10', sshUser: 'example' })).message, /SSH detail: Host key verification failed\./);
    sshError = Object.assign(new Error('synthetic SSH failure'), { stderr: 'ssh: connect to host 192.0.2.10 port 22: No route to host' });
    assert.match((await fleet.testSsh({ machineId: 'tart:example-worker', sshHost: '192.0.2.10', sshUser: 'example' })).message, /app-server could not reach this VM/);
    assert.equal(readFileSync(path, 'utf8'), before);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('Fleet leases persist by machine and session and reject conflicting control', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-owner-test-'));
  let running = false;
  const runner = {
    async run(command, args) {
      assert.equal(command, 'tart');
      if (args[0] === 'list') return JSON.stringify([{ Name: 'example-worker', Running: running }, { Name: 'example-second', Running: false }]);
      if (args[0] === 'stop') { running = false; return ''; }
      throw new Error('Unexpected Fleet test command.');
    },
    launch() { running = true; },
  };
  try {
    const path = join(root, 'fleet.json');
    const fleet = new FleetService(path, 'darwin', runner);
    const owner = { machineId: 'machine-example', sessionId: 'session-example', workspaceId: 'workspace-example' };
    await fleet.reserve('tart:example-worker', owner);
    assert.deepEqual((await new FleetService(path, 'darwin', runner).state()).machines.find((machine) => machine.id === 'tart:example-worker').owner,
      { machineId: owner.machineId, sessionId: owner.sessionId });
    await assert.rejects(fleet.reserve('tart:example-worker', { machineId: 'machine-other', sessionId: 'session-example' }), /already reserved/);
    await assert.rejects(fleet.reserve('tart:example-second', { machineId: 'machine-other', sessionId: 'session-other', workspaceId: 'workspace-example' }), /already staged/);
    await assert.rejects(fleet.start('tart:example-worker'), /reserved/);
    await fleet.start('tart:example-worker', owner);
    await assert.rejects(fleet.stop('tart:example-worker', { machineId: owner.machineId, sessionId: 'session-other' }), /reserved/);
    await fleet.stop('tart:example-worker', owner);
    await assert.rejects(fleet.release('tart:example-worker', { machineId: 'machine-other', sessionId: owner.sessionId }), /Only the owning/);
    await fleet.release('tart:example-worker', owner);
    assert.equal((await fleet.state()).machines.find((machine) => machine.id === 'tart:example-worker').owner, null);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('Fleet includes bases from reachable app servers when another peer fails', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-remote-bases-'));
  try {
    const path = join(root, 'fleet.json');
    writeFileSync(path, JSON.stringify({ version: 2, machineId: 'machine-example', role: 'primary', enabled: true,
      appServers: {
        'server-good': { name: 'Example remote', url: 'https://good.example.ts.net:47174', operatorToken: 'synthetic-operator-token' },
        'server-bad': { name: 'Offline remote', url: 'https://bad.example.ts.net:47174', operatorToken: 'synthetic-operator-token' },
      }
    }));
    const fleet = new FleetService(path, 'darwin');
    const calls = [];
    fleet.callRemote = async (serverId, operation, _input, timeoutMs) => {
      calls.push({ serverId, operation, timeoutMs });
      if (serverId === 'server-bad') throw new Error('Synthetic peer unavailable');
      return { enabled: true, role: 'primary', available: true, machines: [
        { id: 'tart:example-base', name: 'example-base', base: true, state: 'stopped', sshConfigured: true },
        { id: 'tart:example-worker', name: 'example-worker', base: false, state: 'stopped', sshConfigured: true },
      ] };
    };
    assert.deepEqual((await fleet.remoteMachines()).map(({ id, name }) => ({ id, name })), [
      { id: 'remote:server-good:tart:example-base', name: 'Example remote / example-base' }
    ]);
    assert.ok(calls.every((call) => call.operation === 'fleet.state' && call.timeoutMs === 12_000));
    fleet.callRemote = async () => { throw new Error('Synthetic peers unavailable'); };
    await assert.rejects(fleet.remoteMachines(), /Could not load VM inventory/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('Fleet sandbox targets create one keyed sandbox per session and delete it on release', async () => {
  // The SDK's in-process memory provider stands in for a cloud provider; one instance keeps its sandboxes.
  const sdk = createRequire(new URL('../../packages/research-agent/package.json', import.meta.url));
  const { connectSandbox } = await import(sdk.resolve('@opencoredev/sandbox-sdk'));
  const { memory } = await import(sdk.resolve('@opencoredev/sandbox-sdk/memory'));
  const provider = memory();
  const root = mkdtempSync(join(tmpdir(), 'beale-fleet-sandbox-example-'));
  const fleet = new FleetService(join(root, 'fleet.json'), 'linux', undefined, async () => provider);
  try {
    await assert.rejects(fleet.configure({ action: 'set-workspace-targets', workspaceId: 'workspace-example', targetIds: ['sandbox:local'] }),
      /sandbox target is unavailable/);
    const enabled = await fleet.configure({ action: 'set-sandbox-enabled', provider: 'local', enabled: true });
    assert.deepEqual(enabled.sandboxes.find((sandbox) => sandbox.provider === 'local'),
      { provider: 'local', name: 'Docker', enabled: true, credentialsConfigured: true });
    await assert.rejects(fleet.configure({ action: 'set-sandbox-enabled', provider: 'example', enabled: true }), /supported Fleet sandbox provider/);
    await fleet.configure({ action: 'set-workspace-targets', workspaceId: 'workspace-example', targetIds: ['local', 'sandbox:local'] });
    await assert.rejects(fleet.prepareSessionTarget('sandbox:e2b', 'workspace-example', 'session-example'), /sandbox target is unavailable/);

    const target = await fleet.prepareSessionTarget('sandbox:local', 'workspace-example', 'session-example');
    assert.equal(target.sandbox.provider, 'local');
    assert.equal(target.machineId, `sandbox:local:${target.sandbox.sandboxId}`);
    assert.deepEqual({ guestPlatform: target.guestPlatform, guestCwd: target.guestCwd }, { guestPlatform: 'linux', guestCwd: '/workspace' });
    const retried = await fleet.prepareSessionTarget('sandbox:local', 'workspace-example', 'session-example');
    assert.equal(retried.machineId, target.machineId);
    const other = await fleet.prepareSessionTarget('sandbox:local', 'workspace-example', 'session-other');
    assert.notEqual(other.machineId, target.machineId);
    assert.ok(fleet.isReservedBy(target.machineId, { machineId: fleet.machineId(), sessionId: 'session-example' }));
    await assert.rejects(fleet.configure({ action: 'set-sandbox-enabled', provider: 'local', enabled: false }), /Release active sandbox sessions/);

    await fleet.releaseSessionTarget(target.machineId, 'session-example');
    assert.equal(fleet.isReservedBy(target.machineId, { machineId: fleet.machineId(), sessionId: 'session-example' }), false);
    await assert.rejects(connectSandbox(provider, target.sandbox.sandboxId), { code: 'not_found' });
    await fleet.releaseSessionTarget(other.machineId, 'session-other');
    await fleet.configure({ action: 'set-sandbox-enabled', provider: 'local', enabled: false });
    assert.deepEqual(JSON.parse(readFileSync(join(root, 'fleet.json'), 'utf8')).owners, {});
  } finally { rmSync(root, { recursive: true, force: true }); }
});
