import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import {
  AppServerSessionStore,
  generateStoredSteeringSuggestion,
  normalizeSteeringSuggestion,
} from "../packages/research-agent/dist/index.js";

test("steering suggestions use the configured small model and canonical session context", async () => {
  const root = await mkdtemp(join(tmpdir(), "beale-steering-suggestion-"));
  const databasePath = join(root, "memory.sqlite");
  const store = new AppServerSessionStore({ databasePath });
  try {
    store.create({
      id: "session-example",
      workspaceId: "workspace-example",
      attemptId: "attempt-example",
      title: "Parser boundary review",
      prompt: "Inspect a synthetic parser boundary.",
      model: "large-example-model",
      reasoningEffort: "high",
    });
    store.appendEventReceipt("session-example", {
      id: "message-example",
      kind: "beale.transcript",
      timestamp: "2026-09-01T12:00:00.000Z",
      summary: "User steering",
      payload: { record: {
        id: "message-example",
        runId: "session-example",
        role: "user",
        contentMarkdown: "Check the adjacent length field.",
        metadata: { agentPath: "/root" },
      } },
    });
  } finally {
    store.close();
  }
  const calls = [];
  const input = {
    workspaceId: "workspace-example",
    sessionId: "session-example",
    workspaceRoot: root,
    databasePath,
    provider: { id: "openai-codex", smallModel: "small-example-model" },
  };
  try {
    const result = await generateStoredSteeringSuggestion(input, {
      completeText: async (options) => {
        calls.push(options);
        return { text: "Check whether the adjacent length field crosses the same parser boundary." };
      },
    });
    assert.deepEqual(result, { suggestion: "Check whether the adjacent length field crosses the same parser boundary." });
    assert.equal(calls[0].model, "small-example-model");
    assert.equal(calls[0].effort, "low");
    assert.match(calls[0].prompt, /Check the adjacent length field\./);
    await assert.rejects(
      generateStoredSteeringSuggestion({ ...input, workspaceId: "workspace-other" }, {
        completeText: async () => { throw new Error("The model must not be called for another workspace."); },
      }),
      /Session not found in the active workspace/,
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("steering suggestions preserve a complete sentence and reject fragments", () => {
  assert.equal(normalizeSteeringSuggestion("Inspect the adjacent parser length field before closing this lead"),
    "Inspect the adjacent parser length field before closing this lead.");
  assert.equal(normalizeSteeringSuggestion("Inspect the adjacent parser. Then continue elsewhere."), null);
  assert.equal(normalizeSteeringSuggestion("Inspect the adjacent parser and"), null);
});
