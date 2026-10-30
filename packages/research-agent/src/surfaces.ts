import { randomUUID } from "node:crypto";
import type { DatabaseSync } from "node:sqlite";
import { openResearchDatabase } from "./database.js";
import { nowIso } from "./ids.js";
import type { ResearchResourceCatalog, TrackedResearchResource } from "./research-resources.js";
import type { ResearchClaimStore } from "./findings.js";
import type { FindingSummary } from "./knowledge-types.js";
import type { MemoryGraphStore, MemoryNode } from "./memory-graph.js";
import type { ResearchExecutableTool, ResearchToolExecutionContext } from "./tool-registry.js";
import type { ResearchResourceKind, ResearchToolAction } from "./types.js";

export const SURFACE_RANKING_FACTORS = ["impact", "reachability", "relevance", "exposure", "precedence"] as const;
export type SurfaceRankingFactor = typeof SURFACE_RANKING_FACTORS[number];
export type SurfaceRanking = Record<SurfaceRankingFactor, { score: number; basis: string }>;
export const HIGH_VALUE_SURFACE_SIGNALS = ["pre_auth", "sensitive_memory", "historical_sink", "niche_dependency", "default_reachable", "other"] as const;
export type HighValueSurfaceSignal = typeof HIGH_VALUE_SURFACE_SIGNALS[number];
export interface SurfaceScopeAssessment {
  decision: "applicable" | "not_applicable";
  basis: string;
  references: string[];
}

export interface SurfaceFoundation {
  discovery: string;
  scopeAssessment?: SurfaceScopeAssessment | null;
  ranking?: SurfaceRanking | null;
  highValueSignals: Array<{ kind: HighValueSurfaceSignal; basis: string }>;
  techStack: Array<{ technology: string; role: string; version: string; environment: string; evidence: string; confidence: "observed" | "inferred" | "unknown" }>;
  assets: Array<{ name: string; status: "untouched" | "in_progress" | "complete" }>;
  threatModelReviewed: boolean;
  bugHistory: { entries: Array<{ identifier: string; impact: string; affectedVersions: string; reference: string; notes: string }>; summary: string; limits: string };
  coverage: {
    summary: string;
    gaps: string;
    opportunities: string;
    mappingComplete: boolean;
    sinksInvestigated: boolean;
    openQuestions: string[];
    unexploredConnections: string[];
  };
  threatModel: {
    diagram: string;
    securityObjectives: string[];
    risks: string[];
    assumptions: string[];
    /** Retained for foundations written before threat-model lists. */
    riskImpact: string;
    invariants: string;
    sinks: string;
    mitigations: string;
    /** Retained for foundations written before the structured threat model. */
    attackerControl: string;
    trustBoundaries: string;
    assetsAtRisk: string;
    plausibleImpacts: string;
  };
}

export interface SurfaceProgressBlocker {
  reason: string;
  actorId: string;
  setAt: string;
}

export interface SurfaceCoverageState {
  covered: boolean;
  unmetRequirements: string[];
  openLeadIds: string[];
  unblockedFindingIds: string[];
}

export function hasThreatModelReview(foundation: SurfaceFoundation | null): boolean {
  return foundation?.threatModelReviewed === true || threatModelHasContent(foundation?.threatModel);
}

function threatModelHasContent(threatModel: SurfaceFoundation["threatModel"] | undefined): boolean {
  return Object.values(threatModel ?? {}).some((value) => typeof value === "string" && value.trim().length > 0
    || Array.isArray(value) && value.some((item) => typeof item === "string" && item.trim().length > 0));
}

export function assessSurfaceCoverage(
  foundation: SurfaceFoundation | null,
  openLeadIds: string[],
  unblockedFindingIds: string[],
): SurfaceCoverageState {
  const unmetRequirements: string[] = [];
  if (!hasThreatModelReview(foundation)) unmetRequirements.push("threat_model");
  if (openLeadIds.length) unmetRequirements.push("open_leads");
  if (unblockedFindingIds.length) unmetRequirements.push("unblocked_findings");
  return { covered: unmetRequirements.length === 0, unmetRequirements, openLeadIds, unblockedFindingIds };
}

export interface SurfaceProgressClaim {
  id: string;
  status: string;
  staleFromStatus?: string | null;
  surfaceId?: string | null;
  securityTracking?: Record<string, unknown> | null;
}

export function assessSurfaceClaims(surfaceId: string, claims: readonly SurfaceProgressClaim[]): Pick<SurfaceCoverageState, "openLeadIds" | "unblockedFindingIds"> {
  const openLeadIds: string[] = [];
  const unblockedFindingIds: string[] = [];
  for (const claim of claims) {
    const linkedIds = claim.securityTracking?.surfaceIds;
    if (claim.surfaceId !== surfaceId && !(Array.isArray(linkedIds) && linkedIds.includes(surfaceId))) continue;
    if (claim.status === "rejected") continue;
    if (claim.status === "hypothesis" || claim.status === "stale" && claim.staleFromStatus === "hypothesis") {
      openLeadIds.push(claim.id);
    } else if (claim.status !== "verified" && claim.status !== "report_ready" && claim.status !== "disclosed"
      && !claim.securityTracking?.progressBlocker) {
      unblockedFindingIds.push(claim.id);
    }
  }
  return { openLeadIds, unblockedFindingIds };
}

export interface SurfaceFoundationScout {
  status: "running" | "queued" | "unavailable";
  agentPath: string | null;
  reason: string | null;
}

export interface ResearchSurface {
  id: string;
  workspaceId: string;
  resourceId: string;
  parentId: string | null;
  name: string;
  description: string;
  root: boolean;
  firstTouchedAt: string | null;
  foundation: SurfaceFoundation | null;
  progressBlocker: SurfaceProgressBlocker | null;
  revision: number;
  createdAt: string;
  updatedAt: string;
}

export interface SurfaceCard {
  id: string;
  resourceId: string;
  resourceKind: ResearchResourceKind | null;
  resourceSource: TrackedResearchResource["source"] | null;
  parentId: string | null;
  name: string;
  path: string;
  root: boolean;
  focusEligible: boolean;
  firstTouchedAt: string | null;
  foundationReady: boolean;
  threatModelReviewed: boolean;
  progressBlocker: SurfaceProgressBlocker | null;
  coverageState: SurfaceCoverageState;
  updatedAt: string;
}

export interface SurfaceModelContext {
  counts: { roots: number; children: number; touched: number; pendingFoundation: number };
  surfaces: SurfaceCard[];
  omitted: number;
}

type SurfaceRow = Record<string, unknown>;

export function ensureSurfaceSchema(database: DatabaseSync): void {
  database.exec(`
      CREATE TABLE IF NOT EXISTS app_server_surfaces (
        id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, resource_id TEXT NOT NULL,
        parent_id TEXT NOT NULL, name TEXT NOT NULL, description TEXT NOT NULL,
        foundation_json TEXT, revision INTEGER NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
        created_session_id TEXT
      );
      CREATE INDEX IF NOT EXISTS app_server_surfaces_resource ON app_server_surfaces(workspace_id, resource_id, parent_id);
      CREATE UNIQUE INDEX IF NOT EXISTS app_server_surfaces_sibling_name
        ON app_server_surfaces(workspace_id, parent_id, name COLLATE NOCASE);
      CREATE TABLE IF NOT EXISTS app_server_surface_roots (
        resource_id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, foundation_json TEXT,
        revision INTEGER NOT NULL, updated_at TEXT NOT NULL
      );
      CREATE TABLE IF NOT EXISTS app_server_surface_touches (
        surface_id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, first_touched_at TEXT NOT NULL,
        first_session_id TEXT NOT NULL, first_agent_id TEXT NOT NULL
      );
      CREATE TABLE IF NOT EXISTS app_server_surface_focus (
        workspace_id TEXT NOT NULL, session_id TEXT NOT NULL, agent_id TEXT NOT NULL,
        surface_id TEXT NOT NULL, updated_at TEXT NOT NULL,
        PRIMARY KEY(workspace_id, session_id, agent_id)
      );
      CREATE TABLE IF NOT EXISTS app_server_surface_focus_events (
        id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, session_id TEXT NOT NULL,
        agent_id TEXT NOT NULL, previous_surface_id TEXT, surface_id TEXT NOT NULL,
        reason TEXT NOT NULL, moved_at TEXT NOT NULL
      );
      CREATE INDEX IF NOT EXISTS app_server_surface_focus_events_session
        ON app_server_surface_focus_events(workspace_id, session_id, moved_at);
      CREATE INDEX IF NOT EXISTS app_server_surface_focus_events_surface
        ON app_server_surface_focus_events(workspace_id, surface_id, moved_at DESC);
      CREATE TABLE IF NOT EXISTS app_server_surface_update_events (
        id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, surface_id TEXT NOT NULL,
        session_id TEXT NOT NULL, actor_id TEXT NOT NULL,
        kind TEXT NOT NULL CHECK(kind IN ('updated', 'blocked', 'unblocked')),
        changed_at TEXT NOT NULL
      );
      CREATE INDEX IF NOT EXISTS app_server_surface_update_events_surface
        ON app_server_surface_update_events(workspace_id, surface_id, changed_at DESC);
  `);
  const hasCreatedSession = database.prepare("SELECT 1 FROM pragma_table_info('app_server_surfaces') WHERE name = 'created_session_id'").get();
  if (!hasCreatedSession) database.exec('ALTER TABLE app_server_surfaces ADD COLUMN created_session_id TEXT');
}

/** Workspace-scoped surface structure. Root identity is the existing resource ID. */
export class SurfaceStore {
  readonly #database: DatabaseSync;
  readonly #workspaceId: string;
  readonly #sessionId: string;
  readonly #resources: ResearchResourceCatalog;
  #firstTouchHandler: ((surface: ResearchSurface, agentId: string) => SurfaceFoundationScout) | null = null;

  public constructor(input: { databasePath: string; workspaceId: string; sessionId?: string; resources: ResearchResourceCatalog }) {
    this.#database = openResearchDatabase(input.databasePath);
    this.#workspaceId = nonempty(input.workspaceId, "workspaceId");
    this.#sessionId = input.sessionId?.trim() || "external";
    this.#resources = input.resources;
    ensureSurfaceSchema(this.#database);
  }

  public close(): void { this.#database.close(); }

  public setFirstTouchHandler(handler: ((surface: ResearchSurface, agentId: string) => SurfaceFoundationScout) | null): void {
    this.#firstTouchHandler = handler;
  }

  public get(id: string): ResearchSurface | null {
    const resource = this.#resources.get(nonempty(id, "surfaceId"));
    if (resource?.workspaceId === this.#workspaceId) return this.#root(resource);
    const row = this.#database.prepare("SELECT * FROM app_server_surfaces WHERE workspace_id = ? AND id = ?")
      .get(this.#workspaceId, id) as SurfaceRow | undefined;
    return row ? this.#child(row) : null;
  }

  public require(id: string, eligible = false): ResearchSurface {
    const surface = this.get(id);
    if (!surface) throw new Error(`Surface not found in the active workspace: ${id}`);
    if (eligible && !this.#focusEligible(surface)) throw new Error("Surface resource is not eligible for research focus in this workspace.");
    return surface;
  }

  public resourceFor(id: string): TrackedResearchResource | null {
    const surface = this.require(id);
    const resource = this.#resources.get(surface.resourceId);
    return resource?.workspaceId === this.#workspaceId ? resource : null;
  }

  public priorArtFor(id: string, before?: number) {
    const resource = this.resourceFor(id);
    return resource ? this.#resources.priorArt.list([resource.id], before) : { entries: [], nextBefore: null };
  }

  public descendantIds(id: string): string[] {
    this.require(id);
    const children = new Map<string, string[]>();
    for (const surface of this.#all()) {
      if (surface.parentId) children.set(surface.parentId, [...(children.get(surface.parentId) ?? []), surface.id]);
    }
    const found = new Set<string>();
    const pending = [id];
    while (pending.length) {
      const current = pending.pop()!;
      if (found.has(current)) continue;
      found.add(current);
      pending.push(...(children.get(current) ?? []));
    }
    return [...found];
  }

  public list(input: { resourceId?: string; resourceKind?: string; parentId?: string; limit?: number; offset?: number } = {}): { surfaces: SurfaceCard[]; nextOffset: number | null; total: number } {
    const offset = Math.max(0, Math.floor(input.offset ?? 0));
    const limit = bound(input.limit, 50);
    const matches = this.#all().filter((surface) => (!input.resourceId || surface.resourceId === input.resourceId)
      && (!input.resourceKind || this.#resources.get(surface.resourceId)?.kind === input.resourceKind)
      && (input.parentId === undefined || surface.parentId === input.parentId));
    return { surfaces: matches.slice(offset, offset + limit).map((surface) => this.#card(surface)), nextOffset: offset + limit < matches.length ? offset + limit : null, total: matches.length };
  }

  #all(): ResearchSurface[] {
    return [
      ...this.#resources.list().filter((resource) => resource.workspaceId === this.#workspaceId).map((resource) => this.#root(resource)),
      ...(this.#database.prepare("SELECT * FROM app_server_surfaces WHERE workspace_id = ? ORDER BY created_at, id")
        .all(this.#workspaceId) as SurfaceRow[]).map((row) => this.#child(row)),
    ];
  }

  public query(text: string, limit = 20, offset = 0): { surfaces: SurfaceCard[]; nextOffset: number | null; total: number } {
    const phrase = nonempty(text, "query").toLowerCase();
    const words = phrase.split(/\s+/u).slice(0, 12);
    const matches = this.#all().flatMap((surface) => {
      const path = this.path(surface.id);
      const resource = this.#resources.get(surface.resourceId);
      const haystack = [surface.name, surface.description, path,
        resource?.name, resource?.locator, resource?.kind,
        surface.foundation?.discovery,
        ...(surface.foundation?.techStack ?? []).flatMap((item) => [item.technology, item.role, item.version, item.environment, item.evidence]),
        ...(surface.foundation?.assets ?? []).map((item) => item.name),
        ...(surface.foundation?.bugHistory.entries ?? []).flatMap((item) => [item.identifier, item.impact, item.affectedVersions, item.reference, item.notes]),
        surface.foundation?.bugHistory.summary, surface.foundation?.bugHistory.limits, surface.foundation?.coverage.summary,
        surface.foundation?.coverage.gaps, surface.foundation?.coverage.opportunities, surface.progressBlocker?.reason,
        ...Object.values(surface.foundation?.threatModel ?? {}).flatMap((value) => Array.isArray(value) ? value : [value])].join(" ").toLowerCase();
      if (!words.every((word) => haystack.includes(word))) return [];
      const name = surface.name.toLowerCase();
      const score = name === phrase ? 100 : name.includes(phrase) ? 70 : path.toLowerCase().includes(phrase) ? 40 : 0;
      return [{ surface, score }];
    }).sort((left, right) => right.score - left.score || left.surface.name.localeCompare(right.surface.name));
    const start = Math.max(0, Math.floor(offset));
    const pageLimit = bound(limit, 20);
    return { surfaces: matches.slice(start, start + pageLimit).map(({ surface }) => this.#card(surface)), nextOffset: start + pageLimit < matches.length ? start + pageLimit : null, total: matches.length };
  }

  public modelContext(): SurfaceModelContext {
    const all = this.#all();
    const selected = [
      ...all.filter((surface) => surface.root).sort((left, right) => left.name.localeCompare(right.name)).slice(0, 8),
      ...all.filter((surface) => !surface.root).sort((left, right) =>
        Number(Boolean(right.firstTouchedAt && !right.foundation)) - Number(Boolean(left.firstTouchedAt && !left.foundation))
        || right.updatedAt.localeCompare(left.updatedAt)).slice(0, 8),
    ];
    return {
      counts: {
        roots: all.filter((surface) => surface.root).length,
        children: all.filter((surface) => !surface.root).length,
        touched: all.filter((surface) => surface.firstTouchedAt).length,
        pendingFoundation: all.filter((surface) => surface.firstTouchedAt && !surface.foundation).length,
      },
      surfaces: selected.map((surface) => this.#card(surface)),
      omitted: Math.max(0, all.length - selected.length),
    };
  }

  public path(id: string): string {
    return this.hierarchy(id).map((surface) => surface.name).join(" / ");
  }

  public hierarchy(id: string): { id: string; name: string }[] {
    const hierarchy: { id: string; name: string }[] = [];
    let surface = this.require(id);
    for (let depth = 0; depth < 64; depth += 1) {
      hierarchy.unshift({ id: surface.id, name: surface.name });
      if (!surface.parentId) return hierarchy;
      surface = this.require(surface.parentId);
    }
    throw new Error("Surface hierarchy is too deep.");
  }

  public create(parentId: string, name: string, description = ""): ResearchSurface {
    const parent = this.require(parentId, true);
    const at = nowIso();
    const id = `surface_${randomUUID().replaceAll("-", "")}`;
    this.#database.prepare(`INSERT INTO app_server_surfaces
      (id, workspace_id, resource_id, parent_id, name, description, foundation_json, revision, created_at, updated_at, created_session_id)
      VALUES (?, ?, ?, ?, ?, ?, NULL, 1, ?, ?, ?)`).run(
      id, this.#workspaceId, parent.resourceId, parent.id,
      bounded(nonempty(name, "name"), 180), bounded(description, 2_000), at, at, this.#sessionId,
    );
    return this.require(id);
  }

  public update(id: string, expectedRevision: number, foundation?: unknown, name?: string, description?: string,
    progressBlocker?: { reason: string } | null, actorId?: string): ResearchSurface {
    const surface = this.require(id, true);
    if (surface.root && (name !== undefined || description !== undefined)) throw new Error("Root surface name and description belong to the resource catalog.");
    if (surface.revision !== expectedRevision) throw new Error("Surface revision changed; read it again before updating.");
    const normalized = foundation === undefined ? surface.foundation ?? (progressBlocker ? parseFoundation({}) : null)
      : parseFoundation(foundation, surface.foundation);
    const at = nowIso();
    const stored = {
      ...(normalized ?? {}),
      progressBlocker: progressBlocker === undefined ? surface.progressBlocker
        : progressBlocker === null ? null
          : { reason: bounded(nonempty(progressBlocker.reason, "Progress blocker reason"), 2_000),
              actorId: bounded(nonempty(actorId ?? "host", "Progress blocker actor"), 200), setAt: at },
    };
    const storedFoundation = normalized ? JSON.stringify(stored) : null;
    this.#database.exec("BEGIN IMMEDIATE");
    try {
      const changed = surface.root
        ? this.#database.prepare(`INSERT INTO app_server_surface_roots(resource_id, workspace_id, foundation_json, revision, updated_at)
            VALUES (?, ?, ?, 2, ?)
            ON CONFLICT(resource_id) DO UPDATE SET foundation_json = excluded.foundation_json,
              revision = app_server_surface_roots.revision + 1, updated_at = excluded.updated_at
            WHERE app_server_surface_roots.revision = ?`)
          .run(id, this.#workspaceId, storedFoundation, at, expectedRevision).changes
        : this.#database.prepare(`UPDATE app_server_surfaces SET name = ?, description = ?, foundation_json = ?,
            revision = revision + 1, updated_at = ? WHERE workspace_id = ? AND id = ? AND revision = ?`)
          .run(name === undefined ? surface.name : bounded(nonempty(name, "name"), 180),
            description === undefined ? surface.description : bounded(description, 2_000),
            storedFoundation, at, this.#workspaceId, id, expectedRevision).changes;
      if (!changed) throw new Error("Surface revision changed; read it again before updating.");
      const kind = progressBlocker && progressBlocker.reason ? "blocked"
        : progressBlocker === null && surface.progressBlocker ? "unblocked" : "updated";
      this.#database.prepare(`INSERT INTO app_server_surface_update_events
        (id, workspace_id, surface_id, session_id, actor_id, kind, changed_at)
        VALUES (?, ?, ?, ?, ?, ?, ?)`).run(
        `surface_update_${randomUUID().replaceAll("-", "")}`, this.#workspaceId, id, this.#sessionId,
        bounded(nonempty(actorId ?? "host", "Surface update actor"), 200), kind, at,
      );
      this.#database.exec("COMMIT");
    } catch (error) {
      this.#database.exec("ROLLBACK");
      throw error;
    }
    return this.require(id);
  }

  public focus(agentId: string, surfaceId: string, reason = "research"): { surface: ResearchSurface; surfaceTrail: { id: string; name: string }[]; firstTouch: boolean; previousSurfaceId: string | null; foundationScout?: SurfaceFoundationScout } {
    const surface = this.require(surfaceId, true);
    const agent = nonempty(agentId, "agentId");
    const at = nowIso();
    this.#database.exec("BEGIN IMMEDIATE");
    try {
      const previous = this.#database.prepare(`SELECT surface_id FROM app_server_surface_focus
        WHERE workspace_id = ? AND session_id = ? AND agent_id = ?`)
        .get(this.#workspaceId, this.#sessionId, agent) as { surface_id: string } | undefined;
      const touched = this.#database.prepare(`INSERT OR IGNORE INTO app_server_surface_touches
        (surface_id, workspace_id, first_touched_at, first_session_id, first_agent_id)
        VALUES (?, ?, ?, ?, ?)`).run(surfaceId, this.#workspaceId, at, this.#sessionId, agent).changes > 0;
      if (previous?.surface_id !== surfaceId) {
        this.#database.prepare(`INSERT INTO app_server_surface_focus_events
          (id, workspace_id, session_id, agent_id, previous_surface_id, surface_id, reason, moved_at)
          VALUES (?, ?, ?, ?, ?, ?, ?, ?)`).run(
          `surface_move_${randomUUID().replaceAll("-", "")}`, this.#workspaceId, this.#sessionId,
          agent, previous?.surface_id ?? null, surfaceId, bounded(reason, 500), at,
        );
        this.#database.prepare(`INSERT INTO app_server_surface_focus
          (workspace_id, session_id, agent_id, surface_id, updated_at) VALUES (?, ?, ?, ?, ?)
          ON CONFLICT(workspace_id, session_id, agent_id) DO UPDATE SET surface_id = excluded.surface_id,
            updated_at = excluded.updated_at`).run(this.#workspaceId, this.#sessionId, agent, surfaceId, at);
      }
      this.#database.exec("COMMIT");
      const focused = this.require(surfaceId);
      let foundationScout: SurfaceFoundationScout | undefined;
      if (touched) {
        try {
          foundationScout = this.#firstTouchHandler?.(focused, agent) ??
            { status: "unavailable", agentPath: null, reason: "No foundation scout is available in this session." };
        } catch (error) {
          foundationScout = { status: "unavailable", agentPath: null, reason: error instanceof Error ? error.message : String(error) };
        }
      }
      return { surface: focused, surfaceTrail: this.hierarchy(surfaceId), firstTouch: touched, previousSurfaceId: previous?.surface_id ?? null,
        ...(foundationScout ? { foundationScout } : {}) };
    } catch (error) {
      this.#database.exec("ROLLBACK");
      throw error;
    }
  }

  public current(agentId: string): ResearchSurface | null {
    const row = this.#database.prepare(`SELECT surface_id FROM app_server_surface_focus
      WHERE workspace_id = ? AND session_id = ? AND agent_id = ?`)
      .get(this.#workspaceId, this.#sessionId, agentId) as { surface_id: string } | undefined;
    return row ? this.get(row.surface_id) : null;
  }

  #root(resource: TrackedResearchResource): ResearchSurface {
    const row = this.#database.prepare("SELECT * FROM app_server_surface_roots WHERE resource_id = ? AND workspace_id = ?")
      .get(resource.id, this.#workspaceId) as SurfaceRow | undefined;
    const stored = decodeStoredFoundation(row?.foundation_json);
    return {
      id: resource.id, workspaceId: this.#workspaceId, resourceId: resource.id, parentId: null,
      name: resource.name, description: resource.rationale, root: true,
      firstTouchedAt: this.#touchedAt(resource.id),
      foundation: stored?.foundation ?? null, progressBlocker: stored?.progressBlocker ?? null,
      revision: row ? Number(row.revision) : 1,
      createdAt: resource.discoveredAt, updatedAt: row ? String(row.updated_at) : resource.updatedAt,
    };
  }

  #child(row: SurfaceRow): ResearchSurface {
    const stored = decodeStoredFoundation(row.foundation_json);
    return {
      id: String(row.id), workspaceId: String(row.workspace_id), resourceId: String(row.resource_id),
      parentId: String(row.parent_id), name: String(row.name), description: String(row.description), root: false,
      firstTouchedAt: this.#touchedAt(String(row.id)), foundation: stored?.foundation ?? null,
      progressBlocker: stored?.progressBlocker ?? null,
      revision: Number(row.revision), createdAt: String(row.created_at), updatedAt: String(row.updated_at),
    };
  }

  #touchedAt(id: string): string | null {
    const row = this.#database.prepare("SELECT first_touched_at FROM app_server_surface_touches WHERE surface_id = ? AND workspace_id = ?")
      .get(id, this.#workspaceId) as { first_touched_at: string } | undefined;
    return row?.first_touched_at ?? null;
  }

  #card(surface: ResearchSurface): SurfaceCard {
    const resource = this.#resources.get(surface.resourceId);
    return {
      id: surface.id, resourceId: surface.resourceId, resourceKind: resource?.kind ?? null,
      resourceSource: resource?.source ?? null, parentId: surface.parentId,
      name: surface.name, path: this.path(surface.id), root: surface.root,
      focusEligible: this.#focusEligible(surface),
      firstTouchedAt: surface.firstTouchedAt, foundationReady: surface.foundation !== null,
      threatModelReviewed: hasThreatModelReview(surface.foundation),
      progressBlocker: surface.progressBlocker,
      coverageState: this.coverageState(surface.id),
      updatedAt: surface.updatedAt,
    };
  }

  public coverageState(id: string): SurfaceCoverageState {
    const surface = this.require(id);
    const { openLeadIds, unblockedFindingIds } = this.#claimProgress(surface.id);
    return assessSurfaceCoverage(surface.foundation, openLeadIds, unblockedFindingIds);
  }

  #claimProgress(surfaceId: string): Pick<SurfaceCoverageState, "openLeadIds" | "unblockedFindingIds"> {
    const table = this.#database.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'app_server_research_claims'").get();
    if (!table) return { openLeadIds: [], unblockedFindingIds: [] };
    const surfaceColumn = this.#database.prepare("SELECT 1 FROM pragma_table_info('app_server_research_claims') WHERE name = 'surface_id'").get()
      ? "surface_id" : "NULL AS surface_id";
    const staleColumn = this.#database.prepare("SELECT 1 FROM pragma_table_info('app_server_research_claims') WHERE name = 'stale_from_status'").get()
      ? "stale_from_status" : "NULL AS stale_from_status";
    const rows = this.#database.prepare(`SELECT id, status, ${staleColumn}, ${surfaceColumn}, security_tracking_json
      FROM app_server_research_claims WHERE workspace_id = ? AND duplicate_of_claim_id IS NULL ORDER BY id`)
      .all(this.#workspaceId) as { id: string; status: string; stale_from_status: string | null; surface_id: string | null; security_tracking_json: string }[];
    return assessSurfaceClaims(surfaceId, rows.map((row) => {
      let securityTracking: Record<string, unknown> | null = null;
      try {
        const value: unknown = JSON.parse(row.security_tracking_json);
        if (value && typeof value === "object" && !Array.isArray(value)) securityTracking = value as Record<string, unknown>;
      } catch { /* A direct surface assignment remains usable without tracking metadata. */ }
      return { id: row.id, status: row.status, staleFromStatus: row.stale_from_status,
        surfaceId: row.surface_id, securityTracking };
    }));
  }

  #focusEligible(surface: ResearchSurface): boolean {
    const resource = this.#resources.get(surface.resourceId);
    return Boolean(resource && resource.workspaceId === this.#workspaceId
      && resource.direction !== "out_of_scope" && resource.reviewStatus !== "not_relevant"
      && (resource.source !== "runtime_discovery" || resource.reviewStatus === "relevant"));
  }
}

const SURFACE_RELATED_ITEM_LIMIT = 50;

interface SurfaceRelatedStores {
  claims?: ResearchClaimStore;
  memories?: MemoryGraphStore;
}

function claimOnSurface(claim: FindingSummary, ids: ReadonlySet<string>): boolean {
  return Boolean(claim.surfaceId && ids.has(claim.surfaceId)
    || claim.securityTracking?.surfaceIds.some((id) => ids.has(id)));
}

function relatedClaims(claims: readonly FindingSummary[], ids: ReadonlySet<string>, projection: "lead" | "finding") {
  const matched = claims.filter((claim) => claim.projection === projection && claimOnSurface(claim, ids)
    && claim.duplicateOfClaimId === null && (projection !== "lead" || claim.status !== "rejected"));
  return {
    items: matched.slice(0, SURFACE_RELATED_ITEM_LIMIT).map((claim) => ({
      id: claim.id, title: claim.title, status: claim.status,
      rating: claim.rating, surfaceId: claim.surfaceId,
    })),
    total: matched.length,
    truncated: matched.length > SURFACE_RELATED_ITEM_LIMIT,
  };
}

function relatedMemories(memories: MemoryGraphStore | undefined, surfaceIds: readonly string[], type: MemoryNode["type"]) {
  const matched = memories?.search({ scope: "workspace", surfaceIds, types: [type], excludeStatuses: ["rejected"], limit: SURFACE_RELATED_ITEM_LIMIT + 1 }) ?? [];
  return {
    items: matched.slice(0, SURFACE_RELATED_ITEM_LIMIT).map((node) => ({
      id: node.id, type: node.type, title: node.title, status: node.status, surfaceId: node.surfaceId,
    })),
    truncated: matched.length > SURFACE_RELATED_ITEM_LIMIT,
  };
}

type SurfaceViewName = "breakdown" | "recon" | "analysis" | "threat_model" | "resources";
const SURFACE_VIEW_NAMES: readonly SurfaceViewName[] = ["breakdown", "recon", "analysis", "threat_model", "resources"];

function requestedSurfaceViews(value: unknown): SurfaceViewName[] {
  if (!Array.isArray(value) || value.length < 1 || value.length > SURFACE_VIEW_NAMES.length
    || value.some((item) => typeof item !== "string" || !SURFACE_VIEW_NAMES.includes(item as SurfaceViewName))
    || new Set(value).size !== value.length) {
    throw new Error(`views must contain 1 to ${SURFACE_VIEW_NAMES.length} distinct names: ${SURFACE_VIEW_NAMES.join(", ")}.`);
  }
  return value as SurfaceViewName[];
}

function readSurfaceView(store: SurfaceStore, related: SurfaceRelatedStores, surface: ResearchSurface,
  view: SurfaceViewName, input: Record<string, unknown>): unknown {
  switch (view) {
    case "breakdown": return { surfaceId: surface.id, name: surface.name, description: surface.description,
      path: store.path(surface.id), revision: surface.revision, firstTouchedAt: surface.firstTouchedAt,
      updatedAt: surface.updatedAt, ranking: surface.foundation?.ranking ?? null,
      scopeAssessment: surface.foundation?.scopeAssessment ?? null,
      highValueSignals: surface.foundation?.highValueSignals ?? [],
      progressBlocker: surface.progressBlocker, coverageState: store.coverageState(surface.id),
      children: store.list({ parentId: surface.id, limit: typeof input.limit === "number" ? input.limit : 20,
        ...(typeof input.offset === "number" ? { offset: input.offset } : {}) }) };
    case "recon": return { surfaceId: surface.id, name: surface.name, revision: surface.revision,
      discovery: surface.foundation?.discovery ?? "", techStack: surface.foundation?.techStack ?? [],
      assets: surface.foundation?.assets ?? [], bugHistory: surface.foundation?.bugHistory ?? null,
      leads: { available: Boolean(related.claims), ...relatedClaims(related.claims?.list() ?? [], new Set([surface.id]), "lead") } };
    case "analysis": {
      const descendantIds = store.descendantIds(surface.id);
      return { surfaceId: surface.id, name: surface.name, revision: surface.revision,
        findings: { available: Boolean(related.claims), ...relatedClaims(related.claims?.list() ?? [], new Set(descendantIds), "finding") },
        findingsNeedingResolutionIds: store.coverageState(surface.id).unblockedFindingIds,
        trajectories: { available: Boolean(related.memories), ...relatedMemories(related.memories, descendantIds, "trajectory") } };
    }
    case "threat_model": {
      const threatModel = surface.foundation?.threatModel;
      return { surfaceId: surface.id, name: surface.name, revision: surface.revision,
        diagram: threatModel?.diagram ?? "", securityObjectives: threatModel?.securityObjectives ?? [],
        risks: threatModel?.risks ?? [], assumptions: threatModel?.assumptions ?? [],
        invariants: { available: Boolean(related.memories), ...relatedMemories(related.memories, [surface.id], "invariant") },
        sinks: { available: Boolean(related.memories), ...relatedMemories(related.memories, [surface.id], "sink") },
        mitigations: { available: Boolean(related.memories), ...relatedMemories(related.memories, [surface.id], "mitigation") },
        legacyNotes: threatModel ? { riskImpact: threatModel.riskImpact, invariants: threatModel.invariants,
          sinks: threatModel.sinks, mitigations: threatModel.mitigations, attackerControl: threatModel.attackerControl,
          trustBoundaries: threatModel.trustBoundaries, assetsAtRisk: threatModel.assetsAtRisk,
          plausibleImpacts: threatModel.plausibleImpacts } : null };
    }
    case "resources": return { surfaceId: surface.id, name: surface.name, resource: store.resourceFor(surface.id),
      priorArt: store.priorArtFor(surface.id, typeof input.before === "number" ? input.before : undefined) };
  }
}

function rankingFactorSchema(description: string) {
  return { type: "object", additionalProperties: false, required: ["score", "basis"], properties: {
    score: { type: "integer", minimum: 1, maximum: 10, description },
    basis: { type: "string", maxLength: 500,
      description: "Short basis from scope, observed reachability, threat model, or relevant prior art. State uncertainty explicitly." },
  } };
}

export function createSurfaceTools(store: SurfaceStore, related: SurfaceRelatedStores = {}): ResearchExecutableTool[] {
  const foundationSchema = {
    type: "object", additionalProperties: false,
    properties: {
      discovery: { type: "string", description: "General reconnaissance note for this component. May be as detailed as useful; preserve uncertainty and source bounds." },
      scopeAssessment: { anyOf: [{ type: "object", additionalProperties: false,
        required: ["decision", "basis", "references"], properties: {
          decision: { type: "string", enum: ["applicable", "not_applicable"] },
          basis: { type: "string", maxLength: 1_000, description: "Why this concrete component can or cannot affect an eligible target under the recorded program rules." },
          references: { type: "array", minItems: 1, maxItems: 12, items: { type: "string", maxLength: 500 },
            description: "Cite the scope rule, program policy, or evidence that supports this decision." },
        } }, { type: "null" }],
        description: "Record a component-level scope decision only when supported by policy or target evidence. A repository being in scope does not make every test, example, or undeployed subcomponent applicable. Null clears a prior assessment; omit when uncertain." },
      ranking: { type: "object", additionalProperties: false, required: [...SURFACE_RANKING_FACTORS],
        description: "Assess all five equally weighted priority factors on a 1–10 scale, with a concise evidence-based basis for each. One is low, five is moderate, and ten is high priority. Omit until there is enough context; do not assign neutral scores merely because a factor is unknown.",
        properties: {
          impact: rankingFactorSchema("Potential vendor and user security consequence: 1 for minor local effect, 5 for bounded security impact, 10 for broad or severe compromise."),
          reachability: rankingFactorSchema("Practical attacker path into sensitive code: 1 for difficult privileged prerequisites, 5 for conditional access, 10 for direct attacker-controlled reachability."),
          relevance: rankingFactorSchema("Match to authorized scope and research objective: 1 for peripheral in-scope work, 5 for adjacent work, 10 for a core target boundary."),
          exposure: rankingFactorSchema("Who can exercise the entry point: 1 for narrow privileged/internal access, 5 for authenticated or limited access, 10 for public pre-auth exposure."),
          precedence: rankingFactorSchema("Applicable vulnerability history: 1 when a bounded search found no close analogue, 5 for a related bug class with uncertain applicability, 10 for repeated directly relevant issues. Prior issues are leads, not proof of a current flaw."),
        } },
      highValueSignals: { type: "array", maxItems: 12, description: "Evidence-backed attacker-value signals for this component. Record a concrete basis; these guide discovery selection and are not claims of a vulnerability.", items: {
        type: "object", additionalProperties: false, required: ["kind", "basis"], properties: {
          kind: { type: "string", enum: [...HIGH_VALUE_SURFACE_SIGNALS] }, basis: { type: "string", maxLength: 500 },
        },
      } },
      techStack: { type: "array", maxItems: 200, description: "Structured technologies and software environments relevant to this surface. Replace the list when supplied; omit it to preserve existing entries.", items: {
        type: "object", required: ["technology"], additionalProperties: false, properties: {
          technology: { type: "string", description: "Product, framework, library, runtime, or platform name." },
          role: { type: "string", description: "What it does in this surface." },
          version: { type: "string", description: "Observed version or bounded version range, if known." },
          environment: { type: "string", description: "Deployment, platform, configuration, or scope where this technology applies." },
          evidence: { type: "string", description: "Brief source or observation supporting the mapping." },
          confidence: { type: "string", enum: ["observed", "inferred", "unknown"], description: "Whether the mapping is observed, inferred, or still unknown." },
        },
      } },
      assets: { type: "array", maxItems: 500, description: "Concrete items to research within this surface, such as functions, files, modules, binaries, subdomains, or webpages. Replace the list when supplied; omit it to preserve existing entries.", items: {
        type: "object", required: ["name"], additionalProperties: false, properties: {
          name: { type: "string", description: "Item name, path, or URL without a required asset type." },
          status: { type: "string", enum: ["untouched", "in_progress", "complete"], description: "Research state; defaults to untouched when omitted. Asset states are context, not surface completion gates." },
        },
      } },
      bugHistory: { type: "object", properties: {
        entries: { type: "array", maxItems: 200, description: "Individual relevant CVEs, GHSAs, vendor advisories, or other published issues and their impact. Prefer prior_art.search and prior_art.fetch; use the browser to supplement when enabled. Replace the list when supplied; omit it to preserve existing entries.", items: {
          type: "object", required: ["identifier", "impact"], additionalProperties: false, properties: {
            identifier: { type: "string", description: "CVE, GHSA, vendor advisory, or other issue identifier." },
            impact: { type: "string", description: "Security impact described by the source, without assuming it applies to this target." },
            affectedVersions: { type: "string", description: "Published affected versions or applicability limits." },
            reference: { type: "string", description: "Primary advisory or source URL." },
            notes: { type: "string", description: "Why it is relevant to this surface and any uncertainty." },
          },
        } },
        summary: { type: "string", description: "Legacy general note; prefer entries for individual issues and discovery for broader context." },
        limits: { type: "string", description: "Bounds of searched sources, versions, and aliases; a no-match search is only a bounded result." },
      } },
      threatModel: { type: "object", properties: {
        diagram: { type: "string", description: "Raw Mermaid diagram source (no code fence) showing the component, relevant actors, data flows, trust boundaries, and security-relevant sinks. Keep it grounded in observed or explicitly assumed relationships; update it as the model changes." },
        securityObjectives: { type: "array", maxItems: 100, items: { type: "string" }, description: "Distinct protected operations, assets, or security outcomes. Replace the list when supplied; omit it to preserve existing items." },
        risks: { type: "array", maxItems: 100, items: { type: "string" }, description: "Distinct plausible unauthorized effects, each with reachability and evidence limits; ordinary malfunction alone is not security impact. Replace the list when supplied." },
        assumptions: { type: "array", maxItems: 100, items: { type: "string" }, description: "Distinct unknowns or assumptions required for attacker paths or impacts. Replace the list when supplied." },
      } },
    },
  };
  const specs = [
    { name: "surface.list", transportName: "surface_list", effect: "read", properties: { resourceId: { type: "string" }, resourceKind: { type: "string", enum: ["domain", "repository", "binary", "service", "tool", "documentation", "other"], description: "Filter roots and children by resource kind to compare research outside a dominant source index." }, parentId: { type: "string" }, limit: { type: "integer", minimum: 1, maximum: 50 }, offset: { type: "integer", minimum: 0 } }, required: [] },
    { name: "surface.get", transportName: "surface_get", effect: "read", properties: { surfaceId: { type: "string" } }, required: ["surfaceId"] },
    { name: "surface.views", transportName: "surface_views", effect: "read", properties: { surfaceId: { type: "string" },
      views: { type: "array", minItems: 1, maxItems: SURFACE_VIEW_NAMES.length, uniqueItems: true,
        description: "Read several views for one surface in one call. Use the individual view tools for later pages of Breakdown or Resources.",
        items: { type: "string", enum: SURFACE_VIEW_NAMES } } }, required: ["surfaceId", "views"] },
    { name: "surface.search", transportName: "surface_search", effect: "read", properties: { query: { type: "string" }, limit: { type: "integer", minimum: 1, maximum: 50 }, offset: { type: "integer", minimum: 0 } }, required: ["query"] },
    { name: "surface.query", transportName: "surface_query", effect: "read", properties: { query: { type: "string" }, limit: { type: "integer", minimum: 1, maximum: 50 }, offset: { type: "integer", minimum: 0 } }, required: ["query"] },
    { name: "surface.breakdown", transportName: "surface_breakdown", effect: "read", properties: { surfaceId: { type: "string" }, limit: { type: "integer", minimum: 1, maximum: 50 }, offset: { type: "integer", minimum: 0 } }, required: ["surfaceId"] },
    { name: "surface.recon", transportName: "surface_recon", effect: "read", properties: { surfaceId: { type: "string" } }, required: ["surfaceId"] },
    { name: "surface.analysis", transportName: "surface_analysis", effect: "read", properties: { surfaceId: { type: "string" } }, required: ["surfaceId"] },
    { name: "surface.threat_model", transportName: "surface_threat_model", effect: "read", properties: { surfaceId: { type: "string" } }, required: ["surfaceId"] },
    { name: "surface.resources", transportName: "surface_resources", effect: "read", properties: { surfaceId: { type: "string" }, before: { type: "integer", minimum: 1, description: "Use priorArt.nextBefore to read the next page of saved prior art." } }, required: ["surfaceId"] },
    { name: "surface.create", transportName: "surface_create", effect: "write", properties: { parentId: { type: "string" }, name: { type: "string", description: "A stable noun naming a concrete component or subcomponent, never a question, theory, test, or checklist item." }, description: { type: "string", description: "Optional note on the component's role and boundary." } }, required: ["parentId", "name"] },
    { name: "surface.update", transportName: "surface_update", effect: "write", properties: { surfaceId: { type: "string" }, expectedRevision: { type: "integer", minimum: 1 }, name: { type: "string" }, description: { type: "string" }, foundation: { ...foundationSchema, description: "Optional, partial context notes. Omitted fields preserve existing values." },
      progressBlocker: { anyOf: [{ type: "object", required: ["reason"], additionalProperties: false,
        properties: { reason: { type: "string", description: "Briefly describe the concrete external dependency and what would unblock work." } } }, { type: "null" }], description: "Set a concrete external blocker, or pass null to clear it. The existing foundation may be omitted." } }, required: ["surfaceId", "expectedRevision"] },
    { name: "surface.focus", transportName: "surface_focus", effect: "write", properties: { surfaceId: { type: "string" }, reason: { type: "string" } }, required: ["surfaceId"] },
  ] as const;
  return specs.map((spec): ResearchExecutableTool => ({
    descriptor: {
      name: spec.name, transportName: spec.transportName,
      description: spec.name === "surface.focus"
        ? "Associate current work with a component. First focus records its touch and starts a brief context scout. Focus does not restrict research tools."
        : spec.name === "surface.search"
          ? "Search every surface across names, paths, resource locators, and view content. Use this first to find a known component or concept; surface.list only browses a page of the inventory."
          : spec.name === "surface.query"
            ? "Compatibility alias for surface.search. Prefer surface.search for targeted discovery."
            : spec.name === "surface.get"
              ? "Get a compact surface overview, coverage state, and available view lookups. Use surface.views to inspect several views together."
              : spec.name === "surface.views"
                ? "Read 1 to 5 selected views of one surface in a single call, including linked leads, findings, and typed memories. Use individual view tools to page Breakdown or Resources."
              : spec.name === "surface.list"
                ? "Browse a paged surface inventory. Use surface.search when looking for a specific component or concept that may be outside this page."
                : spec.name === "surface.breakdown"
                  ? "Read the Breakdown view: description, five-factor priority ranking, progress blocker, coverage, and paged child surfaces."
                  : spec.name === "surface.recon"
                    ? "Read the Recon view: discovery, tech stack, assets, bug history, and a compact directly linked lead list. Fetch individual claims for details."
                    : spec.name === "surface.analysis"
                      ? "Read the Analysis view: compact findings and trajectory memory lists linked to this surface or descendants, including findings needing resolution. Fetch individual records for details."
                      : spec.name === "surface.threat_model"
                        ? "Read the Threat Model view: Mermaid diagram, objectives, risks, assumptions, and compact directly linked invariant, sink, and mitigation memory lists. Fetch individual memories for details."
                        : spec.name === "surface.resources"
                          ? "Read the Resources view: the owning resource's scope, locator, source, review metadata, and paged saved prior-art list."
                          : spec.name === "surface.update"
                            ? "Update surface context with an expected revision. Omitted foundation fields are preserved. The result is a compact acknowledgement; use surface.views or a specific view lookup to inspect saved details."
                          : `Refine the resource-owned component tree (${spec.name}). Children are concrete components or subcomponents with stable noun names.`,
      actionClasses: [spec.effect === "read" ? "recall" : "synthesize"], sideEffects: spec.effect,
      requiredPermissions: ["research-resource:inventory"],
      inputSchema: { type: "object", required: spec.required, properties: spec.properties, additionalProperties: false },
    },
    parameters: { type: "object", required: spec.required, properties: spec.properties, additionalProperties: false },
    async execute(action: ResearchToolAction, context?: ResearchToolExecutionContext) {
      const input = action.input;
      const startedAt = nowIso();
      try {
        let output: unknown;
        switch (spec.name) {
          case "surface.list": output = store.list({ ...(typeof input.resourceId === "string" ? { resourceId: input.resourceId } : {}), ...(typeof input.resourceKind === "string" ? { resourceKind: input.resourceKind } : {}), ...(typeof input.parentId === "string" ? { parentId: input.parentId } : {}), ...(typeof input.limit === "number" ? { limit: input.limit } : {}), ...(typeof input.offset === "number" ? { offset: input.offset } : {}) }); break;
          case "surface.get": {
            const surface = store.require(nonempty(input.surfaceId, "surfaceId"));
            const { foundation: _foundation, ...overview } = surface;
            output = { ...overview, path: store.path(surface.id), coverageState: store.coverageState(surface.id),
              foundationReady: surface.foundation !== null, threatModelReviewed: hasThreatModelReview(surface.foundation),
              views: ["surface.breakdown", "surface.recon", "surface.analysis", "surface.threat_model",
                ...(surface.root ? ["surface.resources"] : [])] };
            break;
          }
          case "surface.query":
          case "surface.search": output = store.query(nonempty(input.query, "query"), typeof input.limit === "number" ? input.limit : 20, typeof input.offset === "number" ? input.offset : 0); break;
          case "surface.views": {
            const surface = store.require(nonempty(input.surfaceId, "surfaceId"));
            const views = requestedSurfaceViews(input.views);
            output = { surfaceId: surface.id, name: surface.name,
              views: Object.fromEntries(views.map((view) => [view, readSurfaceView(store, related, surface, view, {})])) };
            break;
          }
          case "surface.breakdown":
          case "surface.recon":
          case "surface.analysis":
          case "surface.threat_model":
          case "surface.resources": {
            const surface = store.require(nonempty(input.surfaceId, "surfaceId"));
            output = readSurfaceView(store, related, surface, spec.name.slice("surface.".length) as SurfaceViewName, input);
            break;
          }
          case "surface.create": output = store.create(nonempty(input.parentId, "parentId"), nonempty(input.name, "name"), typeof input.description === "string" ? input.description : ""); break;
          case "surface.update": {
            const surfaceId = nonempty(input.surfaceId, "surfaceId");
            const previousDiagram = store.require(surfaceId).foundation?.threatModel.diagram ?? "";
            const requestedFoundation = input.foundation && typeof input.foundation === "object" && !Array.isArray(input.foundation)
              ? input.foundation as Record<string, unknown> : null;
            const requestedThreatModel = requestedFoundation?.threatModel && typeof requestedFoundation.threatModel === "object"
              && !Array.isArray(requestedFoundation.threatModel)
              ? requestedFoundation.threatModel as Record<string, unknown> : null;
            const submittedDiagram = requestedThreatModel?.diagram;
            const surface = store.update(surfaceId, positive(input.expectedRevision, "expectedRevision"), input.foundation,
              typeof input.name === "string" ? input.name : undefined, typeof input.description === "string" ? input.description : undefined,
              parseBlockerUpdate(input.progressBlocker), context?.agentId ?? "root");
            output = { id: surface.id, name: surface.name, revision: surface.revision,
              updatedAt: surface.updatedAt, progressBlocker: surface.progressBlocker,
              coverageState: store.coverageState(surface.id),
              ...(typeof submittedDiagram === "string" && submittedDiagram.trim()
                ? { diagramChange: previousDiagram.trim() ? "updated" : "created" } : {}) };
            break;
          }
          case "surface.focus": output = store.focus(context?.agentId ?? "root", nonempty(input.surfaceId, "surfaceId"), typeof input.reason === "string" ? input.reason : "research"); break;
        }
        return { action, status: "complete", startedAt, completedAt: nowIso(), summary: `${spec.name} completed.`, output, followUpActions: spec.name === "surface.focus" && typeof output === "object" && output !== null && "foundationScout" in output && output.foundationScout
          ? ["A first-touch foundation scout has been requested. Coordinate with it using its reported status and avoid duplicating its assignment."] : [] };
      } catch (error) {
        return { action, status: "error", startedAt, completedAt: nowIso(), summary: `${spec.name} failed.`, error: { message: error instanceof Error ? error.message : String(error) }, followUpActions: [] };
      }
    },
  }));
}

function nonempty(value: unknown, name: string): string {
  if (typeof value !== "string" || !value.trim()) throw new Error(`${name} must be a non-empty string.`);
  return value.trim();
}
function bounded(value: string, max: number): string { return value.trim().slice(0, max); }
function bound(value: number | undefined, fallback: number): number { return value === undefined ? fallback : Math.max(1, Math.min(50, Math.floor(value))); }
function positive(value: unknown, name: string): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 1) throw new Error(`${name} must be a positive integer.`);
  return value;
}
function section(value: unknown, keys: readonly string[], name: string, previous: Record<string, string> = {}): Record<string, string> {
  if (value !== undefined && (!value || typeof value !== "object" || Array.isArray(value))) throw new Error(`${name} must be an object.`);
  const record = (value ?? {}) as Record<string, unknown>;
  return Object.fromEntries(keys.map((key) => {
    const entry = record[key];
    if (entry !== undefined && typeof entry !== "string") throw new Error(`${name}.${key} must be a string.`);
    return [key, entry === undefined ? previous[key] ?? "" : bounded(entry, 4_000)];
  }));
}
function parseSurfaceRanking(value: unknown): SurfaceRanking | null {
  if (value === null) return null;
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("foundation.ranking must be an object.");
  const record = value as Record<string, unknown>;
  if (Object.keys(record).some((key) => !SURFACE_RANKING_FACTORS.includes(key as SurfaceRankingFactor))) {
    throw new Error("foundation.ranking contains an unknown factor.");
  }
  return Object.fromEntries(SURFACE_RANKING_FACTORS.map((factor) => {
    const assessment = record[factor];
    if (!assessment || typeof assessment !== "object" || Array.isArray(assessment)) {
      throw new Error(`foundation.ranking.${factor} must include score and basis.`);
    }
    const item = assessment as Record<string, unknown>;
    if (Object.keys(item).some((key) => key !== "score" && key !== "basis")) {
      throw new Error(`foundation.ranking.${factor} contains an unknown field.`);
    }
    if (!Number.isSafeInteger(item.score) || Number(item.score) < 1 || Number(item.score) > 10) {
      throw new Error(`foundation.ranking.${factor}.score must be an integer from 1 to 10.`);
    }
    const basis = nonempty(item.basis, `foundation.ranking.${factor}.basis`);
    if (basis.length > 500) throw new Error(`foundation.ranking.${factor}.basis must be at most 500 characters.`);
    return [factor, { score: item.score as number, basis }];
  })) as SurfaceRanking;
}

function parseFoundation(value: unknown, previous: SurfaceFoundation | null = null): SurfaceFoundation {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("foundation must be an object.");
  const record = value as Record<string, unknown>;
  if (record.discovery !== undefined && typeof record.discovery !== "string") throw new Error("foundation.discovery must be a string.");
  if (record.threatModelReviewed !== undefined && typeof record.threatModelReviewed !== "boolean") throw new Error("foundation.threatModelReviewed must be a boolean.");
  const threatModel = parseThreatModel(record.threatModel, previous?.threatModel);
  const ranking = record.ranking === undefined ? previous?.ranking : parseSurfaceRanking(record.ranking);
  const scopeAssessment = record.scopeAssessment === undefined
    ? previous?.scopeAssessment : parseScopeAssessment(record.scopeAssessment);
  return {
    discovery: record.discovery === undefined ? previous?.discovery ?? "" : record.discovery.trim(),
    ...(scopeAssessment === undefined ? {} : { scopeAssessment }),
    ...(ranking === undefined ? {} : { ranking }),
    highValueSignals: record.highValueSignals === undefined ? previous?.highValueSignals ?? []
      : parseHighValueSignals(record.highValueSignals),
    techStack: record.techStack === undefined ? previous?.techStack ?? [] : parseTechStack(record.techStack),
    assets: record.assets === undefined ? previous?.assets ?? [] : parseAssets(record.assets),
    bugHistory: parseBugHistory(record.bugHistory, previous?.bugHistory),
    coverage: parseCoverage(record.coverage, previous?.coverage),
    threatModel,
    threatModelReviewed: record.threatModelReviewed === true || previous?.threatModelReviewed === true || threatModelHasContent(threatModel),
  };
}
function parseScopeAssessment(value: unknown): SurfaceScopeAssessment | null {
  if (value === null) return null;
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("foundation.scopeAssessment must be an object or null.");
  const record = value as Record<string, unknown>;
  if (record.decision !== "applicable" && record.decision !== "not_applicable") {
    throw new Error("foundation.scopeAssessment.decision is invalid.");
  }
  const basis = bounded(nonempty(record.basis, "foundation.scopeAssessment.basis"), 1_000);
  if (!Array.isArray(record.references) || record.references.length < 1 || record.references.length > 12) {
    throw new Error("foundation.scopeAssessment.references must contain 1 to 12 citations.");
  }
  const references = record.references.map((reference) => bounded(nonempty(reference, "foundation.scopeAssessment.reference"), 500));
  return { decision: record.decision, basis, references };
}
function parseHighValueSignals(value: unknown): SurfaceFoundation["highValueSignals"] {
  if (!Array.isArray(value) || value.length > 12) throw new Error("foundation.highValueSignals must contain at most 12 entries.");
  const seen = new Set<string>();
  return value.map((entry, index) => {
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) throw new Error(`foundation.highValueSignals[${index}] must be an object.`);
    const signal = entry as Record<string, unknown>;
    if (!HIGH_VALUE_SURFACE_SIGNALS.includes(signal.kind as HighValueSurfaceSignal) || seen.has(String(signal.kind))) {
      throw new Error(`foundation.highValueSignals[${index}].kind is invalid or repeated.`);
    }
    seen.add(String(signal.kind));
    return { kind: signal.kind as HighValueSurfaceSignal,
      basis: bounded(nonempty(signal.basis, `foundation.highValueSignals[${index}].basis`), 500) };
  });
}
function parseThreatModel(value: unknown, previous?: SurfaceFoundation["threatModel"]): SurfaceFoundation["threatModel"] {
  const fields = section(value, ["riskImpact", "invariants", "sinks", "mitigations",
    "attackerControl", "trustBoundaries", "assetsAtRisk", "plausibleImpacts"], "foundation.threatModel",
    previous ? { riskImpact: previous.riskImpact, invariants: previous.invariants, sinks: previous.sinks,
      mitigations: previous.mitigations, attackerControl: previous.attackerControl,
      trustBoundaries: previous.trustBoundaries, assetsAtRisk: previous.assetsAtRisk,
      plausibleImpacts: previous.plausibleImpacts } : undefined);
  const record = (value ?? {}) as Record<string, unknown>;
  if (record.diagram !== undefined && typeof record.diagram !== "string") throw new Error("foundation.threatModel.diagram must be a string.");
  const legacyRisks = record.riskImpact !== undefined ? fields.riskImpact
    : record.plausibleImpacts !== undefined ? fields.plausibleImpacts : undefined;
  const riskImpact = record.riskImpact === undefined
    ? record.plausibleImpacts !== undefined ? fields.plausibleImpacts : previous?.riskImpact || fields.plausibleImpacts
    : fields.riskImpact;
  return {
    diagram: record.diagram === undefined ? previous?.diagram ?? "" : bounded(record.diagram, 50_000),
    securityObjectives: threatModelItems(record.securityObjectives, previous?.securityObjectives, "securityObjectives"),
    risks: threatModelItems(record.risks === undefined ? legacyRisks : record.risks,
      previous?.risks ?? (fields.riskImpact ? [fields.riskImpact] : fields.plausibleImpacts ? [fields.plausibleImpacts] : []), "risks"),
    assumptions: threatModelItems(record.assumptions, previous?.assumptions, "assumptions"),
    riskImpact: riskImpact ?? "",
    invariants: fields.invariants ?? "", sinks: fields.sinks ?? "", mitigations: fields.mitigations ?? "",
    attackerControl: fields.attackerControl ?? "", trustBoundaries: fields.trustBoundaries ?? "",
    assetsAtRisk: fields.assetsAtRisk ?? "", plausibleImpacts: fields.plausibleImpacts ?? "",
  };
}
function threatModelItems(value: unknown, previous: readonly string[] | undefined, name: string): string[] {
  if (value === undefined) return [...(previous ?? [])];
  if (typeof value === "string") return value.trim() ? [bounded(value, 4_000)] : [];
  if (!Array.isArray(value) || value.length > 100) throw new Error(`foundation.threatModel.${name} must be an array with at most 100 entries.`);
  return value.map((item, index) => bounded(nonempty(item, `foundation.threatModel.${name}[${index}]`), 4_000));
}
function parseTechStack(value: unknown): SurfaceFoundation["techStack"] {
  if (!Array.isArray(value) || value.length > 200) throw new Error("foundation.techStack must be an array with at most 200 entries.");
  return value.map((entry, index) => {
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) throw new Error(`foundation.techStack[${index}] must be an object.`);
    const record = entry as Record<string, unknown>;
    const details = section(record, ["role", "version", "environment", "evidence"], `foundation.techStack[${index}]`);
    const confidence = record.confidence ?? "unknown";
    if (confidence !== "observed" && confidence !== "inferred" && confidence !== "unknown") throw new Error(`foundation.techStack[${index}].confidence is invalid.`);
    return { technology: bounded(nonempty(record.technology, `foundation.techStack[${index}].technology`), 500),
      role: details.role ?? "", version: details.version ?? "", environment: details.environment ?? "", evidence: details.evidence ?? "", confidence };
  });
}
function parseAssets(value: unknown): SurfaceFoundation["assets"] {
  if (!Array.isArray(value) || value.length > 500) throw new Error("foundation.assets must be an array with at most 500 entries.");
  return value.map((entry, index) => {
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) throw new Error(`foundation.assets[${index}] must be an object.`);
    const record = entry as Record<string, unknown>;
    const status = record.status ?? "untouched";
    if (status !== "untouched" && status !== "in_progress" && status !== "complete") throw new Error(`foundation.assets[${index}].status is invalid.`);
    return { name: bounded(nonempty(record.name, `foundation.assets[${index}].name`), 1_000), status };
  });
}
function parseBugHistory(value: unknown, previous?: SurfaceFoundation["bugHistory"]): SurfaceFoundation["bugHistory"] {
  const text = section(value, ["summary", "limits"], "foundation.bugHistory",
    previous ? { summary: previous.summary, limits: previous.limits } : undefined);
  const record = (value ?? {}) as Record<string, unknown>;
  const entries = record.entries === undefined ? previous?.entries ?? [] : parseBugHistoryEntries(record.entries);
  return { summary: text.summary ?? "", limits: text.limits ?? "", entries };
}
function parseBugHistoryEntries(value: unknown): SurfaceFoundation["bugHistory"]["entries"] {
  if (!Array.isArray(value) || value.length > 200) throw new Error("foundation.bugHistory.entries must be an array with at most 200 entries.");
  return value.map((entry, index) => {
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) throw new Error(`foundation.bugHistory.entries[${index}] must be an object.`);
    const record = entry as Record<string, unknown>;
    const details = section(record, ["affectedVersions", "reference", "notes"], `foundation.bugHistory.entries[${index}]`);
    return { identifier: bounded(nonempty(record.identifier, `foundation.bugHistory.entries[${index}].identifier`), 500),
      impact: bounded(nonempty(record.impact, `foundation.bugHistory.entries[${index}].impact`), 4_000),
      affectedVersions: details.affectedVersions ?? "", reference: details.reference ?? "", notes: details.notes ?? "" };
  });
}
function parseProgressBlocker(value: unknown): SurfaceProgressBlocker | null {
  if (value === undefined || value === null) return null;
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("Stored surface progress blocker must be an object.");
  const record = value as Record<string, unknown>;
  const setAt = nonempty(record.setAt, "Progress blocker timestamp");
  if (!Number.isFinite(Date.parse(setAt))) throw new Error("Progress blocker timestamp must be a date-time.");
  return { reason: bounded(nonempty(record.reason, "Progress blocker reason"), 2_000),
    actorId: bounded(nonempty(record.actorId, "Progress blocker actor"), 200), setAt };
}
function parseBlockerUpdate(value: unknown): { reason: string } | null | undefined {
  if (value === undefined || value === null) return value;
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("progressBlocker must be an object or null.");
  return { reason: nonempty((value as Record<string, unknown>).reason, "progressBlocker.reason") };
}
function parseCoverage(value: unknown, previous?: SurfaceFoundation["coverage"]): SurfaceFoundation["coverage"] {
  const text = section(value, ["summary", "gaps", "opportunities"], "foundation.coverage", previous ? { summary: previous.summary, gaps: previous.gaps, opportunities: previous.opportunities } : undefined) as Pick<SurfaceFoundation["coverage"], "summary" | "gaps" | "opportunities">;
  const record = (value ?? {}) as Record<string, unknown>;
  if (record.mappingComplete !== undefined && typeof record.mappingComplete !== "boolean") throw new Error("foundation.coverage.mappingComplete must be a boolean.");
  if (record.sinksInvestigated !== undefined && typeof record.sinksInvestigated !== "boolean") throw new Error("foundation.coverage.sinksInvestigated must be a boolean.");
  return {
    ...text,
    mappingComplete: record.mappingComplete === undefined ? previous?.mappingComplete ?? false : record.mappingComplete === true,
    sinksInvestigated: record.sinksInvestigated === undefined ? previous?.sinksInvestigated ?? false : record.sinksInvestigated === true,
    openQuestions: record.openQuestions === undefined ? previous?.openQuestions ?? [] : coverageList(record.openQuestions, "foundation.coverage.openQuestions"),
    unexploredConnections: record.unexploredConnections === undefined ? previous?.unexploredConnections ?? [] : coverageList(record.unexploredConnections, "foundation.coverage.unexploredConnections"),
  };
}
function coverageList(value: unknown, name: string): string[] {
  if (value === undefined) return [];
  if (!Array.isArray(value) || value.length > 100) throw new Error(`${name} must be an array with at most 100 entries.`);
  return [...new Set(value.map((item) => bounded(nonempty(item, name), 500)))];
}
function decodeStoredFoundation(value: unknown): { foundation: SurfaceFoundation; progressBlocker: SurfaceProgressBlocker | null } | null {
  if (typeof value !== "string") return null;
  const parsed = JSON.parse(value) as unknown;
  return { foundation: parseFoundation(parsed), progressBlocker: parseProgressBlocker((parsed as Record<string, unknown>).progressBlocker) };
}
