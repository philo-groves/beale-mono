import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { once } from "node:events";
import { DatabaseSync } from "node:sqlite";
import test from "node:test";
import {
  DEFAULT_SECURITY_RESEARCH_PROFILE,
  AppServerSessionStore,
  ResearchChannelStore,
  normalizeResearchProfile,
  researchProfileHash,
} from "../packages/research-agent/dist/index.js";
import { invokeAppServerProtocol } from "../app-server/dist/appServerProtocolClient.js";

test("workspace channels retain cross-session transcripts until explicitly deleted", async () => {
  const directory = await mkdtemp(join(tmpdir(), "app-server-channels-"));
  const databasePath = join(directory, "memory.sqlite");
  const first = new ResearchChannelStore({ databasePath });
  const channel = first.create({
    workspaceId: "workspace_channels",
    name: "parser-review",
    title: "Parser review",
    topic: "Carry parser boundary research across sessions.",
    createdBySessionId: "session_one",
    createdByAgentPath: "/root",
  });
  first.append({
    workspaceId: "workspace_channels",
    channel: channel.id,
    sessionId: "session_one",
    attemptId: "attempt_one",
    agentId: "agent_one",
    agentPath: "/root/parser",
    provider: "openai-codex",
    model: "gpt-5.6-sol",
    role: "reviewer",
    status: "completed",
    kind: "evidence",
    contentMarkdown: "The allocation omits a terminator.",
    evidenceRefs: ["code:parser:44"],
  });
  const shared = first.share({
    workspaceId: "workspace_channels",
    channel: channel.id,
    sessionId: "session_one",
    agentId: "agent_one",
    agentPath: "/root/parser",
    role: "reviewer",
    status: "completed",
    kind: "runbook",
    resourceId: "runbook_parser_repro",
    title: "Parser reproducer",
    note: "The bounded reproducer is ready.",
  });
  assert.equal(shared.message.contentMarkdown, "The bounded reproducer is ready.");
  first.close();

  const later = new ResearchChannelStore({ databasePath });
  try {
    const detail = later.get("workspace_channels", "parser-review");
    assert.equal(detail.channel.id, channel.id);
    assert.equal(detail.messages[0].sessionId, "session_one");
    assert.equal(detail.messages[0].contentMarkdown, "The allocation omits a terminator.");
    assert.equal(detail.members[0].status, "completed");
    assert.equal(detail.sharedResources[0].kind, "runbook");
    assert.equal(detail.sharedResources[0].resourceId, "runbook_parser_repro");
    assert.equal(detail.sharedResources[0].messageId, shared.message.id);
    assert.equal(later.list("workspace_channels")[0].messageCount, 2);
    const archived = later.archive("workspace_channels", channel.id, "2026-08-24T12:00:00.000Z");
    assert.equal(archived.archivedAt, "2026-08-24T12:00:00.000Z");
    assert.deepEqual(later.list("workspace_channels"), []);
    assert.equal(later.list("workspace_channels", 200, true)[0].messageCount, 2);
    assert.equal(later.get("workspace_channels", channel.id).sharedResources[0].resourceId, "runbook_parser_repro");
    assert.equal(later.restore("workspace_channels", channel.id).archivedAt, null);
    assert.equal(later.list("workspace_channels")[0].id, channel.id);
    assert.deepEqual(later.list("another_workspace"), []);
    assert.deepEqual(later.delete("workspace_channels", channel.id), { channelId: channel.id, deleted: true });
    assert.equal(later.get("workspace_channels", channel.id), null);
  } finally {
    later.close();
  }
});

test("workspace channel names use at most three lowercase dash-separated words", () => {
  const store = new ResearchChannelStore({ databasePath: ":memory:" });
  try {
    const channel = store.create({
      workspaceId: "workspace_channels",
      name: " Parser / Boundary Review ",
      topic: "Carry parser boundary research across sessions.",
    });
    assert.equal(channel.name, "parser-boundary-review");
    assert.throws(() => store.create({
      workspaceId: "workspace_channels",
      name: "parser boundary review notes",
      topic: "This name has too many words.",
    }), /at most 3 words/);
  } finally {
    store.close();
  }
});

test("hosted channel operations publish typed shared resources", async () => {
  const directory = await mkdtemp(join(tmpdir(), "app-server-channel-share-"));
  const storage = {
    databasePath: join(directory, "memory.sqlite"),
    artifactDirectoryPath: join(directory, "artifacts"),
  };
  const created = await invokeAppServerProtocol("channel.create", {
    args: [],
    storage,
    input: {
      workspaceId: "workspace_channels",
      name: "shared-review",
      topic: "Share durable research artifacts.",
    },
  });
  const shared = await invokeAppServerProtocol("channel.share", {
    args: [],
    storage,
    input: {
      workspaceId: "workspace_channels",
      channel: created.id,
      sessionId: "session_one",
      agentPath: "/root/reviewer",
      kind: "memory",
      resourceId: "memory_parser_boundary",
      title: "Parser boundary",
    },
  });
  const detail = await invokeAppServerProtocol("channel.get", {
    args: [],
    storage,
    input: { workspaceId: "workspace_channels", channel: created.id },
  });
  assert.equal(shared.resource.kind, "memory");
  assert.equal(shared.message.contentMarkdown, "Shared a memory.");
  assert.equal(detail.sharedResources[0].resourceId, "memory_parser_boundary");
});

test("workspace channel migration retains legacy members with unknown status", async () => {
  const directory = await mkdtemp(join(tmpdir(), "app-server-channel-status-"));
  const databasePath = join(directory, "memory.sqlite");
  const original = new ResearchChannelStore({ databasePath });
  const channel = original.create({
    workspaceId: "workspace_channels",
    name: "legacy-research",
    topic: "Retain legacy channel members.",
  });
  original.join({
    workspaceId: "workspace_channels",
    channel: channel.id,
    sessionId: "session_legacy",
    agentPath: "/root/legacy",
  });
  original.close();

  const legacyDatabase = new DatabaseSync(databasePath);
  legacyDatabase.exec("ALTER TABLE app_server_channel_members DROP COLUMN status;");
  legacyDatabase.prepare("DELETE FROM schema_migrations WHERE component = ? AND version >= 2")
    .run("app_server_channels");
  legacyDatabase.close();

  const migrated = new ResearchChannelStore({ databasePath });
  try {
    assert.equal(migrated.get("workspace_channels", channel.id).members[0].status, "unknown");
  } finally {
    migrated.close();
  }
});

test("session store owns creation, lifecycle, capture import, and queries as one revisioned aggregate", () => {
  const store = new AppServerSessionStore({ databasePath: ":memory:" });
  try {
    const created = store.create({
      id: "session_one",
      workspaceId: "workspace_one",
      attemptId: "attempt_one",
      title: "Session one",
      prompt: "Inspect the parser.",
      provider: "openai-codex",
      model: "gpt-5.6-sol",
      reasoningEffort: "high",
    });
    assert.equal(created.revision, 1);

    const withLiveEvent = store.appendEvent(created.id, {
      id: "event_one",
      kind: "agent.event",
      timestamp: "2026-08-15T12:00:00.000Z",
      summary: "Inspected parser",
      payload: { eventType: "tool.completed" },
    });
    assert.equal(withLiveEvent.revision, 2);

    const imported = store.importCapture(created.id, {
      attemptId: "attempt_one",
      expectedRevision: 2,
      capture: captureFixture(),
    });
    assert.equal(imported.status, "completed");
    assert.equal(imported.finalResponse, "The parser is safe.");
    assert.equal(store.get(created.id).events.length, 2);
    assert.equal(imported.attempts[0].capture.schemaVersion, 5);
    assert.equal(imported.attempts[0].capture.raw.eventTimeline, undefined);
    assert.equal(imported.attempts[0].capture.eventStreams.timeline.count, 1);
    assert.equal(imported.attempts[0].capture.raw.agent.raw.agentEvents, undefined);
    assert.equal(imported.attempts[0].capture.eventStreams.agentDiagnostics.count, 2);
    assert.equal(store.get(created.id).attempts[0].capture.schemaVersion, 5);
    assert.equal(store.list("workspace_one")[0].revision, 3);
    assert.throws(
      () => store.transition(created.id, { status: "stopped", summary: "Stale writer", expectedRevision: 2 }),
      /revision conflict/,
    );
  } finally {
    store.close();
  }
});

test("capture import enforces the canonical session profile and workflow", () => {
  const store = new AppServerSessionStore({ databasePath: ":memory:" });
  const profile = normalizeResearchProfile(DEFAULT_SECURITY_RESEARCH_PROFILE);
  const profileHash = researchProfileHash(profile);
  try {
    const created = store.create({
      id: "session_profile",
      workspaceId: "workspace_profile",
      attemptId: "attempt_profile",
      title: "Profile-pinned session",
      prompt: "Inspect the parser.",
      provider: "openai-codex",
      model: "gpt-5.6-sol",
      reasoningEffort: "high",
      workflowId: "discovery",
      profile: { id: profile.id, hash: profileHash },
    });
    assert.throws(
      () => store.importCapture(created.id, { attemptId: "attempt_profile", capture: captureFixture() }),
      /missing the research profile pinned to session/,
    );
    assert.throws(
      () => store.importCapture(created.id, {
        attemptId: "attempt_profile",
        capture: profiledCapture({ hash: "0".repeat(64) }),
      }),
      /research profile hash mismatch/,
    );
    assert.throws(
      () => store.importCapture(created.id, {
        attemptId: "attempt_profile",
        capture: profiledCapture({ workflowId: "chaining" }),
      }),
      /does not match the profile and workflow pinned to session/,
    );
    assert.equal(store.importCapture(created.id, {
      attemptId: "attempt_profile",
      capture: profiledCapture(),
    }).status, "completed");
  } finally {
    store.close();
  }
});

test("session transitions update editable configuration with the lifecycle aggregate", () => {
  const store = new AppServerSessionStore({ databasePath: ":memory:" });
  try {
    const created = store.create({
      id: "session_configuration",
      workspaceId: "workspace_one",
      attemptId: "attempt_configuration",
      title: "Editable session",
      prompt: "Inspect the parser.",
      provider: "openai-codex",
      model: "gpt-5.6-sol",
      reasoningEffort: "high",
      workflowId: "discovery",
    });
    const updated = store.transition(created.id, {
      status: created.status,
      summary: created.summary,
      configuration: {
        prompt: "Review the parser and its callers.",
        provider: "anthropic",
        model: "claude-opus-4-1",
        reasoningEffort: "medium",
        workflowId: "chaining",
      },
    });

    assert.equal(updated.prompt, "Review the parser and its callers.");
    assert.equal(updated.provider, "anthropic");
    assert.equal(updated.model, "claude-opus-4-1");
    assert.equal(updated.reasoningEffort, "medium");
    assert.equal(updated.workflowId, "chaining");
  } finally {
    store.close();
  }
});

test("session recovery atomically pauses interrupted workspace sessions and their active attempts", () => {
  const store = new AppServerSessionStore({ databasePath: ":memory:" });
  try {
    store.create({
      id: "session_interrupted",
      workspaceId: "workspace_recovery",
      attemptId: "attempt_interrupted",
      title: "Interrupted session",
      prompt: "Inspect recovery.",
      model: "gpt-5.6-sol",
      reasoningEffort: "high",
    });
    store.create({
      id: "session_other_workspace",
      workspaceId: "workspace_other",
      attemptId: "attempt_other_workspace",
      title: "Other workspace",
      prompt: "Remain active.",
      model: "gpt-5.6-sol",
      reasoningEffort: "high",
    });

    const report = store.recoverInterrupted("workspace_recovery", {
      reason: "app_restart",
      at: "2026-08-16T12:00:00.000Z",
    });

    assert.deepEqual(report, {
      workspaceId: "workspace_recovery",
      recoveredAt: "2026-08-16T12:00:00.000Z",
      reason: "app_restart",
      interruptedSessions: 1,
      interruptedAttempts: 1,
      sessionIds: ["session_interrupted"],
    });
    const recovered = store.get("session_interrupted");
    assert.equal(recovered?.status, "paused");
    assert.equal(recovered?.attempts[0]?.status, "paused");
    assert.equal(recovered?.endedAt, null);
    assert.equal(recovered?.metadata.interruptedByRecovery, true);
    assert.equal(recovered?.metadata.recoveredAt, "2026-08-16T12:00:00.000Z");
    assert.equal(recovered?.events.at(-1)?.kind, "session.recovery");
    assert.equal(recovered?.events.at(-1)?.payload.interruptedByRecovery, true);
    assert.equal(store.get("session_other_workspace")?.status, "active");
    assert.equal(store.recoverInterrupted("workspace_recovery").interruptedSessions, 0);
  } finally {
    store.close();
  }
});

test("session summary lists stay bounded when canonical sessions contain large event histories", async () => {
  const directory = await mkdtemp(join(tmpdir(), "app-server-session-summary-"));
  const databasePath = join(directory, "memory.sqlite");
  const store = new AppServerSessionStore({ databasePath });
  try {
    store.create({
      id: "session_large_history",
      workspaceId: "workspace_summary",
      attemptId: "attempt_large_history",
      title: "Large history",
      prompt: "Keep list DTOs bounded.",
      model: "gpt-5.6-sol",
      reasoningEffort: "high",
    });
    store.appendEvent("session_large_history", {
      id: "event_large_history",
      kind: "agent.event",
      timestamp: "2026-08-16T12:00:00.000Z",
      summary: "Large event",
      payload: { output: "x".repeat(2 * 1024 * 1024) },
    });
    store.appendEvent("session_large_history", {
      id: "event_token_usage",
      kind: "beale.model_session_update",
      timestamp: "2026-08-16T12:01:00.000Z",
      summary: "Model session usage updated.",
      payload: {
        record: {
          patch: { metadata: { latestReportedTotalTokens: 12_345 } },
        },
      },
    });
    store.create({
      id: "session_other_summary_workspace",
      workspaceId: "workspace_other_summary",
      attemptId: "attempt_other_summary_workspace",
      title: "Other workspace",
      prompt: "Batch workspace catalogs.",
      model: "gpt-5.6-sol",
      reasoningEffort: "high",
    });
  } finally {
    store.close();
  }

  const inspection = new DatabaseSync(databasePath, { readOnly: true });
  try {
    const stored = inspection.prepare(`
      SELECT document_json, document_hash FROM app_server_sessions WHERE id = ?
    `).get("session_large_history");
    const document = JSON.parse(stored.document_json);
    assert.deepEqual(document.events, []);
    assert.ok(stored.document_json.length < 10_000);
    assert.equal(
      stored.document_hash,
      createHash("sha256").update(stored.document_json).digest("hex"),
    );
    const event = inspection.prepare(`
      SELECT event_json, content_hash FROM app_server_session_events WHERE session_id = ?
    `).get("session_large_history");
    assert.equal(
      event.content_hash,
      createHash("sha256").update(event.event_json).digest("hex"),
    );
    assert.ok(event.event_json.length > 2 * 1024 * 1024);
    assert.deepEqual(
      { ...inspection.prepare(`
        SELECT completed_turn_tokens, completed_turn_cost_usd,
          latest_reported_total_tokens, last_message_at
        FROM app_server_session_summary_metrics WHERE session_id = ?
      `).get("session_large_history") },
      {
        completed_turn_tokens: 0,
        completed_turn_cost_usd: 0,
        latest_reported_total_tokens: 12_345,
        last_message_at: null,
      },
    );
  } finally {
    inspection.close();
  }

  const listed = await runHostedOperation(
    ["session", "list-summaries", "--workspace-id", "workspace_summary", "--json"],
    { ...process.env, APP_SERVER_DATABASE_PATH: databasePath },
  );
  assert.equal(listed.operation, "session.list_summaries");
  assert.equal(listed.result[0].id, "session_large_history");
  assert.equal(Object.hasOwn(listed.result[0], "events"), false);
  assert.equal(Object.hasOwn(listed.result[0], "finalResponse"), false);
  assert.equal(Object.hasOwn(listed.result[0].attempts[0], "capture"), false);
  assert.deepEqual(listed.result[0].tokenUsage, { totalTokens: 12_345 });

  const projected = await runHostedOperation(
    ["session", "get-update", "--session-id", "session_large_history", "--max-bytes", "1024", "--json"],
    { ...process.env, APP_SERVER_DATABASE_PATH: databasePath },
  );
  assert.equal(projected.result.events.length, 1);
  assert.equal(projected.result.events[0].payload.detailAvailableOnRequest, true);
  assert.equal(projected.result.events[0].payload.sizeBytes > 2 * 1024 * 1024, true);
  assert.ok(JSON.stringify(projected).length < 20_000);

  const batched = await runHostedOperation(
    [
      "session", "list-summaries",
      "--workspace-id", "workspace_summary",
      "--workspace-id", "workspace_other_summary",
      "--json",
    ],
    { ...process.env, APP_SERVER_DATABASE_PATH: databasePath },
  );
  assert.deepEqual(
    new Set(batched.result.map((session) => session.id)),
    new Set(["session_large_history", "session_other_summary_workspace"]),
  );
});

test("session summaries read token usage from the latest model-session update", () => {
  const store = new AppServerSessionStore({ databasePath: ":memory:" });
  try {
    store.create({
      id: "session_latest_usage",
      workspaceId: "workspace_latest_usage",
      attemptId: "attempt_latest_usage",
      title: "Latest usage",
      prompt: "Read only the latest usage event.",
      model: "gpt-5.6-sol",
      reasoningEffort: "high",
    });
    for (const [id, totalTokens] of [["older", 12_345], ["latest", 23_456]]) {
      store.appendEvent("session_latest_usage", {
        id: `event_token_usage_${id}`,
        kind: "beale.model_session_update",
        timestamp: id === "older" ? "2026-08-16T12:01:00.000Z" : "2026-08-16T12:02:00.000Z",
        summary: "Model session usage updated.",
        payload: {
          record: {
            patch: { metadata: { latestReportedTotalTokens: totalTokens } },
          },
        },
      });
    }

    assert.deepEqual(store.getSummary("session_latest_usage")?.tokenUsage, { totalTokens: 23_456 });
    assert.deepEqual(store.listSummaries("workspace_latest_usage")[0]?.tokenUsage, { totalTokens: 23_456 });
  } finally {
    store.close();
  }
});

test("session summaries expose the latest transcript message time", () => {
  const store = new AppServerSessionStore({ databasePath: ":memory:" });
  try {
    store.create({
      id: "session_last_message",
      workspaceId: "workspace_last_message",
      attemptId: "attempt_last_message",
      title: "Last message",
      prompt: "Track the latest transcript message.",
      model: "gpt-5.6-sol",
      reasoningEffort: "high",
    });
    store.appendEvent("session_last_message", {
      id: "event_last_message",
      kind: "beale.transcript",
      timestamp: "2026-08-16T12:01:01.000Z",
      summary: "beale.transcript",
      payload: {
        record: {
          id: "message_last",
          createdAt: "2026-08-16T12:01:00.000Z",
        },
      },
    });
    store.appendEvent("session_last_message", {
      id: "event_after_message",
      kind: "agent.event",
      timestamp: "2026-08-16T12:02:00.000Z",
      summary: "Tool activity after the message.",
      payload: { type: "tool.completed" },
    });

    assert.equal(store.getSummary("session_last_message")?.lastMessageAt, "2026-08-16T12:01:00.000Z");
    assert.equal(store.getUpdate("session_last_message")?.session.lastMessageAt, "2026-08-16T12:01:00.000Z");
    assert.equal(store.listSummaries("workspace_last_message")[0]?.lastMessageAt, "2026-08-16T12:01:00.000Z");
  } finally {
    store.close();
  }
});

test("session summaries and live updates prefer aggregate completed-turn usage over the latest root turn", () => {
  const store = new AppServerSessionStore({ databasePath: ":memory:" });
  try {
    store.create({
      id: "session_turn_usage",
      workspaceId: "workspace_turn_usage",
      attemptId: "attempt_turn_usage",
      title: "Turn usage",
      prompt: "Aggregate canonical turn usage.",
      model: "gpt-5.6-sol",
      reasoningEffort: "high",
    });
    for (const [turn, usage, cost] of [
      [1, { input: 100, output: 200, cacheRead: 900, cacheWrite: 0, totalTokens: 1_200 }, 0.12],
      [2, { input: 200, output: 100, cacheRead: 500, cacheWrite: 0, totalTokens: 800 }, 0.08],
    ]) {
      store.appendEvent("session_turn_usage", {
        id: `event_turn_usage_${turn}`,
        kind: "agent.event",
        timestamp: `2026-08-16T12:0${turn}:00.000Z`,
        summary: "Model turn completed.",
        payload: {
          type: "turn_completed",
          turn,
          usage: { ...usage, cost: { total: cost } },
        },
      });
    }
    store.appendEvent("session_turn_usage", {
      id: "event_latest_root_turn_usage",
      kind: "beale.model_session_update",
      timestamp: "2026-08-16T12:03:00.000Z",
      summary: "Latest root model usage updated.",
      payload: {
        record: {
          patch: { metadata: { latestReportedTotalTokens: 800 } },
        },
      },
    });

    const expectedUsage = {
      totalTokens: 2_000,
      totalCostUsd: 0.2,
      inputTokens: 1_700,
      outputTokens: 300,
      cacheReadTokens: 1_400,
      cachePromptTokens: 1_700,
    };
    assert.deepEqual(store.getSummary("session_turn_usage")?.tokenUsage, expectedUsage);
    assert.deepEqual(store.getUpdate("session_turn_usage")?.session.tokenUsage, expectedUsage);
    assert.deepEqual(store.listSummaries("workspace_turn_usage")[0]?.tokenUsage, expectedUsage);
  } finally {
    store.close();
  }
});

test("session summaries durably deduplicate canonical memory activity", () => {
  const store = new AppServerSessionStore({ databasePath: ":memory:" });
  try {
    store.create({
      id: "session_memory_activity",
      workspaceId: "workspace_memory_activity",
      attemptId: "attempt_memory_activity",
      title: "Memory activity",
      prompt: "Count canonical memory activity.",
      model: "gpt-5.6-sol",
      reasoningEffort: "high",
    });
    for (const [id, kind, toolActionId, toolName] of [
      ["search_requested", "tool.requested", "search_one", "memory.search"],
      ["search_observed", "tool.observed", "search_one", "memory.search"],
      ["save_observed", "tool.observed", "save_one", "memory.save"],
      ["correct_observed", "tool.observed", "correct_one", "memory.correct"],
    ]) {
      store.appendEvent("session_memory_activity", {
        id,
        kind: "research.event",
        timestamp: "2026-08-16T12:00:00.000Z",
        summary: kind,
        payload: {
          event: {
            id: `nested_${id}`,
            kind,
            payload: { toolActionId, toolName },
          },
        },
      });
    }
    store.appendEvent("session_memory_activity", {
      id: "duplicate_trace_batch",
      kind: "beale.trace_batch",
      timestamp: "2026-08-16T12:01:00.000Z",
      summary: "Duplicate Desktop trace projection.",
      payload: {
        records: [{
          id: "duplicate_search_trace",
          type: "tool_result",
          payload: {
            appServerKind: "tool.observed",
            toolName: "memory.search",
            payload: { toolActionId: "search_one", toolName: "memory.search" },
          },
        }],
      },
    });

    const expected = { memorySearches: 1, memoryUpdates: 2 };
    assert.deepEqual(store.getSummary("session_memory_activity")?.activityCounts, expected);
    assert.deepEqual(store.getUpdate("session_memory_activity")?.session.activityCounts, expected);
    assert.deepEqual(store.listSummaries("workspace_memory_activity")[0]?.activityCounts, expected);
  } finally {
    store.close();
  }
});

test("session migration transactionally normalizes legacy embedded event histories", async () => {
  const directory = await mkdtemp(join(tmpdir(), "app-server-session-migration-"));
  const databasePath = join(directory, "memory.sqlite");
  const legacy = new DatabaseSync(databasePath);
  legacy.exec(`
    CREATE TABLE app_server_sessions (
      id TEXT PRIMARY KEY,
      workspace_id TEXT NOT NULL,
      status TEXT NOT NULL,
      title TEXT NOT NULL,
      summary TEXT NOT NULL,
      document_json TEXT NOT NULL,
      revision INTEGER NOT NULL,
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL
    );
  `);
  const timestamp = "2026-08-16T12:00:00.000Z";
  const legacyDocument = {
    schemaVersion: 1,
    id: "session_legacy",
    workspaceId: "workspace_legacy",
    status: "active",
    title: "Legacy session",
    prompt: "Migrate safely.",
    summary: "Legacy session",
    provider: null,
    model: "gpt-5.6-sol",
    reasoningEffort: "high",
    workflowId: null,
    profile: null,
    metadata: {},
    finalDisposition: null,
    finalResponse: null,
    attempts: [{
      id: "attempt_legacy",
      parentAttemptId: null,
      status: "active",
      summary: "Legacy attempt",
      startedAt: timestamp,
      endedAt: null,
      capture: {
        attemptId: "attempt_legacy",
        capturedAt: timestamp,
        schemaVersion: 5,
        request: {},
        agent: {},
        raw: { retained: true },
      },
      metadata: {},
    }],
    events: [
      {
        id: "event_legacy",
        kind: "agent.event",
        timestamp,
        summary: "Legacy event",
        payload: { retained: true },
      },
      {
        id: "event_legacy_memory_search",
        kind: "research.event",
        timestamp,
        summary: "Legacy memory search",
        payload: {
          event: {
            id: "nested_legacy_memory_search",
            kind: "tool.observed",
            payload: { toolActionId: "legacy_search", toolName: "memory.search" },
          },
        },
      },
    ],
    createdAt: timestamp,
    startedAt: timestamp,
    endedAt: null,
    updatedAt: timestamp,
    revision: 2,
  };
  legacy.prepare(`
    INSERT INTO app_server_sessions (
      id, workspace_id, status, title, summary, document_json, revision, created_at, updated_at
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
  `).run(
    legacyDocument.id,
    legacyDocument.workspaceId,
    legacyDocument.status,
    legacyDocument.title,
    legacyDocument.summary,
    JSON.stringify(legacyDocument),
    legacyDocument.revision,
    timestamp,
    timestamp,
  );
  legacy.close();

  const migrated = new AppServerSessionStore({ databasePath });
  try {
    assert.deepEqual(migrated.get("session_legacy")?.events, legacyDocument.events);
    const capture = migrated.get("session_legacy")?.attempts[0].capture;
    assert.equal(capture?.attemptId, "attempt_legacy");
    assert.equal(capture?.schemaVersion, 5);
    assert.equal(capture?.raw.retained, true);
    assert.equal(capture?.eventStreams.timeline.source, "app_server_session_events");
    assert.deepEqual(migrated.getSummary("session_legacy")?.activityCounts, {
      memorySearches: 1,
      memoryUpdates: 0,
    });
  } finally {
    migrated.close();
  }

  const inspection = new DatabaseSync(databasePath, { readOnly: true });
  try {
    const row = inspection.prepare(`
      SELECT document_json, document_hash FROM app_server_sessions WHERE id = ?
    `).get("session_legacy");
    assert.deepEqual(JSON.parse(row.document_json).events, []);
    assert.equal(JSON.parse(row.document_json).attempts[0].capture, null);
    assert.equal(typeof row.document_hash, "string");
    assert.deepEqual(
      inspection.prepare(`
        SELECT event_id, event_offset FROM app_server_session_events WHERE session_id = ?
      `).all("session_legacy").map((event) => ({ ...event })),
      [
        { event_id: "event_legacy", event_offset: 0 },
        { event_id: "event_legacy_memory_search", event_offset: 1 },
      ],
    );
    assert.deepEqual(
      inspection.prepare(`
        SELECT attempt_id FROM app_server_session_captures WHERE session_id = ?
      `).all("session_legacy").map((capture) => ({ ...capture })),
      [{ attempt_id: "attempt_legacy" }],
    );
    assert.deepEqual(
      { ...inspection.prepare(`
        SELECT completed_turn_tokens, completed_turn_cost_usd,
          latest_reported_total_tokens, last_message_at
        FROM app_server_session_summary_metrics WHERE session_id = ?
      `).get("session_legacy") },
      {
        completed_turn_tokens: 0,
        completed_turn_cost_usd: 0,
        latest_reported_total_tokens: null,
        last_message_at: null,
      },
    );
  } finally {
    inspection.close();
  }
});

test("hosted session operations report actionable integrity and database corruption failures", async () => {
  const integrityDirectory = await mkdtemp(join(tmpdir(), "app-server-session-integrity-"));
  const integrityDatabasePath = join(integrityDirectory, "memory.sqlite");
  const store = new AppServerSessionStore({ databasePath: integrityDatabasePath });
  store.create({
    id: "session_integrity",
    workspaceId: "workspace_integrity",
    attemptId: "attempt_integrity",
    title: "Integrity session",
    prompt: "Detect corruption.",
    model: "gpt-5.6-sol",
    reasoningEffort: "high",
  });
  store.appendEventReceipt("session_integrity", {
    id: "event_integrity",
    kind: "agent.event",
    timestamp: "2026-08-16T12:00:00.000Z",
    summary: "Integrity event",
    payload: { retained: true },
  });
  store.close();
  const tamper = new DatabaseSync(integrityDatabasePath);
  tamper.prepare(`
    UPDATE app_server_session_events SET event_json = ? WHERE session_id = ?
  `).run(JSON.stringify({
    id: "event_integrity",
    kind: "agent.event",
    timestamp: "2026-08-16T12:00:00.000Z",
    summary: "Tampered event",
    payload: null,
  }), "session_integrity");
  tamper.close();

  const integrityFailure = await runHostedOperationFailure(
    ["session", "event-details", "--session-id", "session_integrity", "--event-id", "event_integrity", "--json"],
    { ...process.env, APP_SERVER_DATABASE_PATH: integrityDatabasePath },
  );
  assert.equal(integrityFailure.error.code, "session_integrity_failed");
  assert.match(integrityFailure.error.message, /preserve the database/iu);

  const corruptDirectory = await mkdtemp(join(tmpdir(), "app-server-database-corrupt-"));
  const corruptDatabasePath = join(corruptDirectory, "memory.sqlite");
  await writeFile(corruptDatabasePath, "not a sqlite database");
  const corruptionFailure = await runHostedOperationFailure(
    ["session", "get", "--session-id", "session_corrupt", "--json"],
    { ...process.env, APP_SERVER_DATABASE_PATH: corruptDatabasePath },
  );
  assert.equal(corruptionFailure.error.code, "database_corrupt");
  assert.match(corruptionFailure.error.message, /restore a verified backup or run SQLite recovery/iu);
});

test("session cursor updates omit prior events and capture bodies", async () => {
  const directory = await mkdtemp(join(tmpdir(), "app-server-session-update-"));
  const databasePath = join(directory, "memory.sqlite");
  const store = new AppServerSessionStore({ databasePath });
  try {
    store.create({
      id: "session_update",
      workspaceId: "workspace_update",
      attemptId: "attempt_update",
      title: "Cursor update",
      prompt: "Return only new events.",
      model: "gpt-5.6-sol",
      reasoningEffort: "high",
    });
    store.appendEvent("session_update", {
      id: "event_before_cursor",
      kind: "agent.event",
      timestamp: "2026-08-16T12:00:00.000Z",
      summary: "Before cursor",
      payload: { output: "x".repeat(2 * 1024 * 1024) },
    });
    store.appendEvent("session_update", {
      id: "event_after_cursor",
      kind: "agent.event",
      timestamp: "2026-08-16T12:01:00.000Z",
      summary: "After cursor",
      payload: { output: "bounded" },
    });
  } finally {
    store.close();
  }

  const updated = await runHostedOperation(
    ["session", "get-update", "--session-id", "session_update", "--after-event-id", "event_before_cursor", "--json"],
    { ...process.env, APP_SERVER_DATABASE_PATH: databasePath },
  );
  assert.equal(updated.operation, "session.get_update");
  assert.deepEqual(updated.result.events.map((event) => event.id), ["event_after_cursor"]);
  assert.equal(updated.result.eventOffset, 1);
  assert.equal(Object.hasOwn(updated.result.session, "events"), false);
  assert.equal(Object.hasOwn(updated.result.session.attempts[0], "capture"), false);
  assert.ok(JSON.stringify(updated).length < 20_000);

  const appendPath = join(directory, "append.json");
  await writeFile(appendPath, JSON.stringify({
    id: "event_receipt",
    kind: "agent.event",
    timestamp: "2026-08-16T12:02:00.000Z",
    summary: "Compact append response",
    payload: { output: "receipt" },
  }));
  const appended = await runHostedOperation([
    "session", "append-event-receipt", "--session-id", "session_update", "--input", appendPath, "--json",
  ], { ...process.env, APP_SERVER_DATABASE_PATH: databasePath });
  assert.equal(appended.operation, "session.append_event_receipt");
  assert.equal(appended.result.sessionId, "session_update");
  assert.equal(appended.result.revision, 4);
  assert.ok(JSON.stringify(appended).length < 1_000);
});

test("session event, collaboration, capture, and nested trace reads are targeted and bounded", () => {
  const store = new AppServerSessionStore({ databasePath: ":memory:" });
  try {
    store.create({
      id: "session_targeted",
      workspaceId: "workspace_targeted",
      attemptId: "attempt_targeted",
      title: "Targeted reads",
      prompt: "Read only requested state.",
      model: "gpt-5.6-sol",
      reasoningEffort: "high",
    });
    store.appendEventReceipt("session_targeted", {
      id: "trace_batch_one",
      kind: "beale.trace_batch",
      timestamp: "2026-08-16T12:00:00.000Z",
      summary: "Trace batch",
      payload: { records: [{ id: "trace_nested", summary: "Nested trace", payload: { detail: true } }] },
    });
    store.appendEventReceipt("session_targeted", {
      id: "room_one_create",
      kind: "beale.breakout_room",
      timestamp: "2026-08-16T12:01:00.000Z",
      summary: "Room created",
      payload: { record: { id: "room_one", status: "active" } },
    });
    store.appendEventReceipt("session_targeted", {
      id: "room_one_complete",
      kind: "beale.breakout_room",
      timestamp: "2026-08-16T12:02:00.000Z",
      summary: "Room completed",
      payload: { record: { id: "room_one", status: "completed" } },
    });
    store.appendEventReceipt("session_targeted", {
      id: "event_after_nested",
      kind: "agent.event",
      timestamp: "2026-08-16T12:03:00.000Z",
      summary: "After nested cursor",
      payload: { retained: true },
    });
    store.appendEventReceipt("session_targeted", {
      id: "subagent_completed",
      kind: "agent.event",
      timestamp: "2026-08-16T12:04:00.000Z",
      summary: "Subagent completed",
      payload: { type: "subagent.activity", action: "completed", agentPath: "/root/reviewer" },
      agentPath: "/root/reviewer",
    });

    const update = store.getUpdate("session_targeted", "trace_nested");
    assert.deepEqual(update?.events.map((event) => event.id), [
      "room_one_create", "room_one_complete", "event_after_nested", "subagent_completed",
    ]);
    assert.equal(update?.nextAfterEventId, "subagent_completed");
    assert.equal(update?.hasMore, false);
    assert.deepEqual(store.getEventDetails("session_targeted", ["trace_nested"]).map((event) => event.id), [
      "trace_batch_one",
    ]);
    const collaboration = store.getCollaborationState("session_targeted");
    assert.equal(collaboration.rooms.length, 1);
    assert.equal(collaboration.rooms[0].payload.record.status, "completed");
    assert.deepEqual(collaboration.subagents.map((event) => event.id), ["subagent_completed"]);
    assert.deepEqual(store.getEventPage("session_targeted", { stream: "transcript" }).events, []);
  } finally {
    store.close();
  }
});

test("commentary event pages project transcripts and path-safe tool summaries", () => {
  const store = new AppServerSessionStore({ databasePath: ":memory:" });
  try {
    store.create({
      id: "session_commentary",
      workspaceId: "workspace_commentary",
      attemptId: "attempt_commentary",
      title: "Commentary projection",
      prompt: "Inspect the parser.",
      model: "gpt-5.6-sol",
      reasoningEffort: "high",
    });
    store.appendEventReceipt("session_commentary", {
      id: "tool_read",
      kind: "research.event",
      timestamp: "2026-08-22T20:00:00.000Z",
      summary: "research.event",
      payload: {
        event: {
          id: "tool_read",
          kind: "tool.requested",
          timestamp: "2026-08-22T20:00:00.000Z",
          payload: {
            toolActionId: "read-one",
            toolName: "file.read",
            normalizedInputs: { path: "/Users/alice/private/src/parser.ts" },
          },
          agentPath: "/root",
        },
      },
    });
    store.appendEventReceipt("session_commentary", {
      id: "tool_shell",
      kind: "research.event",
      timestamp: "2026-08-22T20:00:01.000Z",
      summary: "research.event",
      payload: {
        event: {
          id: "tool_shell",
          kind: "tool.requested",
          timestamp: "2026-08-22T20:00:01.000Z",
          payload: {
            toolActionId: "shell-one",
            toolName: "shell.run",
            normalizedInputs: { utility: "/usr/bin/rg", args: ["private-token", "/Users/alice/private"] },
          },
          agentPath: "/root",
        },
      },
    });
    store.appendEventReceipt("session_commentary", {
      id: "transcript_one",
      kind: "beale.transcript",
      timestamp: "2026-08-22T20:00:02.000Z",
      summary: "beale.transcript",
      payload: {
        record: {
          id: "message_one",
          runId: "session_commentary",
          role: "assistant",
          phase: "commentary",
          contentMarkdown: "The parser has one guarded entrypoint.",
          source: "app_server_commentary",
          metadata: { agentPath: "/root" },
          createdAt: "2026-08-22T20:00:02.000Z",
        },
      },
    });
    store.appendEventReceipt("session_commentary", {
      id: "model_commentary",
      kind: "model.output",
      timestamp: "2026-08-22T20:00:03.000Z",
      summary: "model.output",
      payload: {
        phase: "completed",
        messagePhase: "commentary",
        text: "Canonical commentary remains available without a Desktop attachment.",
        responseId: "response_two",
        itemId: "text:0",
        turn: 2,
        agentPath: "/root",
      },
      agentPath: "/root",
    });
    store.appendEventReceipt("session_commentary", {
      id: "model_reasoning",
      kind: "model.thought",
      timestamp: "2026-08-22T20:00:04.000Z",
      summary: "model.thought",
      payload: {
        phase: "completed",
        text: "The reviewer is checking the ownership boundary.",
        responseId: "response_reviewer",
        itemId: "thinking:0",
        turn: 1,
        agentPath: "/root/reviewer",
      },
      agentPath: "/root/reviewer",
    });

    const page = store.getEventPage("session_commentary", { stream: "commentary" });
    assert.equal(page.stream, "commentary");
    assert.deepEqual(page.events.map((event) => event.kind), [
      "beale.tool_summary", "beale.tool_summary", "beale.transcript", "beale.transcript", "beale.transcript",
    ]);
    assert.equal(page.events[0].payload.record.contentMarkdown, "Read parser.ts");
    assert.equal(page.events[0].payload.record.metadata.toolPluralTemplate, "Read {count} files");
    assert.equal(page.events[1].payload.record.contentMarkdown, "Ran rg");
    assert.equal(page.events[2].payload.record.contentMarkdown, "The parser has one guarded entrypoint.");
    assert.equal(page.events[3].payload.record.source, "app_server_commentary");
    assert.equal(page.events[3].payload.record.contentMarkdown, "Canonical commentary remains available without a Desktop attachment.");
    assert.equal(page.events[4].payload.record.source, "openai_reasoning_summary");
    assert.equal(page.events[4].payload.record.metadata.agentPath, "/root/reviewer");
    const serialized = JSON.stringify(page);
    assert.equal(serialized.includes("/Users/alice"), false);
    assert.equal(serialized.includes("private-token"), false);
  } finally {
    store.close();
  }
});

test("versioned hosted session operations import captures and serve the canonical query", async () => {
  const directory = await mkdtemp(join(tmpdir(), "app-server-session-protocol-"));
  const databasePath = join(directory, "memory.sqlite");
  const createPath = join(directory, "create.json");
  const capturePath = join(directory, "capture.json");
  await writeFile(createPath, JSON.stringify({
    id: "session_cli",
    workspaceId: "workspace_cli",
    attemptId: "attempt_cli",
    title: "CLI session",
    prompt: "Inspect the CLI.",
    model: "gpt-5.6-sol",
    reasoningEffort: "high",
  }));
  await writeFile(capturePath, JSON.stringify(captureFixture()));
  const env = { ...process.env, APP_SERVER_DATABASE_PATH: databasePath };

  const created = await runHostedOperation(["session", "create", "--input", createPath, "--json"], env);
  assert.equal(created.operation, "session.create");
  assert.equal(created.ok, true);
  const imported = await runHostedOperation([
    "session", "import-capture",
    "--session-id", "session_cli",
    "--attempt-id", "attempt_cli",
    "--capture", capturePath,
    "--json",
  ], env);
  assert.equal(imported.operation, "session.import_capture");
  const queried = await runHostedOperation(["session", "get", "--session-id", "session_cli", "--json"], env);
  assert.equal(queried.result.status, "completed");
  assert.equal(Object.hasOwn(queried.result, "finalResponse"), false);
  assert.equal(Object.hasOwn(queried.result, "events"), false);
  const update = await runHostedOperation(["session", "get-update", "--session-id", "session_cli", "--tail", "--json"], env);
  assert.equal(update.result.finalResponse, "The parser is safe.");
  const captures = await runHostedOperation(["session", "captures", "--session-id", "session_cli", "--json"], env);
  assert.equal(captures.result[0].attemptId, "attempt_cli");
  assert.equal(captures.result[0].eventStreams.timeline.count, 1);
  const capture = await runHostedOperation([
    "session", "capture", "--session-id", "session_cli", "--attempt-id", "attempt_cli", "--json",
  ], env);
  assert.equal(Object.hasOwn(capture.result.raw, "eventTimeline"), false);
  assert.equal(Object.hasOwn(capture.result.raw.agent.raw, "agentEvents"), false);
});

test("hosted session reads remain available while the runtime holds a write transaction", async () => {
  const directory = await mkdtemp(join(tmpdir(), "app-server-session-read-lock-"));
  const databasePath = join(directory, "memory.sqlite");
  const store = new AppServerSessionStore({ databasePath });
  store.create({
    id: "session_read_lock",
    workspaceId: "workspace_lock",
    attemptId: "attempt_read_lock",
    title: "Readable session",
    prompt: "Read while writing.",
    model: "gpt-5.6-sol",
    reasoningEffort: "high",
  });
  store.close();

  const writer = new DatabaseSync(databasePath);
  writer.exec("PRAGMA journal_mode = WAL; BEGIN IMMEDIATE;");
  writer.prepare("UPDATE app_server_sessions SET summary = summary WHERE id = ?").run("session_read_lock");
  try {
    const queried = await runHostedOperation(
      ["session", "get", "--session-id", "session_read_lock", "--json"],
      { ...process.env, APP_SERVER_DATABASE_PATH: databasePath },
    );
    assert.equal(queried.ok, true);
    assert.equal(queried.result.id, "session_read_lock");
  } finally {
    writer.exec("ROLLBACK;");
    writer.close();
  }
});

test("hosted session writers wait for a short competing writer instead of returning database locked", async () => {
  const directory = await mkdtemp(join(tmpdir(), "app-server-session-write-lock-"));
  const databasePath = join(directory, "memory.sqlite");
  const inputPath = join(directory, "event.json");
  const store = new AppServerSessionStore({ databasePath });
  store.create({
    id: "session_write_lock",
    workspaceId: "workspace_lock",
    attemptId: "attempt_write_lock",
    title: "Writable session",
    prompt: "Wait for the writer.",
    model: "gpt-5.6-sol",
    reasoningEffort: "high",
  });
  store.close();
  await writeFile(inputPath, JSON.stringify({
    id: "event_after_lock",
    kind: "agent.event",
    timestamp: "2026-08-16T12:00:00.000Z",
    summary: "Writer resumed.",
    payload: { eventType: "resumed" },
  }));

  const lockHolder = spawn(process.execPath, [
    "-e",
    "const { DatabaseSync } = require('node:sqlite'); const database = new DatabaseSync(process.argv[1]); database.exec('PRAGMA journal_mode = WAL; BEGIN IMMEDIATE;'); process.stdout.write('locked\\n'); setTimeout(() => { database.exec('ROLLBACK;'); database.close(); }, 250);",
    databasePath,
  ], { stdio: ["ignore", "pipe", "inherit"] });
  const lockClosed = once(lockHolder, "close");
  await once(lockHolder.stdout, "data");
  const appended = await runHostedOperation([
    "session", "append-event-receipt",
    "--session-id", "session_write_lock",
    "--input", inputPath,
    "--json",
  ], { ...process.env, APP_SERVER_DATABASE_PATH: databasePath });
  assert.equal(appended.ok, true);
  assert.equal(appended.result.sessionId, "session_write_lock");
  assert.equal(appended.result.status, "active");
  assert.equal(appended.result.revision, 2);
  assert.equal(Object.hasOwn(appended.result, "events"), false);
  await lockClosed;
});

async function runHostedOperation(args, env) {
  const operation = sessionOperation(args);
  const input = await sessionInput(args);
  const result = await invokeAppServerProtocol(operation, {
    args,
    ...(input !== undefined ? { input } : {}),
    storage: {
      databasePath: env.APP_SERVER_DATABASE_PATH,
      artifactDirectoryPath: env.APP_SERVER_ARTIFACT_DIRECTORY ?? join(env.APP_SERVER_DATABASE_PATH, "..", "artifacts"),
    },
  });
  return { protocol: "app-server", protocolVersion: 1, operation, ok: true, result };
}

async function runHostedOperationFailure(args, env) {
  try {
    await runHostedOperation(args, env);
    assert.fail("Expected the hosted session operation to fail.");
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    if (/database disk image is malformed|file is not a database|database corruption|SQLITE_CORRUPT|SQLITE_NOTADB/iu.test(detail)) {
      return { ok: false, error: {
        code: "database_corrupt",
        message: "app-server database integrity failed. Stop active writers and restore a verified backup or run SQLite recovery against the configured database before retrying.",
      } };
    }
    if (/failed (?:its|the) integrity check/iu.test(detail)) {
      return { ok: false, error: {
        code: "session_integrity_failed",
        message: "app-server session integrity validation failed. Stop active writers, preserve the database, and restore or repair the affected session data before retrying.",
      } };
    }
    throw error;
  }
}

function sessionOperation(args) {
  const command = args[1]?.replaceAll("-", "_");
  if (!command) throw new Error("A session command is required.");
  return `session.${command}`;
}

async function sessionInput(args) {
  const inputPath = option(args, "--input");
  if (inputPath) return JSON.parse(await readFile(inputPath, "utf8"));
  if (args[1] === "import-capture") {
    return {
      attemptId: option(args, "--attempt-id"),
      capture: JSON.parse(await readFile(requiredOption(args, "--capture"), "utf8")),
    };
  }
  return undefined;
}

function option(args, name) {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : undefined;
}

function requiredOption(args, name) {
  const value = option(args, name);
  if (!value) throw new Error(`${name} requires a value.`);
  return value;
}

function captureFixture() {
  return {
    schemaVersion: 5,
    capturedAt: "2026-08-15T12:01:00.000Z",
    request: { prompt: "Inspect the parser." },
    agent: {
      id: "agent_one",
      status: "complete",
      executorName: "fixture",
      startedAt: "2026-08-15T12:00:00.000Z",
      completedAt: "2026-08-15T12:01:00.000Z",
      outputText: "The parser is safe.",
      raw: {
        agentEvents: [
          { eventId: "diagnostic_one", type: "context_composed", turn: 1 },
          { eventId: "diagnostic_two", type: "turn_completed", turn: 1 },
        ],
      },
      finalDisposition: {
        outcome: "objective_achieved",
        summary: "Inspection complete.",
        externalStateRequired: false,
        blockerDependencies: [],
      },
    },
    eventTimeline: [{
      id: "event_two",
      kind: "agent.event",
      timestamp: "2026-08-15T12:00:30.000Z",
      summary: "Reviewed result",
      payload: { eventType: "assistant.message" },
    }],
  };
}

function profiledCapture(overrides = {}) {
  const profile = normalizeResearchProfile(DEFAULT_SECURITY_RESEARCH_PROFILE);
  return {
    ...captureFixture(),
    researchProfile: {
      schemaVersion: profile.schemaVersion,
      id: profile.id,
      version: profile.version,
      hash: researchProfileHash(profile),
      source: "bundled-default",
      workflowId: "discovery",
      snapshot: profile,
      ...overrides,
    },
  };
}
