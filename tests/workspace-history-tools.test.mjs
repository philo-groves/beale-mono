import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  createResearchStorageLayout,
  createResearchToolRegistry,
  createWorkspaceHistorySearchTool,
  createWorkspaceHistoryDuplicateTools,
  ensureResearchStorageLayout,
  MemoryGraphStore,
  ResearchClaimStore,
  RunbookStore,
} from "../packages/research-agent/dist/index.js";

test("workspace history search unifies canonical claims, memories, and runbooks with multi-type filters", async () => {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "beale-workspace-history-"));
  const context = {
    sessionId: "session_history",
    workspaceId: "workspace_history",
    workspaceName: "History",
    subjectId: "subject_history",
    subjectName: "History",
  };
  const memory = new MemoryGraphStore({ workspaceRoot, context });
  const claims = new ResearchClaimStore(memory);
  const runbooks = new RunbookStore(
    memory.databasePath,
    ensureResearchStorageLayout(createResearchStorageLayout({ workspaceRoot })),
    context,
  );
  try {
    const memoryNode = memory.save({
      type: "invariant",
      title: "Parser boundary",
      summary: "The parser trusts a frame length before allocation.",
    });
    let parent = claims.create({
      title: "Parser frame length controls allocation",
      summary: "An unchecked frame length reaches the parser allocator.",
      classification: "security.primitive",
      rating: "high",
    });
    parent = claims.transition(parent.id, {
      expectedRevision: parent.revision,
      toStatus: "observed",
      reason: "The allocator trace directly observed frame-length control.",
      evidence: [{ kind: "code", referenceId: "artifact_parser_trace", summary: "Allocator trace records the unchecked length." }],
    });
    const duplicate = claims.create({
      title: "Unchecked parser allocation length",
      summary: "The same frame length reaches the same allocation.",
      classification: "security.primitive",
      rating: "high",
    });
    claims.markDuplicate(duplicate.id, {
      expectedRevision: duplicate.revision,
      parentClaimId: parent.id,
      reason: "Same parser boundary and allocation behavior.",
    });
    const runbook = runbooks.create({
      title: "Parser allocation reproduction",
      purpose: "Reproduce and classify the parser frame-length allocation behavior.",
    }).runbook;

    const registry = createResearchToolRegistry([
      createWorkspaceHistorySearchTool({ memoryStore: memory, claimStore: claims, runbookStore: runbooks }),
    ]);
    const descriptor = registry.listDescriptors()[0];
    assert.equal(descriptor.name, "history.search");
    assert.equal(descriptor.transportName, "history_search");
    assert.deepEqual(descriptor.inputSchema.properties.types.items.enum, ["claims", "memories", "runbooks"]);

    const all = await registry.execute({
      id: "history_all",
      actionClass: "recall",
      toolName: "history.search",
      input: { query: "parser allocation", limit: 20 },
    }, { agentId: "root" });
    assert.equal(all.result.status, "complete");
    assert.deepEqual(new Set(all.result.output.results.map((result) => result.type)), new Set(["claim", "memory", "runbook"]));
    assert.equal(all.result.output.counts.claims, 1);
    assert.equal(all.result.output.counts.memories, 1);
    assert.equal(all.result.output.counts.runbooks, 1);
    assert.equal(all.result.output.results.some((result) => result.id === duplicate.id), false);
    assert.equal(all.result.output.results.some((result) => result.id === parent.id && result.duplicateCount === 1), true);
    assert.equal(all.result.output.results.find((result) => result.id === parent.id)?.projection, "finding");
    assert.equal(all.result.output.results.find((result) => result.id === parent.id)?.evidenceRefs[0].referenceId, "artifact_parser_trace");

    const selected = await registry.execute({
      id: "history_selected",
      actionClass: "recall",
      toolName: "history.search",
      input: { query: "parser", types: ["claims", "runbooks"] },
    }, { agentId: "peer" });
    assert.deepEqual(new Set(selected.result.output.results.map((result) => result.type)), new Set(["claim", "runbook"]));
    assert.equal(selected.result.output.counts.memories, 0);
    assert.equal(selected.result.output.results.some((result) => result.id === memoryNode.id), false);
    assert.equal(selected.result.output.results.some((result) => result.id === runbook.id), true);

    const unchanged = await registry.execute({
      id: "history_unchanged",
      actionClass: "recall",
      toolName: "history.search",
      input: { query: "parser allocation", limit: 20, afterRevision: all.result.output.revision },
    });
    assert.equal(unchanged.result.output.unchanged, true);
    assert.deepEqual(unchanged.result.output.results, []);
  } finally {
    runbooks.close();
    claims.close();
    memory.close();
    await rm(workspaceRoot, { recursive: true, force: true });
  }
});

test("workspace history duplicate tools coalesce and restore memories and runbooks", async () => {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "beale-workspace-history-deduplication-"));
  const context = {
    sessionId: "session_history_deduplication",
    workspaceId: "workspace_history_deduplication",
    workspaceName: "History deduplication",
    subjectId: "subject_history_deduplication",
    subjectName: "History deduplication",
  };
  const memory = new MemoryGraphStore({ workspaceRoot, context });
  const claims = new ResearchClaimStore(memory);
  const runbooks = new RunbookStore(
    memory.databasePath,
    ensureResearchStorageLayout(createResearchStorageLayout({ workspaceRoot })),
    context,
  );
  try {
    const parentMemory = memory.save({ type: "invariant", title: "Canonical parser boundary" });
    const duplicateMemory = memory.save({ type: "invariant", title: "Repeated parser boundary" });
    const parentRunbook = runbooks.create({ title: "Canonical parser procedure", purpose: "Exercise the parser boundary." }).runbook;
    const duplicateRunbook = runbooks.create({ title: "Repeated parser procedure", purpose: "Exercise the same parser boundary." }).runbook;
    const registry = createResearchToolRegistry([
      createWorkspaceHistorySearchTool({ memoryStore: memory, claimStore: claims, runbookStore: runbooks }),
      ...createWorkspaceHistoryDuplicateTools({ memoryStore: memory, claimStore: claims, runbookStore: runbooks }),
    ]);
    assert.deepEqual(
      registry.listDescriptors().map((descriptor) => descriptor.name),
      ["history.search", "history.mark_duplicate", "history.undo_duplicate"],
    );

    for (const item of [
      { type: "memory", id: duplicateMemory.id, parentId: parentMemory.id, revision: duplicateMemory.revision },
      { type: "runbook", id: duplicateRunbook.id, parentId: parentRunbook.id, revision: duplicateRunbook.revision },
    ]) {
      const result = await registry.execute({
        id: `mark_${item.type}`,
        actionClass: "synthesize",
        toolName: "history.mark_duplicate",
        input: { type: item.type, id: item.id, parentId: item.parentId, expectedRevision: item.revision, reason: "Same underlying record." },
      });
      assert.equal(result.result.status, "complete");
    }
    assert.deepEqual(memory.search({ scope: "workspace", limit: 20 }).map((node) => node.id), [parentMemory.id]);
    assert.equal(memory.get(parentMemory.id).duplicateMemories[0].id, duplicateMemory.id);
    assert.deepEqual(runbooks.list({ limit: 20 }).map((runbook) => runbook.id), [parentRunbook.id]);
    assert.equal(runbooks.get(parentRunbook.id).duplicateRunbooks[0].id, duplicateRunbook.id);
    assert.throws(() => memory.correct(duplicateMemory.id, 2, { summary: "Hidden edit" }), /canonical memory/);
    assert.throws(() => runbooks.append({ id: duplicateRunbook.id, expectedRevision: 2, cells: [{ kind: "markdown", source: "Hidden edit" }] }), /canonical runbook/);

    for (const item of [
      { type: "memory", id: duplicateMemory.id },
      { type: "runbook", id: duplicateRunbook.id },
    ]) {
      const result = await registry.execute({
        id: `undo_${item.type}`,
        actionClass: "synthesize",
        toolName: "history.undo_duplicate",
        input: { type: item.type, id: item.id, expectedRevision: 2, reason: "The records differ after review." },
      });
      assert.equal(result.result.status, "complete");
    }
    assert.equal(memory.search({ scope: "workspace", limit: 20 }).length, 2);
    assert.equal(runbooks.list({ limit: 20 }).length, 2);
  } finally {
    runbooks.close();
    claims.close();
    memory.close();
    await rm(workspaceRoot, { recursive: true, force: true });
  }
});
