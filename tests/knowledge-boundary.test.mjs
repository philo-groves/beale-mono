import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import test from "node:test";
import { invokeAppServerProtocol } from "../app-server/dist/appServerProtocolClient.js";

import {
  CampaignTrackStore,
  DEFAULT_SECURITY_RESEARCH_PROFILE,
  AppServerSessionStore,
  MemoryGraphStore,
  ResearchClaimStore,
  ReportStore,
  RunbookStore,
  buildMemoryDreamingInstructions,
  createResearchStorageLayout,
  ensureResearchStorageLayout,
  getAppServerMemorySummary,
  getKnowledgeReport,
  getKnowledgeRunbook,
  migrateWorkspaceResearchClaims,
  normalizeResearchProfile,
  parseMemoryDreamingPlanOutput,
  researchProfileHash,
  resolveKnowledgeArtifact,
  restoreMemoryDreamingChange,
  runMemoryDreaming,
} from "../packages/research-agent/dist/index.js";

test("app-server exposes reversible canonical claim deduplication", async () => {
  const root = await mkdtemp(join(tmpdir(), "app-server-claim-deduplication-boundary-"));
  const databasePath = join(root, "memory.sqlite");
  const artifactDirectoryPath = join(root, "artifacts");
  const context = {
    workspaceId: "workspace_claim_deduplication",
    workspaceName: "Claim deduplication",
    subjectId: "subject_claim_deduplication",
    subjectName: "Claim deduplication",
  };
  const graph = new MemoryGraphStore({ databasePath, context });
  const claims = new ResearchClaimStore(graph);
  try {
    const parent = claims.create({
      title: "Canonical parser boundary",
      classification: "security.primitive",
      rating: "medium",
    });
    const duplicate = claims.create({
      title: "Redundant parser boundary",
      classification: "security.primitive",
      rating: "medium",
    });

    const markedParent = await invokeAppServerProtocol("claim.mark_duplicate", {
      args: [],
      input: {
        ...context,
        claimId: duplicate.id,
        parentClaimId: parent.id,
        expectedRevision: duplicate.revision,
      },
      storage: { databasePath, artifactDirectoryPath },
    });
    assert.equal(markedParent.id, parent.id);
    assert.equal(markedParent.duplicateClaims[0].id, duplicate.id);

    const summary = await invokeAppServerProtocol("memory.summary", {
      args: [],
      input: context,
      storage: { databasePath, artifactDirectoryPath },
    });
    assert.deepEqual(summary.leads.map((claim) => claim.id), [parent.id]);

    const restored = await invokeAppServerProtocol("claim.undo_duplicate", {
      args: [],
      input: { ...context, claimId: duplicate.id, expectedRevision: duplicate.revision + 1 },
      storage: { databasePath, artifactDirectoryPath },
    });
    assert.equal(restored.duplicateOfClaimId, null);
    assert.equal(claims.list().length, 2);
  } finally {
    claims.close();
    graph.close();
    await rm(root, { recursive: true, force: true });
  }
});

test("app-server exposes unified memory and runbook deduplication", async () => {
  const root = await mkdtemp(join(tmpdir(), "app-server-history-deduplication-boundary-"));
  const databasePath = join(root, "memory.sqlite");
  const artifactDirectoryPath = join(root, "artifacts");
  const context = {
    workspaceId: "workspace_history_deduplication",
    workspaceName: "History deduplication",
    subjectId: "subject_history_deduplication",
    subjectName: "History deduplication",
  };
  const graph = new MemoryGraphStore({ databasePath, context });
  const runbooks = new RunbookStore(
    databasePath,
    ensureResearchStorageLayout(createResearchStorageLayout({ databasePath, artifactDirectoryPath })),
    context,
  );
  try {
    const parentMemory = graph.save({ type: "invariant", title: "Canonical parser boundary" });
    const duplicateMemory = graph.save({ type: "invariant", title: "Repeated parser boundary" });
    const parentRunbook = runbooks.create({
      title: "Canonical parser procedure",
      purpose: "Exercise the parser boundary.",
    }).runbook;
    const duplicateRunbook = runbooks.create({
      title: "Repeated parser procedure",
      purpose: "Exercise the same parser boundary.",
    }).runbook;

    for (const item of [
      { type: "memory", id: duplicateMemory.id, parentId: parentMemory.id, expectedRevision: duplicateMemory.revision },
      { type: "runbook", id: duplicateRunbook.id, parentId: parentRunbook.id, expectedRevision: duplicateRunbook.revision },
    ]) {
      await invokeAppServerProtocol("history.mark_duplicate", {
        args: [],
        input: { ...context, ...item, reason: "Same underlying workspace-history record." },
        storage: { databasePath, artifactDirectoryPath },
      });
    }

    assert.deepEqual(graph.search({ scope: "workspace", limit: 20 }).map((node) => node.id), [parentMemory.id]);
    assert.equal(graph.get(parentMemory.id).duplicateMemories[0].id, duplicateMemory.id);
    assert.deepEqual(runbooks.list({ limit: 20 }).map((runbook) => runbook.id), [parentRunbook.id]);
    assert.equal(runbooks.get(parentRunbook.id).duplicateRunbooks[0].id, duplicateRunbook.id);

    for (const item of [
      { type: "memory", id: duplicateMemory.id },
      { type: "runbook", id: duplicateRunbook.id },
    ]) {
      await invokeAppServerProtocol("history.undo_duplicate", {
        args: [],
        input: { ...context, ...item, expectedRevision: 2, reason: "The records differ after review." },
        storage: { databasePath, artifactDirectoryPath },
      });
    }

    assert.equal(graph.search({ scope: "workspace", limit: 20 }).length, 2);
    assert.equal(runbooks.list({ limit: 20 }).length, 2);
  } finally {
    runbooks.close();
    graph.close();
    await rm(root, { recursive: true, force: true });
  }
});

test("memory summary does not mutate finding staleness", async () => {
  const root = await mkdtemp(join(tmpdir(), "app-server-summary-staleness-"));
  const databasePath = join(root, "memory.sqlite");
  const artifactDirectoryPath = join(root, "artifacts");
  const now = "2026-08-24T12:00:00.000Z";
  try {
    ensureResearchStorageLayout(createResearchStorageLayout({ databasePath, artifactDirectoryPath }));
    migrateWorkspaceResearchClaims(databasePath, "workspace_summary");
    const database = new DatabaseSync(databasePath);
    try {
      database.prepare(`INSERT INTO app_server_research_claims (
        id, workspace_id, subject_id, legacy_memory_node_id, origin_session_id, classification,
        title, summary, impact, rating, status, stale_from_status, confidence, source_revision,
        environment_fingerprint, reproduction_runbook_id, report_id, disclosure_reference,
        stale_reason, security_tracking_json, created_at, updated_at, revision
      ) VALUES ('claim_summary', 'workspace_summary', 'subject_summary', NULL, NULL,
        'security.vulnerability', 'Summary invariant', '', '', 'high', 'observed', NULL, 0.8,
        'git:parser:one', 'environment:parser:one', NULL, NULL, NULL, NULL, 'null', ?, ?, 1)`).run(now, now);
      database.prepare(`INSERT INTO app_server_claim_transitions
        (id, claim_id, claim_revision, from_status, to_status, reason, actor_id, evidence_ids_json, created_at)
        VALUES ('transition_summary', 'claim_summary', 1, 'hypothesis', 'observed', 'Observed.', 'agent', '[]', ?)`).run(now);
    } finally {
      database.close();
    }

    await invokeAppServerProtocol("memory.summary", {
      args: [],
      input: {
        workspaceId: "workspace_summary",
        subjectId: "subject_summary",
        sourceRevision: "git:parser:two",
        environmentFingerprint: "environment:parser:two",
      },
      storage: { databasePath, artifactDirectoryPath },
    });

    const readDatabase = new DatabaseSync(databasePath, { readOnly: true });
    try {
      assert.deepEqual({ ...readDatabase.prepare(`SELECT status, stale_from_status, source_revision,
        environment_fingerprint, stale_reason, revision FROM app_server_research_claims
        WHERE id = 'claim_summary'`).get() }, {
        status: "observed",
        stale_from_status: null,
        source_revision: "git:parser:one",
        environment_fingerprint: "environment:parser:one",
        stale_reason: null,
        revision: 1,
      });
    } finally {
      readDatabase.close();
    }
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("report catalog reads survive unrelated foreign-key violations", async () => {
  const root = await mkdtemp(join(tmpdir(), "app-server-report-catalog-boundary-"));
  const databasePath = join(root, "memory.sqlite");
  const layout = ensureResearchStorageLayout(createResearchStorageLayout({
    databasePath,
    artifactDirectoryPath: join(root, "artifacts"),
  }));
  const reports = new ReportStore(databasePath, layout, {
    workspaceId: "workspace_report_catalog",
    workspaceName: "Report catalog",
  });
  let report;
  try {
    report = reports.create({
      title: "Parser result",
      summary: "A report that remains readable when unrelated memory references are stale.",
      content: "# Parser result\n\nConfirmed.",
    }).report;
  } finally {
    reports.close();
  }

  try {
    const database = new DatabaseSync(databasePath);
    try {
      database.exec("PRAGMA foreign_keys = OFF;");
      database.exec("ALTER TABLE app_server_reports DROP COLUMN triage_status;");
      database.prepare(`INSERT INTO memory_node_workspaces(node_id, workspace_id, workspace_name)
        VALUES ('missing_memory_node', 'workspace_report_catalog', 'Report catalog')`).run();
      assert.equal(database.prepare("PRAGMA foreign_key_check").all().length, 1);
    } finally {
      database.close();
    }

    const catalog = await invokeAppServerProtocol("report.list", {
      args: [],
      input: { workspaceId: "workspace_report_catalog" },
      storage: { databasePath, artifactDirectoryPath: layout.artifactDirectoryPath },
    });
    assert.equal(catalog.length, 1);
    assert.equal(catalog[0].id, report.id);
    assert.equal(catalog[0].title, "Parser result");
    assert.equal(catalog[0].triageStatus, "editing");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("app-server owns memory summaries, documents, artifact resolution, and Dreaming state", async () => {
  const root = await mkdtemp(join(tmpdir(), "app-server-knowledge-boundary-"));
  const databasePath = join(root, "memory.sqlite");
  const layout = ensureResearchStorageLayout(createResearchStorageLayout({
    workspaceRoot: root,
    databasePath,
    artifactDirectoryPath: join(root, "artifacts"),
  }));
  const context = {
    sessionId: "session_one",
    workspaceId: "workspace_one",
    workspaceName: "One",
    subjectId: "subject_one",
    subjectName: "One subject",
  };
  const profile = normalizeResearchProfile(DEFAULT_SECURITY_RESEARCH_PROFILE);
  const profileHash = researchProfileHash(profile);
  const profileInput = {
    profileSnapshot: {
      id: "profile_snapshot_one",
      workspaceId: context.workspaceId,
      profileId: profile.id,
      profileVersion: profile.version,
      profileHash,
      source: "bundled-default",
      sourcePath: null,
      profile,
      active: true,
      createdAt: new Date().toISOString(),
    },
  };
  const memory = new MemoryGraphStore({ databasePath, context });
  const runbooks = new RunbookStore(databasePath, layout, context);
  const reports = new ReportStore(databasePath, layout, context);
  try {
    memory.save({ type: "trajectory", title: "Shared parser state investigation", summary: "The inspection path for cross-request state." });
    const runbook = runbooks.create({
      title: "Parser proof",
      purpose: "Preserve the bounded reproduction.",
      cells: [{ kind: "code", language: "sh", source: "./proof.sh", stdout: "confirmed\n" }],
    }).runbook;
    const report = reports.create({
      title: "Parser result",
      summary: "The confirmed result.",
      content: "# Parser result\n\nConfirmed.",
    }).report;

    const summary = getAppServerMemorySummary({
      databasePath,
      artifactDirectoryPath: layout.artifactDirectoryPath,
      workspaceId: context.workspaceId,
      subjectId: context.subjectId,
      researchProfile: profileInput.profileSnapshot,
    });
    assert.equal(summary.nodeCount, 1, summary.lastError ?? undefined);
    assert.equal(summary.runbookCount, 1);
    assert.equal(summary.reportCount, 1);
    assert.equal(getKnowledgeRunbook(databasePath, layout.artifactDirectoryPath, context.workspaceId, runbook.id).nbformat, 4);
    assert.match(getKnowledgeReport(databasePath, layout.artifactDirectoryPath, context.workspaceId, report.id).content, /Confirmed/);
    assert.equal(resolveKnowledgeArtifact(runbook.artifactId, {
      databasePath,
      artifactDirectoryPath: layout.artifactDirectoryPath,
      expectedKind: "runbook",
    }).kind, "runbook");

    const hostedRunbook = await invokeAppServerProtocol("runbook.get", {
      args: [],
      input: { workspaceId: context.workspaceId, runbookId: runbook.id },
      storage: {
        databasePath,
        artifactDirectoryPath: layout.artifactDirectoryPath,
      },
    });
    assert.equal(hostedRunbook.runbookId, runbook.id);

    const revisedReport = await invokeAppServerProtocol("report.revise_content", {
      args: [],
      input: {
        workspaceId: context.workspaceId,
        workspaceName: context.workspaceName,
        reportId: report.id,
        expectedRevision: 1,
        content: "# Parser result\n\nConfirmed with direct triage edits.",
      },
      storage: {
        databasePath,
        artifactDirectoryPath: layout.artifactDirectoryPath,
      },
    });
    assert.equal(revisedReport.revision, 2);
    assert.match(getKnowledgeReport(databasePath, layout.artifactDirectoryPath, context.workspaceId, report.id).content, /direct triage edits/);

    const submittedReport = await invokeAppServerProtocol("report.update_triage_status", {
      args: [],
      input: {
        workspaceId: context.workspaceId,
        workspaceName: context.workspaceName,
        reportId: report.id,
        expectedRevision: 2,
        triageStatus: "submitted",
      },
      storage: {
        databasePath,
        artifactDirectoryPath: layout.artifactDirectoryPath,
      },
    });
    assert.equal(submittedReport.revision, 3);
    assert.equal(submittedReport.triageStatus, "submitted");

    const replacementPacketPath = join(root, "replacement.zip");
    await writeFile(replacementPacketPath, Buffer.from([0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0]));
    const replacedReport = await invokeAppServerProtocol("report.replace_packet", {
      args: [],
      input: {
        workspaceId: context.workspaceId,
        workspaceName: context.workspaceName,
        workspaceRoot: root,
        reportId: report.id,
        submissionPacketPath: replacementPacketPath,
      },
      storage: {
        databasePath,
        artifactDirectoryPath: layout.artifactDirectoryPath,
      },
    });
    assert.equal(replacedReport.revision, 4);
    assert.equal(replacedReport.submissionPacket.filename, "submission.zip");

    const recordingPath = join(root, "parser-demo.webm");
    await writeFile(recordingPath, Buffer.from("recording"));
    const reportWithRecording = await invokeAppServerProtocol("report.replace_recording", {
      args: [],
      input: {
        workspaceId: context.workspaceId,
        workspaceName: context.workspaceName,
        workspaceRoot: root,
        reportId: report.id,
        recordingPath,
      },
      storage: {
        databasePath,
        artifactDirectoryPath: layout.artifactDirectoryPath,
      },
    });
    assert.equal(reportWithRecording.revision, 5);
    assert.equal(reportWithRecording.recording.filename, "parser-demo.webm");
    const summaryWithRecording = getAppServerMemorySummary({
      databasePath,
      artifactDirectoryPath: layout.artifactDirectoryPath,
      workspaceId: context.workspaceId,
      subjectId: context.subjectId,
      researchProfile: profileInput.profileSnapshot,
    });
    assert.equal(summaryWithRecording.reports.find((candidate) => candidate.id === report.id)?.recording?.filename, "parser-demo.webm");

    const instructions = buildMemoryDreamingInstructions({}, profileInput);
    assert.match(instructions, /strict JSON/i);
    const plan = parseMemoryDreamingPlanOutput("```json\n{\"prune\":[],\"merge\":[],\"revise\":[],\"reclassify\":[]}\n```", profileInput);
    const dreamed = runMemoryDreaming(databasePath, context.workspaceId, plan, {
      provider: "openai",
      model: "test-model",
      reasoningEffort: "high",
      inputNodeCount: 1,
      inputSessionCount: 1,
    }, profileInput);
    assert.equal(dreamed.status, "completed");
    assert.equal(dreamed.editedNodeCount, 0);
    assert.throws(() => restoreMemoryDreamingChange(databasePath, context.workspaceId, "missing"), /not found/);
  } finally {
    reports.close();
    runbooks.close();
    memory.close();
    await rm(root, { recursive: true, force: true });
  }
});
