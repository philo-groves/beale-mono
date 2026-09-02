import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { basename, resolve } from "node:path";
import type { DatabaseSync } from "node:sqlite";
import { openResearchDatabase } from "./database.js";
import {
  normalizeResearchProfile,
  researchProfileHash,
  legacyResearchProfileHash,
  resolveResearchProfile,
  type ResearchProfile,
  type ResolvedResearchProfile,
} from "./research-profile.js";
import { getDefaultMemoryDatabasePath } from "./storage.js";
import type {
  ResearchMemoryContext,
  ResearchWorkspaceAuthorizationContext,
  ResearchWorkspaceResourceContext,
} from "./types.js";

export interface ResolveStoredResearchWorkspaceBindingOptions {
  workspaceRoot?: string;
  databasePath?: string;
  externalSessionId?: string;
  workflowId?: string;
  knownRepositoryRoots?: readonly string[];
  researchProfileId?: string;
  researchProfileHash?: string;
}

export interface StoredResearchWorkspaceBinding {
  schemaVersion: 1;
  source: "beale" | "deterministic";
  memoryContext: ResearchMemoryContext;
  authorization?: ResearchWorkspaceAuthorizationContext;
  resources: readonly ResearchWorkspaceResourceContext[];
  authorizedAssetIds: readonly string[];
  projectNotes: readonly string[];
}

export interface ResolveStoredResearchProfileOptions {
  workspaceRoot?: string;
  databasePath?: string;
  researchProfileId?: string;
  researchProfileHash?: string;
}

export type StoredResolvedResearchProfile = Omit<
  ResolvedResearchProfile,
  "path"
>;

/**
 * Resolve the durable identity and recorded authorization associated with a
 * workspace without returning the database path or any raw host metadata.
 */
export function resolveStoredResearchWorkspaceBinding(
  options: ResolveStoredResearchWorkspaceBindingOptions = {},
): StoredResearchWorkspaceBinding {
  const workspaceRoot = resolve(options.workspaceRoot ?? process.cwd());
  const sessionId = normalizeExternalSessionId(options.externalSessionId);
  const fallback = deterministicBinding(workspaceRoot, sessionId);
  const databasePath =
    options.databasePath ?? getDefaultMemoryDatabasePath(workspaceRoot);
  if (databasePath === ":memory:" || !existsSync(databasePath)) {
    return fallback;
  }

  let database: DatabaseSync | undefined;
  try {
    database = openResearchDatabase(databasePath, { readOnly: true });
    const workspace = readStoredWorkspace(database, workspaceRoot);
    if (!workspace) return fallback;
    const scope = readActiveScope(database, workspace.id);
    const workspaceName = nonEmptyText(scope?.workspace_name)
      ?? (basename(workspaceRoot) || "Workspace");
    const subject = readStoredSubject(database, workspace.id);
    const scopeOwner = nonEmptyText(scope?.scope_owner);
    const subjectName = subject?.name ?? scopeOwner ?? workspaceName;
    const subjectId = subject?.id
      ?? (scopeOwner
        ? stableSubjectId(scopeOwner)
        : fallbackSubjectId(workspace.id));
    const memoryContext: ResearchMemoryContext = {
      ...(sessionId ? { sessionId } : {}),
      workspaceId: workspace.id,
      workspaceName,
      subjectId,
      subjectName,
    };
    const authorization = scope
      ? projectStoredAuthorization(database, scope)
      : undefined;
    const resources = scope
      ? projectStoredResources(database, scope)
      : [];
    const profileSnapshot = readResearchProfileSnapshot(database, workspace.id, options);
    const profile = profileSnapshot
      ? validateStoredResearchProfileSnapshot(profileSnapshot).profile
      : undefined;
    const projectNotes = scope
      ? projectStoredProjectNotes(database, scope, subjectName, profile, options)
      : [];

    return {
      schemaVersion: 1,
      source: "beale",
      memoryContext,
      ...(authorization ? { authorization } : {}),
      resources,
      authorizedAssetIds: resources
        .filter((resource) => resource.direction === "in_scope")
        .map((resource) => resource.id),
      projectNotes,
    };
  } catch {
    throw new Error("Stored workspace binding could not be resolved.");
  } finally {
    database?.close();
  }
}

/**
 * Resolve Beale's active immutable profile snapshot for a workspace, falling
 * back to app-server's workspace profile resolution when no snapshot exists.
 * Host storage paths, including a snapshot's source_path, are never returned.
 */
export async function resolveStoredResearchProfile(
  options: ResolveStoredResearchProfileOptions = {},
): Promise<StoredResolvedResearchProfile> {
  const workspaceRoot = resolve(options.workspaceRoot ?? process.cwd());
  const databasePath =
    options.databasePath ?? getDefaultMemoryDatabasePath(workspaceRoot);
  let snapshot: Record<string, unknown> | undefined;

  if (databasePath !== ":memory:" && existsSync(databasePath)) {
    let database: DatabaseSync | undefined;
    try {
      database = openResearchDatabase(databasePath, { readOnly: true });
      const workspace = readStoredWorkspace(database, workspaceRoot);
      snapshot = workspace
        ? readResearchProfileSnapshot(database, workspace.id, options)
        : undefined;
    } catch {
      throw new Error("Stored research profile could not be resolved.");
    } finally {
      database?.close();
    }
  }

  if (snapshot) {
    try {
      return validateStoredResearchProfileSnapshot(snapshot);
    } catch {
      throw new Error("Stored research profile snapshot failed validation.");
    }
  }

  try {
    return withoutResearchProfilePath(
      await resolveResearchProfile({ workspaceRoot }),
    );
  } catch {
    throw new Error("Workspace research profile could not be resolved.");
  }
}

function deterministicBinding(
  workspaceRoot: string,
  sessionId: string | undefined,
): StoredResearchWorkspaceBinding {
  const workspaceId = `workspace_${createHash("sha256")
    .update(workspaceRoot)
    .digest("hex")
    .slice(0, 20)}`;
  const workspaceName = basename(workspaceRoot) || "Workspace";
  return {
    schemaVersion: 1,
    source: "deterministic",
    memoryContext: {
      ...(sessionId ? { sessionId } : {}),
      workspaceId,
      workspaceName,
      subjectId: fallbackSubjectId(workspaceId),
      subjectName: workspaceName,
    },
    projectNotes: [],
    resources: [],
    authorizedAssetIds: [],
  };
}

interface StoredScopeAsset {
  id: string;
  direction: "in_scope" | "out_of_scope";
  kind: string;
  value: string;
  sensitivity: string;
  attributes: Record<string, unknown>;
}

function projectStoredResources(
  database: DatabaseSync,
  scope: Record<string, unknown>,
): ResearchWorkspaceResourceContext[] {
  return readStoredScopeAssets(database, nonEmptyText(scope.id)).flatMap((asset) => {
    const legacyKind = safeProjectedText(asset.attributes.legacyKind);
    if (asset.kind === "credential_ref" || legacyKind === "account" || legacyKind === "credential_ref") return [];
    const kind = asset.kind === "repo" || asset.kind === "path"
      ? "repository"
      : asset.kind === "domain" || asset.kind === "binary" || asset.kind === "service" || asset.kind === "documentation"
        ? asset.kind
        : "other";
    const locator = safeProjectedText(asset.attributes.clonedDirectory) ?? asset.value;
    return [{
      id: asset.id,
      direction: asset.direction,
      kind,
      locator: boundedText(locator, 1_000),
      ...(safeProjectedText(asset.attributes.displayName) ? { name: boundedText(safeProjectedText(asset.attributes.displayName)!, 300) } : {}),
      ...(asset.sensitivity ? { sensitivity: boundedText(asset.sensitivity, 100) } : {}),
      ...(safeProjectedText(asset.attributes.instruction) ? { instruction: boundedText(safeProjectedText(asset.attributes.instruction)!, 1_000) } : {}),
      source: "explicit_scope" as const,
    }];
  });
}

function projectStoredProjectNotes(
  database: DatabaseSync,
  scope: Record<string, unknown>,
  subjectName: string,
  profile: ResearchProfile | undefined,
  options: ResolveStoredResearchWorkspaceBindingOptions,
): string[] {
  const workspaceName = safeProjectedText(scope.workspace_name);
  const scopeOwner = safeProjectedText(scope.scope_owner);
  const expiresAt = safeProjectedText(scope.expires_at);
  const assets = readStoredScopeAssets(database, nonEmptyText(scope.id));
  const workspaceRules = readStoredWorkspaceRules(database, nonEmptyText(scope.workspace_id));
  const recordedBoundary = assets.some((asset) => asset.direction === "in_scope");
  const workspaceContract = profile?.workspace;
  const boundaryNoun = boundedText(workspaceContract?.boundaryNoun ?? "scope");
  const authorizationRequired = workspaceContract?.authorizationMode === "required_for_live_network";
  const notes = profile
    ? [
        `Research profile: ${boundedText(`${profile.id}@${profile.version}`)}`,
        authorizationRequired
          ? recordedBoundary
            ? `Authorization: An operator-recorded ${boundaryNoun} applies. Follow the recorded inclusions, exclusions, and constraints.`
            : `Authorization: No operator-recorded ${boundaryNoun} is currently available.`
          : recordedBoundary
            ? `${boundaryNoun}: Use the operator-recorded inclusions, exclusions, and constraints.`
            : `${boundaryNoun}: No explicit boundary is currently recorded.`,
        workspaceName ? `${boundedText(profile.workspace.workspaceNoun)}: ${boundedText(workspaceName)}` : "",
        subjectName ? `${boundedText(profile.workspace.subjectNoun)}: ${boundedText(subjectName)}` : "",
        authorizationRequired && scopeOwner
          ? `${boundaryNoun} owner: ${boundedText(scopeOwner)}`
          : "",
        ...profile.workspace.boundaryInstructions
          .slice(0, 16)
          .map((instruction) => `${boundaryNoun} instruction: ${boundedText(instruction, 1_000)}`),
        ...workspaceRules.map((rule) => `Workspace rule: ${boundedText(rule, 2_000)}`),
        expiresAt
          ? `${boundaryNoun} expiry or review date: ${expiresAt}`
          : `${boundaryNoun} expiry or review date: no expiry recorded.`,
      ]
    : [
        "Authorization: This is an operator-recorded authorized security research scope. Treat only explicitly in-scope assets as authorized; exclusions and constraints override research objectives.",
        workspaceName ? `Scope: ${boundedText(workspaceName)}` : "",
        scopeOwner ? `Scope owner or subject: ${boundedText(scopeOwner)}` : "",
        ...workspaceRules.map((rule) => `Workspace rule: ${boundedText(rule, 2_000)}`),
        expiresAt
          ? `Authorization expiry or review date: ${expiresAt}`
          : "Authorization expiry or review date: no expiry recorded.",
      ];

  const knownRoots = new Set((options.knownRepositoryRoots ?? [])
    .map((root) => resolve(root).toLocaleLowerCase()));
  for (const asset of assets.slice(0, 200)) {
    const instruction = safeProjectedText(asset.attributes.instruction);
    const clonedDirectory = safeProjectedText(asset.attributes.clonedDirectory);
    const legacyKind = safeProjectedText(asset.attributes.legacyKind);
    const credentialReference = asset.kind === "credential_ref"
      || legacyKind === "account"
      || legacyKind === "credential_ref";
    if (
      asset.direction === "in_scope"
      && (asset.kind === "repo" || legacyKind === "path")
      && !instruction
      && [clonedDirectory, asset.value].some((value) => {
        if (!value) return false;
        try { return knownRoots.has(resolve(value).toLocaleLowerCase()); } catch { return false; }
      })
    ) {
      continue;
    }
    const value = credentialReference
      ? "[host-held credential reference; value withheld from agent context]"
      : boundedText(asset.value, 1_000);
    notes.push(
      `${asset.direction === "in_scope" ? `Included in ${boundaryNoun}` : `Excluded from ${boundaryNoun}`} (${boundedText(asset.kind, 100)}, ${boundedText(asset.sensitivity, 100)}): ${value}`
        + (instruction ? ` — ${boundedText(instruction, 1_000)}` : ""),
    );
  }
  if (assets.length > 200) {
    notes.push(`${boundaryNoun} asset list truncated: ${assets.length - 200} additional assets remain in Beale.`);
  }
  notes.push(...readStoredReportResourceNotes(database, options.externalSessionId));
  return notes.filter(Boolean);
}

function readStoredReportResourceNotes(
  database: DatabaseSync,
  sessionId: string | undefined,
): string[] {
  if (!sessionId) return [];
  const budget = readStoredRunBudget(database, sessionId);
  const resource = isRecord(budget?.resourceContext) ? budget.resourceContext : undefined;
  if (resource?.kind !== "report") return [];
  const resourceId = safeProjectedText(resource.resourceId);
  if (!resourceId) return [];
  const title = safeProjectedText(resource.title);
  const artifactId = safeProjectedText(resource.artifactId);
  const artifactRelativePath = safeProjectedText(resource.artifactRelativePath);
  const revision = typeof resource.revision === "number" && Number.isSafeInteger(resource.revision)
    ? resource.revision
    : undefined;
  return [
    `Active report resource: ${title ? `"${boundedText(title)}"; ` : ""}resource ID ${resourceId}${revision === undefined ? "" : `; current revision ${revision}`}.`,
    `Canonical report artifact: ${artifactId ?? "resolve through report.get"}${artifactRelativePath ? `; file ${boundedText(artifactRelativePath)}` : ""}.`,
    "Report refinement requirements: read the latest canonical document with report.get before evaluating or editing it; for requested changes, call report.revise with the current expected revision and the complete replacement Markdown; preserve evidence references; describe a revision only after the tool succeeds. The user does not need to supply the report name, resource ID, file, or these requirements.",
  ];
}

function readStoredRunBudget(
  database: DatabaseSync,
  sessionId: string,
): Record<string, unknown> | undefined {
  const runColumns = tableColumns(database, "runs");
  if (runColumns.has("id") && runColumns.has("budget_json")) {
    const row = database.prepare("SELECT budget_json FROM runs WHERE id = ?").get(sessionId) as
      | Record<string, unknown>
      | undefined;
    const parsed = parseStoredRecord(row?.budget_json);
    if (parsed) return parsed;
  }

  const sessionColumns = tableColumns(database, "app_server_sessions");
  if (!sessionColumns.has("id") || !sessionColumns.has("document_json")) return undefined;
  const row = database.prepare("SELECT document_json FROM app_server_sessions WHERE id = ?").get(sessionId) as
    | Record<string, unknown>
    | undefined;
  const session = parseStoredRecord(row?.document_json);
  const metadata = isRecord(session?.metadata) ? session.metadata : undefined;
  const bealeRun = isRecord(metadata?.bealeRun) ? metadata.bealeRun : undefined;
  return isRecord(bealeRun?.budget) ? bealeRun.budget : undefined;
}

function parseStoredRecord(value: unknown): Record<string, unknown> | undefined {
  if (typeof value !== "string") return undefined;
  try {
    const parsed = JSON.parse(value) as unknown;
    return isRecord(parsed) ? parsed : undefined;
  } catch {
    return undefined;
  }
}

function readStoredScopeAssets(database: DatabaseSync, scopeId: string | undefined): StoredScopeAsset[] {
  if (!scopeId) return [];
  const columns = tableColumns(database, "scope_assets");
  if (!["scope_version_id", "direction", "kind", "value", "sensitivity"]
    .every((column) => columns.has(column))) return [];
  const attributesProjection = columns.has("attributes_json")
    ? "attributes_json"
    : "'{}' AS attributes_json";
  return (database.prepare(`
    SELECT id, direction, kind, value, sensitivity, ${attributesProjection}
    FROM scope_assets WHERE scope_version_id = ? ORDER BY created_at, id
  `).all(scopeId) as Record<string, unknown>[]).flatMap((row) => {
    const direction = row.direction;
    const id = safeProjectedText(row.id);
    const kind = safeProjectedText(row.kind);
    const value = safeProjectedText(row.value);
    const sensitivity = safeProjectedText(row.sensitivity);
    if (!id || (direction !== "in_scope" && direction !== "out_of_scope") || !kind || !value || !sensitivity) return [];
    let attributes: unknown = {};
    try { attributes = JSON.parse(typeof row.attributes_json === "string" ? row.attributes_json : "{}"); } catch { attributes = {}; }
    return [{
      id,
      direction,
      kind,
      value,
      sensitivity,
      attributes: isRecord(attributes) ? attributes : {},
    }];
  });
}

function readStoredWorkspaceRules(database: DatabaseSync, workspaceId: string | undefined): string[] {
  if (!workspaceId) return [];
  const columns = tableColumns(database, "workspace_rules");
  if (!columns.has("workspace_id") || !columns.has("text")) return [];
  return (database.prepare(`
    SELECT text FROM workspace_rules WHERE workspace_id = ? ORDER BY created_at, id LIMIT 200
  `).all(workspaceId) as Record<string, unknown>[])
    .flatMap((row) => nonEmptyText(row.text) ?? [])
    .map((rule) => boundedText(rule, 2_000));
}

function boundedText(value: string, maxCharacters = 6_000): string {
  const normalized = value.replace(/\s+/gu, " ").trim();
  return normalized.length <= maxCharacters
    ? normalized
    : `${normalized.slice(0, Math.max(0, maxCharacters - 1))}…`;
}

function readStoredWorkspace(
  database: DatabaseSync,
  workspaceRoot: string,
): { id: string } | undefined {
  const columns = tableColumns(database, "workspaces");
  if (!columns.has("id") || !columns.has("workspace_path")) return undefined;
  const row = database
    .prepare("SELECT id FROM workspaces WHERE workspace_path = ?")
    .get(workspaceRoot) as Record<string, unknown> | undefined;
  const id = nonEmptyText(row?.id);
  return id ? { id } : undefined;
}

function readResearchProfileSnapshot(
  database: DatabaseSync,
  workspaceId: string,
  expected: { researchProfileId?: string; researchProfileHash?: string } = {},
): Record<string, unknown> | undefined {
  const columns = tableColumns(database, "research_profile_snapshots");
  for (const required of [
    "workspace_id",
    "profile_id",
    "profile_version",
    "profile_hash",
    "source",
    "profile_json",
    "active",
  ]) {
    if (!columns.has(required)) return undefined;
  }
  const conditions = ["workspace_id = ?"];
  const parameters: string[] = [workspaceId];
  if (expected.researchProfileId) {
    conditions.push("profile_id = ?");
    parameters.push(expected.researchProfileId);
  }
  if (expected.researchProfileHash) {
    conditions.push("profile_hash = ?");
    parameters.push(expected.researchProfileHash);
  }
  return database.prepare(
    `SELECT profile_id, profile_version, profile_hash, source, profile_json
     FROM research_profile_snapshots
     WHERE ${conditions.join(" AND ")}
     ORDER BY active DESC, created_at DESC
     LIMIT 1`,
  ).get(...parameters) as Record<string, unknown> | undefined;
}

function validateStoredResearchProfileSnapshot(
  snapshot: Record<string, unknown>,
): StoredResolvedResearchProfile {
  const expectedId = nonEmptyText(snapshot.profile_id);
  const expectedVersion = nonEmptyText(snapshot.profile_version);
  const expectedHash = nonEmptyText(snapshot.profile_hash);
  const source = snapshot.source;
  if (
    !expectedId
    || !expectedVersion
    || !expectedHash
    || (source !== "bundled-default"
      && source !== "workspace-default"
      && source !== "explicit")
    || typeof snapshot.profile_json !== "string"
  ) {
    throw new Error("Invalid stored research profile snapshot.");
  }
  const profile = normalizeResearchProfile(JSON.parse(snapshot.profile_json));
  const hash = researchProfileHash(profile);
  const legacyHash = legacyResearchProfileHash(profile);
  if (
    profile.id !== expectedId
    || profile.version !== expectedVersion
    || (hash !== expectedHash && legacyHash !== expectedHash)
  ) {
    throw new Error("Stored research profile snapshot provenance does not match its content.");
  }
  return { profile, hash: expectedHash, source };
}

function withoutResearchProfilePath(
  resolved: ResolvedResearchProfile,
): StoredResolvedResearchProfile {
  return {
    profile: resolved.profile,
    hash: resolved.hash,
    source: resolved.source,
  };
}

function readActiveScope(
  database: DatabaseSync,
  workspaceId: string,
): Record<string, unknown> | undefined {
  const columns = tableColumns(database, "scope_versions");
  if (!columns.has("workspace_id") || !columns.has("status")) return undefined;
  const orderBy = columns.has("version")
    ? "version DESC"
    : columns.has("created_at")
      ? "created_at DESC"
      : "rowid DESC";
  return database
    .prepare(
      `SELECT * FROM scope_versions
       WHERE workspace_id = ? AND status = 'active'
       ORDER BY ${orderBy}
       LIMIT 1`,
    )
    .get(workspaceId) as Record<string, unknown> | undefined;
}

function readStoredSubject(
  database: DatabaseSync,
  workspaceId: string,
): { id: string; name: string } | undefined {
  const columns = tableColumns(database, "workspace_research_subjects");
  if (
    !columns.has("workspace_id")
    || !columns.has("subject_id")
    || !columns.has("display_name")
  ) {
    return undefined;
  }
  const row = database
    .prepare(
      "SELECT subject_id, display_name FROM workspace_research_subjects WHERE workspace_id = ?",
    )
    .get(workspaceId) as Record<string, unknown> | undefined;
  const id = nonEmptyText(row?.subject_id);
  const name = nonEmptyText(row?.display_name);
  return id && name ? { id, name } : undefined;
}

function projectStoredAuthorization(
  database: DatabaseSync,
  scope: Record<string, unknown>,
): ResearchWorkspaceAuthorizationContext | undefined {
  const scopeId = nonEmptyText(scope.id);
  const scopeName = nonEmptyText(scope.workspace_name);
  const scopeOwner = nonEmptyText(scope.scope_owner);
  const description = nonEmptyText(scope.description_markdown);
  const rules = nonEmptyText(scope.rules_markdown);
  const assetCount = scopeId ? countScopeAssets(database, scopeId) : 0;
  const recorded = Boolean(
    (scopeName && scopeName !== "Untitled Workspace")
    || scopeOwner
    || description
    || rules
    || assetCount > 0,
  );
  if (!recorded) return undefined;

  const activeFrom = nonEmptyText(scope.active_from);
  const expiresAt = nonEmptyText(scope.expires_at);
  return {
    recorded: true,
    source: "beale",
    ...(scopeId ? { scopeId } : {}),
    ...(scopeName ? { scopeName } : {}),
    ...(scopeOwner ? { scopeOwner } : {}),
    ...(activeFrom ? { activeFrom } : {}),
    ...(expiresAt ? { expiresAt } : {}),
  };
}

function countScopeAssets(database: DatabaseSync, scopeId: string): number {
  const columns = tableColumns(database, "scope_assets");
  if (!columns.has("scope_version_id")) return 0;
  const row = database
    .prepare("SELECT COUNT(*) AS count FROM scope_assets WHERE scope_version_id = ?")
    .get(scopeId) as Record<string, unknown> | undefined;
  return typeof row?.count === "number" ? row.count : 0;
}

function tableColumns(database: DatabaseSync, table: string): Set<string> {
  const exists = database
    .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?")
    .get(table);
  if (!exists) return new Set();
  return new Set(
    (database.prepare(`PRAGMA table_info(${table})`).all() as Record<string, unknown>[])
      .flatMap((row) => typeof row.name === "string" ? [row.name] : []),
  );
}

function normalizeExternalSessionId(value: string | undefined): string | undefined {
  if (value === undefined) return undefined;
  const normalized = value.trim();
  if (!normalized) return undefined;
  if (normalized.length > 500 || /[\u0000-\u001f\u007f]/u.test(normalized)) {
    throw new Error("External session id must be at most 500 printable characters.");
  }
  return normalized;
}

function stableSubjectId(subjectName: string): string {
  const normalized = subjectName.trim().replace(/\s+/g, " ").toLowerCase();
  return `subject_${createHash("sha256")
    .update(normalized)
    .digest("hex")
    .slice(0, 20)}`;
}

function fallbackSubjectId(workspaceId: string): string {
  return `subject_workspace:${workspaceId}`;
}

function nonEmptyText(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() ? value.trim() : undefined;
}

function safeProjectedText(value: unknown): string | undefined {
  const text = nonEmptyText(value);
  if (!text || text.length > 2_000 || /[\u0000-\u001f\u007f]/u.test(text)) {
    return undefined;
  }
  return text;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
