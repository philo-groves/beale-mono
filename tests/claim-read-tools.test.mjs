import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import {
  FindingStore, MemoryGraphStore, ManagedToolPluginSession, MANAGED_TOOL_PLUGIN_IDS,
  createFindingTools, createResearchToolRegistry, createWorkspaceHistorySearchTool,
  managedToolPluginId, managedToolPluginOptions, projectModelToolResult,
} from "../packages/research-agent/dist/index.js";

async function fixture(t) {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "beale-claim-read-"));
  const context = { workspaceId: "workspace-example", subjectId: "subject-example", sessionId: "session-example" };
  const graph = new MemoryGraphStore({ workspaceRoot, context });
  const store = new FindingStore(graph);
  const registry = createResearchToolRegistry([...createFindingTools(store), createWorkspaceHistorySearchTool({ claimStore: store })]);
  t.after(async () => {
    store.close();
    graph.close();
    await rm(workspaceRoot, { recursive: true, force: true });
  });
  const execute = async (toolName, input) => (await registry.execute({ id: "read-example", toolName, actionClass: "recall", input })).result;
  const read = async (input) => {
    const result = await execute("claim.get", input);
    assert.equal(result.status, "complete", result.error?.message);
    return result.output;
  };
  const create = (overrides = {}) => store.create({
    title: "Example record formatting", summary: "Example formatting assessment.",
    classification: "general.result", rating: "informational", ...overrides,
  }, { provider: "example-provider", model: "example-model" }, "agent-example");
  return { workspaceRoot, context, store, registry, create, execute, read };
}

test("claim.get retrieves persisted evidence and audit context for the same ID before and after promotion", async (t) => {
  const { store, create, execute, read } = await fixture(t);
  let claim = create({ sourceRevision: "git:example-component:revision-one", environmentFingerprint: "environment:example:one" });
  const lead = await read({ id: claim.id });
  assert.equal(lead.projection, "lead");
  assert.equal(lead.claim.originSessionId, "session-example");
  assert.deepEqual(lead.claim.authors, [{ provider: "example-provider", model: "example-model" }]);
  assert.equal(lead.transitions.items[0].actorId, "agent-example");
  assert.equal(lead.evidence.total, 0);

  claim = store.transition(claim.id, {
    expectedRevision: claim.revision, toStatus: "observed", reason: "Reviewed the example format documentation.",
    evidence: [{ kind: "publication", referenceId: "https://example.test/format", contentHash: "sha256:example",
      summary: "The documentation describes the example format.", metadata: { edition: "example-edition" } }],
  }, undefined, "reviewer-example");
  claim = store.revise(claim.id, { expectedRevision: claim.revision, reason: "Clarified the assessment scope.", summary: "The assessment concerns documentation formatting." });
  const before = store.get(claim.id);
  const result = await read({ id: claim.id });
  const { evidence, transitions, duplicateClaims, ...overview } = before;
  assert.equal(result.projection, "finding");
  assert.deepEqual(result.claim, overview);
  assert.deepEqual(result.evidence.items, evidence);
  assert.deepEqual(result.transitions.items, transitions);
  assert.deepEqual(result.duplicates.items, duplicateClaims);
  const focusedOverview = await read({ id: claim.id, section: "overview" });
  assert.deepEqual(focusedOverview.claim, overview);
  assert.equal(focusedOverview.evidence, undefined);
  assert.equal(focusedOverview.transitions, undefined);
  assert.equal(focusedOverview.duplicates, undefined);
  assert.deepEqual(result.transitions.items[1].evidenceIds, [result.evidence.items[0].id]);
  assert.equal(result.evidence.items[0].sessionId, "session-example");
  assert.equal(result.evidence.items[0].actorId, "reviewer-example");
  assert.deepEqual(store.get(claim.id), before, "reads must not mutate durable state");
  const search = await execute("history.search", { types: ["claims"], query: "formatting" });
  assert.equal(search.output.results[0].id, claim.id);
  assert.match(search.output.recall, /claim.get/);
  const modelResult = projectModelToolResult(await execute("claim.get", { id: claim.id }));
  const modelText = modelResult.content.filter((item) => item.type === "text").map((item) => item.text).join("");
  assert.match(modelText, /sha256:example/);
  assert.match(modelText, /Clarified the assessment scope/);
  assert.doesNotMatch(modelText, /Tool result truncated|memory\.sqlite/);
});

test("claim.get pages evidence and transitions without repeating the overview and rejects mixed revisions", async (t) => {
  const { store, create, read, execute } = await fixture(t);
  let claim = create({ evidence: Array.from({ length: 23 }, (_, index) => ({
    kind: "publication", referenceId: `https://example.test/reference/${index}`, summary: `Example reference ${index}.`,
  })) });
  for (let index = 0; index < 12; index += 1) {
    claim = store.revise(claim.id, { expectedRevision: claim.revision, reason: `Example clarification ${index}.`, summary: `Example summary ${index}.` });
  }
  const first = await read({ id: claim.id });
  assert.equal(first.evidence.items.length, 10);
  assert.equal(first.transitions.items.length, 10);
  for (const section of ["evidence", "transitions"]) {
    const items = [...first[section].items];
    let offset = first[section].nextOffset;
    while (offset !== null) {
      const next = await read({ id: claim.id, section, offset, expectedReadRevision: first.readRevision });
      assert.equal(next.claim, undefined);
      assert.deepEqual(Object.keys(next).sort(), ["id", "revision", "readRevision", "projection", "counts", section].sort());
      items.push(...next[section].items);
      offset = next[section].nextOffset;
    }
    assert.deepEqual(items, claim[section]);
  }
  const missingRevision = await execute("claim.get", { id: claim.id, section: "evidence", offset: 10 });
  assert.notEqual(missingRevision.status, "complete");
  store.revise(claim.id, { expectedRevision: claim.revision, reason: "Another example clarification.", summary: "Updated example summary." });
  const stale = await execute("claim.get", { id: claim.id, section: "transitions", offset: 10, expectedReadRevision: first.readRevision });
  assert.notEqual(stale.status, "complete");
  assert.match(stale.error.message, /revision changed.*Restart at offset 0/);
});

test("claim.get retains duplicate identity and canonical parent links without exposing other workspaces", async (t) => {
  const { workspaceRoot, context, store, create, execute, read } = await fixture(t);
  const parent = create();
  const duplicate = create({ title: "Duplicate example formatting assessment" });
  store.markDuplicate(duplicate.id, { parentClaimId: parent.id, expectedRevision: duplicate.revision, reason: "Same example assessment." });
  const child = await read({ id: duplicate.id });
  assert.equal(child.id, duplicate.id);
  assert.equal(child.claim.duplicateOfClaimId, parent.id);
  const parentDetail = await read({ id: parent.id, section: "duplicates" });
  assert.equal(parentDetail.duplicates.items[0].id, duplicate.id);
  assert.equal(parentDetail.claim, undefined);
  store.undoDuplicate(duplicate.id, { expectedRevision: child.revision, reason: "Example records need separate tracking." });
  assert.equal(store.get(parent.id).revision, parentDetail.revision, "duplicate changes do not revise the parent itself");
  const staleDuplicates = await execute("claim.get", {
    id: parent.id, section: "duplicates", offset: 1, expectedReadRevision: parentDetail.readRevision,
  });
  assert.notEqual(staleDuplicates.status, "complete");
  assert.match(staleDuplicates.error.message, /read revision changed/);
  const otherGraph = new MemoryGraphStore({ workspaceRoot, context: { ...context, workspaceId: "workspace-other-example", subjectId: "subject-other-example" } });
  const otherStore = new FindingStore(otherGraph);
  try {
    const other = otherStore.create({ title: "Other workspace record", classification: "general.result", rating: "low" });
    for (const id of [other.id, "claim-missing-example"]) {
      const result = await execute("claim.get", { id });
      assert.notEqual(result.status, "complete");
      assert.match(result.error.message, /not found in this workspace/);
      assert.equal(result.output, undefined);
    }
  } finally {
    otherStore.close();
    otherGraph.close();
  }
});

test("lead and finding catalogs can read past the page limit and distinguish page cache revisions", async (t) => {
  const { store, create, execute } = await fixture(t);
  for (let index = 0; index < 6; index += 1) create({ title: `Example catalog record ${index}` });
  for (const toolName of ["lead.list", "finding.list"]) {
    if (toolName === "finding.list") {
      for (const claim of store.listLeads()) store.transition(claim.id, {
        expectedRevision: claim.revision, toStatus: "observed", reason: "Example documentation review.",
        evidence: [{ kind: "publication", referenceId: "https://example.test/catalog", summary: "Example catalog reference." }],
      });
    }
    const key = toolName === "lead.list" ? "leads" : "findings";
    const first = (await execute(toolName, { query: "catalog", limit: 2 })).output;
    const unchanged = (await execute(toolName, { query: "catalog", limit: 2, afterRevision: first.revision })).output;
    assert.equal(unchanged.unchanged, true);
    assert.equal(unchanged.matched, 6);
    assert.equal(unchanged.nextOffset, 2);
    const ids = first[key].map((claim) => claim.id);
    let offset = first.nextOffset;
    while (offset !== null) {
      const page = (await execute(toolName, { query: "catalog", limit: 2, offset, afterRevision: first.revision })).output;
      assert.equal(page.unchanged, false, "a different page must not use the first page's cache token");
      ids.push(...page[key].map((claim) => claim.id));
      offset = page.nextOffset;
    }
    assert.deepEqual(ids, store.list().map((claim) => claim.id));
    assert.equal(new Set(ids).size, 6);
    assert.match(first.recall, /claim.get/);
  }
});

test("claim.get stays in Claims with read-only permissions and validates page inputs", async (t) => {
  const { registry, create, execute } = await fixture(t);
  const claim = create();
  const tool = registry.find("claim.get");
  assert.equal(tool.descriptor.transportName, "claim_get");
  assert.equal(tool.descriptor.sideEffects, "read");
  assert.deepEqual(tool.descriptor.requiredPermissions, ["memory:read"]);
  assert.equal(managedToolPluginId("claim.get"), "beale-claims");
  const session = new ManagedToolPluginSession(managedToolPluginOptions(registry.listTools(), MANAGED_TOOL_PLUGIN_IDS));
  assert.equal(session.visible("claim_get"), false);
  await session.createLoader().execute({ id: "load-example", toolName: "plugins.load", actionClass: "recall", input: { plugins: ["beale-claims"] } });
  assert.equal(session.visible("claim_get"), true);
  for (const input of [{}, { id: " " }, { id: claim.id, limit: 0 }, { id: claim.id, limit: 51 },
    { id: claim.id, offset: -1 }, { id: claim.id, offset: 0.5 }, { id: claim.id, section: "unknown" }]) {
    assert.notEqual((await execute("claim.get", input)).status, "complete");
  }
});
