import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import test from "node:test";

import {
  createReportTools,
  createResearchStorageLayout,
  createResearchToolRegistry,
  ensureResearchStorageLayout,
  getDefaultMemoryDatabasePath,
  listResearchStorageArtifacts,
  MemoryGraphStore,
  ResearchClaimStore,
  ReportStore,
  RunbookStore,
} from "../packages/research-agent/dist/index.js";

test("reports persist revisioned Markdown artifacts within one workspace", async () => {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "app-server-report-"));
  const layout = ensureResearchStorageLayout(createResearchStorageLayout({ workspaceRoot }));
  const databasePath = getDefaultMemoryDatabasePath(workspaceRoot);
  const store = new ReportStore(databasePath, layout, {
    sessionId: "run_one", workspaceId: "workspace_one", workspaceName: "One",
  });
  try {
    const created = store.create(
      { title: "A useful result", summary: "A short explanation.", content: "# A useful result\n\nHere is what changed." },
      { provider: "openai", model: "gpt-5.6" },
    );
    assert.equal(created.report.status, "complete");
    assert.equal(created.report.triageStatus, "editing");
    assert.equal(created.report.revision, 1);
    assert.equal(created.artifactRef.kind, "report");
    const revised = store.revise(
      { id: created.report.id, expectedRevision: 1, content: "# A useful result\n\nA clearer explanation.", status: "stale" },
      { provider: "zai", model: "glm-5" },
    );
    assert.equal(revised.report.revision, 2);
    assert.equal(revised.report.status, "stale");
    assert.deepEqual(revised.report.authors, [
      { provider: "openai", model: "gpt-5.6" },
      { provider: "zai", model: "glm-5" },
    ]);
    assert.throws(() => store.revise({ id: created.report.id, expectedRevision: 1, content: "stale write" }), /revision conflict/);
    const accepted = store.updateTriageStatus({ id: created.report.id, expectedRevision: 2, triageStatus: "accepted" });
    assert.equal(accepted.triageStatus, "accepted");
    assert.equal(accepted.revision, 3);
    assert.match(store.get(created.report.id).content, /clearer explanation/);
    assert.equal(store.list({ statuses: ["stale"] }).length, 1);
    const artifacts = listResearchStorageArtifacts(layout, { kind: "report" });
    assert.equal(artifacts.length, 1);
    assert.match(await readFile(artifacts[0].path, "utf8"), /clearer explanation/);
    const database = new DatabaseSync(databasePath, { readOnly: true });
    try {
      assert.deepEqual(
        database.prepare(`SELECT artifact_kind, artifact_id, session_id, revision
          FROM app_server_artifact_revisions
          WHERE artifact_id = ? ORDER BY revision`).all(created.report.id).map((row) => ({ ...row })),
        [
          { artifact_kind: "report", artifact_id: created.report.id, session_id: "run_one", revision: 1 },
          { artifact_kind: "report", artifact_id: created.report.id, session_id: "run_one", revision: 2 },
          { artifact_kind: "report", artifact_id: created.report.id, session_id: "run_one", revision: 3 },
        ],
      );
    } finally {
      database.close();
    }
  } finally {
    store.close();
    await rm(workspaceRoot, { recursive: true, force: true });
  }
});

test("report tools expose list, read, create, and revise operations", async () => {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "app-server-report-tools-"));
  const layout = ensureResearchStorageLayout(createResearchStorageLayout({ workspaceRoot }));
  const store = new ReportStore(getDefaultMemoryDatabasePath(workspaceRoot), layout, { workspaceId: "workspace_tools", workspaceName: "Tools" });
  const registry = createResearchToolRegistry(createReportTools(store));
  try {
    assert.deepEqual(registry.listDescriptors().map((tool) => tool.name), ["report.list", "report.get", "report.create", "report.revise"]);
    const created = await registry.execute(
      { id: "create_report", actionClass: "synthesize", toolName: "report.create", input: { title: "Result", summary: "Shareable result.", content: "# Result\n\nReadable prose." } },
      { modelAuthor: { provider: "anthropic", model: "claude-sonnet-4-5" } },
    );
    assert.equal(created.result.status, "complete");
    assert.equal(created.result.artifactRefs[0].kind, "report");
    assert.deepEqual(created.result.output.authors, [{ provider: "anthropic", model: "claude-sonnet-4-5" }]);
  } finally {
    store.close();
    await rm(workspaceRoot, { recursive: true, force: true });
  }
});

test("reports import and retain bounded workspace packet and recording attachments", async () => {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "app-server-report-packet-"));
  const outsideRoot = await mkdtemp(join(tmpdir(), "app-server-report-packet-outside-"));
  const packetPath = join(workspaceRoot, "candidate.zip");
  const outsidePacketPath = join(outsideRoot, "outside.zip");
  const recordingPath = join(workspaceRoot, "parser-demo.mov");
  const outsideRecordingPath = join(outsideRoot, "outside.mov");
  await writeFile(packetPath, Buffer.from([0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0]));
  await writeFile(outsidePacketPath, Buffer.from([0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0]));
  await writeFile(recordingPath, Buffer.from("recording"));
  await writeFile(outsideRecordingPath, Buffer.from("outside recording"));
  const layout = ensureResearchStorageLayout(createResearchStorageLayout({ workspaceRoot }));
  const store = new ReportStore(
    getDefaultMemoryDatabasePath(workspaceRoot),
    layout,
    { workspaceId: "workspace_packet", workspaceName: "Packet" },
    { packetCandidateRoots: [workspaceRoot] },
  );
  try {
    assert.throws(() => store.create({
      title: "Outside packet",
      summary: "Must stay bounded.",
      content: "# Outside packet",
      submissionPacketPath: outsidePacketPath,
    }), /inside the active workspace/);
    const created = store.create({
      title: "Packet report",
      summary: "Includes its proof packet.",
      content: "# Packet report",
      submissionPacketPath: packetPath,
    });
    assert.deepEqual(created.report.submissionPacket, {
      artifactId: `${created.report.id}_submission_packet`,
      filename: "submission.zip",
      sizeBytes: 8,
      contentHash: created.submissionPacketArtifactRef.contentHash,
    });
    assert.equal(created.submissionPacketArtifactRef.kind, "submission-packet");
    const packetArtifacts = listResearchStorageArtifacts(layout, { kind: "submission-packet" });
    assert.equal(packetArtifacts.length, 1);
    assert.match(packetArtifacts[0].relativePath, /report-packets\/workspace_packet\/report_.+\/submission\.zip/);
    assert.deepEqual(await readFile(packetArtifacts[0].path), await readFile(packetPath));

    const revised = store.revise({
      id: created.report.id,
      expectedRevision: 1,
      content: "# Packet report\n\nRevised without replacing the packet.",
    });
    assert.deepEqual(revised.report.submissionPacket, created.report.submissionPacket);
    assert.equal(revised.submissionPacketArtifactRef, undefined);

    assert.throws(() => store.revise({
      id: created.report.id,
      expectedRevision: 2,
      content: "# Packet report\n\nRevised without replacing the packet.",
      recordingPath: outsideRecordingPath,
    }), /inside the active workspace/);
    const withRecording = store.revise({
      id: created.report.id,
      expectedRevision: 2,
      content: "# Packet report\n\nRevised without replacing the packet.",
      recordingPath,
    });
    assert.deepEqual(withRecording.report.submissionPacket, created.report.submissionPacket);
    assert.deepEqual(withRecording.report.recording, {
      artifactId: `${created.report.id}_recording`,
      filename: "parser-demo.mov",
      sizeBytes: 9,
      contentHash: withRecording.recordingArtifactRef.contentHash,
    });
    assert.equal(withRecording.recordingArtifactRef.kind, "report-recording");
    const recordingArtifacts = listResearchStorageArtifacts(layout, { kind: "report-recording" });
    assert.equal(recordingArtifacts.length, 1);
    assert.match(recordingArtifacts[0].relativePath, /report-recordings\/workspace_packet\/report_.+\/recording\.mov/);
    assert.deepEqual(await readFile(recordingArtifacts[0].path), await readFile(recordingPath));
  } finally {
    store.close();
    await rm(workspaceRoot, { recursive: true, force: true });
    await rm(outsideRoot, { recursive: true, force: true });
  }
});

test("security report creation accepts a composite finding reviewed in the same session", async () => {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "app-server-security-report-tools-"));
  const layout = ensureResearchStorageLayout(createResearchStorageLayout({ workspaceRoot }));
  const memoryGraph = new MemoryGraphStore({ workspaceRoot, context: {
    sessionId: "run_security", workspaceId: "workspace_security", workspaceName: "Security",
    subjectId: "subject_security", subjectName: "Security Subject",
  } });
  const claims = new ResearchClaimStore(memoryGraph);
  const runbooks = new RunbookStore(memoryGraph.databasePath, layout, memoryGraph.getContext());
  const packetPath = join(workspaceRoot, "submission.zip");
  await writeFile(packetPath, Buffer.from([0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0]));
  const store = new ReportStore(memoryGraph.databasePath, layout, memoryGraph.getContext(), { packetCandidateRoots: [workspaceRoot] });
  const registry = createResearchToolRegistry(createReportTools(store, {
    requireConfirmedChain: true,
    requireSubmissionPacket: true,
    claimStore: claims,
  }));
  try {
    const componentLead = claims.create({
      title: "Attacker controls the redirect destination",
      classification: "security.primitive",
      evidence: [{ kind: "command", referenceId: "redirect-verifier", summary: "The verifier demonstrated destination control." }],
    });
    const component = claims.transition(componentLead.id, {
      expectedRevision: componentLead.revision,
      toStatus: "observed",
      reason: "Direct verifier output established destination control.",
    });
    const premature = await registry.execute({
      id: "premature_report",
      actionClass: "synthesize",
      toolName: "report.create",
      input: { title: "Premature", summary: "Isolated finding only.", content: "# Premature", sourceFindingId: component.id, submissionPacketPath: packetPath },
    });
    assert.equal(premature.result.status, "error");
    assert.match(premature.result.error.message, /security\.chain composite finding/);

    let chain = claims.create({
      title: "Unvalidated redirect reaches an authenticated callback",
      summary: "An attacker-controlled destination receives the victim authorization result.",
      impact: "The callback discloses the victim authorization result.",
      classification: "security.chain",
      componentClaimIds: [component.id],
      evidence: [{ kind: "command", referenceId: "redirect-chain-verifier", summary: "The complete callback path was observed." }],
    });
    chain = claims.transition(chain.id, {
      expectedRevision: chain.revision,
      toStatus: "observed",
      reason: "The complete callback path was directly observed.",
    });
    const runbook = runbooks.create({
      title: "Reproduce redirect callback chain",
      purpose: "Replay the complete redirect and callback path.",
      cells: [{ kind: "code", language: "sh", source: "./redirect-verifier" }],
    }).runbook;
    const runId = "redirect_chain_run";
    const startedAt = new Date().toISOString();
    runbooks.beginExecution(runbook.id, runId, runbooks.executionPlan(runbook.id).map((cell) => cell.id), "localhost");
    runbooks.completeExecution({
      id: runbook.id, runId, status: "succeeded", startedAt,
      completedAt: new Date().toISOString(), durationMs: 1, proofTarget: "localhost",
    });
    chain = claims.transition(chain.id, {
      expectedRevision: chain.revision,
      toStatus: "reproduced",
      reason: "The reusable proof completed on a clean target.",
      reproductionRunbookId: runbook.id,
      evidence: [{ kind: "runbook_execution", referenceId: runId, summary: "Clean reproduction succeeded." }],
    });
    chain = claims.transition(chain.id, {
      expectedRevision: chain.revision,
      toStatus: "verified",
      reason: "A distinct reviewer reproduced and challenged the complete chain in the same session.",
      evidence: [{
        kind: "independent_verification", referenceId: "independent-redirect-review",
        summary: "Independent replay held.", sessionId: "run_security", independent: true,
      }],
    });
    const missingPacket = await registry.execute({
      id: "confirmed_chain_without_packet",
      actionClass: "synthesize",
      toolName: "report.create",
      input: { title: "Missing packet", summary: "Reportable chain.", content: "# Missing packet", sourceFindingId: chain.id },
    });
    assert.equal(missingPacket.result.status, "blocked");
    assert.match(JSON.stringify(missingPacket.result), /submissionPacketPath/);
    const created = await registry.execute({
      id: "confirmed_chain_report",
      actionClass: "synthesize",
      toolName: "report.create",
      input: { title: "Verified chain", summary: "Reportable chain.", content: "# Verified chain", sourceFindingId: chain.id, submissionPacketPath: packetPath },
    });
    assert.equal(created.result.status, "complete");
    assert.equal(created.result.artifactRefs[0].kind, "report");
    assert.equal(created.result.artifactRefs[1].kind, "submission-packet");
    assert.equal(created.result.output.report.submissionPacket.filename, "submission.zip");
    assert.equal(created.result.output.sourceFinding.id, chain.id);
    assert.equal(created.result.output.sourceFinding.status, "report_ready");
    assert.equal(created.result.output.sourceFinding.transitions.at(-1).sessionId, "run_security");
  } finally {
    store.close();
    runbooks.close();
    claims.close();
    memoryGraph.close();
    await rm(workspaceRoot, { recursive: true, force: true });
  }
});
