import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";
import test from "node:test";

import {
  createDeterministicAgentExecutor,
  createResearchAgentFlowCapture,
  discoverResearchAgentInstructions,
  runResearchAgent,
} from "../packages/research-agent/dist/index.js";

test("AGENTS discovery layers global and project guidance in precedence order", () => {
  const fixture = instructionFixture();
  try {
    const codexHome = join(fixture, "codex-home");
    const projectRoot = join(fixture, "project");
    const nested = join(projectRoot, "services");
    const workingDirectory = join(nested, "security");
    mkdirSync(codexHome, { recursive: true });
    mkdirSync(workingDirectory, { recursive: true });
    writeFileSync(join(codexHome, "AGENTS.override.md"), "  \n");
    writeFileSync(join(codexHome, "AGENTS.md"), "Global guidance");
    writeFileSync(join(codexHome, "config.toml"), [
      'project_doc_fallback_filenames = ["TEAM_GUIDE.md", "AGENTS.md", "../unsafe.md"]',
      'project_root_markers = [".repo"]',
      "project_doc_max_bytes = 32768",
      "",
      "[unrelated]",
      'project_root_markers = ["ignored"]',
    ].join("\n"));
    writeFileSync(join(projectRoot, ".repo"), "root marker");
    writeFileSync(join(projectRoot, "AGENTS.md"), "Root guidance");
    writeFileSync(join(nested, "AGENTS.md"), "Shadowed nested guidance");
    writeFileSync(join(nested, "AGENTS.override.md"), "Nested override");
    writeFileSync(join(workingDirectory, "TEAM_GUIDE.md"), "Workspace VM guidance");

    const instructions = discoverResearchAgentInstructions({ workingDirectory, codexHome });

    assert.equal(
      instructions.content,
      "Global guidance\n\nRoot guidance\n\nNested override\n\nWorkspace VM guidance",
    );
    assert.deepEqual(
      instructions.sources.map((source) => [source.scope, basename(source.path)]),
      [
        ["global", "AGENTS.md"],
        ["project", "AGENTS.md"],
        ["project", "AGENTS.override.md"],
        ["project", "TEAM_GUIDE.md"],
      ],
    );
    assert.equal(instructions.truncated, false);
    assert.equal(instructions.projectDocMaxBytes, 32768);
  } finally {
    rmSync(fixture, { recursive: true, force: true });
  }
});

test("AGENTS discovery checks only the working directory when no project marker exists", () => {
  const fixture = instructionFixture();
  try {
    const parent = join(fixture, "parent");
    const workingDirectory = join(parent, "workspace");
    mkdirSync(workingDirectory, { recursive: true });
    writeFileSync(join(parent, "AGENTS.md"), "Parent guidance must not leak in");
    writeFileSync(join(workingDirectory, "AGENTS.md"), "Workspace-only guidance");

    const instructions = discoverResearchAgentInstructions({
      workingDirectory,
      codexHome: null,
    });

    assert.equal(instructions.content, "Workspace-only guidance");
    assert.deepEqual(instructions.sources.map((source) => source.path), [
      join(workingDirectory, "AGENTS.md"),
    ]);
  } finally {
    rmSync(fixture, { recursive: true, force: true });
  }
});

test("AGENTS discovery keeps global guidance when the workspace directory is unavailable", () => {
  const fixture = instructionFixture();
  try {
    const codexHome = join(fixture, "codex-home");
    mkdirSync(codexHome, { recursive: true });
    writeFileSync(join(codexHome, "AGENTS.md"), "Global fallback guidance");

    const instructions = discoverResearchAgentInstructions({
      workingDirectory: join(fixture, "missing-workspace"),
      codexHome,
    });

    assert.equal(instructions.content, "Global fallback guidance");
    assert.deepEqual(instructions.sources.map((source) => source.scope), ["global"]);
  } finally {
    rmSync(fixture, { recursive: true, force: true });
  }
});

test("an existing empty project override shadows lower-priority filenames", () => {
  const fixture = instructionFixture();
  try {
    writeFileSync(join(fixture, "AGENTS.override.md"), "\n\t\n");
    writeFileSync(join(fixture, "AGENTS.md"), "This file is shadowed");

    const instructions = discoverResearchAgentInstructions({
      workingDirectory: fixture,
      codexHome: null,
    });

    assert.equal(instructions.content, "");
    assert.deepEqual(instructions.sources, []);
  } finally {
    rmSync(fixture, { recursive: true, force: true });
  }
});

test("the combined project byte budget truncates the final file on a UTF-8 boundary", () => {
  const fixture = instructionFixture();
  try {
    const nested = join(fixture, "nested");
    mkdirSync(join(fixture, ".git"), { recursive: true });
    mkdirSync(nested, { recursive: true });
    writeFileSync(join(fixture, "AGENTS.md"), "root");
    writeFileSync(join(nested, "AGENTS.md"), "ééé");

    const instructions = discoverResearchAgentInstructions({
      workingDirectory: nested,
      codexHome: null,
      projectDocMaxBytes: 7,
    });

    assert.equal(instructions.content, "root\n\né");
    assert.equal(instructions.truncated, true);
    assert.equal(instructions.sources[1]?.byteLength, 2);
    assert.equal(instructions.sources[1]?.truncated, true);
  } finally {
    rmSync(fixture, { recursive: true, force: true });
  }
});

test("research bootstrap automatically discovers workspace AGENTS guidance for model context", async () => {
  const fixture = instructionFixture();
  try {
    const marker = "Use the fixture VM for target execution.";
    writeFileSync(join(fixture, "AGENTS.md"), marker);

    const result = await runResearchAgent({
      prompt: "Inspect the authorized target.",
      workspaceRoot: fixture,
      workflowId: "longshot",
      campaignContext: {
        schemaVersion: 1,
        counts: { leads: 2, findings: 1, verifiedFindings: 0, disclosedFindings: 0, coverageGaps: 3, contradictions: 1 },
        momentum: { state: "building", reason: "A primitive is available for composition.", supportingNodeIds: ["finding:one"] },
        nextActions: [],
        contradictions: [],
        activeTrack: null,
        recentTracks: [],
        omitted: { nodes: 296, edges: 182, coverageGaps: 3, contradictions: 1, tracks: 4 },
      },
      executor: createDeterministicAgentExecutor(),
    });

    assert.match(result.agentRun.modelInput.agentInstructions?.content ?? "", /fixture VM/);
    assert.ok(result.agentInstructions.sources.some((source) => source.path === join(fixture, "AGENTS.md")));
    const contextEvent = result.events.find((event) => event.kind === "context.compiled");
    assert.ok(contextEvent);
    assert.doesNotMatch(JSON.stringify(contextEvent.payload.agentInstructions), /fixture VM/);
    assert.ok(contextEvent.payload.contextMetrics.estimatedTokens > 0);
    assert.ok(contextEvent.payload.contextMetrics.sections.agentInstructions > 0);
    assert.equal(
      contextEvent.payload.contextMetrics.counts.collaborationTools,
      result.collaborationTools.length,
    );
    const modelContext = JSON.stringify(result.agentRun.modelInput.contextSections);
    assert.doesNotMatch(modelContext, /Longshot|outputRequirements|promptInstructions/);
    assert.match(modelContext, /A primitive is available for composition/);
    const campaignSection = result.agentRun.modelInput.contextSections.find((section) => section.label === "campaign")?.content;
    assert.equal(Object.hasOwn(campaignSection, "nodes"), false);
    assert.equal(Object.hasOwn(campaignSection, "edges"), false);
    assert.equal(campaignSection.omitted.nodes, 296);
    assert.doesNotMatch(JSON.stringify(createResearchAgentFlowCapture(result)), /fixture VM/);
  } finally {
    rmSync(fixture, { recursive: true, force: true });
  }
});

test("research bootstrap appends selected initial context without restoring campaign context", async () => {
  const fixture = instructionFixture();
  try {
    const initialContext = {
      schemaVersion: 1,
      source: "model-preflight",
      summary: "Resume the parser lead.",
      rationale: "The selected evidence is directly tied to the request.",
      resources: [],
      repositoryRoots: [fixture],
      paths: [join(fixture, "parser.c")],
      research: {
        memoryIds: ["memory_parser"],
        claimIds: ["lead_parser"],
        runbookIds: [],
        reportIds: [],
        trackIds: ["track_parser"],
      },
      keyFacts: [{ summary: "Length reaches the parser boundary.", references: ["memory_parser"] }],
      openQuestions: ["Is the length attacker-controlled?"],
      constraints: ["Use recorded scope."],
      omitted: { resources: 4, repositories: 8, projectNotes: 20, tracks: 12 },
    };
    const modelWorkspaceContext = {
      schemaVersion: 1,
      authorization: null,
      memory: null,
      knownRepositories: [],
      materializedSourcePaths: [],
      projectNotes: ["Keep this invariant."],
    };
    const result = await runResearchAgent({
      prompt: "Continue parser review.",
      workspaceRoot: fixture,
      modelWorkspaceContext,
      initialContext,
      executor: createDeterministicAgentExecutor(),
    });

    assert.equal(result.initialContext, initialContext);
    assert.equal(result.modelWorkspaceContext, modelWorkspaceContext);
    assert.equal(result.agentRun.modelInput.contextSections.some((section) => section.label === "campaign"), false);
    assert.equal(
      result.agentRun.modelInput.contextSections.find((section) => section.label === "selected_context")?.content,
      initialContext,
    );
    const contextEvent = result.events.find((event) => event.kind === "context.compiled");
    assert.equal(contextEvent.payload.initialContext, initialContext);
    assert.ok(contextEvent.payload.contextMetrics.sections.initialContext > 0);
  } finally {
    rmSync(fixture, { recursive: true, force: true });
  }
});

function instructionFixture() {
  return mkdtempSync(join(tmpdir(), "app-server-agents-md-"));
}
