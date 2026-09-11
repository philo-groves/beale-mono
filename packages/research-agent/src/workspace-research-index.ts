import { existsSync, lstatSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import type { DatabaseSync, SQLInputValue } from "node:sqlite";
import { openResearchDatabase } from "./database.js";
import { assertWorkspaceChild, readWorkspaceProject, workspaceContentHash, type WorkspaceResearchIndex } from "./workspace-project.js";
import type { WorkspacePublicationOptions } from "./workspace-publication.js";

type Row = Record<string, unknown>;

export interface WorkspaceResearchCacheState {
  schemaVersion: 1;
  workspaceId: string;
  publicationHash: string;
  state: "ready" | "releasing" | "released";
  updatedAt: string;
}

export interface WorkspaceResearchIndexMaintenanceResult {
  state: "ready" | "released";
  publicationHash: string;
  affectedRows: number;
}

const CACHE_STATE_PATH = ".beale/research-cache.json";
const PUBLICATION_INDEX_PATH = "references/research-index.json";
const json = (value: unknown): string => `${JSON.stringify(value, null, 2)}\n`;

function requireFileAuthority(options: WorkspacePublicationOptions): void {
  const project = readWorkspaceProject(options.workspaceRoot);
  if (!project || project.schemaVersion !== 2 || project.researchAuthority !== "files") {
    throw new Error("Research-index maintenance requires a schema-v2 file-authority workspace.");
  }
  if (project.workspaceId !== options.workspaceId) throw new Error("Research-index workspace identity mismatch.");
}

function publication(options: WorkspacePublicationOptions, verifyFiles = true): { index: WorkspaceResearchIndex; hash: string } {
  requireFileAuthority(options);
  const path = join(options.workspaceRoot, PUBLICATION_INDEX_PATH);
  if (!existsSync(path)) throw new Error("A complete canonical research publication is required before releasing its derived index.");
  const content = readFileSync(path, "utf8");
  const index = JSON.parse(content) as WorkspaceResearchIndex;
  if (index.schemaVersion !== 1 || index.workspaceId !== options.workspaceId || !index.files || Array.isArray(index.files)) {
    throw new Error("Canonical research publication identity or schema is invalid.");
  }
  if (verifyFiles) {
    for (const [relativePath, expectedHash] of Object.entries(index.files)) {
      const candidate = join(options.workspaceRoot, relativePath);
      assertWorkspaceChild(options.workspaceRoot, candidate);
      if (!/^[a-f0-9]{64}$/u.test(expectedHash) || !existsSync(candidate) || lstatSync(candidate).isSymbolicLink()
        || workspaceContentHash(readFileSync(candidate)) !== expectedHash) {
        throw new Error(`Canonical research publication is incomplete or changed: ${relativePath}`);
      }
    }
  }
  return { index, hash: workspaceContentHash(content) };
}

function cacheStatePath(root: string): string {
  return join(root, CACHE_STATE_PATH);
}

export function readWorkspaceResearchCacheState(root: string): WorkspaceResearchCacheState | null {
  const path = cacheStatePath(root);
  if (!existsSync(path)) return null;
  const value = JSON.parse(readFileSync(path, "utf8")) as Partial<WorkspaceResearchCacheState>;
  if (value.schemaVersion !== 1 || typeof value.workspaceId !== "string" || typeof value.publicationHash !== "string"
    || (value.state !== "ready" && value.state !== "releasing" && value.state !== "released") || typeof value.updatedAt !== "string") {
    throw new Error("Unsupported workspace research-cache state.");
  }
  return value as WorkspaceResearchCacheState;
}

function writeCacheState(root: string, value: WorkspaceResearchCacheState): void {
  const path = cacheStatePath(root);
  mkdirSync(dirname(path), { recursive: true });
  const temporary = `${path}.tmp`;
  writeFileSync(temporary, json(value), { mode: 0o600 });
  renameSync(temporary, path);
}

export function markWorkspaceResearchIndexReady(options: WorkspacePublicationOptions): WorkspaceResearchCacheState {
  // Publication already hashed and atomically wrote every managed file. Avoid a
  // second full workspace pass on every research mutation.
  const current = publication(options, false);
  const state: WorkspaceResearchCacheState = {
    schemaVersion: 1,
    workspaceId: options.workspaceId,
    publicationHash: current.hash,
    state: "ready",
    updatedAt: new Date().toISOString(),
  };
  writeCacheState(options.workspaceRoot, state);
  return state;
}

export function workspaceResearchIndexNeedsRebuild(root: string): boolean {
  const project = readWorkspaceProject(root);
  if (!project || project.schemaVersion !== 2 || project.researchAuthority !== "files") return false;
  let state: WorkspaceResearchCacheState["state"] | undefined;
  try { state = readWorkspaceResearchCacheState(root)?.state; }
  catch { return true; }
  // A missing runtime marker is not proof that the database is populated. This
  // covers a fresh checkout and loss of .beale metadata after a prior release.
  if (state === undefined) return existsSync(join(root, PUBLICATION_INDEX_PATH));
  return state === "releasing" || state === "released";
}

function tableExists(database: DatabaseSync, table: string): boolean {
  return Boolean(database.prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?").get(table));
}

function columns(database: DatabaseSync, table: string): string[] {
  return tableExists(database, table)
    ? (database.prepare(`PRAGMA table_info(${table})`).all() as Array<{ name: string }>).map((entry) => entry.name)
    : [];
}

function sqliteValue(value: unknown): SQLInputValue {
  if (value === null || typeof value === "string" || typeof value === "number" || typeof value === "bigint" || value instanceof Uint8Array) return value;
  throw new Error(`Canonical research row contains an unsupported SQLite value: ${typeof value}`);
}

function insertRows(database: DatabaseSync, table: string, source: readonly unknown[]): number {
  const allowed = new Set(columns(database, table));
  if (allowed.size === 0) return 0;
  let inserted = 0;
  for (const candidate of source) {
    if (!candidate || typeof candidate !== "object" || Array.isArray(candidate)) throw new Error(`Canonical ${table} row must be an object.`);
    const row = candidate as Row;
    const names = Object.keys(row).filter((name) => allowed.has(name));
    if (names.length === 0) continue;
    const statement = database.prepare(`INSERT OR IGNORE INTO ${table} (${names.join(",")}) VALUES (${names.map(() => "?").join(",")})`);
    inserted += Number(statement.run(...names.map((name) => sqliteValue(row[name]))).changes);
  }
  return inserted;
}

function assertWorkspaceIdentity(value: unknown, workspaceId: string, context: string): void {
  if (Array.isArray(value)) {
    for (const child of value) assertWorkspaceIdentity(child, workspaceId, context);
    return;
  }
  if (!value || typeof value !== "object") return;
  for (const [key, child] of Object.entries(value)) {
    if ((key === "workspace_id" || key === "workspaceId") && child !== workspaceId) {
      throw new Error(`Canonical ${context} contains a foreign workspace identity.`);
    }
    assertWorkspaceIdentity(child, workspaceId, context);
  }
}

function readJson(root: string, relativePath: string, workspaceId: string): Row {
  const value = JSON.parse(readFileSync(join(root, relativePath), "utf8")) as unknown;
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(`Canonical ${relativePath} must contain a JSON object.`);
  assertWorkspaceIdentity(value, workspaceId, relativePath);
  return value as Row;
}

function nested(row: Row, key: string): unknown[] {
  const value = row[key];
  if (value === undefined) return [];
  if (!Array.isArray(value)) throw new Error(`Canonical ${key} must be an array.`);
  return value;
}

function baseRow(row: Row, nestedKeys: readonly string[]): Row {
  return Object.fromEntries(Object.entries(row).filter(([key]) => key !== "schemaVersion" && !nestedKeys.includes(key)));
}

function canonicalPaths(index: WorkspaceResearchIndex, pattern: RegExp): string[] {
  return Object.keys(index.files).filter((path) => pattern.test(path)).sort();
}

function memoryDocument(root: string, relativePath: string, workspaceId: string): { row: Row; children: Row } {
  const content = readFileSync(join(root, relativePath), "utf8");
  const match = /```json\n([\s\S]*?)\n```\n\n([\s\S]*)$/u.exec(content);
  if (!match) throw new Error(`Canonical ${relativePath} has an invalid memory document.`);
  const metadata = JSON.parse(match[1]!) as Row;
  assertWorkspaceIdentity(metadata, workspaceId, relativePath);
  const body = match[2]!.replace(/\n$/u, "");
  const childKeys = ["workspaces", "tags", "assets", "evidence", "edges", "sessions", "validations", "authorship"];
  return { row: { ...baseRow(metadata, childKeys), body }, children: metadata };
}

function rebuild(database: DatabaseSync, options: WorkspacePublicationOptions, index: WorkspaceResearchIndex): number {
  const root = options.workspaceRoot;
  let affected = 0;
  const claimDocuments = canonicalPaths(index, /^claims\/[^/]+\.json$/u).map((path) => readJson(root, path, options.workspaceId));
  affected += insertRows(database, "app_server_research_claims", claimDocuments.map((row) => baseRow(row, ["evidence", "transitions", "authorship", "components"])));
  affected += insertRows(database, "app_server_claim_evidence", claimDocuments.flatMap((row) => nested(row, "evidence")));
  affected += insertRows(database, "app_server_claim_transitions", claimDocuments.flatMap((row) => nested(row, "transitions")));
  affected += insertRows(database, "app_server_claim_authorship", claimDocuments.flatMap((row) => nested(row, "authorship")));
  affected += insertRows(database, "app_server_claim_components", claimDocuments.flatMap((row) => nested(row, "components")));

  const memories = canonicalPaths(index, /^memories\/[^/]+\.md$/u).map((path) => memoryDocument(root, path, options.workspaceId));
  affected += insertRows(database, "memory_nodes", memories.map((memory) => memory.row));
  affected += insertRows(database, "memory_node_workspaces", memories.flatMap((memory) => nested(memory.children, "workspaces")));
  affected += insertRows(database, "memory_node_tags", memories.flatMap((memory) => nested(memory.children, "tags")));
  affected += insertRows(database, "memory_node_assets", memories.flatMap((memory) => nested(memory.children, "assets")));
  affected += insertRows(database, "memory_evidence_refs", memories.flatMap((memory) => nested(memory.children, "evidence")));
  affected += insertRows(database, "memory_node_sessions", memories.flatMap((memory) => nested(memory.children, "sessions")));
  affected += insertRows(database, "memory_edges", memories.flatMap((memory) => nested(memory.children, "edges")));
  affected += insertRows(database, "memory_node_catalog_validations", memories.flatMap((memory) => nested(memory.children, "validations")));
  affected += insertRows(database, "app_server_model_authorship", memories.flatMap((memory) => nested(memory.children, "authorship")));

  const artifactDocuments = [
    ...canonicalPaths(index, /^runbooks\/[^/]+\/record\.json$/u).map((path) => ({ kind: "runbook", table: "app_server_runbooks", row: readJson(root, path, options.workspaceId) })),
    ...canonicalPaths(index, /^reports\/[^/]+\/record\.json$/u).map((path) => ({ kind: "report", table: "app_server_reports", row: readJson(root, path, options.workspaceId) })),
  ];
  for (const document of artifactDocuments) affected += insertRows(database, document.table, [baseRow(document.row, ["revisions", "authorship"])]);
  affected += insertRows(database, "app_server_artifact_revisions", artifactDocuments.flatMap((document) => nested(document.row, "revisions")));
  affected += insertRows(database, "app_server_model_authorship", artifactDocuments.flatMap((document) => nested(document.row, "authorship")));

  const investigations = canonicalPaths(index, /^investigations\/[^/]+\/record\.json$/u).map((path) => readJson(root, path, options.workspaceId));
  const investigationChildren = ["sessions", "resources", "questions", "experiments", "observations", "nextActions", "memoryClaimReviews", "researchClaimReviews"];
  affected += insertRows(database, "campaign_tracks", investigations.map((row) => baseRow(row, investigationChildren)));
  affected += insertRows(database, "campaign_track_sessions", investigations.flatMap((row) => nested(row, "sessions")));
  affected += insertRows(database, "campaign_track_resources", investigations.flatMap((row) => nested(row, "resources")));
  affected += insertRows(database, "campaign_track_questions", investigations.flatMap((row) => nested(row, "questions")));
  affected += insertRows(database, "campaign_track_experiments", investigations.flatMap((row) => nested(row, "experiments")));
  const observations = investigations.flatMap((row) => nested(row, "observations")) as Row[];
  affected += insertRows(database, "campaign_track_observations", observations.map((row) => baseRow(row, ["evidence"])));
  affected += insertRows(database, "campaign_track_observation_evidence", observations.flatMap((row) => nested(row, "evidence")));
  affected += insertRows(database, "campaign_track_next_actions", investigations.flatMap((row) => nested(row, "nextActions")));
  affected += insertRows(database, "campaign_track_claim_reviews", investigations.flatMap((row) => nested(row, "memoryClaimReviews")));
  affected += insertRows(database, "campaign_track_research_claim_reviews", investigations.flatMap((row) => nested(row, "researchClaimReviews")));

  if (index.files["references/campaign-state.json"]) {
    const state = readJson(root, "references/campaign-state.json", options.workspaceId);
    affected += insertRows(database, "campaign_track_replay_runs", nested(state, "replayRuns"));
    affected += insertRows(database, "campaign_track_consolidations", nested(state, "consolidations"));
  }
  if (index.files["references/resources.json"]) {
    const resources = readJson(root, "references/resources.json", options.workspaceId);
    const rows = nested(resources, "resources") as Row[];
    affected += insertRows(database, "app_server_research_resources", rows.map((row) => baseRow(row, ["touches"])));
    affected += insertRows(database, "app_server_research_resource_touches", rows.flatMap((row) => nested(row, "touches")));
    affected += insertRows(database, "resource_prior_art", nested(resources, "priorArt"));
  }
  const executions = canonicalPaths(index, /^evidence\/execution-[^/]+\.json$/u).map((path) => readJson(root, path, options.workspaceId));
  affected += insertRows(database, "app_server_runbook_executions", executions.map((row) => baseRow(row, ["cells"])));
  affected += insertRows(database, "app_server_runbook_cell_executions", executions.flatMap((row) => nested(row, "cells")));
  return affected;
}

function deleteWhere(database: DatabaseSync, table: string, clause: string, ...values: SQLInputValue[]): number {
  if (!tableExists(database, table)) return 0;
  return Number(database.prepare(`DELETE FROM ${table} WHERE ${clause}`).run(...values).changes);
}

function prune(database: DatabaseSync, workspaceId: string): number {
  let affected = 0;
  const ids = (table: string, column = "id"): string[] => tableExists(database, table)
    ? (database.prepare(`SELECT ${column} AS id FROM ${table} WHERE workspace_id=?`).all(workspaceId) as Array<{ id: string }>).map((row) => row.id)
    : [];
  const claimIds = ids("app_server_research_claims");
  for (const id of claimIds) affected += deleteWhere(database, "app_server_claim_components", "claim_id=? OR component_claim_id=?", id, id);
  affected += deleteWhere(database, "app_server_research_claims", "workspace_id=?", workspaceId);

  const runbookIds = ids("app_server_runbooks");
  const reportIds = ids("app_server_reports");
  for (const [kind, resourceIds] of [["runbook", runbookIds], ["report", reportIds]] as const) {
    for (const id of resourceIds) affected += deleteWhere(database, "app_server_model_authorship", "resource_kind=? AND resource_id=?", kind, id);
  }
  affected += deleteWhere(database, "app_server_artifact_revisions", "workspace_id=?", workspaceId);
  affected += deleteWhere(database, "app_server_runbooks", "workspace_id=?", workspaceId);
  affected += deleteWhere(database, "app_server_reports", "workspace_id=?", workspaceId);
  affected += deleteWhere(database, "campaign_tracks", "workspace_id=?", workspaceId);
  affected += deleteWhere(database, "campaign_track_replay_runs", "workspace_id=?", workspaceId);
  affected += deleteWhere(database, "campaign_track_consolidations", "workspace_id=?", workspaceId);
  affected += deleteWhere(database, "app_server_research_resources", "workspace_id=?", workspaceId);
  affected += deleteWhere(database, "resource_prior_art", "workspace_id=?", workspaceId);

  if (tableExists(database, "memory_node_workspaces")) {
    const memoryIds = (database.prepare("SELECT node_id AS id FROM memory_node_workspaces WHERE workspace_id=?").all(workspaceId) as Array<{ id: string }>).map((row) => row.id);
    affected += deleteWhere(database, "memory_node_workspaces", "workspace_id=?", workspaceId);
    for (const id of memoryIds) {
      const stillOwned = database.prepare("SELECT 1 FROM memory_node_workspaces WHERE node_id=? LIMIT 1").get(id);
      if (!stillOwned) {
        affected += deleteWhere(database, "app_server_model_authorship", "resource_kind='memory' AND resource_id=?", id);
        affected += deleteWhere(database, "memory_nodes", "id=?", id);
      }
    }
  }
  affected += deleteWhere(database, "research_goal_suggestion_cache", "workspace_id=?", workspaceId);
  return affected;
}

export function rebuildWorkspaceResearchIndex(options: WorkspacePublicationOptions): WorkspaceResearchIndexMaintenanceResult {
  const current = publication(options);
  const database = openResearchDatabase(options.databasePath);
  try {
    database.exec("BEGIN IMMEDIATE");
    const affectedRows = rebuild(database, options, current.index);
    database.exec("COMMIT");
    writeCacheState(options.workspaceRoot, { schemaVersion: 1, workspaceId: options.workspaceId, publicationHash: current.hash, state: "ready", updatedAt: new Date().toISOString() });
    return { state: "ready", publicationHash: current.hash, affectedRows };
  } catch (error) {
    try { database.exec("ROLLBACK"); } catch { /* Transaction did not start. */ }
    throw error;
  } finally { database.close(); }
}

export function releaseWorkspaceResearchIndex(options: WorkspacePublicationOptions): WorkspaceResearchIndexMaintenanceResult {
  const current = publication(options);
  // Filesystem and SQLite state cannot share one transaction. Record intent
  // first so a crash after COMMIT is always recovered as a rebuild, never read
  // as an authoritative empty index.
  writeCacheState(options.workspaceRoot, { schemaVersion: 1, workspaceId: options.workspaceId, publicationHash: current.hash, state: "releasing", updatedAt: new Date().toISOString() });
  const database = openResearchDatabase(options.databasePath);
  try {
    database.exec("BEGIN IMMEDIATE");
    const affectedRows = prune(database, options.workspaceId);
    database.exec("COMMIT");
    writeCacheState(options.workspaceRoot, { schemaVersion: 1, workspaceId: options.workspaceId, publicationHash: current.hash, state: "released", updatedAt: new Date().toISOString() });
    return { state: "released", publicationHash: current.hash, affectedRows };
  } catch (error) {
    try { database.exec("ROLLBACK"); } catch { /* Transaction did not start. */ }
    throw error;
  } finally { database.close(); }
}
