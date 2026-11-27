import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";
import { DatabaseSync } from "node:sqlite";
import { WorkspaceDatabase } from "../dist/workspaceDatabase.js";

const temporaryDirectories = [];

after(() => {
  for (const directory of temporaryDirectories) {
    rmSync(directory, { recursive: true, force: true });
  }
});

function createTempDir(prefix) {
  const directory = mkdtempSync(join(tmpdir(), prefix));
  temporaryDirectories.push(directory);
  return directory;
}

function createWorkspaceDb(databasePath, artifactRoot, workspacePath, workspaceId) {
  const database = new WorkspaceDatabase(databasePath, artifactRoot, {
    workspacePath,
    workspaceId,
  });
  database.initialize();
  return database;
}

function assertFtsAligned(rawDb) {
  const docCount = rawDb.prepare("SELECT COUNT(*) AS count FROM project_search_documents").get().count;
  const ftsCount = rawDb.prepare("SELECT COUNT(*) AS count FROM project_search_fts").get().count;
  assert.equal(ftsCount, docCount, "project_search_fts row count must equal project_search_documents row count");

  const mismatched = rawDb
    .prepare(
      `SELECT d.rowid AS doc_rowid, d.id AS doc_id, f.rowid AS fts_rowid, f.document_id AS fts_doc_id
       FROM project_search_documents d
       LEFT JOIN project_search_fts f ON f.rowid = d.rowid
       WHERE f.rowid IS NULL OR f.document_id <> d.id OR f.title <> d.title OR f.body <> d.body`
    )
    .all();
  assert.deepEqual(mismatched, [], "every project_search_documents row must align 1:1 by rowid with project_search_fts");
}

test("keys project_search_fts deletes and joins by rowid without full FTS table scans", () => {
  const directory = createTempDir("beale-fts-plan-");
  const databasePath = join(directory, ".beale", "memory.sqlite");
  const artifactsPath = join(directory, ".beale", "artifacts");

  const dbA = createWorkspaceDb(databasePath, artifactsPath, join(directory, "ws-a"), "workspace-example-a");
  const dbB = createWorkspaceDb(databasePath, artifactsPath, join(directory, "ws-b"), "workspace-example-b");

  const scopeA = dbA.saveScope({
    workspaceName: "Example Workspace A",
    scopeOwner: "ExampleCo",
    descriptionMarkdown: "Synthetic workspace A scope.",
    rulesMarkdown: "",
    expiresAt: null,
    assets: [],
  });
  const scopeB = dbB.saveScope({
    workspaceName: "Example Workspace B",
    scopeOwner: "ExampleCo",
    descriptionMarkdown: "Synthetic workspace B scope.",
    rulesMarkdown: "",
    expiresAt: null,
    assets: [],
  });

  const runA = dbA.createRun({
    scopeVersionId: scopeA.id,
    title: "Example run A",
    promptMarkdown: "Inspect example alpha service.",
    shellSafetyMode: "auto_review",
    mode: "discovery",
    model: "example-model",
    reasoningEffort: "medium",
    attemptStrategy: "iterative_research",
    sandboxProfile: "none",
    budget: {},
  });
  const runB = dbB.createRun({
    scopeVersionId: scopeB.id,
    title: "Example run B",
    promptMarkdown: "Inspect example beta service.",
    shellSafetyMode: "auto_review",
    mode: "discovery",
    model: "example-model",
    reasoningEffort: "medium",
    attemptStrategy: "iterative_research",
    sandboxProfile: "none",
    budget: {},
  });

  // Insert brand-new documents (trace, transcript, artifact) across both workspaces
  const traceA = dbA.appendTraceEvent({
    runId: runA.run.id,
    attemptId: runA.attempt.id,
    type: "research_event",
    source: "agent",
    summary: "Observed alpha_unique_token in example handler",
    payload: { note: "alpha_payload" },
  });
  dbA.createTranscriptMessage({
    runId: runA.run.id,
    attemptId: runA.attempt.id,
    role: "assistant",
    contentMarkdown: "Transcript mentions transcript_alpha_marker",
    source: "agent",
  });
  const artifactA = dbA.createArtifact({
    kind: "log",
    mimeType: "text/plain",
    sensitivity: "internal",
    modelVisible: true,
    source: "agent",
    content: "Artifact body contains artifact_alpha_marker",
  });
  // Link trace -> artifact so setArtifactProvenance indexes and later updates it
  const traceForArtifact = dbA.appendTraceEvent({
    runId: runA.run.id,
    attemptId: runA.attempt.id,
    type: "research_event",
    source: "agent",
    summary: "Recorded artifact provenance",
    artifactId: artifactA.id,
  });
  dbA.setArtifactProvenance(artifactA.id, traceForArtifact.id);

  dbB.appendTraceEvent({
    runId: runB.run.id,
    attemptId: runB.attempt.id,
    type: "research_event",
    source: "agent",
    summary: "Observed beta_isolated_token in workspace B",
    payload: { note: "beta_payload" },
  });

  // Update an existing search document (run update triggers upsert on existing row)
  dbA.updateRunStatus(runA.run.id, "completed", "Completed alpha_updated_summary");

  assert.ok(
    dbA.searchProjectDocumentsForRun(runA.run.id, "alpha_unique_token", 10, { refreshInventory: false }).some((r) => r.entityId === traceA.id),
    "trace document should be searchable"
  );
  assert.ok(
    dbA.searchProjectDocumentsForRun(runA.run.id, "artifact_alpha_marker", 10, { refreshInventory: false }).some((r) => r.entityId === artifactA.id),
    "artifact document should be searchable"
  );
  assert.ok(
    dbA.searchProjectDocumentsForRun(runA.run.id, "alpha_updated_summary", 10, { refreshInventory: false }).some((r) => r.entityId === runA.run.id),
    "updated run summary should be searchable"
  );

  // Marking artifact sensitive removes it from both project_search_documents and project_search_fts
  dbA.markArtifactSensitive(artifactA.id);
  assert.equal(
    dbA.searchProjectDocumentsForRun(runA.run.id, "artifact_alpha_marker", 10, { refreshInventory: false }).length,
    0,
    "sensitive artifact must be removed from FTS index"
  );

  // Rebuilding workspace A index must keep workspace B FTS entries intact
  dbA.rebuildProjectSearchIndex();
  assert.ok(
    dbB.searchProjectDocumentsForRun(runB.run.id, "beta_isolated_token", 10, { refreshInventory: false }).length > 0,
    "rebuilding workspace A must not delete workspace B FTS rows"
  );

  dbA.close();
  dbB.close();

  const raw = new DatabaseSync(databasePath, { readOnly: true });
  try {
    assertFtsAligned(raw);

    const pointDeletePlan = raw
      .prepare("EXPLAIN QUERY PLAN DELETE FROM project_search_fts WHERE rowid = ?")
      .all()
      .map((row) => String(row.detail));
    assert.ok(
      pointDeletePlan.some((line) => line.includes("VIRTUAL TABLE INDEX 0:=")),
      `Expected rowid point lookup in FTS delete plan, got: ${JSON.stringify(pointDeletePlan)}`
    );

    const bulkDeletePlan = raw
      .prepare(
        "EXPLAIN QUERY PLAN DELETE FROM project_search_fts WHERE rowid IN (SELECT rowid FROM project_search_documents WHERE scope_version_id = ? AND entity_type IN ('inventory_item', 'structure_entity'))"
      )
      .all()
      .map((row) => String(row.detail));
    assert.ok(
      bulkDeletePlan.some((line) => line.includes("VIRTUAL TABLE INDEX 0:=")),
      `Expected rowid point lookup in bulk FTS delete plan, got: ${JSON.stringify(bulkDeletePlan)}`
    );
    assert.ok(
      bulkDeletePlan.some((line) => line.includes("idx_project_search_scope_entity")),
      `Expected covering scope/entity index in bulk FTS delete plan, got: ${JSON.stringify(bulkDeletePlan)}`
    );

    const joinPlan = raw
      .prepare(
        "EXPLAIN QUERY PLAN SELECT d.*, bm25(project_search_fts) AS rank FROM project_search_fts JOIN project_search_documents d ON d.rowid = project_search_fts.rowid WHERE project_search_fts MATCH ?"
      )
      .all()
      .map((row) => String(row.detail));
    assert.ok(
      joinPlan.some((line) => line.includes("INTEGER PRIMARY KEY (rowid=?)")),
      `Expected INTEGER PRIMARY KEY rowid join, got: ${JSON.stringify(joinPlan)}`
    );
  } finally {
    raw.close();
  }
});

test("migration 27 realigns pre-existing mismatched project_search_fts rowids and drops orphans", () => {
  const directory = createTempDir("beale-fts-migrate-");
  const databasePath = join(directory, "memory.sqlite");
  const artifactsPath = join(directory, "artifacts");

  const dbA = createWorkspaceDb(databasePath, artifactsPath, join(directory, "ws-a"), "workspace-legacy-a");
  const dbB = createWorkspaceDb(databasePath, artifactsPath, join(directory, "ws-b"), "workspace-legacy-b");
  const scopeA = dbA.getActiveScope();
  const scopeB = dbB.getActiveScope();
  const runA = dbA.createRun({
    scopeVersionId: scopeA.id,
    title: "Legacy run A",
    promptMarkdown: "legacy_prompt_alpha",
    shellSafetyMode: "auto_review",
    mode: "discovery",
    model: "example-model",
    reasoningEffort: "medium",
    attemptStrategy: "iterative_research",
    sandboxProfile: "none",
    budget: {},
  });
  const runB = dbB.createRun({
    scopeVersionId: scopeB.id,
    title: "Legacy run B",
    promptMarkdown: "legacy_prompt_beta",
    shellSafetyMode: "auto_review",
    mode: "discovery",
    model: "example-model",
    reasoningEffort: "medium",
    attemptStrategy: "iterative_research",
    sandboxProfile: "none",
    budget: {},
  });
  dbA.appendTraceEvent({
    runId: runA.run.id,
    attemptId: runA.attempt.id,
    type: "research_event",
    source: "agent",
    summary: "legacy_trace_alpha_token",
  });
  dbB.appendTraceEvent({
    runId: runB.run.id,
    attemptId: runB.attempt.id,
    type: "research_event",
    source: "agent",
    summary: "legacy_trace_beta_token",
  });
  dbA.close();
  dbB.close();

  // Simulate a pre-v27 database where FTS rowids drifted from project_search_documents.rowid and an orphan FTS row exists
  const legacy = new DatabaseSync(databasePath);
  legacy.exec(`
    DELETE FROM project_search_fts;
    INSERT INTO project_search_fts (rowid, document_id, scope_version_id, run_id, entity_type, entity_id, title, body)
    SELECT rowid + 1000, id, scope_version_id, run_id, entity_type, entity_id, title, body
    FROM project_search_documents;
    INSERT INTO project_search_fts (rowid, document_id, scope_version_id, run_id, entity_type, entity_id, title, body)
    VALUES (99999, 'orphan_doc_id', 'orphan_scope', NULL, 'finding', 'orphan_entity', 'orphan_title_token', 'orphan_body_token');
    DELETE FROM schema_migrations WHERE component = 'beale_workbench' AND version >= 27;
  `);
  legacy.close();

  // Reopen and verify migration 27 realigns all rows across both workspaces
  const reopenedA = createWorkspaceDb(databasePath, artifactsPath, join(directory, "ws-a"), "workspace-legacy-a");
  const reopenedB = createWorkspaceDb(databasePath, artifactsPath, join(directory, "ws-b"), "workspace-legacy-b");

  assert.ok(
    reopenedA.searchProjectDocumentsForRun(runA.run.id, "legacy_trace_alpha_token", 10, { refreshInventory: false }).length > 0,
    "migrated workspace A document should be searchable"
  );
  assert.ok(
    reopenedB.searchProjectDocumentsForRun(runB.run.id, "legacy_trace_beta_token", 10, { refreshInventory: false }).length > 0,
    "migrated workspace B document should be searchable"
  );

  // Updating a migrated document should cleanly replace its old FTS text without leaving stale matches
  reopenedA.updateRunStatus(runA.run.id, "completed", "post_migration_summary_gamma");
  assert.ok(
    reopenedA.searchProjectDocumentsForRun(runA.run.id, "post_migration_summary_gamma", 10, { refreshInventory: false }).length > 0,
    "updated migrated document should be searchable"
  );

  reopenedA.close();
  reopenedB.close();

  const verified = new DatabaseSync(databasePath, { readOnly: true });
  try {
    const migration = verified
      .prepare("SELECT version, name FROM schema_migrations WHERE component = 'beale_workbench' AND version = 27")
      .get();
    assert.deepEqual({ ...migration }, { version: 27, name: "align_project_search_fts_rowids" });

    const orphan = verified.prepare("SELECT rowid FROM project_search_fts WHERE document_id = 'orphan_doc_id'").get();
    assert.equal(orphan, undefined, "orphan FTS rows must be cleared by migration 27");

    assertFtsAligned(verified);
  } finally {
    verified.close();
  }
});

test("resolves cross-file and same-file structure relations via scope lookup with exact tie-breaking", () => {
  const directory = createTempDir("beale-structure-relations-");
  const repoDir = join(directory, "repo");
  mkdirSync(join(repoDir, "src"), { recursive: true });

  // File A defines a function `sharedTarget` (rank 0) at line 1 and `crossOnly` at line 5,
  // and calls `crossHandler` (defined in File B) with different casing.
  const fileAPath = join(repoDir, "src", "alpha.ts");
  writeFileSync(
    fileAPath,
    [
      "export function sharedTarget(): number {",
      "  return 1;",
      "}",
      "",
      "export function entryAlpha(): number {",
      "  return CROSSHANDLER();",
      "}",
    ].join("\n")
  );

  // File B defines `crossHandler` twice (class rank 1 on line 1, function rank 0 on line 5 -> function must win)
  // AND defines a lower-priority `sharedTarget` class/type or function at a later line in the same file
  const fileBPath = join(repoDir, "src", "beta.ts");
  writeFileSync(
    fileBPath,
    [
      "export class crossHandler {",
      "  run(): void {}",
      "}",
      "",
      "export function crossHandler(): number {",
      "  return sharedTarget();",
      "}",
      "",
      "export function sharedTarget(): number {",
      "  return 2;",
      "}",
    ].join("\n")
  );

  // File C calls `sharedTarget` (cross-file -> must pick File A's line 1 over File B's line 9 because line_start ASC)
  const fileCPath = join(repoDir, "src", "gamma.ts");
  writeFileSync(
    fileCPath,
    [
      "export function callSharedFromGamma(): number {",
      "  return sharedTarget();",
      "}",
    ].join("\n")
  );

  const db = createWorkspaceDb(join(directory, "memory.sqlite"), join(directory, "artifacts"), directory, "workspace-rel");
  const scope = db.saveScope({
    workspaceName: "Example Relation Workspace",
    scopeOwner: "ExampleCo",
    descriptionMarkdown: "Synthetic relation resolution scope.",
    rulesMarkdown: "",
    expiresAt: null,
    assets: [{
      direction: "in_scope",
      kind: "other",
      value: repoDir,
      attributes: {},
      sensitivity: "internal",
    }],
  });

  const coverage = db.getProjectStructureCoverageRecords(scope.id, { refreshIndex: true });
  const entityById = new Map(coverage.entities.map((entity) => [entity.id, entity]));

  // 1. entryAlpha() -> CROSSHANDLER() resolves case-insensitively to function crossHandler (line 5), NOT class crossHandler (line 1)
  const entryAlpha = coverage.entities.find((e) => e.name === "entryAlpha" && e.entityKind === "function");
  assert.ok(entryAlpha, "entryAlpha function should be indexed");
  const alphaCallSite = coverage.entities.find(
    (e) => e.entityKind === "call_site" && e.parentId === entryAlpha.id && e.name.toLowerCase() === "crosshandler"
  );
  assert.ok(alphaCallSite, "call_site from entryAlpha to CROSSHANDLER should be indexed");
  const alphaToCross = coverage.relations.find(
    (r) => r.sourceEntityId === alphaCallSite.id && r.targetName.toLowerCase() === "crosshandler"
  );
  assert.ok(alphaToCross, "relation from entryAlpha call_site to CROSSHANDLER should exist");
  const crossTarget = alphaToCross.targetEntityId ? entityById.get(alphaToCross.targetEntityId) : null;
  assert.ok(crossTarget, "CROSSHANDLER relation should resolve to a target entity");
  assert.equal(crossTarget.entityKind, "function", "function must beat class in entity_kind priority");
  assert.equal(crossTarget.path, fileBPath);
  assert.equal(crossTarget.lineStart, 5);
  assert.equal(alphaToCross.metadata.targetResolution, "scope_name_match");
  assert.equal(alphaToCross.metadata.targetPath, fileBPath);
  assert.equal(alphaToCross.metadata.targetLineStart, 5);

  // 2. File C callSharedFromGamma() -> sharedTarget() resolves to File A's line 1 (line_start ASC beats File B's line 9)
  const callGamma = coverage.entities.find((e) => e.name === "callSharedFromGamma" && e.entityKind === "function");
  assert.ok(callGamma, "callSharedFromGamma function should be indexed");
  const gammaCallSite = coverage.entities.find(
    (e) => e.entityKind === "call_site" && e.parentId === callGamma.id && e.name === "sharedTarget"
  );
  assert.ok(gammaCallSite, "call_site from callSharedFromGamma to sharedTarget should be indexed");
  const gammaToShared = coverage.relations.find(
    (r) => r.sourceEntityId === gammaCallSite.id && r.targetName === "sharedTarget"
  );
  assert.ok(gammaToShared?.targetEntityId, "callSharedFromGamma -> sharedTarget should resolve");
  const gammaTarget = entityById.get(gammaToShared.targetEntityId);
  assert.equal(gammaTarget.path, fileAPath);
  assert.equal(gammaTarget.lineStart, 1);
  assert.equal(gammaToShared.metadata.targetResolution, "scope_name_match");

  // 3. Same-file preference in resolveProjectStructureRelationTargets:
  // Reset beta's call relation to target_entity_id = NULL so resolveProjectStructureRelationTargets resolves it,
  // and verify it picks File B's own sharedTarget (line 9) over File A's sharedTarget (line 1).
  const betaCallSite = coverage.entities.find(
    (e) => e.entityKind === "call_site" && e.parentId === crossTarget.id && e.name === "sharedTarget"
  );
  assert.ok(betaCallSite, "call_site from crossHandler to sharedTarget should be indexed");
  const betaToSharedBefore = coverage.relations.find(
    (r) => r.sourceEntityId === betaCallSite.id && r.targetName === "sharedTarget"
  );
  assert.ok(betaToSharedBefore, "crossHandler -> sharedTarget relation should exist");

  // Also test symbol targetKind tie-breaking where File B's class (rank 1) vs function (rank 0) compete
  const raw = new DatabaseSync(join(directory, "memory.sqlite"));
  raw.prepare("UPDATE project_structure_relations SET target_entity_id = NULL, metadata_json = '{}' WHERE id = ?").run(betaToSharedBefore.id);
  raw
    .prepare(
      `INSERT INTO project_structure_relations (
         id, scope_version_id, source_entity_id, relation_kind, target_kind, target_name, target_entity_id, metadata_json, indexed_at
       ) VALUES (?, ?, ?, 'references', 'symbol', 'CROSSHANDLER', NULL, '{}', ?)`
    )
    .run("rel_symbol_tiebreak", scope.id, entryAlpha.id, "2026-01-01T00:00:00.000Z");
  raw.close();

  db.resolveProjectStructureRelationTargets(scope.id);

  const afterCoverage = db.getProjectStructureCoverageRecords(scope.id, { refreshIndex: false });
  const afterEntityById = new Map(afterCoverage.entities.map((entity) => [entity.id, entity]));

  const betaToSharedAfter = afterCoverage.relations.find((r) => r.id === betaToSharedBefore.id);
  assert.ok(betaToSharedAfter?.targetEntityId, "resolveProjectStructureRelationTargets should resolve same-file relation");
  const betaTarget = afterEntityById.get(betaToSharedAfter.targetEntityId);
  assert.equal(betaTarget.path, fileBPath);
  assert.equal(betaTarget.lineStart, 9);
  assert.equal(betaToSharedAfter.metadata.targetResolution, "same_file_name_match");

  const symbolTiebreak = afterCoverage.relations.find((r) => r.id === "rel_symbol_tiebreak");
  assert.ok(symbolTiebreak?.targetEntityId, "symbol relation should resolve");
  const symbolTarget = afterEntityById.get(symbolTiebreak.targetEntityId);
  assert.equal(symbolTarget.entityKind, "function", "function (rank 0) must beat class (rank 1) even though class is on line 1");
  assert.equal(symbolTarget.path, fileBPath);
  assert.equal(symbolTarget.lineStart, 5);

  db.close();
});

test("single-pass inventory scan preserves hashes, previews, manifest metadata, source entities, and binary strings", () => {
  const directory = createTempDir("beale-single-read-scan-");
  const repoDir = join(directory, "repo");
  mkdirSync(join(repoDir, "src"), { recursive: true });

  const manifestContent = JSON.stringify({
    name: "example-service",
    version: "1.2.3",
    dependencies: { "example-dep": "^1.0.0" },
  }, null, 2);
  const manifestPath = join(repoDir, "package.json");
  writeFileSync(manifestPath, manifestContent);

  const readmeContent = "# Example Service\r\nContains readme_preview_marker for search.\r\n";
  const readmePath = join(repoDir, "README.md");
  writeFileSync(readmePath, readmeContent);

  const emptyPath = join(repoDir, "empty.txt");
  writeFileSync(emptyPath, "");

  const sourceContent = "export function exampleHandler(input: string): string {\n  return input.trim();\n}\n";
  const sourcePath = join(repoDir, "src", "handler.ts");
  writeFileSync(sourcePath, sourceContent);

  const binaryBuffer = Buffer.concat([
    Buffer.from([0x7f, 0x45, 0x4c, 0x46, 0x00, 0x00, 0x01, 0x02]),
    Buffer.from("https://api.example.test/v1/status\0example_binary_symbol\0/etc/example.cfg\0"),
  ]);
  const binaryPath = join(repoDir, "sample.bin");
  writeFileSync(binaryPath, binaryBuffer);

  // Oversized source (> 1 MiB hash limit, <= 2 MiB structure limit) should skip sha256 while indexing structure
  const largeSourcePath = join(repoDir, "src", "large.ts");
  const largeSourceContent = `export function largeFileHandler(): number {\n  return 42;\n}\n// ${"x".repeat(1024 * 1024 + 64)}\n`;
  writeFileSync(largeSourcePath, largeSourceContent);

  const databasePath = join(directory, "memory.sqlite");
  const db = createWorkspaceDb(databasePath, join(directory, "artifacts"), directory, "workspace-scan");
  const scope = db.saveScope({
    workspaceName: "Example Scan Workspace",
    scopeOwner: "ExampleCo",
    descriptionMarkdown: "Synthetic scan scope.",
    rulesMarkdown: "",
    expiresAt: null,
    assets: [{
      direction: "in_scope",
      kind: "other",
      value: repoDir,
      attributes: {},
      sensitivity: "internal",
    }],
  });
  const run = db.createRun({
    scopeVersionId: scope.id,
    title: "Scan run",
    promptMarkdown: "Check indexed inventory.",
    shellSafetyMode: "auto_review",
    mode: "discovery",
    model: "example-model",
    reasoningEffort: "medium",
    attemptStrategy: "iterative_research",
    sandboxProfile: "none",
    budget: {},
  });

  const coverage = db.getProjectStructureCoverageRecords(scope.id, { refreshIndex: true });

  // Source entities extracted for both normal and >1MiB source files
  assert.ok(coverage.entities.some((e) => e.path === sourcePath && e.name === "exampleHandler"));
  assert.ok(coverage.entities.some((e) => e.path === largeSourcePath && e.name === "largeFileHandler"));
  // Binary entities extracted from single-pass binary string preview
  assert.ok(coverage.entities.some((e) => e.path === binaryPath && e.name.includes("https://api.example.test/v1/status")));

  // Search finds README preview, manifest dependency, and binary string
  assert.ok(
    db.searchProjectDocumentsForRun(run.run.id, "readme_preview_marker", 10, { refreshInventory: false }).some((r) => r.sourcePath === readmePath)
  );
  assert.ok(
    db.searchProjectDocumentsForRun(run.run.id, "example-dep", 10, { refreshInventory: false }).some((r) => r.sourcePath === manifestPath)
  );
  assert.ok(
    db.searchProjectDocumentsForRun(run.run.id, "example_binary_symbol", 10, { refreshInventory: false }).some((r) => r.sourcePath === binaryPath)
  );

  // Trigger a refresh after touching README.md to verify bulk deleteProjectSearchDocuments + re-index leaves FTS aligned
  writeFileSync(readmePath, "# Example Service\r\nUpdated readme_refreshed_marker for search.\r\n");
  db.getProjectStructureCoverageRecords(scope.id, { refreshIndex: true });
  assert.equal(
    db.searchProjectDocumentsForRun(run.run.id, "readme_preview_marker", 10, { refreshInventory: false }).length,
    0,
    "stale inventory FTS content must be deleted on refresh"
  );
  assert.ok(
    db.searchProjectDocumentsForRun(run.run.id, "readme_refreshed_marker", 10, { refreshInventory: false }).some((r) => r.sourcePath === readmePath)
  );

  db.close();

  const raw = new DatabaseSync(databasePath, { readOnly: true });
  try {
    assertFtsAligned(raw);

    const byPath = new Map(
      raw
        .prepare("SELECT path, resource_kind, sha256, metadata_json FROM project_inventory_items WHERE scope_version_id = ?")
        .all(scope.id)
        .map((row) => [row.path, row])
    );

    assert.equal(byPath.get(manifestPath).sha256, createHash("sha256").update(manifestContent).digest("hex"));
    const manifestMeta = JSON.parse(byPath.get(manifestPath).metadata_json);
    assert.equal(manifestMeta.previewIndexed, true);
    assert.deepEqual(manifestMeta.manifest.dependencies.dependencies, ["example-dep"]);

    assert.equal(byPath.get(emptyPath).sha256, createHash("sha256").update("").digest("hex"));
    assert.equal(byPath.get(sourcePath).sha256, createHash("sha256").update(sourceContent).digest("hex"));
    assert.equal(byPath.get(binaryPath).sha256, createHash("sha256").update(binaryBuffer).digest("hex"));
    assert.equal(JSON.parse(byPath.get(binaryPath).metadata_json).binaryStringsIndexed, true);

    assert.equal(byPath.get(largeSourcePath).sha256, null, "files above 1 MiB hash cap must not be hashed");
    assert.equal(JSON.parse(byPath.get(largeSourcePath).metadata_json).hashIndexed, false);
  } finally {
    raw.close();
  }
});
