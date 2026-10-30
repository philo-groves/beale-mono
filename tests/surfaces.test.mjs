import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import test from "node:test";
import {
  createFindingTools,
  createMemoryGraphTools,
  createSurfaceTools,
  createToolObservedEvent,
  FindingStore,
  initializeWorkspaceProject,
  MemoryGraphStore,
  publishWorkspaceResearch,
  rebuildWorkspaceResearchIndex,
  releaseWorkspaceResearchIndex,
  ResearchResourceCatalog,
  ResearchToolRegistry,
  SubagentManager,
  SurfaceStore,
} from "../packages/research-agent/dist/index.js";

const workspaceId = "workspace-example-surfaces";
const foundation = {
  discovery: `Synthetic reconnaissance note. ${"Context ".repeat(650)}`,
  techStack: [{ technology: "Example Parser", role: "Request parsing", version: "2.x", environment: "Example service",
    evidence: "Synthetic package manifest", confidence: "observed" }],
  assets: [{ name: "src/request_parser.ts", status: "untouched" },
    { name: "parseExampleRequest()", status: "in_progress" },
    { name: "https://example.test/parser", status: "complete" }],
  bugHistory: { summary: "Earlier bounded search note.", limits: "Only the dated public sources inspected so far.", entries: [
    { identifier: "EXAMPLE-ADVISORY-001", impact: "Malformed requests may bypass validation.", affectedVersions: "1.x",
      reference: "https://example.test/advisory", notes: "Applicability to 2.x is unknown." }
  ] },
  coverage: { summary: "Reviewed one parser path.", gaps: "Alternate input modes remain unexamined.", opportunities: "Check the boundary before dispatch." },
  threatModel: { diagram: 'flowchart LR\n  client["Client"] --> parser["Request Parser"]\n  parser --> state["Service State"]',
    securityObjectives: "Preserve service state integrity.", riskImpact: "Unauthorized state change.",
    assumptions: "Reachability requires the enabled service.", invariants: "Only validated requests change state.",
    sinks: "State update operation.", mitigations: "Input validation before dispatch.",
    attackerControl: "Supplied request bytes.", trustBoundaries: "Untrusted request to privileged parser.",
    assetsAtRisk: "ExampleCo service state.", plausibleImpacts: "Unauthorized state change if parsing fails open." },
};
const completeFoundation = {
  ...foundation,
  coverage: { ...foundation.coverage, gaps: "", mappingComplete: true, sinksInvestigated: true,
    openQuestions: [], unexploredConnections: [] },
};

function fixture() {
  const directory = mkdtempSync(join(tmpdir(), "beale-surfaces-example-"));
  const databasePath = join(directory, "memory.sqlite");
  const resources = new ResearchResourceCatalog({
    databasePath, workspaceId,
    explicitResources: [
      { id: "asset-example", direction: "in_scope", kind: "service", locator: "service.example.test", name: "Example service", source: "explicit_scope" },
      { id: "asset-excluded", direction: "out_of_scope", kind: "service", locator: "excluded.example.test", name: "Excluded service", source: "explicit_scope" },
    ],
  });
  const surface = new SurfaceStore({ databasePath, workspaceId, sessionId: "session-example-one", resources });
  return { directory, databasePath, resources, surface, rootId: resources.list().find((item) => item.direction === "in_scope").id,
    close() { surface.close(); resources.close(); rmSync(directory, { recursive: true, force: true }); } };
}

test("surface scope assessments require evidence and survive partial updates", async () => {
  const state = fixture();
  try {
    const root = state.surface.require(state.rootId);
    assert.throws(() => state.surface.update(root.id, root.revision, {
      scopeAssessment: { decision: "not_applicable", basis: "Synthetic example only.", references: [] }
    }), /references/);
    const assessed = state.surface.update(root.id, root.revision, {
      scopeAssessment: { decision: "not_applicable", basis: "The example policy excludes this demonstration component.",
        references: ["policy-example-section"] }
    });
    assert.equal(assessed.foundation.scopeAssessment.decision, "not_applicable");
    const updated = state.surface.update(root.id, assessed.revision, { discovery: "Further synthetic context." });
    assert.equal(updated.foundation.scopeAssessment.references[0], "policy-example-section");
    const cleared = state.surface.update(root.id, updated.revision, { scopeAssessment: null });
    assert.equal(cleared.foundation.scopeAssessment, null);
    const tool = createSurfaceTools(state.surface).find((entry) => entry.descriptor.name === "surface.update");
    assert.deepEqual(tool.descriptor.inputSchema.properties.foundation.properties.scopeAssessment.anyOf[0]
      .properties.decision.enum, ["applicable", "not_applicable"]);
    const result = await tool.execute({ id: "action-example-scope", toolName: "surface.update",
      actionClass: "synthesize", input: { surfaceId: root.id, expectedRevision: cleared.revision,
        foundation: { scopeAssessment: { decision: "applicable", basis: "The example policy includes this service.",
          references: ["policy-example-section"] } } } });
    assert.equal(result.status, "complete");
    assert.equal(state.surface.get(root.id).foundation.scopeAssessment.decision, "applicable");
    const breakdown = createSurfaceTools(state.surface).find((entry) => entry.descriptor.name === "surface.breakdown");
    const view = await breakdown.execute({ id: "action-example-scope-view", toolName: "surface.breakdown",
      actionClass: "observe", input: { surfaceId: root.id } });
    assert.equal(view.output.scopeAssessment.decision, "applicable");
  } finally { state.close(); }
});

test("surface rankings require grounded 1–10 factors and survive partial updates", async () => {
  const state = fixture();
  const ranking = {
    impact: { score: 8, basis: "Example service state could be modified." },
    reachability: { score: 7, basis: "The synthetic API accepts untrusted requests." },
    relevance: { score: 9, basis: "The service is in the example authorized scope." },
    exposure: { score: 8, basis: "The parser runs before example authentication." },
    precedence: { score: 4, basis: "One older advisory concerns a related parser version." },
  };
  try {
    assert.throws(() => state.surface.update(state.rootId, 1,
      { ranking: { ...ranking, impact: { ...ranking.impact, score: 0 } } }), /1 to 10/);
    assert.throws(() => state.surface.update(state.rootId, 1,
      { ranking: { ...ranking, precedence: { ...ranking.precedence, basis: " " } } }), /basis/);
    const scored = state.surface.update(state.rootId, 1, { ranking });
    const revised = state.surface.update(state.rootId, scored.revision, { discovery: "Synthetic follow-up." });
    assert.deepEqual(revised.foundation.ranking, ranking);
    const breakdownTool = createSurfaceTools(state.surface).find((tool) => tool.descriptor.name === "surface.breakdown");
    const result = await breakdownTool.execute({ id: "action-example-ranking", toolName: "surface.breakdown",
      actionClass: "observe", input: { surfaceId: state.rootId } });
    assert.equal(result.status, "complete");
    assert.deepEqual(result.output.ranking, ranking);
    const restored = new SurfaceStore({ databasePath: state.databasePath, workspaceId, resources: state.resources });
    try { assert.deepEqual(restored.get(state.rootId).foundation.ranking, ranking); }
    finally { restored.close(); }
  } finally { state.close(); }
});

test("successful surface diagram updates emit a session-visible diagram marker", async () => {
  const state = fixture();
  try {
    const tool = createSurfaceTools(state.surface).find((entry) => entry.descriptor.name === "surface.update");
    assert.ok(tool);
    const source = 'flowchart LR\n  input["Example input"] --> parser["Example parser"]';
    const action = { id: "action-example-diagram", toolName: "surface.update", actionClass: "synthesize",
      input: { surfaceId: state.rootId, expectedRevision: 1, foundation: { threatModel: { diagram: source } } } };
    const result = await tool.execute(action);
    assert.equal(result.status, "complete");
    assert.deepEqual(createToolObservedEvent(result).payload.surfaceDiagram,
      { source, surfaceId: state.rootId, surfaceName: "Example service", changeKind: "created" });
    const unchanged = await tool.execute({ ...action, input: { surfaceId: state.rootId, expectedRevision: 2,
      foundation: { discovery: "Example context update." } } });
    assert.equal(unchanged.status, "complete");
    assert.equal(createToolObservedEvent(unchanged).payload.surfaceDiagram, undefined);
    const revisedSource = 'flowchart LR\n  input["Example input"] --> validator["Example validator"]';
    const revised = await tool.execute({ ...action, input: { surfaceId: state.rootId, expectedRevision: 3,
      foundation: { threatModel: { diagram: revisedSource } } } });
    assert.equal(revised.status, "complete");
    assert.deepEqual(createToolObservedEvent(revised).payload.surfaceDiagram,
      { source: revisedSource, surfaceId: state.rootId, surfaceName: "Example service", changeKind: "updated" });
  } finally { state.close(); }
});

test("resource roots, agent-discovered children, lookup, and one-time first touch share workspace state", () => {
  const state = fixture();
  try {
    const root = state.surface.get(state.rootId);
    assert.equal(root.id, state.rootId);
    assert.equal(root.root, true);
    assert.equal(root.firstTouchedAt, null);
    assert.equal(state.surface.list().total, 2);
    assert.equal(state.surface.list().surfaces.find((item) => item.id === root.id).focusEligible, true);
    assert.equal(state.surface.list({ resourceKind: 'service' }).total, 2);
    assert.equal(state.surface.list({ resourceKind: 'repository' }).total, 0);
    assert.equal(state.surface.list().surfaces.find((item) => item.id === root.id).resourceKind, 'service');

    const child = state.surface.create(root.id, "Request parser", "Lower-trust request parser boundary.");
    assert.equal(child.parentId, root.id);
    assert.equal(child.resourceId, root.id);
    assert.throws(() => state.surface.create(root.id, "request parser", "Duplicate boundary"), /UNIQUE constraint/);
    assert.equal(state.surface.path(child.id), "Example service / Request parser");
    assert.equal(state.surface.query("request parser").surfaces[0].id, child.id);
    assert.equal(state.surface.query("parser", 1, 1).surfaces.length, 0);
    assert.equal(state.surface.list({ resourceId: root.id, limit: 1 }).nextOffset, 1);

    const first = state.surface.focus("agent-one", child.id);
    assert.equal(first.firstTouch, true);
    assert.deepEqual(first.surfaceTrail, [
      { id: root.id, name: "Example service" }, { id: child.id, name: "Request parser" },
    ]);
    assert.equal(first.previousSurfaceId, null);
    assert.equal(state.surface.focus("agent-two", child.id).firstTouch, false);
    assert.equal(state.surface.focus("agent-one", root.id).firstTouch, true);
    assert.equal(state.surface.focus("agent-one", child.id).firstTouch, false);
    assert.equal(state.surface.current("agent-one").id, child.id);
    assert.equal(state.surface.modelContext().counts.pendingFoundation, 2);
    const rootUpdated = state.surface.update(root.id, root.revision, foundation);
    assert.equal(rootUpdated.revision, 2);
    assert.equal(state.surface.get(root.id).foundation.bugHistory.summary, foundation.bugHistory.summary);
    assert.equal(state.surface.get(root.id).foundation.discovery.length, foundation.discovery.trim().length);
    assert.equal(state.surface.get(root.id).foundation.techStack[0].technology, "Example Parser");
    assert.equal(state.surface.get(root.id).foundation.assets[1].status, "in_progress");
    assert.equal(state.surface.get(root.id).foundation.bugHistory.entries[0].identifier, "EXAMPLE-ADVISORY-001");

    const updated = state.surface.update(child.id, child.revision, foundation);
    assert.equal(updated.revision, 2);
    assert.equal(state.surface.query("parseExampleRequest").surfaces.some((item) => item.id === child.id), true);
    assert.equal(state.surface.query("privileged parser").surfaces.some((item) => item.id === child.id), true);
    assert.equal(state.surface.query("EXAMPLE-ADVISORY-001").surfaces.some((item) => item.id === child.id), true);
    assert.throws(() => state.surface.update(child.id, child.revision, foundation), /revision changed/i);

    const otherSession = new SurfaceStore({ databasePath: state.databasePath, workspaceId, sessionId: "session-example-two", resources: state.resources });
    try {
      assert.equal(otherSession.focus("agent-three", child.id).firstTouch, false);
      assert.equal(otherSession.get(child.id).foundation.coverage.gaps, foundation.coverage.gaps);
    } finally { otherSession.close(); }

    const excludedId = state.resources.list().find((item) => item.direction === "out_of_scope").id;
    assert.equal(state.surface.list().surfaces.find((item) => item.id === excludedId).focusEligible, false);
    assert.throws(() => state.surface.create(excludedId, "Child", "Disallowed"), /not eligible/i);
    assert.throws(() => state.surface.focus("agent-one", excludedId), /not eligible/i);
    const database = new DatabaseSync(state.databasePath, { readOnly: true });
    try {
      assert.equal(database.prepare("SELECT COUNT(*) AS count FROM app_server_surface_touches").get().count, 2);
      assert.equal(database.prepare("SELECT COUNT(*) AS count FROM app_server_surface_focus_events").get().count, 5);
      assert.equal(database.prepare("SELECT COUNT(*) AS count FROM app_server_surface_update_events").get().count, 2);
    } finally { database.close(); }
  } finally { state.close(); }
});

test("recon lists preserve partial updates and validate structured entries", () => {
  const state = fixture();
  try {
    const root = state.surface.get(state.rootId);
    const legacy = state.surface.update(root.id, root.revision, { discovery: "Earlier reconnaissance." });
    assert.deepEqual(legacy.foundation.assets, []);
    const first = state.surface.update(root.id, legacy.revision, foundation);
    const second = state.surface.update(root.id, first.revision, { discovery: "Updated discovery note." });
    assert.equal(second.foundation.discovery, "Updated discovery note.");
    assert.equal(second.foundation.techStack[0].technology, "Example Parser");
    assert.deepEqual(second.foundation.assets, foundation.assets);
    assert.equal(second.foundation.bugHistory.entries[0].identifier, "EXAMPLE-ADVISORY-001");
    assert.throws(() => state.surface.update(root.id, second.revision, { techStack: [{ role: "Parser" }] }), /technology/);
    assert.throws(() => state.surface.update(root.id, second.revision, { assets: [{ name: "example.ts", status: "queued" }] }), /status is invalid/);
    assert.throws(() => state.surface.update(root.id, second.revision, { assets: [{ status: "complete" }] }), /name/);
    assert.throws(() => state.surface.update(root.id, second.revision,
      { bugHistory: { entries: [{ identifier: "EXAMPLE-ADVISORY-002" }] } }), /impact/);
    const progressed = state.surface.update(root.id, second.revision, { assets: [
      { name: "src/request_parser.ts", status: "complete" }, { name: "parseExampleRequest()" }
    ] });
    assert.deepEqual(progressed.foundation.assets, [
      { name: "src/request_parser.ts", status: "complete" }, { name: "parseExampleRequest()", status: "untouched" }
    ]);
    const cleared = state.surface.update(root.id, progressed.revision, { techStack: [], assets: [], bugHistory: { entries: [] } });
    assert.deepEqual(cleared.foundation.techStack, []);
    assert.deepEqual(cleared.foundation.assets, []);
    assert.deepEqual(cleared.foundation.bugHistory.entries, []);
    assert.equal(cleared.foundation.bugHistory.summary, foundation.bugHistory.summary);
    const signaled = state.surface.update(root.id, cleared.revision, { highValueSignals: [
      { kind: "pre_auth", basis: "The example request is accepted before authentication." },
    ] });
    assert.deepEqual(signaled.foundation.highValueSignals, [
      { kind: "pre_auth", basis: "The example request is accepted before authentication." },
    ]);
    assert.throws(() => state.surface.update(root.id, signaled.revision, { highValueSignals: [
      { kind: "pre_auth", basis: "First example basis." },
      { kind: "pre_auth", basis: "Duplicate example basis." },
    ] }), /invalid or repeated/);
  } finally { state.close(); }
});

test("threat models preserve older notes while accepting partial diagram updates", () => {
  const state = fixture();
  try {
    const root = state.surface.get(state.rootId);
    const legacy = state.surface.update(root.id, root.revision, { threatModel: {
      attackerControl: "Example request bytes.", trustBoundaries: "Request to parser.",
      assetsAtRisk: "Example state.", plausibleImpacts: "Unauthorized update.", assumptions: "Service enabled."
    } });
    assert.equal(legacy.foundation.threatModel.riskImpact, "Unauthorized update.");
    assert.deepEqual(legacy.foundation.threatModel.risks, ["Unauthorized update."]);
    assert.deepEqual(legacy.foundation.threatModel.assumptions, ["Service enabled."]);
    assert.equal(legacy.foundation.threatModel.diagram, "");
    const database = new DatabaseSync(state.databasePath);
    try {
      const row = database.prepare("SELECT foundation_json FROM app_server_surface_roots WHERE resource_id=?").get(root.id);
      const stored = JSON.parse(row.foundation_json);
      delete stored.threatModelReviewed;
      database.prepare("UPDATE app_server_surface_roots SET foundation_json=? WHERE resource_id=?")
        .run(JSON.stringify(stored), root.id);
    } finally { database.close(); }
    assert.equal(state.surface.get(root.id).foundation.threatModelReviewed, true);
    const updated = state.surface.update(root.id, legacy.revision, { threatModel: {
      diagram: 'flowchart LR\n  input["Request"] --> parser["Parser"]',
      securityObjectives: ["Preserve example state.", "Reject invalid requests."],
      risks: ["Unauthorized state change."], assumptions: ["Example service is enabled."]
    } });
    assert.equal(updated.foundation.threatModel.attackerControl, "Example request bytes.");
    assert.deepEqual(updated.foundation.threatModel.assumptions, ["Example service is enabled."]);
    assert.equal(updated.foundation.threatModel.riskImpact, "Unauthorized update.");
    assert.deepEqual(updated.foundation.threatModel.risks, ["Unauthorized state change."]);
    assert.deepEqual(updated.foundation.threatModel.securityObjectives, ["Preserve example state.", "Reject invalid requests."]);
    const cleared = state.surface.update(root.id, updated.revision, { threatModel: { risks: [] } });
    assert.deepEqual(cleared.foundation.threatModel.risks, []);
    assert.deepEqual(cleared.foundation.threatModel.securityObjectives, updated.foundation.threatModel.securityObjectives);
    assert.throws(() => state.surface.update(root.id, cleared.revision, { threatModel: { risks: [7] } }), /risks\[0\]/);
    assert.throws(() => state.surface.update(root.id, cleared.revision, { threatModel: { risks: null } }), /risks must be an array/);
    assert.throws(() => state.surface.update(root.id, cleared.revision, { threatModel: { diagram: 7 } }), /diagram must be a string/);
  } finally { state.close(); }
});

test("new memories and findings persist the focused surface in their canonical records", async () => {
  const state = fixture();
  const graph = new MemoryGraphStore({ workspaceRoot: state.directory, databasePath: state.databasePath,
    context: { workspaceId, workspaceName: "Example Workspace", subjectId: "subject-example",
      subjectName: "Example subject", sessionId: "session-example-one" } });
  const claims = new FindingStore(graph);
  try {
    const child = state.surface.create(state.rootId, "Request parser", "Example request boundary.");
    state.surface.focus("agent-example", child.id);
    const memory = createMemoryGraphTools(graph, state.surface).find((tool) => tool.descriptor.name === "memory.save");
    const finding = createFindingTools(claims, { surfaceStore: state.surface }).find((tool) => tool.descriptor.name === "finding.create");
    assert.ok(memory);
    assert.ok(finding);
    assert.ok(memory.parameters.properties.surfaceId);
    assert.ok(finding.parameters.properties.surfaceId);
    const context = { agentId: "agent-example" };
    const saved = await memory.execute({ id: "save-example", toolName: "memory.save", actionClass: "synthesize",
      input: { type: "invariant", title: "Example parser boundary" } }, context);
    assert.equal(saved.status, "complete");
    assert.equal(saved.output.surfaceId, child.id);
    const memoryCorrect = createMemoryGraphTools(graph, state.surface).find((tool) => tool.descriptor.name === "memory.correct");
    const observed = await finding.execute({ id: "finding-example", toolName: "finding.create", actionClass: "synthesize",
      input: { title: "Example parser issue", classification: "security.vulnerability", rating: "low",
        evidence: [{ kind: "code", referenceId: "src/example-parser.ts:42", summary: "Example boundary observed." }] } }, context);
    assert.equal(observed.status, "complete");
    assert.equal(observed.output.surfaceId, child.id);
    const findingRevise = createFindingTools(claims, { surfaceStore: state.surface }).find((tool) => tool.descriptor.name === "finding.revise");
    const database = new DatabaseSync(state.databasePath, { readOnly: true });
    try {
      assert.equal(database.prepare("SELECT surface_id FROM memory_nodes WHERE id=?").get(saved.output.id).surface_id, child.id);
      assert.equal(database.prepare("SELECT surface_id FROM app_server_research_claims WHERE id=?").get(observed.output.id).surface_id, child.id);
    } finally { database.close(); }
    const movedMemory = await memoryCorrect.execute({ id: "correct-example", toolName: "memory.correct", actionClass: "synthesize",
      input: { id: saved.output.id, expectedRevision: saved.output.revision, surfaceId: state.rootId } }, context);
    assert.equal(movedMemory.status, "complete");
    assert.equal(movedMemory.output.surfaceId, state.rootId);
    const movedFinding = await findingRevise.execute({ id: "revise-example", toolName: "finding.revise", actionClass: "synthesize",
      input: { id: observed.output.id, expectedRevision: observed.output.revision, reason: "Assign the broader component.",
        surfaceId: state.rootId } }, context);
    assert.equal(movedFinding.status, "complete");
    assert.equal(movedFinding.output.surfaceId, state.rootId);
    const invalid = await memory.execute({ id: "invalid-example", toolName: "memory.save", actionClass: "synthesize",
      input: { type: "invariant", title: "Another example boundary", surfaceId: "surface-missing" } }, context);
    assert.equal(invalid.status, "error");
  } finally { claims.close(); graph.close(); state.close(); }
});

test("surface tools expose simple query and track focus without duplicate touch", async () => {
  const state = fixture();
  try {
    const tools = Object.fromEntries(createSurfaceTools(state.surface).map((tool) => [tool.descriptor.name, tool]));
    assert.match(tools["surface.create"].descriptor.description, /component tree/);
    assert.match(tools["surface.create"].parameters.properties.name.description, /concrete component or subcomponent/);
    assert.deepEqual(tools["surface.create"].parameters.required, ["parentId", "name"]);
    assert.equal(tools["surface.update"].parameters.properties.foundation.properties.coverage, undefined);
    assert.match(tools["surface.update"].parameters.properties.foundation.properties.bugHistory.properties.entries.description,
      /Prefer prior_art\.search and prior_art\.fetch; use the browser to supplement when enabled/);
    assert.equal(tools["surface.update"].parameters.properties.foundation.properties.techStack.items.required[0], "technology");
    assert.deepEqual(tools["surface.update"].parameters.properties.foundation.properties.assets.items.required, ["name"]);
    assert.deepEqual(tools["surface.update"].parameters.properties.foundation.properties.assets.items.properties.status.enum,
      ["untouched", "in_progress", "complete"]);
    assert.deepEqual(tools["surface.update"].parameters.properties.foundation.properties.ranking.required,
      ["impact", "reachability", "relevance", "exposure", "precedence"]);
    assert.equal(tools["surface.update"].parameters.properties.foundation.properties.ranking.properties.impact.properties.score.maximum, 10);
    assert.match(tools["surface.update"].parameters.properties.foundation.properties.threatModel.properties.risks.description,
      /ordinary malfunction alone is not security impact/);
    assert.equal(tools["surface.update"].parameters.properties.foundation.properties.threatModel.properties.securityObjectives.type, "array");
    assert.equal(tools["surface.update"].parameters.properties.foundation.properties.threatModel.properties.invariants, undefined);
    assert.equal(tools["surface.update"].parameters.properties.foundation.properties.threatModel.properties.sinks, undefined);
    assert.equal(tools["surface.update"].parameters.properties.foundation.properties.threatModel.properties.mitigations, undefined);
    assert.match(tools["surface.update"].parameters.properties.foundation.properties.threatModel.properties.diagram.description,
      /Raw Mermaid diagram source/);
    assert.match(tools["surface.update"].parameters.properties.progressBlocker.anyOf[0].properties.reason.description, /external dependency/);
    assert.deepEqual(tools["surface.update"].parameters.required, ["surfaceId", "expectedRevision"]);
    assert.equal(tools["surface.update"].parameters.properties.progressBlocker.anyOf[1].type, "null");
    const action = (name, input) => ({ id: `action-${name}`, toolName: name, actionClass: "recall", input });
    const entered = await tools["surface.focus"].execute(action("surface.focus", { surfaceId: state.rootId }), { agentId: "agent-example" });
    assert.equal(entered.output.firstTouch, true);
    const repeated = await tools["surface.focus"].execute(action("surface.focus", { surfaceId: state.rootId }), { agentId: "agent-example" });
    assert.equal(repeated.output.firstTouch, false);
    const query = await tools["surface.query"].execute(action("surface.query", { query: "example service" }));
    assert.equal(query.output.surfaces[0].id, state.rootId);
    const services = await tools["surface.list"].execute(action("surface.list", { resourceKind: "service" }));
    assert.equal(services.output.total, 2);
    assert.equal(services.output.surfaces[0].resourceKind, "service");
    const missing = await tools["surface.get"].execute(action("surface.get", { surfaceId: "surface-missing" }));
    assert.equal(missing.status, "error");
    const created = await tools["surface.create"].execute(action("surface.create", { parentId: state.rootId, name: "Request decoder" }));
    assert.equal(created.status, "complete");
    assert.equal(created.output.description, "");
  } finally { state.close(); }
});

test("surface view lookups return focused context and related workspace records", async () => {
  const state = fixture();
  const graph = new MemoryGraphStore({ workspaceRoot: state.directory, databasePath: state.databasePath,
    context: { workspaceId, workspaceName: "Example Workspace", subjectId: "subject-example",
      subjectName: "Example subject", sessionId: "session-example-one" } });
  const claims = new FindingStore(graph);
  try {
    const child = state.surface.create(state.rootId, "Request parser", "Example request boundary.");
    const other = state.surface.create(state.rootId, "Other component", "Separate example boundary.");
    state.surface.update(child.id, child.revision, { discovery: "Example parser discovery.",
      techStack: [{ technology: "Example Parser", role: "Parsing", version: "1", environment: "Example service",
        evidence: "Synthetic manifest", confidence: "observed" }],
      assets: [{ name: "src/example-parser.ts", status: "in_progress" }],
      threatModel: { diagram: 'flowchart LR\n  input["Example input"] --> parser["Parser"]',
        securityObjectives: ["Validate input."], risks: ["Unauthorized state change."],
        assumptions: ["Example service is enabled."] } });
    const invariant = graph.save({ type: "invariant", title: "Parser invariant", summary: "Example invariant.",
      body: "Detailed example invariant body.", surfaceId: child.id });
    graph.save({ type: "sink", title: "Parser sink", surfaceId: child.id });
    graph.save({ type: "mitigation", title: "Parser mitigation", surfaceId: child.id });
    graph.save({ type: "trajectory", title: "Parser trajectory", surfaceId: child.id });
    graph.save({ type: "invariant", title: "Other invariant", surfaceId: other.id });
    claims.create({ title: "Parser lead", classification: "security.vulnerability", surfaceId: child.id });
    const finding = claims.createObserved({ title: "Parser finding", classification: "security.vulnerability", surfaceId: child.id,
      evidence: [{ kind: "code", referenceId: "src/example-parser.ts:42", summary: "Example observation." }] });
    claims.createObserved({ title: "Other finding", classification: "security.vulnerability", surfaceId: other.id,
      evidence: [{ kind: "code", referenceId: "src/other.ts:7", summary: "Separate example observation." }] });
    state.resources.priorArt.save(state.rootId, "action-example-prior-art", {
      query: "Example parser", aliases: [], fetchedAt: "2026-09-01T00:00:00.000Z", sources: [], records: [],
      resultCount: 0, returnedSoFar: 0, nextCursor: null, complete: true,
      disposition: "no_matches_found", caveat: "Limited synthetic search."
    });
    const tools = Object.fromEntries(createSurfaceTools(state.surface, { claims, memories: graph })
      .map((tool) => [tool.descriptor.name, tool]));
    const read = async (name, input) => {
      const result = await tools[name].execute({ id: `action-${name}`, toolName: name, actionClass: "recall", input });
      assert.equal(result.status, "complete", result.error?.message);
      return result.output;
    };
    const overview = await read("surface.get", { surfaceId: child.id });
    assert.equal(Object.hasOwn(overview, "foundation"), false);
    assert.ok(overview.views.includes("surface.threat_model"));
    const breakdown = await read("surface.breakdown", { surfaceId: state.rootId });
    assert.equal(breakdown.children.total, 2);
    const recon = await read("surface.recon", { surfaceId: child.id });
    assert.equal(recon.discovery, "Example parser discovery.");
    assert.equal(recon.assets[0].name, "src/example-parser.ts");
    assert.equal(recon.leads.items[0].title, "Parser lead");
    assert.equal(Object.hasOwn(recon, "diagram"), false);
    const analysis = await read("surface.analysis", { surfaceId: state.rootId });
    assert.equal(analysis.findings.total, 2);
    assert.equal(analysis.trajectories.items[0].title, "Parser trajectory");
    assert.ok(analysis.findings.items.some((item) => item.id === finding.id));
    const threatModel = await read("surface.threat_model", { surfaceId: child.id });
    assert.match(threatModel.diagram, /Example input/);
    assert.deepEqual(threatModel.securityObjectives, ["Validate input."]);
    assert.equal(threatModel.invariants.items[0].id, invariant.id);
    assert.equal(threatModel.sinks.items[0].title, "Parser sink");
    assert.equal(threatModel.mitigations.items[0].title, "Parser mitigation");
    assert.equal(Object.hasOwn(threatModel.invariants.items[0], "body"), false);
    assert.equal((await read("surface.threat_model", { surfaceId: state.rootId })).invariants.items.length, 0);
    const resources = await read("surface.resources", { surfaceId: child.id });
    assert.equal(resources.resource.locator, "service.example.test");
    assert.equal(resources.priorArt.entries[0].title, "Example parser");
    const requestedViews = ["recon", "threat_model", "analysis", "breakdown", "resources"];
    const batched = await read("surface.views", { surfaceId: child.id, views: requestedViews });
    assert.equal(batched.surfaceId, child.id);
    assert.equal(batched.name, child.name);
    assert.deepEqual(Object.keys(batched.views), requestedViews);
    assert.deepEqual(batched.views.recon, recon);
    assert.deepEqual(batched.views.threat_model, threatModel);
    assert.deepEqual(batched.views.analysis, await read("surface.analysis", { surfaceId: child.id }));
    assert.deepEqual(batched.views.breakdown, await read("surface.breakdown", { surfaceId: child.id }));
    assert.deepEqual(batched.views.resources, resources);
    assert.equal(tools["surface.views"].descriptor.sideEffects, "read");
    assert.equal(tools["surface.views"].descriptor.inputSchema.properties.views.maxItems, 5);
    for (const views of [[], ["recon", "recon"], ["unknown"]]) {
      const result = await tools["surface.views"].execute({ id: "action-invalid-views", toolName: "surface.views",
        actionClass: "recall", input: { surfaceId: child.id, views } });
      assert.equal(result.status, "error");
      assert.match(result.error.message, /views must contain/);
    }
    assert.equal((await read("surface.list", { limit: 1 })).surfaces.some((item) => item.id === child.id), false);
    assert.equal((await read("surface.search", { query: "request parser" })).surfaces[0].id, child.id);
    assert.equal((await read("surface.search", { query: "Unauthorized state change" })).surfaces[0].id, child.id);
    assert.equal((await read("surface.search", { query: "service.example.test" })).surfaces.some((item) => item.id === child.id), true);
  } finally { claims.close(); graph.close(); state.close(); }
});

test("a surface blocker records a concrete dependency after first touch and remains visible until cleared", async () => {
  const state = fixture();
  try {
    const child = state.surface.create(state.rootId, "Authenticated endpoint", "Session boundary.");
    const reason = "Mapped public routes and tested pre-auth inputs; an authorized account is required to test the authenticated endpoint. Provide a test account to resume.";
    state.surface.focus("agent-example", child.id);
    const tools = Object.fromEntries(createSurfaceTools(state.surface).map((tool) => [tool.descriptor.name, tool]));
    const action = (input) => ({ id: "action-surface-update", toolName: "surface.update", actionClass: "recall", input });
    const invalid = await tools["surface.update"].execute(action({ surfaceId: child.id, expectedRevision: child.revision,
      foundation, progressBlocker: "missing account" }), { agentId: "agent-example" });
    assert.equal(invalid.status, "error");
    assert.match(invalid.error.message, /progressBlocker must be an object/);
    const set = await tools["surface.update"].execute(action({ surfaceId: child.id, expectedRevision: child.revision,
      foundation, progressBlocker: { reason } }), { agentId: "agent-example" });
    assert.equal(set.status, "complete");
    assert.equal(set.output.progressBlocker.reason, reason);
    assert.equal(Object.hasOwn(set.output, "foundation"), false);
    assert.equal(set.output.progressBlocker.actorId, "agent-example");
    assert.ok(Number.isFinite(Date.parse(set.output.progressBlocker.setAt)));
    assert.equal(state.surface.coverageState(child.id).covered, true);
    assert.equal(state.surface.list().surfaces.find((item) => item.id === child.id).progressBlocker.reason, reason);
    assert.equal(state.surface.query("authorized account").surfaces[0].id, child.id);
    const updated = state.surface.update(child.id, set.output.revision, foundation);
    assert.equal(updated.progressBlocker.reason, reason);
    const registry = new ResearchToolRegistry([tools["surface.update"]]);
    const cleared = (await registry.execute({ ...action({ surfaceId: child.id, expectedRevision: updated.revision,
      progressBlocker: null }), actionClass: "synthesize" }, { agentId: "agent-example" })).result;
    assert.equal(cleared.status, "complete");
    assert.deepEqual(state.surface.get(child.id).foundation, updated.foundation);
    assert.equal(Object.hasOwn(cleared.output, "foundation"), false);
    assert.equal(cleared.output.progressBlocker, null);
    const stale = await tools["surface.update"].execute(action({ surfaceId: child.id, expectedRevision: updated.revision,
      progressBlocker: null }), { agentId: "agent-example" });
    assert.equal(stale.status, "error");
    assert.match(stale.error.message, /revision changed/i);
    const completed = state.surface.update(child.id, cleared.output.revision, completeFoundation);
    assert.equal(completed.progressBlocker, null);
    assert.equal(state.surface.coverageState(child.id).covered, true);
  } finally { state.close(); }
});

test("a concrete external blocker can be recorded without completing a foundation form", () => {
  const state = fixture();
  try {
    const child = state.surface.create(state.rootId, "Session verifier");
    const blocked = state.surface.update(child.id, child.revision, undefined, undefined, undefined,
      { reason: "An example account is needed to continue." });
    assert.equal(blocked.progressBlocker.reason, "An example account is needed to continue.");
    assert.equal(blocked.foundation.coverage.mappingComplete, false);
  } finally { state.close(); }
});

test("surface progress follows a threat-model review and live claim resolution", () => {
  const state = fixture();
  try {
    const child = state.surface.create(state.rootId, "Request parser", "Parser boundary.");
    assert.deepEqual(state.surface.coverageState(child.id).unmetRequirements, ["threat_model"]);
    const renamed = state.surface.update(child.id, child.revision, undefined, "Request decoder");
    assert.equal(renamed.name, "Request decoder");
    assert.equal(renamed.foundation, null);
    const noted = state.surface.update(child.id, renamed.revision, {
      coverage: { summary: "Reviewed decoding boundaries.", mappingComplete: true }
    });
    assert.equal(noted.foundation.coverage.summary, "Reviewed decoding boundaries.");
    assert.deepEqual(state.surface.coverageState(child.id).unmetRequirements, ["threat_model"]);
    const modeled = state.surface.update(child.id, noted.revision, { threatModel: {
      diagram: 'flowchart LR\n  request["Request"] --> parser["Parser"]'
    } });
    assert.equal(modeled.foundation.threatModelReviewed, true);
    assert.equal(state.surface.coverageState(child.id).covered, true);
    const clearedDiagram = state.surface.update(child.id, modeled.revision, { threatModel: { diagram: "" } });
    assert.equal(clearedDiagram.foundation.threatModelReviewed, true);
    assert.equal(state.surface.list({ parentId: state.rootId }).surfaces[0].threatModelReviewed, true);
    state.surface.update(state.rootId, 1, { threatModel: { securityObjectives: "Protect example state." } });
    assert.equal(state.surface.coverageState(state.rootId).covered, true);
    const database = new DatabaseSync(state.databasePath);
    try {
      database.exec(`CREATE TABLE app_server_research_claims (
        id TEXT, workspace_id TEXT, status TEXT, stale_from_status TEXT,
        surface_id TEXT, duplicate_of_claim_id TEXT, security_tracking_json TEXT
      )`);
      const insert = database.prepare(`INSERT INTO app_server_research_claims
        (id, workspace_id, status, stale_from_status, surface_id, duplicate_of_claim_id, security_tracking_json)
        VALUES (?, ?, ?, ?, ?, ?, ?)`);
      insert.run("lead-example", workspaceId, "hypothesis", null, child.id, null, "{}");
      insert.run("refuted-example", workspaceId, "rejected", null, child.id, null, "{}");
      insert.run("duplicate-example", workspaceId, "hypothesis", null, child.id, "lead-example", "{}");
      insert.run("unassigned-example", workspaceId, "hypothesis", null, null, null, "{}");
      assert.deepEqual(state.surface.coverageState(child.id).openLeadIds, ["lead-example"]);
      insert.run("stale-lead-example", workspaceId, "stale", "hypothesis", null, null,
        JSON.stringify({ surfaceIds: [child.id] }));
      assert.deepEqual(state.surface.coverageState(child.id).openLeadIds, ["lead-example", "stale-lead-example"]);
      database.prepare("UPDATE app_server_research_claims SET status='rejected' WHERE id='stale-lead-example'").run();
      assert.deepEqual(state.surface.coverageState(child.id).unmetRequirements, ["open_leads"]);
      assert.equal(state.surface.coverageState(state.rootId).covered, true);
      database.prepare("UPDATE app_server_research_claims SET status='observed' WHERE id='lead-example'").run();
      assert.deepEqual(state.surface.coverageState(child.id).unblockedFindingIds, ["lead-example"]);
      assert.deepEqual(state.surface.coverageState(child.id).unmetRequirements, ["unblocked_findings"]);
      database.prepare("UPDATE app_server_research_claims SET security_tracking_json=? WHERE id='lead-example'")
        .run(JSON.stringify({ progressBlocker: { reason: "Example account required." } }));
      assert.equal(state.surface.coverageState(child.id).covered, true);
      database.prepare("UPDATE app_server_research_claims SET status='verified', security_tracking_json='{}' WHERE id='lead-example'").run();
      assert.equal(state.surface.coverageState(child.id).covered, true);
      database.prepare("UPDATE app_server_research_claims SET status='stale', stale_from_status='verified' WHERE id='lead-example'").run();
      assert.deepEqual(state.surface.coverageState(child.id).unblockedFindingIds, ["lead-example"]);
      database.prepare("UPDATE app_server_research_claims SET status='rejected' WHERE id='lead-example'").run();
      assert.equal(state.surface.coverageState(child.id).covered, true);
      assert.equal(state.surface.focus("agent-later", child.id).surface.id, child.id);
    } finally { database.close(); }
  } finally { state.close(); }
});

test("the first focus automatically launches one foundation scout and reports its result to the root", async () => {
  const state = fixture();
  try {
    const requests = [];
    const activities = [];
    const manager = new SubagentManager({
      rootProvider: "openai", rootModel: "example-model", surfaceStore: state.surface,
      allowManualDelegation: false,
      onActivity(activity) { activities.push(activity); },
      async run(request) {
        requests.push(request);
        const current = state.surface.get(state.rootId);
        state.surface.update(current.id, current.revision, completeFoundation);
        return { messages: [], text: "Foundation recorded with no remaining questions.", turnCount: 1,
          toolCallCount: 1, modelCalls: [], toolEvents: [] };
      },
    });
    assert.deepEqual(manager.createTools("root"), []);
    const first = state.surface.focus("root", state.rootId);
    assert.equal(first.foundationScout.status, "running");
    assert.match(first.foundationScout.agentPath, /^\/root\/surface_foundation_/);
    await manager.settle();
    assert.equal(requests.length, 1);
    assert.match(requests[0].prompt, /Give this component a concise research foundation/);
    assert.match(requests[0].prompt, /progress follows the current claims and can change later/);
    assert.match(requests[0].prompt, /Keep hypotheses and tests in research records/);
    assert.deepEqual(activities.find((activity) => activity.type === "spawned")?.surfaceTrail,
      [{ id: state.rootId, name: "Example service" }]);
    assert.equal(state.surface.coverageState(state.rootId).covered, true);
    assert.match(JSON.stringify(manager.collaborationFollowUp("root")), /Foundation recorded/);
    assert.equal(state.surface.focus("root", state.rootId).firstTouch, false);
    assert.equal(requests.length, 1);
  } finally { state.close(); }
});

test("first touch stays one-time even when the foundation remains incomplete", async () => {
  const state = fixture();
  try {
    let firstRuns = 0;
    const firstManager = new SubagentManager({
      rootProvider: "openai", rootModel: "example-model", surfaceStore: state.surface,
      allowManualDelegation: false,
      async run() {
        firstRuns += 1;
        return { messages: [], text: "Foundation remains incomplete.", turnCount: 1,
          toolCallCount: 0, modelCalls: [], toolEvents: [] };
      },
    });
    const firstFocus = state.surface.focus("root", state.rootId);
    assert.equal(firstFocus.firstTouch, true);
    assert.equal(firstFocus.foundationScout.status, "running");
    assert.equal(state.surface.focus("root", state.rootId).foundationScout, undefined);
    await firstManager.settle();
    assert.equal(firstRuns, 1);

    const laterSurface = new SurfaceStore({
      databasePath: state.databasePath, workspaceId, sessionId: "session-example-later", resources: state.resources,
    });
    try {
      new SubagentManager({
        rootProvider: "openai", rootModel: "example-model", surfaceStore: laterSurface,
        async run() { throw new Error("The later session must not launch another foundation scout."); },
      });
      const laterFocus = laterSurface.focus("root", state.rootId);
      assert.equal(laterFocus.firstTouch, false);
      assert.equal(laterFocus.foundationScout, undefined);
      assert.equal(laterSurface.coverageState(state.rootId).covered, false);
    } finally { laterSurface.close(); }
  } finally { state.close(); }
});

test("subagent spawn requires a valid surface assignment and records first touch", async () => {
  const state = fixture();
  try {
    const requests = [];
    const manager = new SubagentManager({
      rootProvider: "openai", rootModel: "example-model", surfaceStore: state.surface,
      async run(request) {
        requests.push(request);
        return { messages: [], text: "Complete.", turnCount: 1, toolCallCount: 0, modelCalls: [], toolEvents: [] };
      },
    });
    const spawn = manager.createTools("root").find((tool) => tool.name === "spawn_agent");
    assert.ok(spawn.parameters.required.includes("surface_id"));
    await assert.rejects(spawn.execute("missing-surface", { task_name: "missing", message: "Inspect." }), /surface_id/);
    const excludedId = state.resources.list().find((item) => item.direction === "out_of_scope").id;
    await assert.rejects(spawn.execute("excluded-surface", { task_name: "excluded", message: "Inspect.", surface_id: excludedId }), /not eligible/i);
    const child = state.surface.create(state.rootId, "Parser", "Parser boundary");
    const result = await spawn.execute("assigned-surface", { task_name: "parser", message: "Inspect parser.", surface_id: child.id });
    assert.equal(result.details.surface_id, child.id);
    assert.equal(state.surface.get(child.id).firstTouchedAt !== null, true);
    await manager.settle();
    assert.match(requests[0].prompt, /Assigned surface: Example service \/ Parser/);
    assert.match(requests[0].prompt, /brief foundation when useful/);
    assert.equal(state.surface.current(result.details.agent_id).id, child.id);
    state.surface.focus(result.details.agent_id, state.rootId, "Moved to parent boundary");
    const listed = await manager.createTools("root").find((tool) => tool.name === "list_agents").execute("list-after-move", {});
    assert.equal(listed.details.agents[0].surface_id, state.rootId);
    assert.equal(listed.details.agents[0].assigned_surface_id, child.id);
  } finally { state.close(); }
});

test("surface foundations, first touch, and movement survive workspace publication and rebuild", () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-surfaces-publication-example-"));
  const workspaceRoot = join(directory, "workspace");
  const databasePath = join(directory, "runtime", "memory.sqlite");
  const options = { workspaceRoot, databasePath, artifactDirectoryPath: join(directory, "runtime", "artifacts"), workspaceId };
  mkdirSync(workspaceRoot);
  initializeWorkspaceProject(workspaceRoot, workspaceId);
  const memory = new MemoryGraphStore({ workspaceRoot, databasePath, context: { workspaceId, workspaceName: "Example", subjectId: "subject-example", subjectName: "Example" } });
  const resources = new ResearchResourceCatalog({ databasePath, workspaceId, explicitResources: [{ id: "asset-example", direction: "in_scope", kind: "repository", locator: "https://example.test/repository", name: "Example repository", source: "explicit_scope" }] });
  const surfaces = new SurfaceStore({ databasePath, workspaceId, sessionId: "session-example", resources });
  try {
    const child = surfaces.create(resources.list()[0].id, "Parser", "Parser boundary");
    surfaces.focus("agent-example", child.id);
    surfaces.update(child.id, child.revision, foundation, undefined, undefined,
      { reason: "Public parser paths were reviewed; an authorized account is required to test the session path. Provide a test account to resume." }, "agent-example");
    publishWorkspaceResearch(options);
    const published = JSON.parse(readFileSync(join(workspaceRoot, "references", "resources.json"), "utf8"));
    assert.equal(published.surfaces.length, 1);
    assert.equal(published.surfaces[0].created_session_id, "session-example");
    assert.match(JSON.parse(published.surfaces[0].foundation_json).progressBlocker.reason, /authorized account/);
    assert.equal(published.surfaceTouches.length, 1);
    assert.equal(published.surfaceFocusEvents.length, 1);
    assert.equal(published.surfaceUpdateEvents.length, 1);
    surfaces.close(); resources.close(); memory.close();
    releaseWorkspaceResearchIndex(options);
    const emptied = new DatabaseSync(databasePath);
    try {
      for (const table of ['app_server_surface_update_events', 'app_server_surface_focus_events',
        'app_server_surface_focus', 'app_server_surface_touches', 'app_server_surfaces', 'app_server_surface_roots']) {
        emptied.exec(`DROP TABLE ${table}`);
      }
    } finally { emptied.close(); }
    rebuildWorkspaceResearchIndex(options);
    const restoredResources = new ResearchResourceCatalog({ databasePath, workspaceId, explicitResources: [{ id: "asset-example", direction: "in_scope", kind: "repository", locator: "https://example.test/repository", name: "Example repository", source: "explicit_scope" }] });
    const restored = new SurfaceStore({ databasePath, workspaceId, sessionId: "session-later", resources: restoredResources });
    try {
      assert.equal(restored.get(child.id).foundation.threatModel.attackerControl, foundation.threatModel.attackerControl);
      assert.equal(restored.get(child.id).foundation.threatModel.diagram, foundation.threatModel.diagram);
      assert.equal(restored.get(child.id).foundation.discovery, foundation.discovery.trim());
      assert.equal(restored.get(child.id).foundation.techStack[0].technology, "Example Parser");
      assert.deepEqual(restored.get(child.id).foundation.assets, foundation.assets);
      assert.equal(restored.get(child.id).foundation.bugHistory.entries[0].identifier, "EXAMPLE-ADVISORY-001");
      assert.match(restored.get(child.id).progressBlocker.reason, /authorized account/);
      assert.equal(restored.focus("agent-later", child.id).firstTouch, false);
      const database = new DatabaseSync(databasePath, { readOnly: true });
      try {
        assert.equal(database.prepare('SELECT created_session_id FROM app_server_surfaces WHERE id=?').get(child.id).created_session_id,
          'session-example');
        assert.equal(database.prepare('SELECT COUNT(*) AS count FROM app_server_surface_update_events WHERE surface_id=?').get(child.id).count, 1);
      } finally { database.close(); }
    } finally { restored.close(); restoredResources.close(); }
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
