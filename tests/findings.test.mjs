import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { recordRunbookExecution } from "./fixtures/record-runbook-execution.mjs";
import { DatabaseSync } from "node:sqlite";

import {
  DEFAULT_SECURITY_RESEARCH_PROFILE,
  FindingStore,
  attentionHeatForClaim,
  MemoryGraphStore,
  ReportStore,
  RunbookStore,
  buildCampaignGraph,
  createCampaignModelContext,
  createFindingTools,
  createWorkspaceHistoryDuplicateTools,
  createResearchToolRegistry,
  createResearchStorageLayout,
  ensureResearchStorageLayout,
  getAppServerMemorySummary,
  migrateWorkspaceResearchClaims,
} from "../packages/research-agent/dist/index.js";

const workspace = { workspaceId: "workspace_findings", workspaceName: "Findings", subjectId: "subject_findings", subjectName: "Findings" };

test("model tools create, list, and promote Leads in the canonical claim lifecycle", async () => {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "beale-lead-lifecycle-example-"));
  const layout = ensureResearchStorageLayout(createResearchStorageLayout({ workspaceRoot }));
  const graph = new MemoryGraphStore({ workspaceRoot, context: { ...workspace, sessionId: "session_lead_example" } });
  const claims = new FindingStore(graph);
  try {
    const registry = createResearchToolRegistry(createFindingTools(claims));
    const created = await registry.execute({ id: "lead_create_example", toolName: "lead.create", actionClass: "synthesize",
      input: { title: "Example parser candidate", classification: "security.vulnerability", rating: "medium",
        summary: "A frame length may reach an unchecked allocation." } });
    assert.equal(created.result.status, "complete");
    const lead = created.result.output;
    assert.equal(lead.projection, "lead");
    assert.equal(lead.maturity, "lead");
    const duplicate = await registry.execute({ id: "lead_duplicate_example", toolName: "lead.create", actionClass: "synthesize",
      input: { title: "Example parser candidate", classification: "security.vulnerability", rating: "medium" } });
    assert.equal(duplicate.result.status, "error");
    assert.match(duplicate.result.error.message, /Potential existing claims/);
    const listed = await registry.execute({ id: "lead_list_example", toolName: "lead.list", actionClass: "recall", input: {} });
    assert.equal(listed.result.output.leads.some((claim) => claim.id === lead.id), true);
    const leadOnlySummary = getAppServerMemorySummary({
      databasePath: graph.databasePath,
      artifactDirectoryPath: layout.artifactDirectoryPath,
      workspaceId: workspace.workspaceId,
      subjectId: workspace.subjectId,
    });
    assert.equal(leadOnlySummary.status, "ready");
    assert.deepEqual(leadOnlySummary.leads.map((claim) => claim.id), [lead.id]);
    assert.deepEqual(leadOnlySummary.findings, []);
    const promoted = await registry.execute({ id: "lead_promote_example", toolName: "finding.transition", actionClass: "synthesize",
      input: { id: lead.id, expectedRevision: lead.revision, toStatus: "observed",
        reason: "The example parser branch is reachable.",
        evidence: [{ kind: "code", referenceId: "src/example-parser.ts:42", summary: "Length reaches allocation." }] } });
    assert.equal(promoted.result.status, "complete");
    assert.equal(promoted.result.output.projection, "finding");
    assert.equal(promoted.result.output.maturity, "observed");
    const after = await registry.execute({ id: "lead_list_after_promotion", toolName: "lead.list", actionClass: "recall", input: {} });
    assert.equal(after.result.output.leads.some((claim) => claim.id === lead.id), false);
  } finally {
    claims.close();
    graph.close();
    await rm(workspaceRoot, { recursive: true, force: true });
  }
});

test("legacy risk and affected version fields remain readable and are dropped on the next finding revision", async () => {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "beale-legacy-finding-tracking-"));
  const graph = new MemoryGraphStore({ workspaceRoot, context: { ...workspace, sessionId: "session_legacy" } });
  try {
    const claims = new FindingStore(graph);
    const created = claims.create({ title: "Example parser boundary", classification: "security.vulnerability", rating: "medium" });
    claims.close();
    const database = new DatabaseSync(graph.databasePath);
    try {
      database.prepare("UPDATE app_server_research_claims SET security_tracking_json = ? WHERE id = ?").run(
        JSON.stringify({ ...created.securityTracking, riskTreatment: "accepted", riskDecisions: [{ treatment: "accepted", actorId: "operator_example", rationale: "Legacy test decision.", decidedAt: "2026-01-01T00:00:00.000Z", expiresAt: null }], affectedVersions: [{ assetId: "asset-example", range: "1.x", fixedVersion: "2.0" }] }),
        created.id,
      );
    } finally {
      database.close();
    }
    const reopened = new FindingStore(graph);
    try {
      const legacy = reopened.get(created.id);
      assert.deepEqual(legacy.securityTracking, created.securityTracking);
      const revised = reopened.revise(created.id, {
        expectedRevision: legacy.revision,
        reason: "Recorded the affected example asset.",
        securityTracking: { affectedAssetIds: ["asset-example"] },
      });
      assert.deepEqual(revised.securityTracking.affectedAssetIds, ["asset-example"]);
    } finally {
      reopened.close();
    }
    const persisted = new DatabaseSync(graph.databasePath);
    try {
      const row = persisted.prepare("SELECT security_tracking_json FROM app_server_research_claims WHERE id = ?").get(created.id);
      assert.equal(Object.hasOwn(JSON.parse(row.security_tracking_json), "riskTreatment"), false);
      assert.equal(Object.hasOwn(JSON.parse(row.security_tracking_json), "riskDecisions"), false);
      assert.equal(Object.hasOwn(JSON.parse(row.security_tracking_json), "affectedVersions"), false);
    } finally {
      persisted.close();
    }
  } finally {
    graph.close();
    await rm(workspaceRoot, { recursive: true, force: true });
  }
});

test("same-status finding transitions append evidence without creating duplicate claims", async () => {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "app-server-finding-evidence-append-"));
  const graph = new MemoryGraphStore({ workspaceRoot, context: { ...workspace, sessionId: "session_append" } });
  const claims = new FindingStore(graph);
  try {
    let claim = claims.create({
      title: "Parser accepts an ambiguous frame",
      summary: "A crafted frame reaches an ambiguous parser branch.",
      classification: "security.vulnerability",
      rating: "medium",
    });
    claim = claims.transition(claim.id, {
      expectedRevision: claim.revision,
      toStatus: "observed",
      reason: "The ambiguous branch is directly reachable.",
      evidence: [{ kind: "code", referenceId: "src/frame-parser.ts:84", summary: "The branch accepts both frame forms." }],
    });
    const firstEvidenceId = claim.evidence[0].id;

    assert.throws(() => claims.transition(claim.id, {
      expectedRevision: claim.revision,
      toStatus: "observed",
      reason: "No new evidence was supplied.",
    }), /requires at least one new evidence item/);

    const registry = createResearchToolRegistry(createFindingTools(claims, {
      surfaceStore: { current: () => ({ id: "surface-example" }) },
    }));
    const appended = await registry.execute({
      id: "append_same_status_evidence",
      toolName: "finding.transition",
      actionClass: "synthesize",
      input: {
        id: claim.id,
        expectedRevision: claim.revision,
        toStatus: "observed",
        reason: "A second source location confirms the same observed behavior.",
        evidence: [{ kind: "code", referenceId: "src/frame-validator.ts:31", summary: "Validation preserves the ambiguous form." }],
      },
    });
    assert.equal(appended.result.status, "complete");
    claim = appended.result.output;

    assert.equal(claim.status, "observed");
    assert.equal(claim.maturity, "observed");
    assert.equal(claim.revision, 3);
    assert.equal(claim.evidence.length, 2);
    assert.equal(claim.evidence[0].id, firstEvidenceId);
    assert.equal(claim.transitions.at(-1).fromStatus, "observed");
    assert.equal(claim.transitions.at(-1).toStatus, "observed");
    assert.deepEqual(claim.securityTracking.surfaceIds, ["surface-example"]);
    assert.deepEqual(claim.transitions.at(-1).evidenceIds, [claim.evidence[1].id]);
    assert.equal(claims.list().length, 1);
  } finally {
    claims.close();
    graph.close();
    await rm(workspaceRoot, { recursive: true, force: true });
  }
});

test("a progress blocker preserves observed maturity and can be cleared without refuting the finding", async () => {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "beale-finding-progress-blocker-"));
  const graph = new MemoryGraphStore({ workspaceRoot, context: { ...workspace, sessionId: "session_blocker" } });
  const claims = new FindingStore(graph);
  try {
    let claim = claims.create({ title: "Privileged parser primitive", classification: "security.vulnerability", rating: "medium" });
    claim = claims.transition(claim.id, { expectedRevision: claim.revision, toStatus: "observed",
      reason: "The parser behavior was directly observed.",
      evidence: [{ kind: "code", referenceId: "src/example-parser.ts:40", summary: "The parser reaches the privileged branch." }] });
    const registry = createResearchToolRegistry(createFindingTools(claims));
    const block = await registry.execute({ id: "set_progress_blocker", toolName: "finding.revise", actionClass: "synthesize",
      input: { id: claim.id, expectedRevision: claim.revision, reason: "Current entry points cannot reach the primitive.",
        securityTracking: { surfaceIds: ["surface-example"], progressBlocker: { reason: "All authorized entry paths tested terminate before this primitive; advancing exploitability requires a new reachable entry path." } } } });
    assert.equal(block.result.status, "complete");
    claim = block.result.output;
    assert.equal(claim.status, "observed");
    assert.equal(claim.securityTracking.progressBlocker.reason, "All authorized entry paths tested terminate before this primitive; advancing exploitability requires a new reachable entry path.");
    assert.deepEqual(claim.securityTracking.surfaceIds, ["surface-example"]);
    const clear = await registry.execute({ id: "clear_progress_blocker", toolName: "finding.revise", actionClass: "synthesize",
      input: { id: claim.id, expectedRevision: claim.revision, reason: "A separate reachable chain now uses the primitive.",
        securityTracking: { progressBlocker: null } } });
    assert.equal(clear.result.status, "complete");
    assert.equal(clear.result.output.status, "observed");
    assert.equal(clear.result.output.securityTracking.progressBlocker, null);
    assert.deepEqual(clear.result.output.securityTracking.surfaceIds, ["surface-example"]);
    const blockedAgain = claims.revise(claim.id, {
      expectedRevision: clear.result.output.revision,
      reason: "The newly available route was tested and cannot reach the primitive.",
      securityTracking: { progressBlocker: { reason: "The new authorized route also terminates before this primitive; advancing exploitability now requires a different reachable entry path." } },
    });
    assert.ok(blockedAgain.securityTracking.progressBlocker);
    const stale = claims.transition(claim.id, {
      expectedRevision: blockedAgain.revision, toStatus: "stale",
      reason: "The source revision changed.",
    });
    assert.equal(stale.securityTracking.progressBlocker, null);
  } finally {
    claims.close(); graph.close(); await rm(workspaceRoot, { recursive: true, force: true });
  }
});

test("observed finding creation requires evidence and permits reviewed distinct findings", async () => {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "app-server-lead-create-match-"));
  const graph = new MemoryGraphStore({ workspaceRoot, context: { ...workspace, sessionId: "session_match" } });
  const claims = new FindingStore(graph);
  try {
    let original = claims.create({
      title: "Unchecked frame length reaches allocation",
      summary: "An unchecked length controls the allocation size.",
      classification: "security.vulnerability",
      rating: "high",
    });
    original = claims.transition(original.id, {
      expectedRevision: original.revision,
      toStatus: "observed",
      reason: "The parser passes the unchecked length to allocation.",
      evidence: [{ kind: "code", referenceId: "src/example-frame.ts:42", summary: "Allocation receives the frame length." }],
    });
    const registry = createResearchToolRegistry(createFindingTools(claims));
    const invoke = (id, input) => registry.execute({ id, toolName: "finding.create", actionClass: "synthesize", input });
    const blocked = await invoke("matching_lead", {
      title: "Frame length reaches unchecked allocation",
      summary: "The unchecked frame length controls allocation.",
      classification: "security.vulnerability", rating: "high",
      evidence: [{ kind: "code", referenceId: "src/example-frame.ts:42", summary: "A second review confirms the same path." }],
    });
    assert.equal(blocked.result.status, "error");
    assert.match(blocked.result.error.message, new RegExp(original.id));
    assert.match(blocked.result.error.message, /finding\.transition.*finding\.revise/);
    assert.equal(claims.list().length, 1);

    original = claims.transition(original.id, {
      expectedRevision: original.revision,
      toStatus: original.status,
      reason: "Add the second review to the existing claim.",
      evidence: [{ kind: "code", referenceId: "src/example-validator.ts:17", summary: "Validation leaves the length unchanged." }],
    });
    original = claims.revise(original.id, {
      expectedRevision: original.revision,
      reason: "Clarify the affected allocation path.",
      summary: "An unchecked frame length controls the parser allocation size.",
    });
    assert.equal(original.evidence.length, 2);
    assert.equal(claims.list().length, 1);

    const distinctInput = {
      title: "Frame length bypasses output validation",
      summary: "The same source location also affects a separate output validation path.",
      stepsToReproduce: "Run the synthetic output validator and inspect the returned frame.",
      classification: "security.vulnerability", rating: "medium",
      evidence: [{ kind: "code", referenceId: "src/example-frame.ts:42", summary: "The value also reaches output validation." }],
    };
    const needsReview = await invoke("distinct_needs_review", distinctInput);
    assert.equal(needsReview.result.status, "error");
    assert.match(needsReview.result.error.message, /distinctFromClaimIds/);
    const distinct = await invoke("distinct_reviewed", {
      ...distinctInput,
      distinctFromClaimIds: [original.id],
      distinctReason: "The affected output validation path is separate from the allocation path.",
    });
    assert.equal(distinct.result.status, "complete");
    assert.notEqual(distinct.result.output.id, original.id);
    assert.equal(distinct.result.output.stepsToReproduce, distinctInput.stepsToReproduce);
    assert.match(distinct.result.output.transitions[0].reason, /affected output validation path is separate/);

    const sameClass = await invoke("same_class_new_issue", {
      title: "Unrelated authorization check skips an actor",
      classification: "security.vulnerability", rating: "high",
      evidence: [{ kind: "code", referenceId: "src/example-access.ts:9", summary: "The actor check is skipped." }],
    });
    assert.equal(sameClass.result.status, "complete");
    assert.equal(claims.list().length, 3);
    const unobserved = await invoke("unobserved_new_issue", {
      title: "Unobserved path", classification: "security.vulnerability", rating: "medium", evidence: [],
    });
    assert.notEqual(unobserved.result.status, "complete");
    assert.equal(claims.list().length, 3);
  } finally {
    claims.close();
    graph.close();
    await rm(workspaceRoot, { recursive: true, force: true });
  }
});

test("duplicate claims are hidden from catalogs, nested under their canonical parent, and restorable", async () => {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "app-server-claim-deduplication-"));
  const layout = ensureResearchStorageLayout(createResearchStorageLayout({ workspaceRoot }));
  const graph = new MemoryGraphStore({ workspaceRoot, context: { ...workspace, sessionId: "session_deduplicate" } });
  const claims = new FindingStore(graph);
  try {
    const parent = claims.createObserved({
      title: "Frame length is trusted before validation",
      summary: "The parser allocates from an unvalidated frame length.",
      classification: "security.vulnerability",
      rating: "high",
      evidence: [{ kind: "code", referenceId: "src/example-frame.ts:14", summary: "The frame length reaches allocation." }],
    });
    const duplicate = claims.createObserved({
      title: "Unchecked frame length reaches allocation",
      summary: "An unvalidated frame length controls parser allocation.",
      classification: "security.vulnerability",
      rating: "high",
      evidence: [{ kind: "code", referenceId: "src/example-frame.ts:18", summary: "The same path reaches allocation." }],
    });
    const registry = createResearchToolRegistry([
      ...createFindingTools(claims),
      ...createWorkspaceHistoryDuplicateTools({ claimStore: claims }),
    ]);
    const marked = await registry.execute({
      id: "mark_duplicate_claim",
      toolName: "history.mark_duplicate",
      actionClass: "synthesize",
      input: {
        type: "claim",
        id: duplicate.id,
        expectedRevision: duplicate.revision,
        parentId: parent.id,
        reason: "Both claims describe the same unchecked frame length reaching the same allocation.",
      },
    }, { modelAuthor: { provider: "openai", model: "gpt-5.6" }, agentId: "agent_deduplicator" });
    assert.equal(marked.result.status, "complete");
    assert.equal(marked.result.output.id, parent.id);
    assert.equal(marked.result.output.duplicateClaims.length, 1);
    assert.deepEqual(marked.result.output.duplicateClaims[0], {
      id: duplicate.id,
      projection: "finding",
      maturity: "observed",
      rating: "high",
      classification: "security.vulnerability",
      title: "Unchecked frame length reaches allocation",
      status: "observed",
      revision: 2,
      markedAt: marked.result.output.duplicateClaims[0].markedAt,
    });
    assert.equal(claims.get(duplicate.id).duplicateOfClaimId, parent.id);
    assert.equal(claims.get(duplicate.id).authors.some((author) => author.model === "gpt-5.6"), true);
    assert.deepEqual(claims.list().map((claim) => claim.id), [parent.id]);

    const findingCatalog = await registry.execute({
      id: "list_after_deduplication",
      toolName: "finding.list",
      actionClass: "recall",
      input: { query: "frame length" },
    });
    assert.deepEqual(findingCatalog.result.output.findings.map((claim) => claim.id), [parent.id]);
    assert.equal(findingCatalog.result.output.findings[0].duplicateCount, 1);
    const summary = getAppServerMemorySummary({
      databasePath: layout.databasePath,
      artifactDirectoryPath: layout.artifactDirectoryPath,
      workspaceId: workspace.workspaceId,
      subjectId: workspace.subjectId,
    });
    assert.deepEqual(summary.leads, []);
    assert.deepEqual(summary.findings.map((claim) => claim.id), [parent.id]);
    assert.equal(summary.findings[0].duplicateClaims[0].id, duplicate.id);

    assert.throws(() => claims.transition(duplicate.id, {
      expectedRevision: 2,
      toStatus: "observed",
      reason: "A retained stale action should not mutate the hidden duplicate.",
      evidence: [{ kind: "note", summary: "Synthetic observation." }],
    }), new RegExp(`canonical parent ${parent.id}`));
    assert.throws(() => claims.revise(duplicate.id, {
      expectedRevision: 2,
      title: "Stale duplicate edit",
      reason: "A retained stale action should not revise the hidden duplicate.",
    }), new RegExp(`canonical parent ${parent.id}`));
    assert.throws(
      () => claims.completionChecklist(duplicate.id),
      new RegExp(`canonical parent ${parent.id}`),
    );

    const restored = await registry.execute({
      id: "undo_duplicate_claim",
      toolName: "history.undo_duplicate",
      actionClass: "synthesize",
      input: {
        type: "claim",
        id: duplicate.id,
        expectedRevision: 2,
        reason: "Further review showed that the allocation paths differ.",
      },
    });
    assert.equal(restored.result.status, "complete");
    assert.equal(restored.result.output.duplicateOfClaimId, null);
    assert.equal(restored.result.output.revision, 3);
    assert.equal(claims.get(parent.id).duplicateClaims.length, 0);
    assert.equal(claims.list().length, 2);
  } finally {
    claims.close();
    graph.close();
    await rm(workspaceRoot, { recursive: true, force: true });
  }
});

test("finding lifecycle is canonical, evidence-gated, and supports same-session independent verification and reporting", async () => {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "app-server-findings-"));
  const layout = ensureResearchStorageLayout(createResearchStorageLayout({ workspaceRoot }));
  const originGraph = new MemoryGraphStore({ workspaceRoot, context: { ...workspace, sessionId: "session_origin" } });
  const findings = new FindingStore(originGraph);
  const runbooks = new RunbookStore(originGraph.databasePath, layout, originGraph.getContext());
  let findingId;
  try {
    assert.throws(() => findings.createObserved({
      title: "Missing observation", classification: "security.vulnerability",
      evidence: [{ kind: "code", summary: "An unsupported assertion." }],
    }), /direct code, artifact, command, URL, calculation, proof, or publication evidence/);
    const created = findings.createObserved({
      title: "Parser state crosses requests",
      summary: "A shared parser may retain attacker-controlled state.",
      classification: "security.vulnerability",
      rating: "high",
      sourceRevision: "git:parser:one",
      environmentFingerprint: "environment:parser:one",
      evidence: [{ kind: "code", referenceId: "src/parser.ts:30", summary: "Shared state survives an error path." }],
    }, { provider: "openai", model: "gpt-5.6" }, "agent_origin");
    findingId = created.id;
    assert.equal(created.projection, "finding");
    assert.equal(created.memoryNodeId, null);
    assert.equal(created.rating, "high");
    const findingRegistry = createResearchToolRegistry(createFindingTools(findings));
    const reviseTool = findingRegistry.listTools().find((tool) => tool.descriptor.name === "finding.revise");
    const createTool = findingRegistry.listTools().find((tool) => tool.descriptor.name === "finding.create");
    const transitionTool = findingRegistry.listTools().find((tool) => tool.descriptor.name === "finding.transition");
    assert.ok(reviseTool);
    assert.equal(Object.hasOwn(createTool.parameters.properties, "stepsToReproduce"), true);
    assert.equal(Object.hasOwn(reviseTool.parameters.properties, "stepsToReproduce"), true);
    assert.equal(Object.hasOwn(transitionTool.parameters.properties, "stepsToReproduce"), true);
    assert.equal(Object.hasOwn(reviseTool.parameters.properties.securityTracking.properties, "affectedVersions"), false);
    assert.match(reviseTool.parameters.properties.securityTracking.properties.progressBlocker.description, /Missing proof.*unfinished work, not a blocker/);
    assert.match(reviseTool.parameters.properties.securityTracking.properties.progressBlocker.properties.reason.description, /action that cannot proceed.*concrete impediment.*unblock/);
    assert.ok(transitionTool);
    for (const tool of [createTool, reviseTool, transitionTool]) {
      assert.equal(Object.hasOwn(tool.parameters.properties, "componentClaimIds"), false);
    }
    assert.equal("operatorOverride" in transitionTool.parameters.properties, false);
    assert.equal(findingRegistry.listTools().some((tool) => tool.descriptor.name === "claim.board_transition"), false);
    assert.ok(createTool.parameters.required.includes("rating"));
    assert.ok(createTool.parameters.required.includes("evidence"));
    assert.equal(findingRegistry.listTools().some((tool) => tool.descriptor.name === "lead.create"), true);
    assert.equal(findingRegistry.listTools().some((tool) => tool.descriptor.name === "lead.list"), true);
    assert.deepEqual(createTool.parameters.properties.rating.enum, ["informational", "low", "medium", "high", "critical"]);
    const obsoleteVersionUpdate = await findingRegistry.execute({
      id: "finding_reject_affected_versions",
      toolName: "finding.revise",
      actionClass: "synthesize",
      input: { id: created.id, expectedRevision: created.revision, reason: "Attempt obsolete version tracking.",
        securityTracking: { affectedVersions: [] } },
    });
    assert.equal(obsoleteVersionUpdate.result.status, "error");
    assert.match(obsoleteVersionUpdate.result.error.message, /Affected version tracking is no longer supported/u);
    const catalog = await findingRegistry.execute({
      id: "finding_catalog_initial",
      toolName: "finding.list",
      actionClass: "recall",
      input: {},
    });
    assert.equal(catalog.result.output.total, 1);
    assert.equal(catalog.result.output.findings[0].evidenceCount, 1);
    assert.equal("evidence" in catalog.result.output.findings[0], false);
    const unchangedCatalog = await findingRegistry.execute({
      id: "finding_catalog_unchanged",
      toolName: "finding.list",
      actionClass: "recall",
      input: { afterRevision: catalog.result.output.revision },
    });
    assert.equal(unchangedCatalog.result.output.unchanged, true);
    assert.deepEqual(unchangedCatalog.result.output.findings, []);
    assert.deepEqual(created.authors, [{ provider: "openai", model: "gpt-5.6" }]);
    let finding = findings.transition(findingId, {
      expectedRevision: 1,
      toStatus: "observed",
      reason: "Directly observed in the parser implementation.",
      evidence: [{ kind: "code", referenceId: "src/parser.ts:42", contentHash: "sha256:code", summary: "State is retained on the error path." }],
    }, undefined, "agent_origin");
    assert.equal(finding.id, findingId);
    assert.equal(finding.projection, "finding");
    assert.equal(findings.listLeads().length, 0);
    assert.equal(findings.listFindings()[0].id, findingId);
    assert.throws(() => findings.revise(findingId, {
      expectedRevision: finding.revision,
      reason: "Unsupported reachability",
      securityTracking: {
        reachability: { state: "reachable", conditions: "The network listener accepts the request.", evidenceIds: ["missing_evidence"] },
      },
    }), /unknown finding evidence/);
    finding = findings.revise(findingId, {
      expectedRevision: finding.revision,
      reason: "Record evidence-backed security tracking.",
      impact: "An unauthenticated peer can corrupt parser state across request boundaries.",
      rating: "critical",
      securityTracking: {
        reachability: {
          state: "reachable",
          conditions: "An unauthenticated request reaches the shared parser through the public listener.",
          evidenceIds: [finding.evidence[0].id],
        },
        cvssAssessment: {
          version: "4.0",
          vector: "CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:L/VI:H/VA:N/SC:N/SI:N/SA:N",
          score: 8.7,
          nomenclature: "CVSS-B",
          assessedAt: "2026-08-23T12:00:00.000Z",
          environmentFingerprint: "environment:one",
        },
        affectedAssetIds: ["asset_public_parser"],
        externalReferences: [
          { kind: "cwe", identifier: "CWE-362", url: "https://cwe.mitre.org/data/definitions/362.html" },
          { kind: "prior_art_search", identifier: "shared parser state @ 2026-08-24: no matching advisory", url: null },
        ],
      },
    }, { provider: "openai", model: "gpt-5.6" }, "agent_origin");
    assert.equal(finding.securityTracking.reachability.state, "reachable");
    assert.equal(finding.rating, "critical");
    assert.equal(finding.securityTracking.cvssAssessments[0].score, 8.7);
    assert.deepEqual(finding.securityTracking.affectedAssetIds, ["asset_public_parser"]);
    assert.equal(Object.hasOwn(finding.securityTracking, "affectedVersions"), false);
    assert.equal(finding.securityTracking.externalReferences[0].identifier, "CWE-362");
    const trackedCatalog = await findingRegistry.execute({
      id: "finding_catalog_tracked",
      toolName: "finding.list",
      actionClass: "recall",
      input: {},
    });
    assert.equal(trackedCatalog.result.output.findings[0].securityTracking.reachability.state, "reachable");
    assert.equal(trackedCatalog.result.output.findings[0].completion.ready, false);
    assert.ok(trackedCatalog.result.output.findings[0].completion.missingRequired.includes("reproduction"));
    const observedChecklist = await findingRegistry.execute({
      id: "finding_completion_observed",
      toolName: "finding.completion_check",
      actionClass: "recall",
      input: { id: findingId, targetStatus: "observed" },
    });
    assert.equal(observedChecklist.result.output.ready, true);
    assert.equal(observedChecklist.result.output.items.find((item) => item.key === "steps_to_reproduce").required, false);
    assert.ok(findings.completionChecklist(findingId, "reproduced").missingRequired.includes("steps_to_reproduce"));
    const runbook = runbooks.create({
      title: "Reproduce parser state retention",
      purpose: "Replay the two-request sequence on a clean target.",
      cells: [{ kind: "code", language: "sh", source: "./reproduce.sh", features: ["runtime"] }],
    }).runbook;
    const runId = "runbook_run_one";
    assert.throws(() => findings.transition(findingId, {
      expectedRevision: finding.revision, toStatus: "reproduced", reason: "Missing written steps", reproductionRunbookId: runbook.id,
      evidence: [{ kind: "runbook_execution", referenceId: runId, summary: "Unverified claim" }],
    }), /steps to reproduce/);
    assert.throws(() => findings.transition(findingId, {
      expectedRevision: finding.revision, toStatus: "reproduced", reason: "Unbacked reproduction", reproductionRunbookId: runbook.id,
      stepsToReproduce: "Run the example parser verifier against a clean target and observe retained state.",
      evidence: [{ kind: "runbook_execution", referenceId: runId, summary: "Unverified claim", metadata: { status: "succeeded" } }],
    }), /successful runbook execution/);
    recordRunbookExecution(runbooks, runbook.id, runId, {
      sourceRevision: finding.sourceRevision, environmentFingerprint: finding.environmentFingerprint, actorId: "agent_origin",
    });
    const exposedRunId = runbooks.get(runbook.id).execution.latestSuccessfulRunId;
    assert.equal(exposedRunId, runId);
    finding = findings.transition(findingId, {
      expectedRevision: finding.revision,
      toStatus: "reproduced",
      reason: "The reusable runbook succeeded.",
      reproductionRunbookId: runbook.id,
      stepsToReproduce: "Run the example parser verifier against a clean target and observe retained state.",
      evidence: [{ kind: "runbook_execution", referenceId: exposedRunId, summary: "Clean-state execution succeeded." }],
    }, undefined, "agent_origin");
    assert.equal(finding.stepsToReproduce, "Run the example parser verifier against a clean target and observe retained state.");
    assert.throws(() => findings.revise(findingId, {
      expectedRevision: finding.revision, reason: "Remove reproduction procedure.", stepsToReproduce: "",
    }), /steps to reproduce/);
    assert.throws(() => findings.transition(findingId, {
      expectedRevision: finding.revision,
      toStatus: "verified",
      reason: "Unreviewed verification",
      evidence: [{ kind: "independent_verification", referenceId: "review_origin", summary: "The author repeated the result." }],
    }), /independent reviewer/);
  } finally {
    runbooks.close();
    findings.close();
    originGraph.close();
  }

  const verifierGraph = new MemoryGraphStore({ workspaceRoot, context: { ...workspace, sessionId: "session_origin" } });
  const verifierFindings = new FindingStore(verifierGraph);
  const reports = new ReportStore(verifierGraph.databasePath, layout, verifierGraph.getContext());
  try {
    let finding = verifierFindings.get(findingId);
    finding = verifierFindings.transition(findingId, {
      expectedRevision: finding.revision,
      toStatus: "verified",
      reason: "A distinct reviewer challenged the result and its assumptions in the originating session.",
      evidence: [{ kind: "independent_verification", referenceId: "runbook_run_one", summary: "Same-session independent review held.", independent: true }],
    }, undefined, "agent_reviewer_example", { freshSubagentContext: true });
    assert.equal(finding.status, "verified");
    assert.equal(finding.evidence.at(-1).sessionId, "session_origin");
    const verifiedChecklist = verifierFindings.completionChecklist(findingId, "verified");
    assert.equal(verifiedChecklist.ready, true);
    assert.deepEqual(verifiedChecklist.missingRequired, []);
    assert.equal(verifiedChecklist.items.some((item) => item.key === "affected_versions"), false);
    assert.equal(verifiedChecklist.items.some((item) => item.key === "components"), false);
    const report = reports.create({ title: "Parser state retention", summary: "Verified cross-request state retention.", content: "# Parser state retention\n\nEvidence-backed report." }).report;
    assert.throws(() => verifierFindings.transition(findingId, {
      expectedRevision: finding.revision,
      toStatus: "report_ready",
      reason: "The evidence points at a different report.",
      reportId: report.id,
      evidence: [{ kind: "report", referenceId: "report_other", summary: "Mismatched report artifact." }],
    }), /report reference and report evidence/);
    finding = verifierFindings.transition(findingId, {
      expectedRevision: finding.revision,
      toStatus: "report_ready",
      reason: "A durable report now cites the accepted proof.",
      reportId: report.id,
      evidence: [{ kind: "report", referenceId: report.id, summary: "Complete report artifact." }],
    });
    finding = verifierFindings.transition(findingId, {
      expectedRevision: finding.revision,
      toStatus: "disclosed",
      reason: "Submitted to the authorized program.",
      disclosureReference: "program:submission:123",
      evidence: [{ kind: "disclosure", referenceId: "program:submission:123", summary: "Submission receipt." }],
    });
    assert.equal(finding.status, "disclosed");
    assert.equal(finding.maturity, "verified");
    assert.deepEqual(finding.transitions.map((transition) => transition.revision), [1, 2, 3, 4, 5, 6, 7]);
    assert.deepEqual(finding.transitions.map((transition) => transition.toStatus), ["observed", "observed", "observed", "reproduced", "verified", "report_ready", "disclosed"]);
    assert.deepEqual(
      verifierFindings.refreshStaleness(
        "source:c723f0e579427c695dfa452ab1ed7c5d",
        "environment:0e18bb17e0e84ca7de8cbdab685e3880",
      ),
      [],
    );
    assert.deepEqual(verifierFindings.refreshStaleness("git:other:two", "environment:other:two"), []);
    const stale = verifierFindings.refreshStaleness("git:parser:two", "environment:parser:one")[0];
    assert.equal(stale.status, "stale");
    assert.equal("freshness" in stale, false);
    assert.equal(stale.staleFromStatus, "disclosed");
    assert.match(stale.staleReason, /differs from current revision/);
    assert.equal(stale.sourceRevision, "git:parser:one");
    assert.equal(stale.environmentFingerprint, "environment:parser:one");
    assert.equal(stale.transitions.at(-1).revision, 8);
  } finally {
    reports.close();
    verifierFindings.close();
    verifierGraph.close();
    await rm(workspaceRoot, { recursive: true, force: true });
  }
});

test("campaign graph does not infer asset coverage from a single memory", () => {
  const now = new Date().toISOString();
  const nodes = [
    { id: "memory_sink", sessionIds: [], workspaces: [{ id: "workspace_findings", name: "Findings" }], subjectId: "subject", subjectName: "Subject", type: "sink", title: "Shared state renderer", summary: "Candidate dangerous effect", body: "", status: "suspected", confidence: 0.5, assetIds: ["asset_one"], tags: [], attributes: {}, evidenceRefs: [], createdAt: now, updatedAt: now, revision: 1, authors: [] },
    { id: "memory_refutation", sessionIds: [], workspaces: [{ id: "workspace_findings", name: "Findings" }], subjectId: "subject", subjectName: "Subject", type: "invariant", title: "Cleanup always runs", summary: "Contradictory claim", body: "", status: "supported", confidence: 0.7, assetIds: ["asset_one"], tags: [], attributes: {}, evidenceRefs: [{ id: "evidence", kind: "code", pathBase: "repository", path: "src/parser.ts", locator: {}, summary: "finally block", createdAt: now }], createdAt: now, updatedAt: now, revision: 1, authors: [] },
  ];
  const campaign = buildCampaignGraph({
    nodes,
    edges: [{ fromId: "memory_sink", toId: "memory_refutation", relation: "contradicts", note: null, createdAt: now, updatedAt: now }],
    findings: [researchClaim({ id: "claim_shared_state", title: "Shared state crosses requests" })], runbooks: [], reports: [], assetIds: ["asset_one", "asset_two"],
  });
  assert.equal(campaign.nodes.find((node) => node.id === "asset:asset_one")?.status, "tracked");
  assert.equal(campaign.nodes.find((node) => node.id === "asset:asset_two")?.status, "tracked");
  assert.equal(campaign.coverageGaps.some((gap) => gap.kind === "unexplored_asset"), false);
  assert.ok(campaign.coverageGaps.some((gap) => gap.kind === "unsupported_memory"));
  assert.ok(campaign.coverageGaps.some((gap) => gap.kind === "unobserved_hypothesis"));
  assert.equal(campaign.contradictions.length, 1);
  assert.equal(campaign.momentum.state, "blocked");
  assert.equal(campaign.nextActions[0].priority, "critical");
  const modelContext = createCampaignModelContext(campaign);
  assert.equal(modelContext.schemaVersion, 1);
  assert.deepEqual(modelContext.counts, campaign.counts);
  assert.equal(modelContext.momentum.state, "blocked");
  assert.equal(modelContext.omitted.nodes, campaign.nodes.length);
  assert.equal("nodes" in modelContext, false);
  assert.equal("edges" in modelContext, false);

  const terminalFindingWithTrackedAsset = buildCampaignGraph({
    nodes: [], edges: [], runbooks: [], reports: [], assetIds: ["asset_two"],
    findings: [{
      id: "finding_done", workspaceId: "workspace_findings", subjectId: "subject", memoryNodeId: "memory_done",
      originSessionId: "session", title: "Closed path", summary: "Rejected", impact: "", status: "rejected",
      projection: "lead", maturity: "refuted",
      rating: "informational",
      classification: "security.vulnerability",
      staleFromStatus: null, confidence: 0.1, sourceRevision: null, environmentFingerprint: null,
      reproductionRunbookId: null, reportId: null, disclosureReference: null, staleReason: null,
      evidence: [], transitions: [], authors: [], createdAt: now, updatedAt: now, revision: 1,
    }],
  });
  assert.equal(terminalFindingWithTrackedAsset.momentum.state, "complete");
  assert.equal(terminalFindingWithTrackedAsset.coverageGaps.some((gap) => gap.kind === "unexplored_asset"), false);
});

test("legacy claim-shaped memories migrate once into stable lead and finding projections", async () => {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "app-server-claim-migration-"));
  const layout = ensureResearchStorageLayout(createResearchStorageLayout({ workspaceRoot }));
  const legacyMemory = structuredClone(DEFAULT_SECURITY_RESEARCH_PROFILE.memory);
  legacyMemory.types = legacyMemory.types.map((definition) =>
    ["hypothesis", "primitive", "chain"].includes(definition.id)
      ? { ...definition, lifecycle: "active", creatable: true }
      : definition
  );
  const graph = new MemoryGraphStore({
    workspaceRoot,
    context: { ...workspace, sessionId: "session_legacy" },
    profileMemory: legacyMemory,
  });
  try {
    const leadMemory = graph.save({
      type: "hypothesis",
      title: "Parser state may cross requests",
      summary: "A shared parser could retain attacker-controlled state.",
      status: "suspected",
    });
    const primitiveMemory = graph.save({
      type: "primitive",
      title: "Parser state crosses requests",
      summary: "The error path retains attacker-controlled parser state.",
      status: "confirmed",
      attributes: { rootCause: "Parser cleanup is skipped on error.", rootCauseKey: "parser-cleanup-skipped" },
      evidence: [{ kind: "code", pathBase: "repository", path: "src/parser.ts", locator: { line: 42 }, summary: "Cleanup bypass." }],
    });
    const chainMemory = graph.save({
      type: "chain",
      title: "Retained state reaches privileged parser mode",
      summary: "The retained flag changes the next request's parser mode.",
      status: "confirmed",
      attributes: {
        rootCause: "Retained state crosses the request boundary.",
        rootCauseKey: "retained-state-crosses-request",
        impact: "A later request enters a privileged parser mode.",
        reachability: "Two requests share one parser instance.",
      },
      evidence: [{ kind: "command", pathBase: "workspace", path: "research/reproduce.txt", locator: {}, summary: "Two-request reproduction." }],
    });
    graph.link(chainMemory.id, primitiveMemory.id, "composes");

    const claims = new FindingStore(graph);
    const firstIds = claims.list().map((claim) => claim.id).sort();
    claims.close();
    const legacyDatabase = new DatabaseSync(graph.databasePath);
    try {
      legacyDatabase.prepare("UPDATE app_server_research_claims SET classification = 'security.primitive' WHERE legacy_memory_node_id = ?").run(primitiveMemory.id);
      legacyDatabase.prepare("UPDATE app_server_research_claims SET classification = 'security.chain' WHERE legacy_memory_node_id = ?").run(chainMemory.id);
    } finally {
      legacyDatabase.close();
    }
    const reopened = new FindingStore(graph);
    try {
      assert.deepEqual(reopened.list().map((claim) => claim.id).sort(), firstIds);
      assert.equal(reopened.listLeads().find((claim) => claim.memoryNodeId === leadMemory.id)?.classification, "security.vulnerability");
      assert.equal(reopened.listFindings().find((claim) => claim.memoryNodeId === primitiveMemory.id)?.classification, "security.vulnerability");
      const chain = reopened.listFindings().find((claim) => claim.memoryNodeId === chainMemory.id);
      assert.equal(chain?.classification, "security.vulnerability");
      assert.equal(chain?.securityTracking?.reachability.state, "not_assessed");
      assert.equal(chain?.rating, "informational");
      assert.equal(Object.hasOwn(chain, "componentClaimIds"), false);
      assert.equal(reopened.findCreationMatches({
        title: "Requests parser state crosses", classification: "security.vulnerability",
      }).some((match) => match.id === reopened.listFindings().find((claim) => claim.memoryNodeId === primitiveMemory.id)?.id), true);
    } finally {
      reopened.close();
    }

    const summary = getAppServerMemorySummary({
      databasePath: graph.databasePath,
      artifactDirectoryPath: layout.artifactDirectoryPath,
      workspaceId: workspace.workspaceId,
      subjectId: workspace.subjectId,
    });
    assert.equal(summary.leads.length, 1);
    assert.equal(summary.findings.length, 2);
    assert.equal(summary.campaign.counts.leads, 0);
    assert.equal(summary.campaign.coverageGaps.some((gap) => gap.kind === "unobserved_hypothesis"), false);
    assert.equal(summary.nodes.some((node) => [leadMemory.id, primitiveMemory.id, chainMemory.id].includes(node.id)), false);
  } finally {
    graph.close();
    await rm(workspaceRoot, { recursive: true, force: true });
  }
});

test("claim schema initializes before a workspace has any knowledge-memory tables", async () => {
  const directory = await mkdtemp(join(tmpdir(), "app-server-claim-empty-"));
  const databasePath = join(directory, "memory.sqlite");
  try {
    migrateWorkspaceResearchClaims(databasePath, "workspace_empty");
    const database = new DatabaseSync(databasePath, { readOnly: true });
    try {
      assert.equal(database.prepare("SELECT COUNT(*) AS count FROM app_server_research_claims").get().count, 0);
      assert.equal(database.prepare("SELECT COUNT(*) AS count FROM pragma_table_info('app_server_research_claims') WHERE name = 'security_tracking_json'").get().count, 1);
      assert.equal(database.prepare("SELECT COUNT(*) AS count FROM pragma_table_info('app_server_research_claims') WHERE name = 'rating'").get().count, 1);
      assert.equal(database.prepare("SELECT MAX(version) AS version FROM schema_migrations WHERE component = 'app_server_research_claims'").get().version, 10);
      assert.equal(database.prepare("SELECT COUNT(*) AS count FROM pragma_table_info('app_server_research_claims') WHERE name = 'surface_id'").get().count, 1);
      assert.equal(database.prepare("SELECT COUNT(*) AS count FROM pragma_table_info('app_server_research_claims') WHERE name = 'steps_to_reproduce'").get().count, 1);
      assert.equal(database.prepare("SELECT COUNT(*) AS count FROM pragma_table_info('app_server_claim_transitions') WHERE name = 'session_id'").get().count, 1);
      assert.equal(database.prepare("SELECT COUNT(*) AS count FROM pragma_table_info('app_server_claim_transitions') WHERE name = 'operator_override'").get().count, 1);
      assert.equal(database.prepare("SELECT COUNT(*) AS count FROM sqlite_master WHERE type = 'table' AND name = 'app_server_findings'").get().count, 0);
    } finally {
      database.close();
    }
    const oldDatabase = new DatabaseSync(databasePath);
    try {
      oldDatabase.exec("ALTER TABLE app_server_claim_transitions DROP COLUMN operator_override");
      oldDatabase.exec("ALTER TABLE app_server_research_claims DROP COLUMN steps_to_reproduce");
      oldDatabase.exec("DROP INDEX IF EXISTS app_server_research_claims_surface_idx");
      oldDatabase.exec("ALTER TABLE app_server_research_claims DROP COLUMN surface_id");
      oldDatabase.exec("DELETE FROM schema_migrations WHERE component = 'app_server_research_claims' AND version IN (8, 9, 10)");
    } finally {
      oldDatabase.close();
    }
    migrateWorkspaceResearchClaims(databasePath, "workspace_empty");
    const upgraded = new DatabaseSync(databasePath, { readOnly: true });
    try {
      assert.equal(upgraded.prepare("SELECT COUNT(*) AS count FROM pragma_table_info('app_server_claim_transitions') WHERE name = 'operator_override'").get().count, 1);
      assert.equal(upgraded.prepare("SELECT COUNT(*) AS count FROM pragma_table_info('app_server_research_claims') WHERE name = 'steps_to_reproduce'").get().count, 1);
      assert.equal(upgraded.prepare("SELECT COUNT(*) AS count FROM pragma_table_info('app_server_research_claims') WHERE name = 'surface_id'").get().count, 1);
    } finally {
      upgraded.close();
    }
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

function researchClaim(overrides = {}) {
  const now = "2026-08-23T12:00:00.000Z";
  return {
    id: "claim_default", workspaceId: "workspace_findings", subjectId: "subject", memoryNodeId: null,
    originSessionId: "session", projection: "lead", maturity: "lead",
    rating: "informational",
    classification: "security.vulnerability", title: "Candidate", summary: "", impact: "", stepsToReproduce: "",
    securityTracking: null,
    status: "hypothesis", staleFromStatus: null, confidence: 0.5, sourceRevision: null, environmentFingerprint: null,
    reproductionRunbookId: null, reportId: null, disclosureReference: null, staleReason: null,
    evidence: [], transitions: [], authors: [], createdAt: now, updatedAt: now, revision: 1,
    ...overrides,
  };
}

test("finding attention derives from status without a workflow field", () => {
  const finding = researchClaim({ projection: "finding", maturity: "verified", status: "verified" });
  assert.equal(attentionHeatForClaim(finding), "critical");
  assert.equal(attentionHeatForClaim({ ...finding, status: "report_ready" }), "high");
  assert.equal(attentionHeatForClaim({ ...finding, status: "stale", staleFromStatus: "verified" }), "high");
  assert.equal(attentionHeatForClaim({ ...finding, status: "stale", staleFromStatus: "disclosed" }), "none");
  assert.equal(attentionHeatForClaim({ ...finding, status: "disclosed" }), "none");
  assert.equal(attentionHeatForClaim({ ...finding, status: "rejected" }), "none");
});
