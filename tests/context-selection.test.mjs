import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";

import {
  buildCampaignGraph,
  createFallbackResearchContextSelection,
  createResearchContextPreflightIndex,
  createResearchContextSelectionCatalog,
  discoverInstructionDirectoryHints,
  parseResearchContextSelection,
  projectSelectedModelWorkspaceContext,
} from "../packages/research-agent/dist/index.js";

test("context preflight accepts only canonical IDs and allowed inspection paths", () => {
  const fixture = contextFixture();
  try {
    const workspace = workspaceContext(fixture);
    const campaign = campaignContext();
    const catalog = createResearchContextSelectionCatalog({
      workspaceContext: workspace,
      campaign,
      memoryIds: ["memory_target"],
      inspectionRoots: [fixture.workspace, fixture.priorResearch],
    });
    const selection = parseResearchContextSelection([
      "<context_selection>",
      JSON.stringify({
        schemaVersion: 1,
        summary: "Target orientation",
        rationale: "The selected repository and memory match the request.",
        selectedResourceIds: ["resource_target", "fabricated_resource"],
        selectedRepositoryRoots: [fixture.targetRepository, "/unconfigured/repository"],
        selectedMemoryIds: ["memory_target", "fabricated_memory"],
        selectedClaimIds: ["fabricated_claim"],
        selectedRunbookIds: [],
        selectedReportIds: [],
        selectedTrackIds: ["track_target", "fabricated_track"],
        selectedProjectNoteIndexes: [0, 99],
        selectedPaths: [join(fixture.priorResearch, "notes"), "/etc/passwd", "relative/path"],
        keyFacts: [{ summary: "Prior work ruled out path A.", references: ["memory_target", "fabricated_evidence"] }],
        openQuestions: ["Does path B remain reachable?"],
        constraints: ["Use the recorded scope."],
      }),
      "</context_selection>",
    ].join("\n"), catalog);

    assert.deepEqual(selection.selectedResourceIds, ["resource_target"]);
    assert.deepEqual(selection.selectedRepositoryRoots, [fixture.targetRepository]);
    assert.deepEqual(selection.selectedMemoryIds, ["memory_target"]);
    assert.deepEqual(selection.selectedClaimIds, []);
    assert.deepEqual(selection.selectedTrackIds, ["track_target"]);
    assert.deepEqual(selection.selectedProjectNoteIndexes, [0]);
    assert.deepEqual(selection.selectedPaths, [join(fixture.priorResearch, "notes")]);
    assert.deepEqual(selection.keyFacts[0].references, ["memory_target"]);
  } finally {
    rmSync(fixture.root, { recursive: true, force: true });
  }
});

test("instruction directory discovery includes research roots without exposing credential paths", () => {
  const fixture = contextFixture();
  try {
    const hints = discoverInstructionDirectoryHints({
      schemaVersion: 1,
      content: [
        `Prior Research Directory: ${fixture.priorResearch}`,
        `Source Directory: ${fixture.targetRepository}`,
        `Credential Directory: ${fixture.secretDirectory}`,
      ].join("\n"),
      sources: [],
      truncated: false,
      projectDocMaxBytes: 32_768,
    }, fixture.workspace);

    assert.deepEqual(hints, [fixture.workspace, fixture.priorResearch, fixture.targetRepository]);
  } finally {
    rmSync(fixture.root, { recursive: true, force: true });
  }
});

test("workspace projection retains safety and exclusions while removing unrelated assets", () => {
  const fixture = contextFixture();
  try {
    const workspace = workspaceContext(fixture);
    const projected = projectSelectedModelWorkspaceContext(workspace, {
      schemaVersion: 1,
      summary: "Target context",
      rationale: "Only target assets are relevant.",
      selectedResourceIds: ["resource_target"],
      selectedRepositoryRoots: [fixture.targetRepository],
      selectedMemoryIds: [],
      selectedClaimIds: [],
      selectedRunbookIds: [],
      selectedReportIds: [],
      selectedTrackIds: [],
      selectedProjectNoteIndexes: [],
      selectedPaths: [],
      keyFacts: [],
      openQuestions: [],
      constraints: [],
    });

    assert.deepEqual(projected.knownRepositories.map((repository) => repository.rootPath), [fixture.targetRepository]);
    assert.doesNotMatch(JSON.stringify(projected), /unrelated-repository/);
    assert.ok(projected.projectNotes.includes("Excluded in authorized scope: production devices"));
    assert.ok(projected.projectNotes.includes("Run target code only in the operator VM."));
    assert.ok(!projected.projectNotes.includes("Included in Authorized scope: unrelated asset inventory"));
  } finally {
    rmSync(fixture.root, { recursive: true, force: true });
  }
});

test("preflight index and fallback stay compact and request-directed", () => {
  const fixture = contextFixture();
  try {
    const workspace = workspaceContext(fixture);
    const campaign = campaignContext();
    const instructions = {
      schemaVersion: 1,
      content: `Prior Research Directory: ${fixture.priorResearch}`,
      sources: [],
      truncated: false,
      projectDocMaxBytes: 32_768,
    };
    const index = createResearchContextPreflightIndex({ workspaceContext: workspace, campaign, agentInstructions: instructions });
    assert.equal(index.research.tracks[0].id, "track_target");
    assert.equal(Object.hasOwn(index.research.tracks[0], "objective"), false);
    assert.equal(Object.hasOwn(index.workspace.repositories[0], "contentRoots"), false);
    assert.equal(Object.hasOwn(index.workspace.repositories[0], "notes"), false);

    const fallback = createFallbackResearchContextSelection({
      prompt: "Continue target-kernel review",
      workspaceContext: workspace,
      campaign,
      memoryIds: ["memory_target", "memory_unrelated"],
      inspectionRoots: [fixture.workspace],
    });
    assert.deepEqual(fallback.selectedResourceIds, ["resource_target"]);
    assert.deepEqual(fallback.selectedRepositoryRoots, [fixture.targetRepository]);
    assert.deepEqual(fallback.selectedTrackIds, ["track_target"]);
    assert.deepEqual(fallback.selectedMemoryIds, ["memory_target", "memory_unrelated"]);
  } finally {
    rmSync(fixture.root, { recursive: true, force: true });
  }
});

function campaignContext() {
  const base = buildCampaignGraph({
    nodes: [],
    edges: [],
    findings: [],
    runbooks: [],
    reports: [],
  });
  const counts = {
    questions: 2,
    openQuestions: 1,
    experiments: 3,
    observations: 4,
    openNextActions: 1,
    memoryNodes: 1,
    evidenceRefs: 2,
    findings: 0,
    runbooks: 0,
    reports: 0,
  };
  return {
    ...base,
    activeTrackId: "track_target",
    tracks: [{
      id: "track_target",
      title: "Target kernel",
      objective: "A very long historical objective that must not enter the compact index.",
      status: "active",
      stage: "testing",
      source: "runtime",
      sessionIds: ["session_target"],
      updatedAt: "2026-08-28T12:00:00.000Z",
      revision: 3,
      questions: [],
      experiments: [],
      observations: [],
      counts,
    }],
  };
}

function workspaceContext(fixture) {
  return {
    schemaVersion: 1,
    workspaceRoot: fixture.workspace,
    authorization: { recorded: true, source: "beale", scopeName: "Target program" },
    memoryContext: {
      sessionId: "session_target",
      workspaceId: "workspace_target",
      workspaceName: "Target",
      subjectId: "subject_vendor",
      subjectName: "Vendor",
    },
    knownRepositories: [
      {
        rootPath: fixture.targetRepository,
        contentRoots: [join(fixture.targetRepository, "src")],
        label: "target-kernel",
        role: "known_repository",
        notes: ["Verbose repository metadata must not enter the preflight index."],
      },
      { rootPath: fixture.unrelatedRepository, label: "unrelated-repository", role: "known_repository" },
    ],
    materializedSourcePaths: [fixture.targetRepository, fixture.unrelatedRepository],
    resources: [
      { id: "resource_target", direction: "in_scope", kind: "repository", locator: fixture.targetRepository, name: "target-kernel", source: "explicit_scope" },
      { id: "resource_unrelated", direction: "in_scope", kind: "repository", locator: fixture.unrelatedRepository, name: "other-project", source: "explicit_scope" },
    ],
    projectNotes: [
      "Included in Authorized scope: unrelated asset inventory",
      "Excluded in authorized scope: production devices",
      "Run target code only in the operator VM.",
    ],
  };
}

function contextFixture() {
  const root = mkdtempSync(join(tmpdir(), "app-server-context-selection-"));
  const workspace = join(root, "workspace");
  const targetRepository = join(root, "target-kernel");
  const unrelatedRepository = join(root, "unrelated-repository");
  const priorResearch = join(root, "prior-research");
  const secretDirectory = join(root, ".ssh");
  for (const path of [workspace, targetRepository, unrelatedRepository, join(priorResearch, "notes"), secretDirectory]) {
    mkdirSync(path, { recursive: true });
  }
  return {
    root,
    workspace: resolve(workspace),
    targetRepository: resolve(targetRepository),
    unrelatedRepository: resolve(unrelatedRepository),
    priorResearch: resolve(priorResearch),
    secretDirectory: resolve(secretDirectory),
  };
}
