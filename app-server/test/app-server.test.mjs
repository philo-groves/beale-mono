import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { afterEach, beforeEach, test } from "node:test";
import {
  AppServerHostRegistry,
  AppServerHostService,
  acquireDiscoveryLock,
  createAppServerPairingPayload,
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
  BEALE_APP_SERVER_CAPABILITIES,
  BEALE_APP_SERVER_CONTRACT_TIMESTAMP,
  BEALE_APP_SERVER_CONTROL_VERSION,
} from "@beale/app-server-runtime/protocol";
import { AppServerSessionStore } from "../../packages/research-agent/dist/index.js";

const requireFromHere = createRequire(import.meta.url);
const WebSocket = requireFromHere("ws");

const servers = [];
const temporaryDirectories = [];
const originalMockMode = process.env.BEALE_APP_SERVER_MOCK;

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
  assert.ok(payload.providers[0].models.some((model) => model.id === "grok-4.6"));
  assert.doesNotMatch(JSON.stringify(payload), /credential|token|auth|workspacePath|databasePath/u);
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

test("routes campaign-track operations through registered workspace storage", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-campaign-operation-"));
  temporaryDirectories.push(directory);
  const calls = [];
  const service = new AppServerHostService({
    registry: hostRegistryFixture(directory, { memoryBackend: "app-server" }),
    invokeProtocol: async (operation, options) => {
      calls.push({ operation, options });
      return [{ id: "investigation_one" }];
    },
  });

  const result = await service.executeOperation({
    operation: "investigation.list",
    input: { workspaceId: "workspace-test" },
  });

  assert.deepEqual(result, [{ id: "investigation_one" }]);
  assert.equal(calls[0].operation, "investigation.list");
  assert.equal(calls[0].options.storage.databasePath, join(directory, "memory.sqlite"));

  const disabled = new AppServerHostService({
    registry: hostRegistryFixture(directory, { memoryBackend: "disabled" }),
    invokeProtocol: async () => [],
  });
  await assert.rejects(
    disabled.executeOperation({ operation: "investigation.replay", input: { workspaceId: "workspace-test" } }),
    /memory disabled/,
  );
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

test("serves host workspaces and canonical app-server reads from one authenticated control plane", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-control-"));
  temporaryDirectories.push(directory);
  const server = await startAppServer({ hostService: testHostService(directory) });
  servers.push(server);
  const headers = { authorization: `Bearer ${server.operatorToken}` };

  const workspaceResponse = await fetch(`${server.url}/v1/workspaces`, { headers });
  assert.equal(workspaceResponse.status, 200);
  assert.equal((await workspaceResponse.json()).workspaces[0].workspaceId, "workspace-test");

  const reads = [
    ["memory", "memory"],
    ["memory-notifications", "memory-notifications"],
    ["sessions", "sessions"],
    ["channels", "channels"],
    ["channels/channel-test", "channel"],
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

  const createdChannel = await fetch(`${server.url}/v1/workspaces/workspace-test/channels`, {
    method: "POST", headers: { ...headers, "content-type": "application/json" }, body: JSON.stringify({ name: "new", topic: "New research" }),
  });
  assert.equal(createdChannel.status, 201);
  assert.equal((await createdChannel.json()).result.kind, "channel-created");
  const postedMessage = await fetch(`${server.url}/v1/workspaces/workspace-test/channels/channel-test`, {
    method: "POST", headers: { ...headers, "content-type": "application/json" }, body: JSON.stringify({ contentMarkdown: "Evidence" }),
  });
  assert.equal(postedMessage.status, 201);
  assert.equal((await postedMessage.json()).result.kind, "channel-message");
  const archivedChannel = await fetch(`${server.url}/v1/workspaces/workspace-test/channels/channel-test/archive`, { method: "POST", headers });
  assert.equal(archivedChannel.status, 200);
  assert.equal((await archivedChannel.json()).result.kind, "channel-archived");
  const restoredChannel = await fetch(`${server.url}/v1/workspaces/workspace-test/channels/channel-test/restore`, { method: "POST", headers });
  assert.equal(restoredChannel.status, 200);
  assert.equal((await restoredChannel.json()).result.kind, "channel-restored");
  const deletedChannel = await fetch(`${server.url}/v1/workspaces/workspace-test/channels/channel-test`, { method: "DELETE", headers });
  assert.equal(deletedChannel.status, 200);
  assert.equal((await deletedChannel.json()).result.kind, "channel-deleted");
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
  assert.equal(registry.resolveWorkspace("workspace-quick-chats").name, "Quick Chats");
  assert.equal(registry.resolveWorkspace("workspace-test").workspacePath, workspacePath);
  assert.equal(registry.resolveWorkspace("workspace-test").memoryBackend, "disabled");
  assert.equal(registry.resolveWorkspace("registry-workspace-test").workspaceDirectories.length, 2);
  assert.deepEqual(registry.providerSettings(), {
    defaultProviderId: "openai-codex",
    modelDefaults: {
      "openai-codex": { leadModel: "gpt-lead", smallModel: "gpt-small", reasoningEffort: "high" },
    },
    authenticationPreferences: { "openai-codex": "subscription" },
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
          freshness: "current", workflow: "active", classification: "security.primitive",
          componentClaimIds: [], title: "Parser race", summary: "A shared parser crosses requests.",
          impact: "Integrity loss.", rating: "high", confidence: 0.9, evidence: [],
          duplicateClaims: [{
            id: "claim-duplicate", projection: "lead", maturity: "observed", rating: "high",
            classification: "security.primitive", title: "Duplicate parser race", status: "observed",
            revision: 2, markedAt: "2026-08-22T00:03:00.000Z",
          }],
          securityTracking: {
            reachability: { state: "reachable", conditions: "Public listener.", assessedAt: "2026-08-22T00:03:00.000Z" },
            riskTreatment: "remediate",
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
    }],
    leads: [],
    findings: [{
      id: "claim-1",
      sessionIds: ["session-1"],
      projection: "finding",
      maturity: "verified",
      freshness: "current",
      workflow: "active",
      classification: "security.primitive",
      componentClaimIds: [],
      duplicateClaims: [{
        id: "claim-duplicate",
        projection: "lead",
        maturity: "observed",
        rating: "high",
        classification: "security.primitive",
        title: "Duplicate parser race",
        status: "observed",
        revision: 2,
        markedAt: "2026-08-22T00:03:00.000Z",
      }],
      title: "Parser race",
      summary: "A shared parser crosses requests.",
      impact: "Integrity loss.",
      rating: "high",
      securityTracking: {
        reachability: { state: "reachable", conditions: "Public listener.", assessedAt: "2026-08-22T00:03:00.000Z" },
        riskTreatment: "remediate",
        cvssAssessments: [{ version: "4.0", vector: "CVSS:4.0/AV:N", score: 8.7, nomenclature: "CVSS-B", assessedAt: "2026-08-22T00:03:00.000Z" }],
        affectedAssetIds: ["asset-parser"],
        affectedVersions: [{ assetId: "asset-parser", range: "1.x", fixedVersion: "2.0" }],
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

test("app-server preserves OpenAI Fast mode through restart metadata and runtime arguments", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-fast-mode-"));
  temporaryDirectories.push(directory);
  const calls = [];
  const service = new AppServerHostService({
    registry: hostRegistryFixture(directory),
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
  };

  const prepared = await service.prepareSession(request, "generated-session");

  assert.equal(prepared.launch.provider.fastMode, true);
  assert.ok(appServerSessionArgs(prepared.launch, {}).includes("--fast-mode"));
  const createCall = calls.find((call) => call.operation === "session.create");
  assert.equal(
    createCall.options.input.metadata.appServerRestartLaunch.launch.provider.fastMode,
    true,
  );

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
});

test("app-server owns built-in plugins and pins canonical session profile identity", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-app-server-host-policy-"));
  temporaryDirectories.push(directory);
  const calls = [];
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
        return {
          skillDirs: [],
          selectedSkillIds: [],
          allowedMcpServers: ["beale-introspection.beale", "example.tools"],
        };
      }
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
    },
  }, "generated-session");

  assert.equal(prepared.launch.researchProfileId, "security-research");
  assert.equal(prepared.launch.researchProfileHash, hash);
  assert.equal(prepared.launch.memoryBackend, "disabled");
  assert.deepEqual(prepared.launch.pluginRuntime.allowedMcpServers, ["example.tools"]);
  assert.deepEqual(prepared.launch.provider, {
    id: "xai",
    model: "grok-4.6",
    reasoningEffort: "high",
    riskAcknowledgements: ["openai-codex", "anthropic", "xai"],
    authenticationPreferences: { xai: "api_key" },
    title: { model: "grok-4.3", effort: "medium" },
    shellReview: {
      models: { "openai-codex": "gpt-small", xai: "grok-4.3" },
      effort: "medium",
    },
  });
  const pluginCall = calls.find((call) => call.operation === "plugin.runtime");
  assert.equal(pluginCall.options.input.builtinPlugins.length, 2);
  assert.ok(pluginCall.options.input.builtinPlugins.every((plugin) =>
    plugin.path.includes(join("app-server", "resources", "agent-plugins")) && existsSync(plugin.path)
  ));
  const createCall = calls.find((call) => call.operation === "session.create");
  assert.deepEqual(createCall.options.input.profile, { id: "security-research", hash });
  assert.deepEqual(createCall.options.input.metadata.appServerRestartLaunch, {
    schemaVersion: 1,
    eligible: true,
    launch: {
      workspaceId: "workspace-test",
      promptMarkdown: "Test the typed app-server launch contract.",
      provider: { id: "xai", model: "grok-4.6", reasoningEffort: "high" },
      shellSafetyMode: "manual_approval",
      researchProfileId: "security-research",
      researchProfileHash: hash,
    },
  });
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
    launchVersion: 2,
    sessionId: "session-automation",
    launch: {
      workspaceId: "workspace-test",
      promptMarkdown: "Recheck the parser boundary.",
      goal: { objective: "Keep the parser audit moving." },
      provider: { id: "openai-codex", model: "gpt-5.6-sol", reasoningEffort: "high", fastMode: true },
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
  const pluginCall = calls.find((call) => call.operation === "plugin.runtime");

  assert.equal(prepared.launch.pluginRuntime.allowedMcpServers[0], "beale-introspection.beale");
  assert.deepEqual(prepared.launch.introspection, {
    url: "http://127.0.0.1:42123",
    token: "quick-chat-token",
  });
  assert.equal(pluginCall.options.input.registryDirectory, join(directory, "quick-chat-plugin-runtime"));
  assert.equal(pluginCall.options.input.runtimeEnvironment, undefined);
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
  assert.equal(args[args.indexOf("--attempt-id") + 1], "attempt-test");
  assert.equal(args[args.indexOf("--memory-backend") + 1], "app-server");
  assert.ok(args.includes("--no-default-tool-config"));
  assert.ok(args.includes("--fast-mode"));
  assert.ok(args.includes("--profile-tool-family-ceiling"));
  assert.ok(args.includes("--profile-side-effect-ceiling"));
  assert.equal(args.includes("--disable-tool-family"), false);
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

  const liveSessionResponse = await fetch(`${server.url}/v1/sessions/session-facade`, {
    headers: { authorization: `Bearer ${server.operatorToken}` },
  });
  assert.equal(liveSessionResponse.status, 200);
  const liveSession = await liveSessionResponse.json();
  assert.equal(liveSession.controlVersion, BEALE_APP_SERVER_CONTROL_VERSION);
  assert.equal(liveSession.session.sessionId, "session-facade");
  assert.equal(liveSession.session.state, "running");
  assert.deepEqual(liveSession.session.replay, {
    bufferedFrames: 0,
    bufferedBytes: 0,
    droppedFrames: 0,
  });

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

  assert.equal(states.includes("paused"), true);
  assert.equal(states.includes("stopped"), true);
  upstream.complete();
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
        stop: () => finish(null),
      };
    },
    sendEvent: emit,
    controls,
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
    launchVersion: 2,
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
    async workspaceMemoryNotifications() { return canonicalFixture("memory-notifications"); },
    async workspaceSessions() { return canonicalFixture("sessions"); },
    async workspaceChannels() { return canonicalFixture("channels"); },
    async workspaceChannel() { return canonicalFixture("channel"); },
    async createWorkspaceChannel() { return canonicalFixture("channel-created"); },
    async postWorkspaceChannelMessage() { return canonicalFixture("channel-message"); },
    async archiveWorkspaceChannel() { return canonicalFixture("channel-archived"); },
    async restoreWorkspaceChannel() { return canonicalFixture("channel-restored"); },
    async deleteWorkspaceChannel() { return canonicalFixture("channel-deleted"); },
    async sessionUpdate() { return canonicalFixture("update"); },
    async sessionEvents() { return canonicalFixture("events"); },
    async sessionEventDetails() { return canonicalFixture("event-details"); },
    async sessionCollaboration() { return canonicalFixture("collaboration"); },
    async sessionCaptures() { return canonicalFixture("captures"); },
  };
}

function hostRegistryFixture(directory, options = {}) {
  return {
    registryDirectory: directory,
    registryPath: join(directory, "workspace-registry.sqlite"),
    shellOptionsPath: join(directory, "shell-options.json"),
    listWorkspaces: () => [],
    resolveWorkspace: (identifier) => identifier === "workspace-test" ? {
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
    } : null,
    providerSettings: () => ({
      defaultProviderId: "openai-codex",
      modelDefaults: {},
      authenticationPreferences: {},
      riskAcknowledgements: [],
    }),
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
