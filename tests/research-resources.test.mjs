import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { DatabaseSync } from "node:sqlite";
import test from "node:test";

import {
  createResearchResourceScopeAuthorizer,
  createResearchResourceTool,
  createResearchToolRegistry,
  ResearchResourceCatalog,
} from "../packages/research-agent/dist/index.js";

test("resource discovery is non-authoring and first touch requires relevance Auto-Review", async () => {
  const root = await mkdtemp(join(tmpdir(), "honeycrisp-resources-"));
  const databasePath = join(root, "memory.sqlite");
  const reviews = [];
  const catalog = new ResearchResourceCatalog({
    databasePath,
    workspaceId: "workspace_asb",
    explicitResources: [{
      id: "asset_macos",
      direction: "in_scope",
      kind: "service",
      locator: "com.apple.WindowServer",
      name: "WindowServer",
      source: "explicit_scope",
    }, {
      id: "asset_excluded",
      direction: "out_of_scope",
      kind: "service",
      locator: "excluded.example.test",
      source: "explicit_scope",
    }],
  });
  try {
    const tool = createResearchResourceTool({
      catalog,
      campaignObjective: "Research macOS default components for an authorized Apple Security Bounty campaign.",
      authorizationRecorded: true,
      environmentFingerprint: "macos-26A1",
      authorizeScopeRelevance: async (request) => {
        reviews.push(request);
        return {
          decision: request.resource.name === "unrelated-helper" ? "not_relevant" : "relevant",
          source: "auto_review",
          reason: "The default platform binary is directly relevant to the recorded campaign.",
        };
      },
    });
    const registry = createResearchToolRegistry([tool]);
    const discovered = await registry.execute({
      id: "discover_launchctl",
      actionClass: "inspect",
      toolName: "resource.catalog",
      input: {
        operation: "discover",
        kind: "binary",
        name: "launchctl",
        locator: "/bin/launchctl",
        rationale: "Default macOS binary that reaches launchd service-management interfaces.",
      },
    });
    assert.equal(discovered.result.status, "complete");
    assert.equal(discovered.result.output.discoveryIsNonAuthoring, true);
    assert.equal(discovered.result.output.authorizationChanged, false);
    assert.equal(discovered.result.output.firstTouchTriggered, false);
    assert.equal(reviews.length, 0);

    const resourceId = discovered.result.output.resource.id;
    const firstTouch = await registry.execute({
      id: "touch_launchctl",
      actionClass: "inspect",
      toolName: "resource.catalog",
      input: {
        operation: "touch",
        resourceId,
        purpose: "Map reachable service-management sinks and historical fixes.",
      },
    });
    assert.equal(firstTouch.result.status, "complete");
    assert.equal(firstTouch.result.output.firstTouch, true);
    assert.equal(firstTouch.result.output.authorizationChanged, false);
    assert.match(firstTouch.result.output.reminder.join(" "), /release notes/i);
    assert.match(firstTouch.result.output.reminder.join(" "), /Apple Open Source/i);
    assert.match(firstTouch.result.output.reminder.join(" "), /upstream/i);
    assert.equal(reviews.length, 1);

    const repeated = await registry.execute({
      id: "touch_launchctl_again",
      actionClass: "inspect",
      toolName: "resource.catalog",
      input: { operation: "touch", resourceId, purpose: "Continue the same bounded inspection." },
    });
    assert.equal(repeated.result.output.firstTouch, false);
    assert.equal(reviews.length, 1);

    const excluded = catalog.list().find((resource) => resource.direction === "out_of_scope");
    const excludedTouch = await registry.execute({
      id: "touch_excluded",
      actionClass: "inspect",
      toolName: "resource.catalog",
      input: { operation: "touch", resourceId: excluded.id, purpose: "Inspect it." },
    });
    assert.equal(excludedTouch.result.status, "blocked");
    assert.match(excludedTouch.result.summary, /explicitly out of scope/i);
    assert.equal(reviews.length, 1);

    const database = new DatabaseSync(databasePath, { readOnly: true });
    try {
      const authorshipTables = database.prepare(`
        SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE '%authorship%'
      `).all();
      assert.deepEqual(authorshipTables, []);
      const touches = database.prepare("SELECT COUNT(*) AS count FROM honeycrisp_research_resource_touches").get();
      assert.equal(touches.count, 1);
    } finally {
      database.close();
    }
  } finally {
    catalog.close();
    await rm(root, { recursive: true, force: true });
  }
});

test("resource scope authorizer recognizes ambient platform dependencies without granting authorization", async () => {
  let captured;
  const authorize = createResearchResourceScopeAuthorizer({
    researchProfileName: "Security",
    workspaceRoot: "/tmp/workspace",
    getReviewerSelection: () => ({
      provider: "anthropic",
      model: "fixture-reviewer",
      reasoningEffort: "medium",
    }),
    completeClaudeText: async (input) => {
      captured = input;
      return {
        text: JSON.stringify({ decision: "relevant", reason: "Firecracker is the isolation boundary used by the sandbox." }),
        usage: { input: 10, output: 5 },
      };
    },
  });
  const decision = await authorize({
    resource: {
      id: "resource_firecracker",
      workspaceId: "workspace_vercel",
      kind: "service",
      name: "Firecracker microVM",
      locator: "firecracker",
      source: "runtime_discovery",
      direction: null,
      scopeAssetId: null,
      rationale: "Execution isolation boundary used by Vercel Sandbox.",
      reviewStatus: "unreviewed",
      reviewReason: null,
      discoveredAt: "2026-08-24T00:00:00Z",
      updatedAt: "2026-08-24T00:00:00Z",
    },
    purpose: "Inspect the sandbox escape boundary.",
    campaignObjective: "Research Vercel Sandbox.",
    authorizationRecorded: true,
  });
  assert.equal(decision.decision, "relevant");
  assert.equal(decision.source, "auto_review");
  assert.match(captured.systemPrompt, /Firecracker/);
  assert.match(captured.systemPrompt, /does not.*grant authorization/i);
});

test("resource catalog replaces stale explicit scope classification on a new active scope", async () => {
  const root = await mkdtemp(join(tmpdir(), "honeycrisp-resource-scope-sync-"));
  const databasePath = join(root, "memory.sqlite");
  const original = new ResearchResourceCatalog({
    databasePath,
    workspaceId: "workspace_scope_sync",
    explicitResources: [{
      id: "asset_old",
      direction: "in_scope",
      kind: "binary",
      locator: "C:/Windows/System32/old.exe",
      source: "explicit_scope",
    }],
  });
  original.close();
  const refreshed = new ResearchResourceCatalog({
    databasePath,
    workspaceId: "workspace_scope_sync",
    explicitResources: [{
      id: "asset_current",
      direction: "in_scope",
      kind: "binary",
      locator: "C:/Windows/System32/current.exe",
      source: "explicit_scope",
    }],
  });
  try {
    const old = refreshed.list().find((resource) => resource.locator.endsWith("old.exe"));
    const current = refreshed.list().find((resource) => resource.locator.endsWith("current.exe"));
    assert.equal(old.source, "runtime_discovery");
    assert.equal(old.direction, null);
    assert.equal(old.reviewStatus, "unreviewed");
    assert.equal(current.source, "explicit_scope");
    assert.equal(current.direction, "in_scope");
  } finally {
    refreshed.close();
    await rm(root, { recursive: true, force: true });
  }
});
