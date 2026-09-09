import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import test from "node:test";
import {
  FindingStore, MemoryGraphStore, RunbookStore, createFindingTools, createRunbookExecutor,
  createRunbookExecutionTool, createRunbookTools, createResearchStorageLayout, ensureResearchStorageLayout,
} from "../packages/research-agent/dist/index.js";

async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), "beale-evidence-example-"));
  const context = { workspaceId: "workspace-example", workspaceName: "Example", subjectId: "subject-example", subjectName: "Example", sessionId: "session-example" };
  const graph = new MemoryGraphStore({ workspaceRoot: root, context });
  const claims = new FindingStore(graph);
  const runbooks = new RunbookStore(graph.databasePath, ensureResearchStorageLayout(createResearchStorageLayout({ workspaceRoot: root })), context);
  const database = new DatabaseSync(graph.databasePath);
  t.after(async () => { database.close(); runbooks.close(); claims.close(); graph.close(); await rm(root, { recursive: true, force: true }); });
  const sourceRevision = "git:example-component:revision-one";
  const environmentFingerprint = "environment:example-target:build-one";
  let claim = claims.create({ title: "Example state mismatch", summary: "Synthetic proof obligation.", classification: "security.primitive", sourceRevision, environmentFingerprint }, undefined, "author-example");
  claim = claims.transition(claim.id, { expectedRevision: claim.revision, toStatus: "observed", reason: "Example direct observation.",
    evidence: [{ kind: "code", referenceId: "src/example.ts:1", summary: "Example observation." }] }, undefined, "author-example");
  const book = runbooks.create({ title: "Example proof", purpose: "Run setup and the assertion.", cells: [
    { kind: "code", language: "javascript", source: "console.log('example setup')", features: ["setup"] },
    { kind: "code", language: "javascript", source: "console.log('example assertion')", features: ["runtime"] },
  ] }).runbook;
  const calls = [];
  const execute = createRunbookExecutor({ store: runbooks, shellTool: { async execute(action, toolContext) {
    calls.push(toolContext.runbookContext.cellId);
    return { action, status: "complete", startedAt: new Date().toISOString(), completedAt: new Date().toISOString(),
      summary: "Example assertion passed.", output: { stdout: "example passed", stderr: "", exitCode: 0 }, followUpActions: [] };
  } } });
  const run = (overrides = {}) => execute({ runbookId: book.id, proofTarget: "localhost", sourceRevision, environmentFingerprint, actorId: "author-example", ...overrides });
  const reproduce = (runId, overrides = {}) => {
    const current = claims.get(claim.id);
    return claims.transition(claim.id, { expectedRevision: current.revision, toStatus: "reproduced", reason: "Example reproduction.", reproductionRunbookId: book.id,
      evidence: [{ kind: "runbook_execution", referenceId: runId, summary: "Example run." }], ...overrides }, undefined, "author-example");
  };
  return { root, graph, context, claims, runbooks, database, claim, book, execute, run, reproduce, calls, sourceRevision, environmentFingerprint };
}

test("verification resolves execution evidence and rejects self-review, missing identity, and fabricated references", async (t) => {
  const f = await fixture(t);
  const execution = await f.run();
  let claim = f.reproduce(execution.runId);
  const tool = createFindingTools(f.claims).find((candidate) => candidate.descriptor.name === "finding.transition");
  const review = (actorId, referenceId, extra = {}) => tool.execute({ id: "review-example", toolName: "finding.transition", actionClass: "synthesize",
    input: { id: claim.id, expectedRevision: claim.revision, toStatus: "verified", reason: "Example independent review.", evidence: [
      { kind: "independent_verification", independent: true, referenceId, summary: "Example review conclusion.", ...extra },
    ] } }, actorId ? { agentId: actorId } : {});
  for (const [actor, reference] of [["author-example", execution.runId], [undefined, execution.runId], ["reviewer-example", "missing-review-example"]]) {
    const result = await review(actor, reference);
    assert.equal(result.status, "error");
    assert.match(result.error.message, /independent reviewer/);
  }
  const spoofed = await review("author-example", execution.runId, { actorId: "reviewer-example", sessionId: "another-session-example", metadata: { validated: true } });
  assert.equal(spoofed.status, "error");
  assert.equal(f.claims.get(claim.id).revision, claim.revision);
  const result = await review("reviewer-example", execution.runId);
  assert.equal(result.status, "complete", result.error?.message);
  claim = result.output;
  assert.equal(claim.status, "verified");
  assert.equal(claim.evidence.at(-1).actorId, "reviewer-example");
  assert.equal(claim.evidence.at(-1).sessionId, f.context.sessionId);
  assert.equal(claim.evidence.at(-1).validated, true);
  assert.match(claim.evidence.at(-1).claimBindingHash, /^sha256:[a-f0-9]{64}$/);
  claim = f.claims.revise(claim.id, { expectedRevision: claim.revision, reason: "Changed the example conclusion.", summary: "A different example property requires review." }, undefined, "author-example");
  assert.equal(claim.evidence.at(-1).validated, false);
  assert.ok(f.claims.completionChecklist(claim.id, "verified").missingRequired.includes("independent_verification"));
});

test("reproduction rejects setup-only runs, changed environments, and obsolete notebook revisions", async (t) => {
  const f = await fixture(t);
  const setup = f.runbooks.executionPlan(f.book.id)[0];
  const partial = await f.run({ cellId: setup.id });
  assert.equal(partial.status, "succeeded");
  assert.equal(f.calls.length, 1);
  assert.throws(() => f.reproduce(partial.runId), /every active cell/);
  assert.equal(f.runbooks.get(f.book.id).execution.latestSuccessfulRunId, null);
  const wrongEnvironment = await f.run({ environmentFingerprint: "environment:example-target:other-build" });
  assert.throws(() => f.reproduce(wrongEnvironment.runId), /environmentFingerprint/);
  const full = await f.run();
  assert.throws(() => f.reproduce(full.runId, { sourceRevision: "git:example-component:revision-two" }), /sourceRevision/);
  let claim = f.reproduce(full.runId);
  assert.equal(claim.evidence.at(-1).validated, true);
  const snapshot = f.database.prepare("SELECT * FROM app_server_runbook_executions WHERE run_id = ?").get(full.runId);
  assert.equal(snapshot.full_run, 1);
  assert.equal(snapshot.content_revision, 1);
  assert.match(snapshot.content_hash, /^sha256:[a-f0-9]{64}$/);
  assert.equal(JSON.parse(snapshot.snapshot_json).cells.length, 2);
  const originalDetail = f.claims.readDetail(claim.id, "evidence", 0, 1);
  const current = f.runbooks.get(f.book.id);
  f.runbooks.append({ id: f.book.id, expectedRevision: current.revision, cells: [
    { kind: "code", language: "javascript", source: "throw new Error('example new obligation')", features: ["runtime"] },
  ] });
  assert.equal(f.runbooks.get(f.book.id).execution.latestSuccessfulRunId, null);
  assert.equal(f.database.prepare("SELECT snapshot_json FROM app_server_runbook_executions WHERE run_id = ?").get(full.runId).snapshot_json, snapshot.snapshot_json);
  const historical = f.runbooks.getExecution(f.book.id, full.runId, { limit: 1 });
  assert.equal(historical.contentRevision, 1);
  assert.equal(historical.cells[0].source, "console.log('example setup')");
  assert.equal(historical.cells[0].result.stdout, "example passed");
  assert.equal(historical.nextOffset, 1);
  const getTool = createRunbookTools(f.runbooks).find((tool) => tool.descriptor.name === "runbook.get");
  const recalled = await getTool.execute({ id: "recall-example", toolName: "runbook.get", actionClass: "recall",
    input: { id: f.book.id, runId: full.runId, offset: 1, limit: 1 } });
  assert.equal(recalled.status, "complete", recalled.error?.message);
  assert.equal(recalled.output.cells[0].source, "console.log('example assertion')");
  assert.equal(recalled.output.nextOffset, null);
  assert.throws(() => f.runbooks.getExecution("another-runbook-example", full.runId), /not found in this workspace/);
  const otherBooks = new RunbookStore(f.graph.databasePath, ensureResearchStorageLayout(createResearchStorageLayout({ workspaceRoot: f.root })),
    { ...f.context, workspaceId: "workspace-other-example" });
  try { assert.throws(() => otherBooks.getExecution(f.book.id, full.runId), /not found in this workspace/); }
  finally { otherBooks.close(); }
  assert.throws(() => f.claims.readDetail(claim.id, "evidence", 1, 1, originalDetail.readRevision), /read revision changed/);
  claim = f.claims.get(claim.id);
  assert.equal(claim.evidence.at(-1).validated, false);
  assert.ok(f.claims.completionChecklist(claim.id, "reproduced").missingRequired.includes("reproduction"));
  assert.throws(() => f.reproduce(full.runId), /current content revision/);
});

test("reviewers cannot verify content they revised or change claim content during review", async (t) => {
  const f = await fixture(t);
  const run = await f.run();
  let claim = f.reproduce(run.runId);
  const review = { toStatus: "verified", reason: "Example review.", evidence: [
    { kind: "independent_verification", independent: true, referenceId: run.runId, summary: "Example review." },
  ] };
  assert.throws(() => f.claims.transition(claim.id, { ...review, expectedRevision: claim.revision, classification: "security.chain" },
    undefined, "reviewer-example"), /existing claim content/);
  claim = f.claims.revise(claim.id, { expectedRevision: claim.revision, summary: "Revised example conclusion.", reason: "Example content revision." },
    undefined, "reviewer-example");
  assert.throws(() => f.claims.transition(claim.id, { ...review, expectedRevision: claim.revision }, undefined, "reviewer-example"), /independent reviewer/);
  claim = f.claims.transition(claim.id, { ...review, expectedRevision: claim.revision }, undefined, "another-reviewer-example");
  assert.equal(claim.evidence.at(-1).validated, true);
});

test("legacy execution and review records are preserved without inventing verification provenance", async (t) => {
  const f = await fixture(t);
  const execution = await f.run();
  const claim = f.reproduce(execution.runId);
  f.database.prepare("UPDATE app_server_runbook_executions SET full_run = 0, snapshot_json = NULL, content_revision = NULL, content_hash = NULL WHERE run_id = ?").run(execution.runId);
  f.database.prepare(`INSERT INTO app_server_claim_evidence (id, claim_id, kind, reference_id, summary, session_id, actor_id, independent, metadata_json, created_at)
    VALUES ('review-legacy-example', ?, 'independent_verification', ?, 'Example historical review', 'session-example', 'reviewer-example', 1, '{}', '2026-01-01T00:00:00Z')`).run(claim.id, execution.runId);
  assert.equal(f.claims.get(claim.id).evidence.find((item) => item.id === "review-legacy-example").validated, false);
  assert.equal(f.runbooks.getExecution(f.book.id, execution.runId).snapshotAvailable, false);
  assert.equal(f.runbooks.get(f.book.id).execution.latestSuccessfulRunId, null);
  assert.throws(() => f.reproduce(execution.runId), /successful runbook execution/);
});

test("execution snapshots reject content races, missing cell results, and repeated result writes", async (t) => {
  const f = await fixture(t);
  const cells = f.runbooks.executionPlan(f.book.id);
  assert.throws(() => f.runbooks.beginExecution(f.book.id, "race-example", cells.map((cell) => cell.id), "localhost", undefined,
    { expectedContentRevision: 0 }), /content changed/);
  const startedAt = new Date().toISOString();
  f.runbooks.beginExecution(f.book.id, "incomplete-example", cells.map((cell) => cell.id), "localhost");
  assert.throws(() => f.runbooks.completeExecution({ id: f.book.id, runId: "incomplete-example", status: "succeeded", startedAt,
    completedAt: startedAt, durationMs: 0, proofTarget: "localhost" }), /every selected cell/);
  const result = { id: f.book.id, runId: "incomplete-example", cellId: cells[0].id, status: "succeeded", exitCode: 0,
    startedAt, completedAt: startedAt, durationMs: 0, proofTarget: "localhost", stdout: "original example output" };
  f.runbooks.completeCellExecution(result);
  assert.throws(() => f.runbooks.completeCellExecution({ ...result, stdout: "replacement example output" }), /UNIQUE/);
  const stored = f.database.prepare("SELECT result_json FROM app_server_runbook_cell_executions WHERE run_id = ? AND cell_id = ?").get(result.runId, result.cellId);
  assert.equal(JSON.parse(stored.result_json).stdout, result.stdout);
});

test("runbook tool records host actor identity and declared source/environment provenance", async (t) => {
  const f = await fixture(t);
  const tool = createRunbookExecutionTool(f.execute);
  assert.equal(tool.parameters.properties.actorId, undefined);
  const result = await tool.execute({ id: "run-example", toolName: "runbook.run", actionClass: "experiment", input: {
    id: f.book.id, proofTarget: "localhost", sourceRevision: f.sourceRevision, environmentFingerprint: f.environmentFingerprint, actorId: "spoof-example",
  } }, { agentId: "host-actor-example" });
  assert.equal(result.status, "complete", result.error?.message);
  const run = f.database.prepare("SELECT actor_id, source_revision, environment_fingerprint FROM app_server_runbook_executions WHERE run_id = ?").get(result.output.runId);
  assert.equal(run.actor_id, "host-actor-example");
  assert.equal(run.source_revision, f.sourceRevision);
  assert.equal(run.environment_fingerprint, f.environmentFingerprint);
});

test("provenance migrations preserve old rows and do not backfill invented execution evidence", async () => {
  const root = await mkdtemp(join(tmpdir(), "beale-migration-example-"));
  const context = { workspaceId: "workspace-example", workspaceName: "Example", subjectId: "subject-example", sessionId: "session-example" };
  const layout = ensureResearchStorageLayout(createResearchStorageLayout({ workspaceRoot: root }));
  let graph = new MemoryGraphStore({ workspaceRoot: root, context });
  let claims = new FindingStore(graph);
  let books = new RunbookStore(graph.databasePath, layout, graph.getContext());
  const databasePath = graph.databasePath;
  const claim = claims.create({ title: "Example historical claim", classification: "security.primitive" }, undefined, "author-example");
  const book = books.create({ title: "Example historical procedure", purpose: "Preserve the existing record." }).runbook;
  books.close(); claims.close(); graph.close();
  const database = new DatabaseSync(databasePath);
  try {
    database.prepare(`INSERT INTO app_server_runbook_executions
      (run_id, runbook_id, workspace_id, status, proof_target, started_at) VALUES ('legacy-run-example', ?, ?, 'succeeded', 'localhost', '2026-01-01T00:00:00Z')`).run(book.id, context.workspaceId);
    database.prepare(`INSERT INTO app_server_claim_evidence (id, claim_id, kind, reference_id, summary, independent, created_at)
      VALUES ('legacy-review-example', ?, 'independent_verification', 'legacy-run-example', 'Example historical review', 1, '2026-01-01T00:00:00Z')`).run(claim.id);
    database.exec("DROP TABLE app_server_runbook_cell_executions");
    for (const column of ["content_revision", "content_hash", "snapshot_json", "selected_cell_ids_json", "required_cell_ids_json", "source_revision", "environment_fingerprint", "session_id", "actor_id", "full_run"]) {
      database.exec(`ALTER TABLE app_server_runbook_executions DROP COLUMN ${column}`);
    }
    database.exec("ALTER TABLE app_server_claim_evidence DROP COLUMN claim_binding_hash");
    database.exec("DELETE FROM schema_migrations WHERE (component = 'app_server_core' AND version = 18) OR (component = 'app_server_research_claims' AND version = 7)");
  } finally { database.close(); }
  try {
    graph = new MemoryGraphStore({ workspaceRoot: root, context });
    claims = new FindingStore(graph);
    books = new RunbookStore(databasePath, layout, graph.getContext());
    const historical = books.getExecution(book.id, "legacy-run-example");
    assert.equal(historical.status, "succeeded");
    assert.equal(historical.snapshotAvailable, false);
    assert.equal(historical.fullRun, false);
    assert.equal(books.get(book.id).execution.latestSuccessfulRunId, null);
    assert.equal(claims.get(claim.id).evidence[0].independent, true);
    assert.equal(claims.get(claim.id).evidence[0].validated, false);
  } finally {
    books.close(); claims.close(); graph.close();
    await rm(root, { recursive: true, force: true });
  }
});
