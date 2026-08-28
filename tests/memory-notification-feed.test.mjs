import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import test from "node:test";

import { MemoryGraphStore, ResearchClaimStore } from "../packages/research-agent/dist/index.js";
import { invokeHoneycrispProtocol } from "../app-server/dist/honeycrispProtocolClient.js";

test("memory notification feed includes only path-free heat-bearing nodes", async () => {
  const directory = await mkdtemp(join(tmpdir(), "honeycrisp-memory-notifications-"));
  const databasePath = join(directory, "memory.sqlite");
  const artifactDirectoryPath = join(directory, "artifacts");
  const context = {
    sessionId: "session-test",
    workspaceId: "workspace-test",
    workspaceName: "Test workspace",
    subjectId: "subject_workspace:workspace-test",
    subjectName: "Test subject",
  };
  const store = new MemoryGraphStore({ databasePath, workspaceRoot: directory, context });
  const claims = new ResearchClaimStore(store);
  try {
    const host = new DatabaseSync(databasePath);
    try {
      host.exec(`
        CREATE TABLE IF NOT EXISTS workspaces (
          id TEXT PRIMARY KEY,
          workspace_path TEXT NOT NULL UNIQUE,
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS workspace_research_subjects (
          workspace_id TEXT PRIMARY KEY,
          subject_id TEXT NOT NULL,
          display_name TEXT NOT NULL,
          source TEXT NOT NULL,
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL
        );
      `);
      host.prepare("INSERT INTO workspaces VALUES (?, ?, ?, ?)").run(
        context.workspaceId,
        directory,
        "2026-01-01T00:00:00.000Z",
        "2026-01-01T00:00:00.000Z",
      );
      host.prepare("INSERT INTO workspace_research_subjects VALUES (?, ?, ?, ?, ?, ?)").run(
        context.workspaceId,
        context.subjectId,
        context.subjectName,
        "explicit",
        "2026-01-01T00:00:00.000Z",
        "2026-01-01T00:00:00.000Z",
      );
    } finally {
      host.close();
    }
    store.save({ type: "asset", title: "Parser service", status: "confirmed" });
    const lead = claims.create({
      title: "Parser state confusion",
      summary: "Shared parser state crosses requests.",
      classification: "security.primitive",
      rating: "high",
      evidence: [{ kind: "code", referenceId: "src/parser.c:42", summary: "Shared state crosses the request boundary." }],
    });
    const finding = claims.transition(lead.id, {
      expectedRevision: lead.revision,
      toStatus: "observed",
      reason: "Directly observed in source.",
    });
    const feed = await invokeHoneycrispProtocol("memory.notification_feed", {
      args: [],
      input: {
        workspaceId: context.workspaceId,
        workspaceRoot: directory,
        researchProfileId: "security-research",
      },
      storage: { databasePath, artifactDirectoryPath },
    });
    assert.equal(feed.schemaVersion, 3);
    assert.deepEqual(feed.nodes, [{
      id: finding.id,
      kind: "claim",
      sessionIds: ["session-test"],
      type: "security.primitive",
      typeName: "Security Finding",
      title: "Parser state confusion",
      summary: "Shared parser state crosses requests.",
      status: "observed",
      heat: "medium",
      rating: "high",
      createdAt: finding.createdAt,
      updatedAt: finding.updatedAt,
      revision: finding.revision,
    }]);
    const serialized = JSON.stringify(feed);
    assert.equal(serialized.includes(databasePath), false);
    assert.equal(serialized.includes(artifactDirectoryPath), false);
    assert.equal(serialized.includes("storageRoot"), false);
  } finally {
    claims.close();
    store.close();
    await rm(directory, { recursive: true, force: true });
  }
});
