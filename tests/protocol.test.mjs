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
  decodeBealeAppServerSessionAttachResult,
  decodeBealeAppServerSessionStartResult,
  decodeBealeAppServerSessionStopResult,
  decodeBealeAppServerShutdownResult,
  decodeAppServerProtocolEnvelope,
  decodeAppServerSessionLaunchRequest,
  decodeAppServerServerMessage,
  APP_SERVER_PROTOCOL_OPERATIONS,
  APP_SERVER_PROTOCOL_VERSION,
  APP_SERVER_SESSION_LAUNCH_VERSION,
  appServerProtocolFailure,
  appServerProtocolDescriptor,
  appServerProtocolSuccess,
  parseAppServerProtocolArguments,
} from "../packages/app-server-runtime/dist/protocol.js";

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

test("protocol describe exposes a runtime-bound v13 report-catalog contract for app-server and WebSocket clients", () => {
  const descriptor = appServerProtocolDescriptor();
  assert.deepEqual(descriptor.operations, APP_SERVER_PROTOCOL_OPERATIONS);
  assert.equal(descriptor.contractVersion, 13);
  assert.match(descriptor.runtime.buildId, /^[a-f0-9]{24}$/);
  assert.equal(descriptor.schemas.memorySummary, 11);
  assert.equal(descriptor.schemas.finding, 4);
  assert.equal(descriptor.schemas.campaignGraph, 4);
  assert.equal(descriptor.schemas.goalSuggestions, 1);
  assert.ok(descriptor.capabilities.includes("knowledge.findings"));
  assert.ok(descriptor.capabilities.includes("knowledge.campaign_graph"));
  assert.ok(descriptor.capabilities.includes("knowledge.campaign_tracks.v2"));
  assert.ok(descriptor.capabilities.includes("knowledge.claims.v2"));
  assert.ok(descriptor.capabilities.includes("knowledge.claim_security_tracking"));
  assert.ok(descriptor.capabilities.includes("session.bounded_reads"));
  assert.ok(descriptor.capabilities.includes("session.targeted_details"));
  assert.ok(descriptor.capabilities.includes("workspace.channels.v2"));
  assert.ok(descriptor.capabilities.includes("workspace.goal-suggestions.v1"));
  assert.ok(descriptor.capabilities.includes("workspace.prompt-expansion.v1"));
  assert.ok(APP_SERVER_PROTOCOL_OPERATIONS.includes("channel.list"));
  assert.ok(APP_SERVER_PROTOCOL_OPERATIONS.includes("channel.share"));
  assert.ok(APP_SERVER_PROTOCOL_OPERATIONS.includes("suggestion.generate"));
  assert.ok(APP_SERVER_PROTOCOL_OPERATIONS.includes("suggestion.select"));
  assert.ok(APP_SERVER_PROTOCOL_OPERATIONS.includes("prompt.expand"));
  assert.ok(APP_SERVER_PROTOCOL_OPERATIONS.includes("report.list"));
  assert.ok(APP_SERVER_PROTOCOL_OPERATIONS.includes("report.revise_content"));
  assert.ok(APP_SERVER_PROTOCOL_OPERATIONS.includes("report.update_triage_status"));
  assert.ok(APP_SERVER_PROTOCOL_OPERATIONS.includes("report.replace_recording"));
  assert.ok(descriptor.capabilities.includes("knowledge.report-content-revise.v1"));
  assert.ok(descriptor.capabilities.includes("knowledge.report-triage-status.v1"));
  assert.ok(descriptor.capabilities.includes("knowledge.report-recording-replace.v1"));
  assert.ok(descriptor.capabilities.includes("knowledge.report-list.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("workspace.channels.v2"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("workspace.goal-suggestions.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("session.startup-recovery.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("session.openai-fast-mode.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("workspace.prompt-expansion.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("knowledge.campaign-tracks.v2"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("source.clone-modes.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("maintenance.repository-consolidation.v1"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("memory.notifications.v3"));
  assert.ok(BEALE_APP_SERVER_CAPABILITIES.includes("knowledge.report-list.v1"));
  assert.equal(descriptor.transports.websocket.path, "/v1/session");
  assert.equal(descriptor.transports.appServer.path, "/v1/operations");
  assert.equal(descriptor.transports.appServer.authentication, "operator-bearer");
  assert.equal(descriptor.transports.websocket.framing, "json-message");
  assert.equal(descriptor.transports.websocket.errors, "protocol-error-message");
  assert.equal(descriptor.transports.websocket.correlation, "request-id");
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

test("the typed session launch carries an OpenAI Fast mode preference", () => {
  const request = {
    launchVersion: APP_SERVER_SESSION_LAUNCH_VERSION,
    launch: {
      workspaceId: "workspace-example",
      promptMarkdown: "Inspect the parser boundary.",
      provider: { id: "openai-codex", model: "gpt-5.6-sol", fastMode: true },
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
      defaultLeadModel: "gpt-5.6-sol",
      defaultSubagentModel: "gpt-5.6-luna",
      defaultReasoningEffort: "high",
      models: [{
        id: "gpt-5.6-sol",
        name: "GPT-5.6 Sol",
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
