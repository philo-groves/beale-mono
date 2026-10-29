import assert from "node:assert/strict";
import test from "node:test";
import {
  BEALE_APP_SERVER_CAPABILITIES,
  BEALE_APP_SERVER_CONTRACT_TIMESTAMP,
  BEALE_APP_SERVER_CONTROL_VERSION,
  BEALE_APP_SERVER_MAX_REPLAY_BYTES,
  BEALE_APP_SERVER_MAX_REPLAY_FRAMES,
  BEALE_APP_SERVER_PROVIDERS_PATH,
  BEALE_APP_SERVER_SESSIONS_PATH,
  BEALE_APP_SERVER_SERVER_PATH,
  BEALE_APP_SERVER_SHUTDOWN_PATH,
  BEALE_APP_SERVER_WORKSPACES_PATH,
  decodeBealeAppServerDescriptor,
  decodeBealeAppServerErrorResponse,
  decodeBealeAppServerProviderCatalog,
  decodeBealeWorkspaceResearchTrack,
  decodeBealeWorkspaceResearchTrackRequest,
  decodeBealeAppServerSessionAttachResult,
  decodeBealeAppServerSessionContinuationRequest,
  decodeBealeAppServerSessionStartResult,
  decodeBealeAppServerSessionStopResult,
  decodeBealeAppServerShutdownResult,
  decodeAppServerProtocolEnvelope,
  decodeAppServerSessionLaunchRequest,
  decodeWorkspaceProjectRequest,
  decodeClaimBoardTransitionRequest,
  decodeFleetSshTestInput,
  decodeFleetAppServerStatuses,
  decodeFleetRemoteResearchInput,
  decodeFleetRemotePromptExpansionInput,
  decodeBealeAppServerSessionControlRequest,
  decodeBealeAppServerSessionControlResult,
  decodeGeneratedResearchGoalSuggestions,
  decodeAppServerServerMessage,
  APP_SERVER_PROTOCOL_OPERATIONS,
  APP_SERVER_CONTRACT_VERSION,
  APP_SERVER_PROTOCOL_VERSION,
  APP_SERVER_SESSION_LAUNCH_VERSION,
  appServerProtocolFailure,
  appServerProtocolDescriptor,
  appServerProtocolSuccess,
  parseAppServerProtocolArguments,
} from "../packages/app-server-runtime/dist/protocol.js";

test('research-track control decodes a bounded workspace progress snapshot', () => {
  assert.deepEqual(decodeBealeWorkspaceResearchTrackRequest({ workspaceId: ' workspace-example ', action: 'start' }),
    { workspaceId: 'workspace-example', action: 'start' });
  const configuration = { provider: { id: 'openai-codex', model: 'example-model', reasoningEffort: 'high' },
    targetIds: ['local', 'browser:example'], collaboration: { subagentMode: 'advanced' } };
  assert.deepEqual(decodeBealeWorkspaceResearchTrackRequest({ workspaceId: 'workspace-example', action: 'start', configuration }),
    { workspaceId: 'workspace-example', action: 'start', configuration });
  assert.equal(decodeBealeWorkspaceResearchTrackRequest({ workspaceId: 'workspace-example', action: 'start',
    configuration: { ...configuration, guardianEnabled: false } }).configuration.guardianEnabled, false);
  assert.throws(() => decodeBealeWorkspaceResearchTrackRequest({ workspaceId: 'workspace-example', action: 'start',
    configuration: { ...configuration, guardianEnabled: 'disabled' } }), /Guardian must be enabled or disabled/);
  const offConfiguration = { ...configuration, provider: { ...configuration.provider, reasoningEffort: 'off' } };
  assert.deepEqual(decodeBealeWorkspaceResearchTrackRequest({ workspaceId: 'workspace-example', action: 'start',
    configuration: offConfiguration }).configuration, offConfiguration);
  assert.equal(decodeAppServerSessionLaunchRequest({ launchVersion: APP_SERVER_SESSION_LAUNCH_VERSION,
    launch: { workspaceId: 'workspace-example', promptMarkdown: 'Synthetic research instruction.',
      provider: offConfiguration.provider, targetIds: ['local'], machineId: 'local' } }).launch.provider.reasoningEffort,
    'off');
  assert.throws(() => decodeBealeWorkspaceResearchTrackRequest({ workspaceId: 'workspace-example', action: 'resume', configuration }),
    /Invalid workspace research track configuration: only start accepts settings/);
  assert.throws(() => decodeBealeWorkspaceResearchTrackRequest({ workspaceId: 'workspace-example', action: 'start',
    configuration: { ...configuration, targetIds: ['local', 'local'] } }), /choose 1 to 16 distinct targets/);
  assert.throws(() => decodeBealeWorkspaceResearchTrackRequest({ workspaceId: 'workspace-example', action: 'start',
    configuration: { ...configuration, provider: { ...configuration.provider, reasoningEffort: '' } } }),
    /choose a reasoning effort/);
  assert.throws(() => decodeBealeWorkspaceResearchTrackRequest({ workspaceId: 'workspace-example', action: 'reset' }),
    /Invalid workspace research track request/);
  const snapshot = {
    id: 'track-example', workspaceId: 'workspace-example', status: 'running', pass: 1,
    scopeId: 'scope-example', provider: { id: 'openai-codex' }, guardianEnabled: true, targetIds: ['local'],
    activeSessionId: 'session-example', activeSessionPhase: 'preparing',
    activeSurfaceId: 'surface-example', epochCount: 1,
    lastError: null, createdAt: '2026-01-01T00:00:00.000Z', updatedAt: '2026-01-01T00:00:00.000Z',
    surfaces: [{ surfaceId: 'surface-example', name: 'Example service', status: 'active', impact: 'high',
      theory: 'A privileged trust boundary may be crossed.', nextAction: 'Test the boundary.',
      evidenceRefs: ['memory:memory-example'], actionabilityScore: 70,
      priority: { baseScore: 115, actionability: 70, impactAdjustment: 30,
        testableAngleBonus: 15, targetValidationPenalty: 0, noProgressPenalty: 0 },
      angles: [{ name: 'Example authority path', status: 'open',
        vector: { attackerControl: 'Control an example input', trustBoundary: 'Example authority boundary',
          failureMode: 'Possible policy mismatch', unauthorizedEffect: 'Access an example protected asset' },
        proofQuestion: 'Can the input cross the example authority boundary?',
        evidenceRefs: [], nextAction: 'Test a distinct input.' }],
      validation: { targetDependent: true, environment: 'device', reason: 'Observed on a synthetic test device.' },
      epochs: 1, noProgressEpochs: 0, revisits: 0,
      revisitAfter: null, lastWorkedAt: '2026-01-01T00:00:00.000Z', updatedAt: '2026-01-01T00:00:00.000Z' }],
  };
  assert.deepEqual(decodeBealeWorkspaceResearchTrack(snapshot), snapshot);
  assert.equal(decodeBealeWorkspaceResearchTrack(null), null);
  assert.throws(() => decodeBealeWorkspaceResearchTrack({ ...snapshot, guardianEnabled: 'enabled' }),
    /invalid workspace research track/);
  assert.throws(() => decodeBealeWorkspaceResearchTrack({ ...snapshot, activeSessionPhase: 'unknown' }),
    /invalid workspace research track/);
  assert.throws(() => decodeBealeWorkspaceResearchTrack({ ...snapshot,
    surfaces: [{ ...snapshot.surfaces[0], evidenceRefs: [12] }] }), /invalid workspace research track/);
  assert.throws(() => decodeBealeWorkspaceResearchTrack({ ...snapshot,
    surfaces: [{ ...snapshot.surfaces[0], actionabilityScore: 101 }] }), /invalid workspace research track/);
  assert.throws(() => decodeBealeWorkspaceResearchTrack({ ...snapshot,
    surfaces: [{ ...snapshot.surfaces[0], priority: { ...snapshot.surfaces[0].priority, baseScore: 116 } }] }),
  /invalid workspace research track/);
  assert.throws(() => decodeBealeWorkspaceResearchTrack({ ...snapshot,
    surfaces: [{ ...snapshot.surfaces[0], angles: [{ ...snapshot.surfaces[0].angles[0], status: 'unverified' }] }] }),
  /invalid workspace research track/);
  assert.throws(() => decodeBealeWorkspaceResearchTrack({ ...snapshot,
    surfaces: [{ ...snapshot.surfaces[0], angles: [{ ...snapshot.surfaces[0].angles[0], status: 'supported' }] }] }),
  /invalid workspace research track/);
  assert.throws(() => decodeBealeWorkspaceResearchTrack({ ...snapshot,
    surfaces: [{ ...snapshot.surfaces[0], angles: [{ ...snapshot.surfaces[0].angles[0],
      vector: { ...snapshot.surfaces[0].angles[0].vector, trustBoundary: '' } }] }] }),
  /invalid workspace research track/);
  assert.equal(decodeBealeWorkspaceResearchTrack({ ...snapshot,
    surfaces: [{ ...snapshot.surfaces[0], angles: [{ ...snapshot.surfaces[0].angles[0], vector: undefined }] }] })
    .surfaces[0].angles[0].name, 'Example authority path');
  assert.throws(() => decodeBealeWorkspaceResearchTrack({ ...snapshot,
    surfaces: [{ ...snapshot.surfaces[0], validation: { targetDependent: true, environment: 'unknown', reason: '' } }] }), /invalid workspace research track/);
});

test('research suggestion responses reject malformed lists and the wrong lane', () => {
  const valid = { phase: 'discovery', suggestions: ['Inspect an example component.'] };
  assert.deepEqual(decodeGeneratedResearchGoalSuggestions(valid, 'discovery'), valid);
  for (const invalid of [null, { phase: 'discovery' }, { phase: 'discovery', suggestions: null },
    { phase: 'discovery', suggestions: [null] }, { phase: 'reporting', suggestions: [] }]) {
    assert.throws(() => decodeGeneratedResearchGoalSuggestions(invalid, 'discovery'), /invalid research suggestion response/);
  }
});

test('research workspace operations require explicit revisions for canonical imports', () => {
  assert.deepEqual(decodeWorkspaceProjectRequest({ workspaceId: 'workspace-example', action: 'sync' }), { workspaceId: 'workspace-example', action: 'sync' });
  assert.deepEqual(decodeWorkspaceProjectRequest({ workspaceId: 'workspace-example', action: 'export' }), { workspaceId: 'workspace-example', action: 'export' });
  assert.deepEqual(decodeWorkspaceProjectRequest({ workspaceId: 'workspace-example', action: 'rebuild-index' }), { workspaceId: 'workspace-example', action: 'rebuild-index' });
  assert.deepEqual(decodeWorkspaceProjectRequest({ workspaceId: 'workspace-example', action: 'release-index' }), { workspaceId: 'workspace-example', action: 'release-index' });
  assert.deepEqual(decodeWorkspaceProjectRequest({ workspaceId: 'workspace-example', action: 'import', path: 'claims/example.json', expectedRevision: 2 }), { workspaceId: 'workspace-example', action: 'import', path: 'claims/example.json', expectedRevision: 2 });
  for (const input of [null, { workspaceId: '', action: 'status' }, { workspaceId: 'workspace-example', action: 'reset' }, { workspaceId: 'workspace-example', action: 'checkpoint' }, { workspaceId: 'workspace-example', action: 'repair-preview' }, { workspaceId: 'workspace-example', action: 'repair', fingerprint: 'a'.repeat(64) }, { workspaceId: 'workspace-example', action: 'import', path: 'claims/example.json' }, { workspaceId: 'workspace-example', action: 'import', path: 'claims/example.json', expectedRevision: 1.5 }]) assert.throws(() => decodeWorkspaceProjectRequest(input));
});

test('finding board transitions accept Lead and reject Reported', () => {
  const input = { workspaceId: 'workspace-example', claimId: 'claim-example', expectedRevision: 2, targetMaturity: 'lead' };
  assert.deepEqual(decodeClaimBoardTransitionRequest(input), input);
  for (const invalid of [
    { ...input, workspaceId: '' },
    { ...input, claimId: '' },
    { ...input, expectedRevision: 0 },
    { ...input, expectedRevision: 1.5 },
    { ...input, targetMaturity: 'proposed' },
    { ...input, targetMaturity: 'reported' },
  ]) assert.throws(() => decodeClaimBoardTransitionRequest(invalid));
});

test('Fleet SSH test accepts draft settings and rejects malformed input', () => {
  const input = { machineId: 'tart:example-worker', sshHost: '', sshUser: 'example', sshIdentityFile: '~/example-key' };
  assert.deepEqual(decodeFleetSshTestInput(input), input);
  assert.throws(() => decodeFleetSshTestInput({ ...input, sshKnownHostsFile: 4 }), /Invalid Fleet SSH/);
});

test('Fleet app-server statuses are typed and contain no connection credentials', () => {
  const statuses = [{ id: 'server-example', state: 'issue', message: 'Incompatible control contract.' }];
  assert.deepEqual(decodeFleetAppServerStatuses(statuses), statuses);
  assert.deepEqual(decodeFleetAppServerStatuses([{ ...statuses[0], operatorToken: 'synthetic-token' }]), statuses);
  assert.throws(() => decodeFleetAppServerStatuses([{ ...statuses[0], state: 'unknown' }]), /Invalid Fleet app-server statuses/);
  assert.throws(() => decodeFleetAppServerStatuses([{ ...statuses[0], operatorToken: 'synthetic-token', message: null }]), /Invalid Fleet app-server statuses/);
});

test("protocol envelopes are versioned, correlated, and strictly decoded", () => {
  const success = appServerProtocolSuccess("protocol.describe", { available: true }, "request-1");
  assert.deepEqual(decodeAppServerProtocolEnvelope(success), success);

  const failure = appServerProtocolFailure("protocol.describe", "unavailable", "Protocol discovery is unavailable.");
  assert.deepEqual(decodeAppServerProtocolEnvelope(failure), failure);
  assert.throws(
    () => decodeAppServerProtocolEnvelope({ ...success, protocolVersion: 2 }),
    /Invalid or unsupported/,
  );
});

test("protocol describe omits removed workflow operations", () => {
  const descriptor = appServerProtocolDescriptor();
  assert.deepEqual(descriptor.operations, APP_SERVER_PROTOCOL_OPERATIONS);
  assert.equal(descriptor.contractVersion, APP_SERVER_CONTRACT_VERSION);
  assert.ok(!descriptor.capabilities.some((capability) => capability.startsWith('session.workflows.')));
  assert.ok(!descriptor.operations.some((operation) => operation.startsWith('workflow.')));
  assert.match(descriptor.runtime.buildId, /^[a-f0-9]{24}$/);
  assert.equal(descriptor.schemas.memorySummary, 14);
  assert.equal(descriptor.schemas.finding, 7);
  assert.equal(descriptor.schemas.campaignGraph, 5);
  assert.equal(descriptor.schemas.goalSuggestions, 1);
  assert.ok(descriptor.capabilities.includes("knowledge.findings"));
  assert.ok(descriptor.capabilities.includes("knowledge.campaign_graph"));
  assert.ok(!descriptor.capabilities.includes("knowledge.campaign_tracks.v2"));
  assert.ok(descriptor.capabilities.includes("knowledge.claims.v2"));
  assert.ok(descriptor.capabilities.includes("knowledge.claim_security_tracking"));
  assert.ok(descriptor.capabilities.includes("knowledge.claim_deduplication"));
  assert.ok(descriptor.capabilities.includes("knowledge.claim_board_transition"));
  assert.ok(descriptor.capabilities.includes("knowledge.history_deduplication"));
  assert.ok(descriptor.capabilities.includes("session.bounded_reads"));
  assert.ok(descriptor.capabilities.includes("session.targeted_details"));
  assert.ok(descriptor.capabilities.includes("session.event_identity"));
  assert.ok(descriptor.capabilities.includes("workspace.goal-suggestions.v1"));
  assert.ok(descriptor.capabilities.includes("workspace.prompt-expansion.v1"));
  assert.ok(descriptor.capabilities.includes("workspace.state"));
  assert.ok(descriptor.capabilities.includes("registry.state"));
  assert.ok(descriptor.capabilities.includes("registry.workspace_sync.v2"));
  assert.ok(!APP_SERVER_PROTOCOL_OPERATIONS.some((operation) => operation.startsWith("topic.") || operation.startsWith("channel.")));
  assert.ok(APP_SERVER_PROTOCOL_OPERATIONS.includes("suggestion.generate"));
  assert.ok(APP_SERVER_PROTOCOL_OPERATIONS.includes("suggestion.select"));
  assert.ok(APP_SERVER_PROTOCOL_OPERATIONS.includes("suggestion.steering"));
  assert.ok(APP_SERVER_PROTOCOL_OPERATIONS.includes("prompt.expand"));
  assert.ok(APP_SERVER_PROTOCOL_OPERATIONS.includes("report.list"));
  assert.ok(APP_SERVER_PROTOCOL_OPERATIONS.includes("report.revise_content"));
  assert.ok(APP_SERVER_PROTOCOL_OPERATIONS.includes("report.update_triage_status"));
  assert.ok(APP_SERVER_PROTOCOL_OPERATIONS.includes("report.replace_recording"));
  assert.ok(APP_SERVER_PROTOCOL_OPERATIONS.includes("claim.mark_duplicate"));
  assert.ok(APP_SERVER_PROTOCOL_OPERATIONS.includes("claim.undo_duplicate"));
  assert.ok(APP_SERVER_PROTOCOL_OPERATIONS.includes("claim.board_transition"));
  assert.ok(APP_SERVER_PROTOCOL_OPERATIONS.includes("history.mark_duplicate"));
  assert.ok(APP_SERVER_PROTOCOL_OPERATIONS.includes("history.undo_duplicate"));
  assert.ok(APP_SERVER_PROTOCOL_OPERATIONS.includes("workspace.state"));
  assert.ok(APP_SERVER_PROTOCOL_OPERATIONS.includes("registry.state"));
  assert.ok(APP_SERVER_PROTOCOL_OPERATIONS.includes("research.tools.list"));
  assert.ok(APP_SERVER_PROTOCOL_OPERATIONS.includes("research.tools.read"));
  assert.ok(APP_SERVER_PROTOCOL_OPERATIONS.includes("research.tools.mutate"));
  assert.ok(!APP_SERVER_PROTOCOL_OPERATIONS.some((operation) => operation.startsWith("investigation.")));
  assert.ok(descriptor.capabilities.includes("knowledge.report-content-revise.v1"));
  assert.ok(descriptor.capabilities.includes("knowledge.report-triage-status.v1"));
  assert.ok(descriptor.capabilities.includes("knowledge.report-recording-replace.v1"));
  assert.ok(descriptor.capabilities.includes("knowledge.report-list.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("workspace.goal-suggestions.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("session.startup-recovery.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("session.openai-fast-mode.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("session.event-identity.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("session.continuation.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("session.typed-launch.v3"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("fleet.multi-target-session.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("workspace.prompt-expansion.v1"));
  assert.ok(!BEALE_APP_SERVER_CAPABILITIES.includes("knowledge.campaign-tracks.v2"));
  assert.ok(!BEALE_APP_SERVER_CAPABILITIES.includes("maintenance.repository-consolidation.v1"));
  assert.ok(!APP_SERVER_PROTOCOL_OPERATIONS.some((operation) => operation.startsWith("dreaming.") || operation.startsWith("maintenance.")));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("source.clone-modes.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("memory.notifications.v3"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("knowledge.report-list.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("knowledge.claim-deduplication.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("knowledge.claim-board-transition.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("knowledge.history-deduplication.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("workspace.state.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("workspace.research-subject-mutation.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("workspace.research-project.v3"));
  assert.ok(!BEALE_APP_SERVER_CAPABILITIES.includes("workspace.checkpoint-repair.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("session.openai-daybreak-blue.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("registry.state.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("registry.workspace-sync.v2"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("session.http-control.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("research.tools.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("runbook.delegated-cell-runtime.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("runbook.host-cell-runtime.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("knowledge.claim-evidence-validation.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("knowledge.claim-sql-pagination.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("runbook.execution-snapshots.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("tart.guest-helper-repair.v1"));
  assert.equal(descriptor.transports.websocket.path, "/v1/session");
  assert.equal(descriptor.transports.appServer.path, "/v1/operations");
  assert.equal(descriptor.transports.appServer.authentication, "operator-bearer");
  assert.equal(descriptor.transports.websocket.framing, "json-message");
  assert.equal(descriptor.transports.websocket.errors, "protocol-error-message");
  assert.equal(descriptor.transports.websocket.correlation, "request-id");
  assert.ok(descriptor.transports.websocket.capabilities.includes("session.event-identity.v1"));
});

test("protocol argument and WebSocket DTO decoders share correlation and error semantics", () => {
  assert.deepEqual(
    parseAppServerProtocolArguments(["protocol", "describe", "--request-id", "request-2", "--json"]),
    { args: ["protocol", "describe", "--json"], requestId: "request-2" },
  );
  assert.throws(
    () => parseAppServerProtocolArguments(["--request-id", "one", "--request-id", "two"]),
    /only be provided once/,
  );
  assert.deepEqual(decodeAppServerServerMessage({
    protocolVersion: 1,
    type: "protocol.error",
    sessionId: "session-1",
    requestId: "request-2",
    error: { code: "invalid_message", message: "Bad message.", retryable: false },
    message: "Bad message.",
  }).error, { code: "invalid_message", message: "Bad message.", retryable: false });
});

test("HTTP session control DTOs are bounded and correlated", () => {
  assert.deepEqual(
    decodeBealeAppServerSessionControlRequest({ type: "steer", instruction: "Inspect the alternate parser." }),
    { type: "steer", instruction: "Inspect the alternate parser." },
  );
  assert.deepEqual(
    decodeBealeAppServerSessionControlResult({
      controlVersion: 1,
      accepted: true,
      sessionId: "session-1",
      requestId: "request-1",
      type: "steer",
    }),
    {
      controlVersion: 1,
      accepted: true,
      sessionId: "session-1",
      requestId: "request-1",
      type: "steer",
    },
  );
  assert.throws(
    () => decodeBealeAppServerSessionControlRequest({ type: "steer", instruction: "" }),
    /instruction/,
  );
  assert.throws(
    () => decodeBealeAppServerSessionControlResult({
      controlVersion: 1,
      accepted: true,
      sessionId: "session-1",
      requestId: "",
      type: "steer",
    }),
    /control response/,
  );
});

test("the typed session launch carries OpenAI Fast mode and Daybreak Blue", () => {
  const request = {
    launchVersion: APP_SERVER_SESSION_LAUNCH_VERSION,
    launch: {
      workspaceId: "workspace-example",
      promptMarkdown: "Inspect the parser boundary.",
      provider: { id: "openai-codex", model: "gpt-5.6-sol", fastMode: true, daybreakBlue: true },
    },
  };
  assert.deepEqual(decodeAppServerSessionLaunchRequest(request), request);
  assert.throws(
    () => decodeAppServerSessionLaunchRequest({
      ...request,
      launch: { ...request.launch, provider: { ...request.launch.provider, fastMode: "yes" } },
    }),
    /fastMode must be a boolean/,
  );
  assert.throws(
    () => decodeAppServerSessionLaunchRequest({
      ...request,
      launch: { ...request.launch, provider: { ...request.launch.provider, daybreakBlue: "yes" } },
    }),
    /daybreakBlue must be a boolean/,
  );
  assert.throws(
    () => decodeAppServerSessionLaunchRequest({
      ...request,
      launch: { ...request.launch, provider: { id: "openrouter", model: "auto", daybreakBlue: false } },
    }),
    /daybreakBlue requires the openai-codex provider/,
  );
});

test("Guardian launch preference accepts only a boolean", () => {
  const request = { launchVersion: APP_SERVER_SESSION_LAUNCH_VERSION,
    launch: { workspaceId: "workspace-example", promptMarkdown: "Inspect example.test.", guardianEnabled: false } };
  assert.deepEqual(decodeAppServerSessionLaunchRequest(request), request);
  assert.throws(() => decodeAppServerSessionLaunchRequest({
    ...request, launch: { ...request.launch, guardianEnabled: "false" },
  }), /guardianEnabled must be a boolean/);
  assert.deepEqual(decodeFleetRemoteResearchInput({
    promptMarkdown: "Inspect example.test.", targetIds: ["local"], guardianEnabled: false,
  }, "workspace-example").guardianEnabled, false);
});

test("a connected primary can request a browser relay for its local session", () => {
  const request = { launchVersion: APP_SERVER_SESSION_LAUNCH_VERSION,
    launch: { workspaceId: 'workspace-example', promptMarkdown: 'Inspect the example scope.', machineId: 'local', browserRelay: true } };
  assert.deepEqual(decodeAppServerSessionLaunchRequest(request), request);
  assert.throws(() => decodeAppServerSessionLaunchRequest({ ...request,
    launch: { ...request.launch, browserRelay: 'yes' } }), /browserRelay must be a boolean/u);
});

test("one research launch accepts distinct selected targets and rejects inconsistent routing", () => {
  const request = { launchVersion: APP_SERVER_SESSION_LAUNCH_VERSION,
    launch: { workspaceId: 'workspace-example', promptMarkdown: 'Inspect the example scope.',
      machineId: 'local', targetIds: ['local', 'vm:run:tart:example-vm', 'browser:example'] } };
  assert.deepEqual(decodeAppServerSessionLaunchRequest(request), request);
  for (const targetIds of [[], ['local', 'local'], [''], Array(17).fill('local')]) {
    assert.throws(() => decodeAppServerSessionLaunchRequest({ ...request,
      launch: { ...request.launch, targetIds } }), /targetIds must contain/u);
  }
  assert.throws(() => decodeAppServerSessionLaunchRequest({ ...request,
    launch: { ...request.launch, machineId: 'vm:run:tart:example-vm' } }), /must run on the workspace app-server/u);
  assert.throws(() => decodeAppServerSessionLaunchRequest({ ...request,
    launch: { ...request.launch, targetIds: ['local'], machineId: 'browser:example' } }), /must match machineId/u);
});

test('remote New Research input validates targets, launch settings, and repeat schedule', () => {
  const input = { promptMarkdown: 'Inspect example.test.', targetIds: ['local', 'browser:example'],
    provider: { id: 'openai-codex', model: 'gpt-example', reasoningEffort: 'high' },
    repeatSchedule: { type: 'hourly', interval: 2 } };
  assert.deepEqual(decodeFleetRemoteResearchInput(input, 'workspace-example'), input);
  assert.throws(() => decodeFleetRemoteResearchInput(input, ''), /workspaceId/u);
  for (const invalid of [
    { ...input, targetIds: ['local', 'local'] },
    { ...input, repeatSchedule: { type: 'hourly', interval: 0 } },
    { ...input, provider: { id: 'openai-codex', fastMode: 'yes' } },
  ]) assert.throws(() => decodeFleetRemoteResearchInput(invalid, 'workspace-example'));
});

test('remote Add Context input only accepts bounded workspace prompt settings', () => {
  const input = { promptMarkdown: 'Inspect example.test.', phase: 'discovery',
    provider: { id: 'openai-codex', model: 'gpt-example' } };
  assert.deepEqual(decodeFleetRemotePromptExpansionInput(input), input);
  assert.throws(() => decodeFleetRemotePromptExpansionInput({ ...input, promptMarkdown: 'x'.repeat(12_001) }));
  assert.throws(() => decodeFleetRemotePromptExpansionInput({ ...input, provider: { id: 'openai-codex' } }));
});

test("app-server control DTOs share strict version, route, replay, and error semantics", () => {
  const health = {
    ok: true,
    controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
    contractTimestamp: BEALE_APP_SERVER_CONTRACT_TIMESTAMP,
    capabilities: BEALE_APP_SERVER_CAPABILITIES,
  };
  const descriptor = {
    ...health,
    sessionLaunchVersion: APP_SERVER_SESSION_LAUNCH_VERSION,
    appServerProtocolVersion: APP_SERVER_PROTOCOL_VERSION,
    endpoints: {
      sessions: BEALE_APP_SERVER_SESSIONS_PATH,
      workspaces: BEALE_APP_SERVER_WORKSPACES_PATH,
      providers: BEALE_APP_SERVER_PROVIDERS_PATH,
      shutdown: BEALE_APP_SERVER_SHUTDOWN_PATH,
    },
    limits: {
      requestBodyBytes: 524_288,
      frameBytes: 1_048_576,
      replayBytes: BEALE_APP_SERVER_MAX_REPLAY_BYTES,
      replayFrames: BEALE_APP_SERVER_MAX_REPLAY_FRAMES,
    },
  };
  assert.deepEqual(decodeBealeAppServerDescriptor(descriptor), descriptor);
  const providerCatalog = {
    controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
    defaultProviderId: "openai-codex",
    providers: [{
      providerId: "openai-codex",
      providerName: "OpenAI",
      defaultLeadModel: "gpt-6-sol",
      defaultSubagentModel: "gpt-6-luna",
      defaultReasoningEffort: "high",
      models: [{
        id: "gpt-6-sol",
        name: "GPT-6 Sol",
        reasoning: true,
        effortLevels: ["low", "medium", "high"],
      }],
    }],
  };
  assert.deepEqual(decodeBealeAppServerProviderCatalog(providerCatalog), providerCatalog);
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("knowledge.claim-security-tracking.v1"));
  assert.equal(BEALE_APP_SERVER_SERVER_PATH, "/v1/server");

  const session = {
    sessionId: "session-1",
    state: "running",
    startedAt: "2026-08-22T01:00:00.000Z",
    endedAt: null,
    exitCode: null,
    clientConnected: false,
    diagnostic: null,
    replay: { bufferedFrames: 0, bufferedBytes: 0, droppedFrames: 0 },
  };
  const started = {
    controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
    session,
    attemptId: "attempt-1",
    transport: {
      path: "/v1/sessions/session-1/transport",
      protocolVersion: APP_SERVER_PROTOCOL_VERSION,
      authentication: "bearer",
      token: "session-token",
      reconnect: "replay",
    },
  };
  assert.deepEqual(decodeBealeAppServerSessionStartResult(started), started);
  assert.deepEqual(decodeBealeAppServerSessionContinuationRequest({
    workspaceId: "workspace-1",
    instruction: "Continue with the retained history.",
  }), {
    workspaceId: "workspace-1",
    instruction: "Continue with the retained history.",
  });
  assert.throws(
    () => decodeBealeAppServerSessionContinuationRequest({ workspaceId: "workspace-1", instruction: "" }),
    /instruction/,
  );
  const attachment = {
    controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
    session,
    transport: { ...started.transport, token: "mobile-session-token" },
  };
  assert.deepEqual(decodeBealeAppServerSessionAttachResult(attachment), attachment);
  assert.throws(
    () => decodeBealeAppServerSessionStartResult({
      ...started,
      transport: { ...started.transport, path: "/v1/sessions/another-session/transport" },
    }),
    /transport path/,
  );
  assert.deepEqual(decodeBealeAppServerSessionStopResult({
    controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
    stopped: true,
    sessionId: "session-1",
  }), {
    controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
    stopped: true,
    sessionId: "session-1",
  });
  assert.deepEqual(decodeBealeAppServerShutdownResult({
    controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
    shuttingDown: true,
  }), {
    controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
    shuttingDown: true,
  });
  assert.deepEqual(decodeBealeAppServerErrorResponse({
    controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
    error: { code: "temporarily_unavailable", message: "Try again.", retryable: true },
  }).error, { code: "temporarily_unavailable", message: "Try again.", retryable: true });
});
