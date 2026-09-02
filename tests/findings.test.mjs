import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { DatabaseSync } from "node:sqlite";

import {
  DEFAULT_SECURITY_RESEARCH_PROFILE,
  FindingStore,
  MemoryGraphStore,
  ReportStore,
  RunbookStore,
  buildCampaignGraph,
  createCampaignModelContext,
  createFindingTools,
  createResearchToolRegistry,
  createResearchStorageLayout,
  ensureResearchStorageLayout,
  getAppServerMemorySummary,
  migrateWorkspaceResearchClaims,
} from "../packages/research-agent/dist/index.js";

const workspace = { workspaceId: "workspace_findings", workspaceName: "Findings", subjectId: "subject_findings", subjectName: "Findings" };

test("finding lifecycle is canonical, evidence-gated, and supports same-session independent verification and reporting", async () => {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "app-server-findings-"));
  const layout = ensureResearchStorageLayout(createResearchStorageLayout({ workspaceRoot }));
  const originGraph = new MemoryGraphStore({ workspaceRoot, context: { ...workspace, sessionId: "session_origin" } });
  const findings = new FindingStore(originGraph);
  const runbooks = new RunbookStore(originGraph.databasePath, layout, originGraph.getContext());
  let findingId;
  try {
    const created = findings.create({
      title: "Parser state crosses requests",
      summary: "A shared parser may retain attacker-controlled state.",
      classification: "security.vulnerability",
      rating: "high",
      sourceRevision: "git:parser:one",
      environmentFingerprint: "environment:parser:one",
    }, { provider: "openai", model: "gpt-5.6" }, "agent_origin");
    findingId = created.id;
    assert.equal(created.projection, "lead");
    assert.equal(created.memoryNodeId, null);
    assert.equal(created.rating, "high");
    const findingRegistry = createResearchToolRegistry(createFindingTools(findings));
    const reviseTool = findingRegistry.listTools().find((tool) => tool.descriptor.name === "finding.revise");
    const createTool = findingRegistry.listTools().find((tool) => tool.descriptor.name === "lead.create");
    assert.ok(reviseTool);
    assert.ok(createTool.parameters.required.includes("rating"));
    assert.deepEqual(createTool.parameters.properties.rating.enum, ["informational", "low", "medium", "high", "critical"]);
    assert.equal("riskTreatment" in reviseTool.parameters.properties.securityTracking.properties, false);
    const catalog = await findingRegistry.execute({
      id: "finding_catalog_initial",
      toolName: "lead.list",
      actionClass: "recall",
      input: {},
    });
    assert.equal(catalog.result.output.total, 1);
    assert.equal(catalog.result.output.leads[0].evidenceCount, 0);
    assert.equal("evidence" in catalog.result.output.leads[0], false);
    const unchangedCatalog = await findingRegistry.execute({
      id: "finding_catalog_unchanged",
      toolName: "lead.list",
      actionClass: "recall",
      input: { afterRevision: catalog.result.output.revision },
    });
    assert.equal(unchangedCatalog.result.output.unchanged, true);
    assert.deepEqual(unchangedCatalog.result.output.leads, []);
    assert.deepEqual(created.authors, [{ provider: "openai", model: "gpt-5.6" }]);
    assert.throws(() => findings.transition(findingId, {
      expectedRevision: 1, toStatus: "observed", reason: "Claimed without a durable reference",
      evidence: [{ kind: "code", summary: "Parser assignment" }],
    }), /direct code, artifact, command, URL, calculation, proof, or publication evidence/);

    let finding = findings.transition(findingId, {
      expectedRevision: 1,
      toStatus: "observed",
      reason: "Directly observed in the parser implementation.",
      evidence: [{ kind: "code", referenceId: "src/parser.ts:42", contentHash: "sha256:code", summary: "State is retained on the error path." }],
    });
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
        affectedVersions: [{ assetId: "asset_public_parser", range: "1.0.0 - 1.4.x", fixedVersion: "1.5.0" }],
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
    const runbook = runbooks.create({
      title: "Reproduce parser state retention",
      purpose: "Replay the two-request sequence on a clean target.",
      cells: [{ kind: "code", language: "sh", source: "./reproduce.sh" }],
    }).runbook;
    const runId = "runbook_run_one";
    assert.throws(() => findings.transition(findingId, {
      expectedRevision: finding.revision, toStatus: "reproduced", reason: "Unbacked reproduction", reproductionRunbookId: runbook.id,
      evidence: [{ kind: "runbook_execution", referenceId: runId, summary: "Unverified claim", metadata: { status: "succeeded" } }],
    }), /successful runbook execution/);
    const startedAt = new Date().toISOString();
    runbooks.beginExecution(runbook.id, runId, runbooks.executionPlan(runbook.id).map((cell) => cell.id), "localhost");
    runbooks.completeExecution({
      id: runbook.id,
      runId,
      status: "succeeded",
      startedAt,
      completedAt: new Date().toISOString(),
      durationMs: 1,
      proofTarget: "localhost",
    });
    const exposedRunId = runbooks.get(runbook.id).execution.latestSuccessfulRunId;
    assert.equal(exposedRunId, runId);
    finding = findings.transition(findingId, {
      expectedRevision: finding.revision,
      toStatus: "reproduced",
      reason: "The reusable runbook succeeded.",
      reproductionRunbookId: runbook.id,
      evidence: [{ kind: "runbook_execution", referenceId: exposedRunId, summary: "Clean-state execution succeeded." }],
    });
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
      evidence: [{ kind: "independent_verification", referenceId: "verification_run_two", summary: "Same-session independent review held.", independent: true }],
    });
    assert.equal(finding.status, "verified");
    assert.equal(finding.evidence.at(-1).sessionId, "session_origin");
    const verifiedChecklist = verifierFindings.completionChecklist(findingId, "verified");
    assert.equal(verifiedChecklist.ready, true);
    assert.deepEqual(verifiedChecklist.missingRequired, []);
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
    assert.throws(() => verifierFindings.revise(findingId, {
      expectedRevision: finding.revision,
      reason: "A model attempted to accept organizational risk.",
      securityTracking: {
        riskTreatment: "accepted",
        riskDecision: { rationale: "The exposure is acceptable." },
      },
    }, { provider: "openai", model: "gpt-5.6" }, "agent_verifier"), /human operator/);
    finding = verifierFindings.revise(findingId, {
      expectedRevision: finding.revision,
      reason: "The authorized owner accepted the residual risk for this deployment.",
      securityTracking: {
        riskTreatment: "accepted",
        riskDecision: {
          rationale: "The affected deployment is isolated pending retirement.",
          expiresAt: "2026-12-31T23:59:59.000Z",
        },
      },
    }, undefined, "operator_alice");
    assert.equal(finding.securityTracking.riskTreatment, "accepted");
    assert.equal(finding.securityTracking.riskDecisions.at(-1).actorId, "operator_alice");
    assert.deepEqual(finding.transitions.map((transition) => transition.revision), [1, 2, 3, 4, 5, 6, 7, 8]);
    assert.deepEqual(finding.transitions.map((transition) => transition.toStatus), ["hypothesis", "observed", "observed", "reproduced", "verified", "report_ready", "disclosed", "disclosed"]);
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
    assert.equal(stale.staleFromStatus, "disclosed");
    assert.match(stale.staleReason, /differs from current revision/);
    assert.equal(stale.sourceRevision, "git:parser:one");
    assert.equal(stale.environmentFingerprint, "environment:parser:one");
    assert.equal(stale.transitions.at(-1).revision, 9);
  } finally {
    reports.close();
    verifierFindings.close();
    verifierGraph.close();
    await rm(workspaceRoot, { recursive: true, force: true });
  }
});

test("campaign graph exposes uncovered territory, lifecycle gates, contradictions, and typed momentum", () => {
  const now = new Date().toISOString();
  const nodes = [
    { id: "memory_source", sessionIds: [], workspaces: [{ id: "workspace_findings", name: "Findings" }], subjectId: "subject", subjectName: "Subject", type: "flow-endpoint", title: "Shared state source", summary: "Candidate ingress", body: "", status: "suspected", confidence: 0.5, assetIds: ["asset_one"], tags: [], attributes: { role: "source" }, evidenceRefs: [], createdAt: now, updatedAt: now, revision: 1, authors: [] },
    { id: "memory_refutation", sessionIds: [], workspaces: [{ id: "workspace_findings", name: "Findings" }], subjectId: "subject", subjectName: "Subject", type: "invariant", title: "Cleanup always runs", summary: "Contradictory claim", body: "", status: "supported", confidence: 0.7, assetIds: ["asset_one"], tags: [], attributes: {}, evidenceRefs: [{ id: "evidence", kind: "code", pathBase: "repository", path: "src/parser.ts", locator: {}, summary: "finally block", createdAt: now }], createdAt: now, updatedAt: now, revision: 1, authors: [] },
  ];
  const campaign = buildCampaignGraph({
    nodes,
    edges: [{ fromId: "memory_source", toId: "memory_refutation", relation: "contradicts", note: null, createdAt: now, updatedAt: now }],
    findings: [researchClaim({ id: "claim_shared_state", title: "Shared state crosses requests" })], runbooks: [], reports: [], assetIds: ["asset_one", "asset_two"],
  });
  assert.ok(campaign.coverageGaps.some((gap) => gap.kind === "unexplored_asset"));
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

  const terminalFindingWithUnexploredAsset = buildCampaignGraph({
    nodes: [], edges: [], runbooks: [], reports: [], assetIds: ["asset_two"],
    findings: [{
      id: "finding_done", workspaceId: "workspace_findings", subjectId: "subject", memoryNodeId: "memory_done",
      originSessionId: "session", title: "Closed path", summary: "Rejected", impact: "", status: "rejected",
      projection: "lead", maturity: "refuted", freshness: "current", workflow: "closed",
      rating: "informational",
      classification: "security.vulnerability", componentClaimIds: [],
      staleFromStatus: null, confidence: 0.1, sourceRevision: null, environmentFingerprint: null,
      reproductionRunbookId: null, reportId: null, disclosureReference: null, staleReason: null,
      evidence: [], transitions: [], authors: [], createdAt: now, updatedAt: now, revision: 1,
    }],
  });
  assert.equal(terminalFindingWithUnexploredAsset.momentum.state, "exploring");
  assert.ok(terminalFindingWithUnexploredAsset.coverageGaps.some((gap) => gap.kind === "unexplored_asset"));
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
    const reopened = new FindingStore(graph);
    try {
      assert.deepEqual(reopened.list().map((claim) => claim.id).sort(), firstIds);
      assert.equal(reopened.listLeads().find((claim) => claim.memoryNodeId === leadMemory.id)?.classification, "security.vulnerability");
      assert.equal(reopened.listFindings().find((claim) => claim.memoryNodeId === primitiveMemory.id)?.classification, "security.primitive");
      const chain = reopened.listFindings().find((claim) => claim.memoryNodeId === chainMemory.id);
      assert.equal(chain?.classification, "security.chain");
      assert.equal(chain?.securityTracking?.reachability.state, "not_assessed");
      assert.equal(chain?.securityTracking?.riskTreatment, "unreviewed");
      assert.equal(chain?.rating, "informational");
      assert.deepEqual(chain?.componentClaimIds, [
        reopened.listFindings().find((claim) => claim.memoryNodeId === primitiveMemory.id)?.id,
      ]);
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
      assert.equal(database.prepare("SELECT MAX(version) AS version FROM schema_migrations WHERE component = 'app_server_research_claims'").get().version, 5);
      assert.equal(database.prepare("SELECT COUNT(*) AS count FROM pragma_table_info('app_server_claim_transitions') WHERE name = 'session_id'").get().count, 1);
      assert.equal(database.prepare("SELECT COUNT(*) AS count FROM sqlite_master WHERE type = 'table' AND name = 'app_server_findings'").get().count, 0);
    } finally {
      database.close();
    }
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

function researchClaim(overrides = {}) {
  const now = "2026-08-23T12:00:00.000Z";
  return {
    id: "claim_default", workspaceId: "workspace_findings", subjectId: "subject", memoryNodeId: null,
    originSessionId: "session", projection: "lead", maturity: "proposed", freshness: "current", workflow: "open",
    rating: "informational",
    classification: "security.vulnerability", componentClaimIds: [], title: "Candidate", summary: "", impact: "",
    securityTracking: null,
    status: "hypothesis", staleFromStatus: null, confidence: 0.5, sourceRevision: null, environmentFingerprint: null,
    reproductionRunbookId: null, reportId: null, disclosureReference: null, staleReason: null,
    evidence: [], transitions: [], authors: [], createdAt: now, updatedAt: now, revision: 1,
    ...overrides,
  };
}
