import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import test from "node:test";

import {
  CampaignTrackStore,
  createCampaignTrackAssignmentTools,
} from "../packages/research-agent/dist/index.js";

function openStore(root) {
  return new CampaignTrackStore({
    databasePath: join(root, "memory.sqlite"),
    context: {
      workspaceId: "workspace-example",
      workspaceName: "Example",
      subjectId: "subject-example",
      subjectName: "Example",
    },
  });
}

test("a new session does not attach through continuation words or resource similarity", async () => {
  const root = await mkdtemp(join(tmpdir(), "beale-track-assignment-"));
  const store = openStore(root);
  try {
    const existing = store.create({
      title: "Parser proof",
      objective: "Establish the parser proof chain.",
      originSessionId: "session-existing",
    });
    store.linkResource(existing.id, "runbook", "runbook_0123456789abcdef", "proof");

    const created = store.ensureForSession({
      sessionId: "session-new",
      objective: "Complete runbook_0123456789abcdef.",
      source: "runtime",
    });

    assert.notEqual(created.id, existing.id);
    assert.equal(store.getForSession("session-existing")?.id, existing.id);
    assert.equal(store.getForSession("session-new")?.id, created.id);
  } finally {
    store.close();
    await rm(root, { recursive: true, force: true });
  }
});

test("a session investigation assignment is immutable and idempotent", async () => {
  const root = await mkdtemp(join(tmpdir(), "beale-track-immutable-"));
  const store = openStore(root);
  try {
    const first = store.create({ title: "First proof", objective: "Prove the first behavior." });
    const second = store.create({ title: "Second proof", objective: "Prove a separate behavior." });
    store.linkSession(first.id, "session-example", {
      source: "reasoned",
      rationale: "Same concrete proof chain.",
    });
    store.linkSession(first.id, "session-example");

    assert.equal(store.getForSession("session-example")?.id, first.id);
    assert.throws(
      () => store.linkSession(second.id, "session-example"),
      /already assigned.*immutable/iu,
    );
    assert.equal(store.getForSession("session-example")?.id, first.id);
    const database = new DatabaseSync(join(root, "memory.sqlite"));
    try {
      assert.throws(() => database.prepare(`
        INSERT INTO campaign_track_sessions(
          investigation_id, session_id, linked_at, assignment_source, assignment_rationale
        ) VALUES (?, ?, ?, 'bypass', '')
      `).run(second.id, "session-example", new Date().toISOString()), /UNIQUE constraint failed/iu);
    } finally {
      database.close();
    }
  } finally {
    store.close();
    await rm(root, { recursive: true, force: true });
  }
});

test("resuming an archived investigation keeps the original session assignment", async () => {
  const root = await mkdtemp(join(tmpdir(), "beale-track-archived-"));
  const store = openStore(root);
  try {
    const track = store.create({
      title: "Archived proof",
      objective: "Preserve the original assignment.",
      originSessionId: "session-example",
    });
    store.updateTrack(track.id, track.revision, { status: "archived" });

    const resumed = store.ensureForSession({
      sessionId: "session-example",
      objective: "Resume this session.",
      source: "runtime",
    });
    assert.equal(resumed.id, track.id);
  } finally {
    store.close();
    await rm(root, { recursive: true, force: true });
  }
});

test("the root agent assigns an oriented session exactly once", async () => {
  const root = await mkdtemp(join(tmpdir(), "beale-track-reasoned-"));
  const store = openStore(root);
  let activeId = null;
  try {
    const candidate = store.create({
      title: "Existing parser proof",
      objective: "Prove the existing parser boundary.",
    });
    const tools = createCampaignTrackAssignmentTools(
      store,
      "session-reasoned",
      "Investigate an independent decoder boundary.",
      { get: () => activeId, assign: (id) => { activeId = id; } },
    );
    const candidates = tools.find((tool) => tool.descriptor.name === "investigation.candidates");
    const assign = tools.find((tool) => tool.descriptor.name === "investigation.assign");
    const blind = await assign.execute({
      id: "blind-assignment",
      toolName: "investigation.assign",
      actionClass: "synthesize",
      input: {
        decision: "attach",
        investigationId: candidate.id,
        orientationSummary: "No candidate inspection was performed.",
        rationale: "Blind selection.",
      },
    }, { agentId: "root" });
    assert.equal(blind.status, "error");
    assert.match(blind.error.message, /investigation\.candidates/iu);
    const listed = await candidates.execute({
      id: "list-candidates",
      toolName: "investigation.candidates",
      actionClass: "recall",
      input: {},
    }, { agentId: "root" });
    assert.equal(listed.status, "complete");
    assert.equal(listed.output[0].id, candidate.id);
    const result = await assign.execute({
      id: "assign-example",
      toolName: "investigation.assign",
      actionClass: "synthesize",
      input: {
        decision: "create",
        title: "Independent decoder proof",
        objective: "Establish the decoder boundary with independent evidence.",
        orientationSummary: "Inspected the source boundary and the existing candidate details.",
        rationale: "The decoder is a distinct mechanism and requires a different proof outcome.",
      },
    }, { agentId: "root" });

    assert.equal(result.status, "complete");
    assert.notEqual(activeId, candidate.id);
    assert.equal(store.getForSession("session-reasoned")?.id, activeId);

    const repeated = await assign.execute({
      id: "assign-again",
      toolName: "investigation.assign",
      actionClass: "synthesize",
      input: {
        decision: "attach",
        investigationId: candidate.id,
        orientationSummary: "Reconsidered the candidate.",
        rationale: "Attempted to change the decision.",
      },
    }, { agentId: "root" });
    assert.equal(repeated.status, "error");
    assert.match(repeated.error.message, /cannot be changed/iu);
    assert.equal(store.getForSession("session-reasoned")?.id, activeId);
  } finally {
    store.close();
    await rm(root, { recursive: true, force: true });
  }
});
