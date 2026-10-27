import { randomUUID } from "node:crypto";
import type { DatabaseSync } from "node:sqlite";
import { openResearchDatabase } from "./database.js";
import type { ResearchExecutableTool, ResearchToolExecutionContext } from "./tool-registry.js";
import type { ResearchToolAction } from "./types.js";

export type ResearchTrackStatus = "running" | "paused" | "waiting" | "complete" | "stopped";
export type ResearchTrackWorkStatus = "unexamined" | "active" | "deferred" | "blocked" | "complete" | "excluded";
export type ResearchTrackImpact = "unknown" | "low" | "medium" | "high" | "critical";
export type ResearchTrackValidationEnvironment = "device" | "vm" | "web" | "localhost" | "none";
export interface ResearchTrackValidation {
  targetDependent: boolean;
  environment: ResearchTrackValidationEnvironment;
  reason: string;
}

export interface ResearchTrackVector {
  attackerControl: string;
  trustBoundary: string;
  failureMode: string;
  unauthorizedEffect: string;
}

export interface ResearchTrackAngle {
  name: string;
  status: "open" | "supported" | "ruled_out" | "blocked";
  vector: ResearchTrackVector;
  proofQuestion: string;
  evidenceRefs: string[];
  nextAction: string;
}

type StoredResearchTrackAngle = Omit<ResearchTrackAngle, "vector"> & { vector?: ResearchTrackVector };

export interface ResearchTrackSurfaceState {
  surfaceId: string;
  name: string;
  status: ResearchTrackWorkStatus;
  impact: ResearchTrackImpact;
  theory: string;
  nextAction: string;
  evidenceRefs: string[];
  angles: StoredResearchTrackAngle[];
  actionabilityScore: number;
  validation: ResearchTrackValidation | null;
  epochs: number;
  noProgressEpochs: number;
  revisits: number;
  revisitAfter: string | null;
  lastWorkedAt: string | null;
  updatedAt: string;
}

export interface ResearchTrackSnapshot {
  id: string;
  workspaceId: string;
  status: ResearchTrackStatus;
  pass: number;
  scopeId: string;
  provider: { id: string; model?: string; reasoningEffort?: string };
  guardianEnabled: boolean;
  targetIds: string[];
  collaboration?: Record<string, unknown>;
  activeSessionId: string | null;
  activeSurfaceId: string | null;
  epochCount: number;
  lastError: string | null;
  createdAt: string;
  updatedAt: string;
  surfaces: ResearchTrackSurfaceState[];
}

export interface ResearchTrackSurfaceProjection {
  id: string;
  resourceId?: string;
  resourceKind?: string;
  resourceSource?: string;
  parentId: string | null;
  name: string;
  coverageState: { covered: boolean; unmetRequirements: string[]; unblockedFindingIds?: string[] };
  progressBlocker: { reason: string } | null;
}

type Row = Record<string, unknown>;

const IMPACT_RANK: Record<ResearchTrackImpact, number> = { unknown: 2, low: 1, medium: 2, high: 3, critical: 4 };
const INITIAL_ROOT_ACTIONABILITY = 35;
const INITIAL_CHILD_ACTIONABILITY = 55;
const ACTIONABILITY_DECAY_INTERVAL_MS = 6 * 60 * 60_000;

function schedulingScore(work: ResearchTrackSurfaceState): number {
  const testableAngle = work.angles.some((angle) => angle.status === "open" || angle.status === "supported");
  const impact = { unknown: 0, low: -25, medium: 0,
    high: testableAngle ? 30 : 10, critical: testableAngle ? 40 : 20 }[work.impact];
  const targetValidationGap = work.validation?.targetDependent
    && (work.validation.environment === "none" || work.validation.environment === "localhost") ? 50 : 0;
  return work.actionabilityScore + impact + (testableAngle && targetValidationGap === 0 ? 15 : 0)
    - targetValidationGap - work.noProgressEpochs * 10;
}

export function researchTrackProgress(track: Pick<ResearchTrackSnapshot, "surfaces">): {
  investigated: number; complete: number; total: number;
} {
  const included = track.surfaces.filter((surface) => surface.status !== "excluded");
  return {
    investigated: included.filter((surface) => surface.epochs > 0 || surface.status === "complete").length,
    complete: included.filter((surface) => surface.status === "complete").length,
    total: included.length,
  };
}

function decayedActionability(score: number, updatedAt: string, now: number): number {
  const elapsed = now - Date.parse(updatedAt);
  return Math.max(0, Math.min(100, score) - (Number.isFinite(elapsed) && elapsed > 0
    ? Math.floor(elapsed / ACTIONABILITY_DECAY_INTERVAL_MS) * 3 : 0));
}

/** Durable workspace-wide research work. The app-server owns scheduling; model tools only record research. */
export class ResearchTrackStore {
  readonly #database: DatabaseSync;
  readonly #workspaceId: string;
  readonly #sessionId: string | null;

  public constructor(input: { databasePath: string; workspaceId: string; sessionId?: string }) {
    this.#database = openResearchDatabase(input.databasePath);
    this.#workspaceId = required(input.workspaceId, "workspaceId");
    this.#sessionId = input.sessionId ?? null;
    this.#database.exec(`
      CREATE TABLE IF NOT EXISTS app_server_research_tracks (
        workspace_id TEXT PRIMARY KEY, id TEXT NOT NULL UNIQUE, status TEXT NOT NULL,
        pass INTEGER NOT NULL, scope_id TEXT NOT NULL, provider_json TEXT NOT NULL,
        target_ids_json TEXT NOT NULL, collaboration_json TEXT, guardian_enabled INTEGER NOT NULL DEFAULT 1,
        active_session_id TEXT, active_surface_id TEXT,
        epoch_count INTEGER NOT NULL, last_surface_id TEXT, consecutive_surface_epochs INTEGER NOT NULL,
        last_error TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL
      );
      CREATE TABLE IF NOT EXISTS app_server_research_track_surfaces (
        workspace_id TEXT NOT NULL, surface_id TEXT NOT NULL, name TEXT NOT NULL, status TEXT NOT NULL,
        impact TEXT NOT NULL, theory TEXT NOT NULL, next_action TEXT NOT NULL,
        evidence_refs_json TEXT NOT NULL, epochs INTEGER NOT NULL, no_progress_epochs INTEGER NOT NULL,
        revisits INTEGER NOT NULL, revisit_after TEXT, last_worked_at TEXT, updated_at TEXT NOT NULL,
        actionability_score INTEGER NOT NULL DEFAULT 35, score_updated_at TEXT, validation_json TEXT,
        angles_json TEXT NOT NULL DEFAULT '[]',
        PRIMARY KEY(workspace_id, surface_id)
      );
      CREATE TABLE IF NOT EXISTS app_server_research_track_history (
        id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, pass INTEGER NOT NULL,
        snapshot_json TEXT NOT NULL, saved_at TEXT NOT NULL
      );
    `);
    const columns = this.#database.prepare("PRAGMA table_info(app_server_research_tracks)").all() as Array<{ name: string }>;
    if (!columns.some((column) => column.name === "collaboration_json")) {
      this.#database.exec("ALTER TABLE app_server_research_tracks ADD COLUMN collaboration_json TEXT");
    }
    if (!columns.some((column) => column.name === "guardian_enabled")) {
      this.#database.exec("ALTER TABLE app_server_research_tracks ADD COLUMN guardian_enabled INTEGER NOT NULL DEFAULT 1");
    }
    const surfaceColumns = this.#database.prepare("PRAGMA table_info(app_server_research_track_surfaces)").all() as Array<{ name: string }>;
    if (!surfaceColumns.some((column) => column.name === "actionability_score")) {
      this.#database.exec("ALTER TABLE app_server_research_track_surfaces ADD COLUMN actionability_score INTEGER NOT NULL DEFAULT 35");
      if (this.#database.prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_server_surfaces'").get()) {
        this.#database.prepare(`UPDATE app_server_research_track_surfaces SET actionability_score=?
          WHERE epochs=0 AND EXISTS (SELECT 1 FROM app_server_surfaces
            WHERE id=app_server_research_track_surfaces.surface_id
              AND workspace_id=app_server_research_track_surfaces.workspace_id AND parent_id IS NOT NULL)`)
          .run(INITIAL_CHILD_ACTIONABILITY);
      }
    }
    if (!surfaceColumns.some((column) => column.name === "score_updated_at")) {
      this.#database.exec("ALTER TABLE app_server_research_track_surfaces ADD COLUMN score_updated_at TEXT");
      this.#database.prepare(`UPDATE app_server_research_track_surfaces SET score_updated_at=?
        WHERE score_updated_at IS NULL`).run(new Date().toISOString());
    }
    if (!surfaceColumns.some((column) => column.name === "validation_json")) {
      this.#database.exec("ALTER TABLE app_server_research_track_surfaces ADD COLUMN validation_json TEXT");
    }
    if (!surfaceColumns.some((column) => column.name === "angles_json")) {
      this.#database.exec("ALTER TABLE app_server_research_track_surfaces ADD COLUMN angles_json TEXT NOT NULL DEFAULT '[]'");
    }
  }

  public close(): void { this.#database.close(); }

  public get(): ResearchTrackSnapshot | null {
    const row = this.#database.prepare("SELECT * FROM app_server_research_tracks WHERE workspace_id = ?")
      .get(this.#workspaceId) as Row | undefined;
    if (!row) return null;
    const surfaces = this.#database.prepare("SELECT * FROM app_server_research_track_surfaces WHERE workspace_id = ? ORDER BY updated_at, surface_id")
      .all(this.#workspaceId) as Row[];
    return {
      id: String(row.id), workspaceId: this.#workspaceId, status: row.status as ResearchTrackStatus,
      pass: Number(row.pass), scopeId: String(row.scope_id),
      provider: JSON.parse(String(row.provider_json)) as ResearchTrackSnapshot["provider"],
      guardianEnabled: Number(row.guardian_enabled) !== 0,
      targetIds: JSON.parse(String(row.target_ids_json)) as string[],
      ...(row.collaboration_json ? { collaboration: JSON.parse(String(row.collaboration_json)) as Record<string, unknown> } : {}),
      activeSessionId: nullable(row.active_session_id), activeSurfaceId: nullable(row.active_surface_id),
      epochCount: Number(row.epoch_count), lastError: nullable(row.last_error),
      createdAt: String(row.created_at), updatedAt: String(row.updated_at),
      surfaces: surfaces.map((surface) => ({
        surfaceId: String(surface.surface_id), name: String(surface.name), status: surface.status as ResearchTrackWorkStatus,
        impact: surface.impact as ResearchTrackImpact, theory: String(surface.theory),
        nextAction: String(surface.next_action), evidenceRefs: JSON.parse(String(surface.evidence_refs_json)) as string[],
        angles: JSON.parse(String(surface.angles_json)) as StoredResearchTrackAngle[],
        epochs: Number(surface.epochs), noProgressEpochs: Number(surface.no_progress_epochs),
        actionabilityScore: decayedActionability(Number(surface.actionability_score),
          nullable(surface.score_updated_at) ?? String(surface.updated_at), Date.now()),
        validation: surface.validation_json ? JSON.parse(String(surface.validation_json)) as ResearchTrackValidation : null,
        revisits: Number(surface.revisits), revisitAfter: nullable(surface.revisit_after),
        lastWorkedAt: nullable(surface.last_worked_at), updatedAt: String(surface.updated_at),
      })),
    };
  }

  public currentAssignment(): { track: ResearchTrackSnapshot; surface: ResearchTrackSurfaceState } {
    const track = this.get();
    const surface = track?.surfaces.find((work) => work.surfaceId === track.activeSurfaceId);
    if (!track || !surface || !this.#sessionId || track.activeSessionId !== this.#sessionId) {
      throw new Error("The active research track assigned a different surface to this session.");
    }
    return { track, surface };
  }

  public start(input: {
    scopeId: string;
    provider: ResearchTrackSnapshot["provider"];
    guardianEnabled?: boolean;
    targetIds: string[];
    collaboration?: Record<string, unknown>;
    surfaces: readonly ResearchTrackSurfaceProjection[];
    replaceUnfinished?: boolean;
  }): ResearchTrackSnapshot {
    if (input.surfaces.length === 0) throw new Error("A research track requires an eligible scope surface.");
    if (input.targetIds.length === 0 || input.targetIds.length > 16
      || new Set(input.targetIds).size !== input.targetIds.length) {
      throw new Error("A research track requires 1 to 16 distinct research targets.");
    }
    const existing = this.get();
    if (existing?.status === "running" || existing?.activeSessionId && existing.status !== "stopped"
      || (existing?.status === "paused" || existing?.status === "waiting") && !input.replaceUnfinished) {
      throw new Error("This workspace already has an unfinished research track.");
    }
    const now = new Date().toISOString();
    const id = `track_${randomUUID().replaceAll("-", "")}`;
    this.#database.exec("BEGIN IMMEDIATE");
    try {
      if (existing) this.#database.prepare(`INSERT INTO app_server_research_track_history
        (id, workspace_id, pass, snapshot_json, saved_at) VALUES (?, ?, ?, ?, ?)`)
        .run(`track_history_${randomUUID().replaceAll("-", "")}`, this.#workspaceId,
          existing.pass, JSON.stringify(existing), now);
      this.#database.prepare(`INSERT INTO app_server_research_tracks
        (workspace_id, id, status, pass, scope_id, provider_json, target_ids_json, collaboration_json, guardian_enabled, active_session_id,
         active_surface_id, epoch_count, last_surface_id, consecutive_surface_epochs, last_error, created_at, updated_at)
        VALUES (?, ?, 'running', ?, ?, ?, ?, ?, ?, NULL, NULL, 0, NULL, 0, NULL, ?, ?)
        ON CONFLICT(workspace_id) DO UPDATE SET id=excluded.id, status='running', pass=excluded.pass,
          scope_id=excluded.scope_id, provider_json=excluded.provider_json,
          target_ids_json=excluded.target_ids_json, collaboration_json=excluded.collaboration_json,
          guardian_enabled=excluded.guardian_enabled,
          active_session_id=NULL, active_surface_id=NULL,
          epoch_count=0, last_surface_id=NULL, consecutive_surface_epochs=0,
          last_error=NULL, created_at=excluded.created_at, updated_at=excluded.updated_at`)
        .run(this.#workspaceId, id, (existing?.pass ?? 0) + 1, required(input.scopeId, "scopeId"),
          JSON.stringify(input.provider), JSON.stringify(input.targetIds),
          input.collaboration ? JSON.stringify(input.collaboration) : null, input.guardianEnabled === false ? 0 : 1, now, now);
      if (existing) this.#database.prepare("DELETE FROM app_server_research_track_surfaces WHERE workspace_id = ?").run(this.#workspaceId);
      this.#insertSurfaces(input.surfaces, now);
      this.#database.exec("COMMIT");
    } catch (error) { this.#database.exec("ROLLBACK"); throw error; }
    return this.get()!;
  }

  public syncSurfaces(surfaces: readonly ResearchTrackSurfaceProjection[]): ResearchTrackSnapshot | null {
    const track = this.get();
    if (!track) return null;
    const now = new Date().toISOString();
    const visibleIds = new Set(surfaces.map((surface) => surface.id));
    const newSurface = surfaces.some((surface) => !track.surfaces.some((work) => work.surfaceId === surface.id));
    this.#insertSurfaces(surfaces, now);
    for (const work of track.surfaces) {
      if (!visibleIds.has(work.surfaceId) && work.status !== "excluded") {
        this.#database.prepare(`UPDATE app_server_research_track_surfaces SET status='excluded', updated_at=?
          WHERE workspace_id=? AND surface_id=?`).run(now, this.#workspaceId, work.surfaceId);
      }
    }
    for (const surface of surfaces) {
      this.#database.prepare(`UPDATE app_server_research_track_surfaces SET name=?
        WHERE workspace_id=? AND surface_id=?`).run(surface.name, this.#workspaceId, surface.id);
      const work = track.surfaces.find((item) => item.surfaceId === surface.id);
      if (!work) continue;
      if (work.status === "excluded") {
        this.#database.prepare(`UPDATE app_server_research_track_surfaces SET status=?, updated_at=?
          WHERE workspace_id=? AND surface_id=?`).run(surface.progressBlocker ? "blocked" : "deferred", now, this.#workspaceId, surface.id);
      } else if (surface.progressBlocker && work.status !== "blocked") {
        this.#database.prepare(`UPDATE app_server_research_track_surfaces SET status='blocked', updated_at=?
          WHERE workspace_id=? AND surface_id=?`).run(now, this.#workspaceId, surface.id);
      } else if (!surface.progressBlocker && work.status === "blocked") {
        this.#database.prepare(`UPDATE app_server_research_track_surfaces SET status='deferred', updated_at=?
          WHERE workspace_id=? AND surface_id=?`).run(now, this.#workspaceId, surface.id);
      } else if (work.status === "complete" && !this.#coverageSatisfied(surface, track)) {
        this.#database.prepare(`UPDATE app_server_research_track_surfaces SET status='deferred', next_action=?, updated_at=?
          WHERE workspace_id=? AND surface_id=?`).run("Revisit changed or incomplete security coverage.", now, this.#workspaceId, surface.id);
      }
    }
    if (newSurface && track.status === "complete") this.pauseWithError("A new eligible security surface appeared. Resume for another pass.");
    return this.get();
  }

  public control(action: "pause" | "resume" | "stop"): ResearchTrackSnapshot {
    const track = this.get();
    if (!track) throw new Error("No research track exists in this workspace.");
    if (action === "resume" && track.status !== "paused" && track.status !== "waiting") {
      throw new Error("Only a paused or waiting research track can resume.");
    }
    if (action === "pause" && track.status !== "running") throw new Error("Only a running research track can pause.");
    if (action === "stop") {
      this.#database.exec("BEGIN IMMEDIATE");
      try {
        if (track.activeSurfaceId) {
          this.#database.prepare(`UPDATE app_server_research_track_surfaces SET status='deferred', updated_at=?
            WHERE workspace_id=? AND surface_id=? AND status='active'`)
            .run(new Date().toISOString(), this.#workspaceId, track.activeSurfaceId);
        }
        this.#database.prepare(`UPDATE app_server_research_tracks SET status='stopped', active_session_id=NULL,
          active_surface_id=NULL, last_error=NULL, updated_at=? WHERE workspace_id=?`)
          .run(new Date().toISOString(), this.#workspaceId);
        this.#database.exec("COMMIT");
      } catch (error) { this.#database.exec("ROLLBACK"); throw error; }
      return this.get()!;
    }
    const status = action === "resume" ? "running" : "paused";
    this.#database.prepare(`UPDATE app_server_research_tracks SET status=?,
      last_error=CASE WHEN ? THEN NULL ELSE last_error END, updated_at=? WHERE workspace_id=?`)
      .run(status, action === "resume" ? 1 : 0, new Date().toISOString(), this.#workspaceId);
    return this.get()!;
  }

  public pauseWithError(reason: string): ResearchTrackSnapshot | null {
    if (!this.get()) return null;
    this.#database.prepare(`UPDATE app_server_research_tracks SET status='paused', last_error=?, updated_at=?
      WHERE workspace_id=?`).run(bounded(required(reason, "reason"), 1_000), new Date().toISOString(), this.#workspaceId);
    return this.get();
  }

  public reserveNext(sessionId: string, surfaces: readonly ResearchTrackSurfaceProjection[]): { track: ResearchTrackSnapshot; surface: ResearchTrackSurfaceProjection } | null {
    const track = this.syncSurfaces(surfaces);
    if (!track || track.status !== "running" || track.activeSessionId) return null;
    const byId = new Map(surfaces.map((surface) => [surface.id, surface]));
    const actionable = track.surfaces.filter((work) => {
      const surface = byId.get(work.surfaceId);
      if (!surface || work.status === "complete" || work.status === "blocked" || work.status === "excluded") return false;
      if (surface.parentId === null && surface.coverageState.unmetRequirements.length > 0
        && surface.coverageState.unmetRequirements.every((item) => item === "child_surfaces")
        && track.surfaces.some((child) => byId.get(child.surfaceId)?.parentId === surface.id
          && child.status !== "complete" && child.status !== "excluded")) return false;
      return true;
    });
    const ready = actionable.filter((work) => !work.revisitAfter || work.revisitAfter <= new Date().toISOString());
    // A second look should follow other available work, but never leave a
    // running track idle solely because its remaining revisit time is future.
    const eligible = ready.length > 0 ? ready : actionable;
    const workById = new Map(track.surfaces.map((work) => [work.surfaceId, work]));
    const score = (work: ResearchTrackSurfaceState): number => {
      const surface = byId.get(work.surfaceId);
      const parent = surface?.parentId ? workById.get(surface.parentId) : null;
      const recentParent = parent?.lastWorkedAt && Date.now() - Date.parse(parent.lastWorkedAt) < 60 * 60_000;
      return schedulingScore(work) + (surface?.parentId && work.epochs === 0 && recentParent ? 10 : 0);
    };
    const rank = (left: ResearchTrackSurfaceState, right: ResearchTrackSurfaceState): number =>
      score(right) - score(left)
      || (left.lastWorkedAt ?? "").localeCompare(right.lastWorkedAt ?? "")
      || left.surfaceId.localeCompare(right.surfaceId);
    const resourceIdForSurface = (surfaceId: string): string => {
      const surface = byId.get(surfaceId);
      if (surface?.resourceId) return surface.resourceId;
      let root = surface;
      const seen = new Set<string>();
      while (root?.parentId && !seen.has(root.id)) {
        seen.add(root.id);
        root = byId.get(root.parentId);
      }
      return root?.id ?? surfaceId;
    };
    const resourceId = (work: ResearchTrackSurfaceState): string => resourceIdForSurface(work.surfaceId);
    const resourceKind = (work: ResearchTrackSurfaceState): string =>
      byId.get(work.surfaceId)?.resourceKind ?? byId.get(resourceId(work))?.resourceKind ?? "unknown";
    const resourceSource = (work: ResearchTrackSurfaceState): string | undefined =>
      byId.get(work.surfaceId)?.resourceSource ?? byId.get(resourceId(work))?.resourceSource;
    const kindEpochs = new Map<string, number>();
    const resourceEpochs = new Map<string, number>();
    for (const work of track.surfaces) {
      if (work.epochs === 0 || !byId.has(work.surfaceId)) continue;
      const kind = resourceKind(work);
      kindEpochs.set(kind, (kindEpochs.get(kind) ?? 0) + work.epochs);
      const id = resourceId(work);
      resourceEpochs.set(id, (resourceEpochs.get(id) ?? 0) + work.epochs);
    }
    const breadthScore = (work: ResearchTrackSurfaceState): number => {
      const kind = resourceKind(work);
      const kindBonus = kind === "unknown" ? 0 : Math.max(0, 30 - (kindEpochs.get(kind) ?? 0) * 6);
      const alternateKindBonus = kind !== "repository" && kind !== "unknown" ? 8 : 0;
      const executionBonus = kind === "binary" || kind === "service" ? 8 : 0;
      return score(work) + kindBonus + alternateKindBonus + executionBonus
        + (resourceSource(work) === "explicit_scope" ? 12 : 0);
    };
    const breadthRank = (left: ResearchTrackSurfaceState, right: ResearchTrackSurfaceState): number =>
      breadthScore(right) - breadthScore(left) || rank(left, right);
    const breadth = eligible.filter((work) => !byId.get(work.surfaceId)?.parentId && work.epochs === 0).sort(breadthRank);
    const depth = eligible.filter((work) => byId.get(work.surfaceId)?.parentId || work.epochs > 0).sort(rank);
    const strongestDepth = depth[0];
    const unexploredKind = breadth.some((work) => resourceKind(work) !== "unknown"
      && !kindEpochs.has(resourceKind(work)));
    const kindStarvation = track.epochCount >= 3 && kindEpochs.size === 1 && unexploredKind;
    // Keep three epochs available for depth, then give another resource a turn.
    // Escape a one-kind streak immediately when a reviewed new kind appears.
    const explorationDue = track.epochCount >= 3 && (track.epochCount % 4 === 3 || kindStarvation);
    const exploreBreadth = breadth.length > 0 && (!strongestDepth || score(strongestDepth) < 45 || explorationDue);
    const history = this.#database.prepare("SELECT last_surface_id, consecutive_surface_epochs FROM app_server_research_tracks WHERE workspace_id=?")
      .get(this.#workspaceId) as { last_surface_id: string | null; consecutive_surface_epochs: number };
    const lastResourceId = history.last_surface_id ? resourceIdForSurface(history.last_surface_id) : null;
    const alternateResources = eligible.filter((work) => resourceId(work) !== lastResourceId).sort((left, right) =>
      (resourceEpochs.get(resourceId(left)) ?? 0) - (resourceEpochs.get(resourceId(right)) ?? 0)
      || (kindEpochs.get(resourceKind(left)) ?? 0) - (kindEpochs.get(resourceKind(right)) ?? 0)
      || breadthRank(left, right));
    const candidates = exploreBreadth ? breadth
      : explorationDue && breadth.length === 0 && alternateResources.length > 0 ? alternateResources
        : depth.length > 0 ? depth : breadth;
    const selected = history.consecutive_surface_epochs >= 2 && candidates.length > 1 && score(candidates[0]!) < 80
      ? candidates.find((candidate) => candidate.surfaceId !== history.last_surface_id) ?? candidates[0]
      : candidates[0];
    if (!selected) {
      const status = track.surfaces.every((work) => work.status === "complete" || work.status === "excluded") ? "complete"
        : track.surfaces.some((work) => work.status === "blocked") ? "waiting" : "paused";
      const error = status === "paused" ? "No actionable surface could be selected; review remaining coverage gaps." : null;
      this.#database.prepare("UPDATE app_server_research_tracks SET status=?, last_error=?, updated_at=? WHERE workspace_id=?")
        .run(status, error, new Date().toISOString(), this.#workspaceId);
      return null;
    }
    const now = new Date().toISOString();
    this.#database.exec("BEGIN IMMEDIATE");
    try {
      const reserved = this.#database.prepare(`UPDATE app_server_research_tracks SET active_session_id=?, active_surface_id=?,
        epoch_count=epoch_count+1, last_surface_id=?,
        consecutive_surface_epochs=CASE WHEN last_surface_id=? THEN consecutive_surface_epochs+1 ELSE 1 END,
        updated_at=? WHERE workspace_id=? AND status='running' AND active_session_id IS NULL`)
        .run(required(sessionId, "sessionId"), selected.surfaceId, selected.surfaceId, selected.surfaceId, now, this.#workspaceId).changes;
      if (!reserved) { this.#database.exec("ROLLBACK"); return null; }
      this.#database.prepare(`UPDATE app_server_research_track_surfaces SET status='active', epochs=epochs+1,
        last_worked_at=?, updated_at=? WHERE workspace_id=? AND surface_id=?`)
        .run(now, now, this.#workspaceId, selected.surfaceId);
      this.#database.exec("COMMIT");
    } catch (error) { this.#database.exec("ROLLBACK"); throw error; }
    return { track: this.get()!, surface: byId.get(selected.surfaceId)! };
  }

  public checkpoint(input: {
    surfaceId: string;
    outcome: "continue" | "defer" | "blocked" | "complete";
    impact: ResearchTrackImpact;
    theory: string;
    nextAction: string;
    evidenceRefs: string[];
    angles?: ResearchTrackAngle[];
    validation?: ResearchTrackValidation;
  }, surface: ResearchTrackSurfaceProjection): ResearchTrackSurfaceState {
    const track = this.get();
    if (!track || !this.#sessionId || track.activeSessionId !== this.#sessionId
      || track.activeSurfaceId !== input.surfaceId || surface.id !== input.surfaceId) {
      throw new Error("The active research track assigned a different surface to this session.");
    }
    if (input.outcome === "complete" && (!this.#coverageSatisfied(surface, track) || input.evidenceRefs.length === 0)) {
      throw new Error("A surface needs complete threat-model coverage, report-ready findings, and durable evidence before track completion.");
    }
    if (input.outcome === "blocked" && !surface.progressBlocker) {
      throw new Error("Set a concrete surface progress blocker before marking track work blocked.");
    }
    const evidenceRefs = [...new Set(input.evidenceRefs.map((ref) => required(ref, "evidenceRef")))].slice(0, 32);
    const work = track.surfaces.find((item) => item.surfaceId === input.surfaceId)!;
    if (evidenceRefs.some((ref) => !this.#validEvidenceRef(ref))) {
      throw new Error("Track checkpoints may cite only existing canonical evidence in this workspace.");
    }
    const angles = this.#mergeAngles(work.angles, input.angles, evidenceRefs);
    if (input.outcome === "complete" && !evidenceRefs.some((ref) => this.#validEvidenceRef(ref, work.lastWorkedAt))) {
      throw new Error("Completed track work must cite canonical evidence updated during this research epoch.");
    }
    if (input.outcome === "complete" && (input.impact === "high" || input.impact === "critical")
      && !evidenceRefs.some((ref) => ref.startsWith("finding:"))
      && angles.filter((angle) => angle.status === "supported" || angle.status === "ruled_out").length < 2) {
      throw new Error("Complete high-impact track work only after testing two distinct attacker paths or citing a finding.");
    }
    const secondLook = input.outcome === "complete" && (input.impact === "high" || input.impact === "critical") && work.revisits === 0;
    const status = input.outcome === "continue" || input.outcome === "defer" || secondLook ? "deferred" : input.outcome;
    const prior = work;
    const now = new Date().toISOString();
    const revisitAfter = secondLook ? new Date(Date.now() + 30 * 60_000).toISOString() : null;
    const nextAction = secondLook
      ? `Revisit this high-impact surface from a different attacker path after other surface work. ${input.nextAction}`
      : input.nextAction;
    const freshEvidence = evidenceRefs.some((ref) => this.#validEvidenceRef(ref, work.lastWorkedAt));
    const changed = freshEvidence || status === "complete" || status === "blocked";
    const validation = input.validation ?? null;
    if (validation) {
      if (typeof validation.targetDependent !== "boolean"
        || !["device", "vm", "web", "localhost", "none"].includes(validation.environment)
        || typeof validation.reason !== "string" || !validation.reason.trim() || validation.reason.length > 1_000) {
        throw new Error("Track validation needs an environment, target-dependence decision, and concrete reason.");
      }
      if (validation.targetDependent && (validation.environment === "localhost" || validation.environment === "none")
        && validation.reason.trim().length < 20) {
        throw new Error("Explain the target-validation gap or why target testing was not possible.");
      }
      if (input.outcome === "complete" && validation.targetDependent
        && (validation.environment === "localhost" || validation.environment === "none")) {
        throw new Error("Target-dependent work cannot complete from localhost or untested behavior alone.");
      }
    }
    const evidenceGain = validation?.targetDependent && (validation.environment === "localhost" || validation.environment === "none")
      ? 5 : validation?.targetDependent === false ? 15 : 25;
    const actionabilityScore = status === "blocked" ? 0 : status === "complete" ? 100
      : Math.max(0, Math.min(100, work.actionabilityScore + (freshEvidence
        ? evidenceGain + (IMPACT_RANK[input.impact] - 2) * 8 : -18)));
    this.#database.prepare(`UPDATE app_server_research_track_surfaces SET status=?, impact=?, theory=?, next_action=?,
      evidence_refs_json=?, angles_json=?, no_progress_epochs=?, revisits=?, revisit_after=?, actionability_score=?, score_updated_at=?, validation_json=?, updated_at=?
      WHERE workspace_id=? AND surface_id=?`)
      .run(status, input.impact, bounded(input.theory, 2_000), bounded(nextAction, 2_000),
        JSON.stringify(evidenceRefs), JSON.stringify(angles), changed ? 0 : prior.noProgressEpochs + 1,
        input.outcome === "complete" ? prior.revisits + 1 : prior.revisits,
        revisitAfter, actionabilityScore, now, validation ? JSON.stringify(validation) : null, now, this.#workspaceId, input.surfaceId);
    return this.get()!.surfaces.find((work) => work.surfaceId === input.surfaceId)!;
  }

  #mergeAngles(previous: readonly StoredResearchTrackAngle[], updates: readonly ResearchTrackAngle[] | undefined,
    checkpointEvidence: readonly string[]): StoredResearchTrackAngle[] {
    if (updates === undefined) return [...previous];
    if (!Array.isArray(updates) || updates.length < 1 || updates.length > 4) {
      throw new Error("Record 1 to 4 distinct attacker paths at each track checkpoint.");
    }
    const merged = new Map(previous.map((angle) => [angle.name.toLowerCase(), angle]));
    const seen = new Set<string>();
    for (const angle of updates) {
      if (!angle || typeof angle !== "object") throw new Error("A track attacker path is malformed.");
      const name = required(angle.name, "angle name");
      const vector = angle.vector;
      if (!vector || typeof vector !== "object" || Array.isArray(vector)) {
        throw new Error("A track attacker path needs an attacker-control-to-impact vector.");
      }
      const attackerControl = required(vector.attackerControl, "angle vector.attackerControl");
      const trustBoundary = required(vector.trustBoundary, "angle vector.trustBoundary");
      const failureMode = required(vector.failureMode, "angle vector.failureMode");
      const unauthorizedEffect = required(vector.unauthorizedEffect, "angle vector.unauthorizedEffect");
      const proofQuestion = required(angle.proofQuestion, "angle proofQuestion");
      const nextAction = required(angle.nextAction, "angle nextAction");
      if (name.length > 160 || [attackerControl, trustBoundary, failureMode, unauthorizedEffect].some((part) => part.length > 500)
        || proofQuestion.length > 500 || nextAction.length > 500
        || !["open", "supported", "ruled_out", "blocked"].includes(angle.status)
        || !Array.isArray(angle.evidenceRefs) || angle.evidenceRefs.length > 8
        || !angle.evidenceRefs.every((ref: unknown) => typeof ref === "string" && ref.length <= 168)) {
        throw new Error("A track attacker path exceeds its bounds or has an invalid status.");
      }
      const key = name.toLowerCase();
      if (seen.has(key)) throw new Error("A track checkpoint cannot repeat an attacker path.");
      seen.add(key);
      const refs = [...new Set(angle.evidenceRefs as string[])];
      if (refs.some((ref) => !checkpointEvidence.includes(ref) || !this.#validEvidenceRef(ref))) {
        throw new Error("Track attacker path evidence must be cited by the checkpoint and exist in this workspace.");
      }
      if ((angle.status === "supported" || angle.status === "ruled_out") && refs.length === 0) {
        throw new Error("A tested attacker path needs canonical evidence; record negative evidence in memory.");
      }
      merged.set(key, { name, status: angle.status,
        vector: { attackerControl, trustBoundary, failureMode, unauthorizedEffect },
        proofQuestion, evidenceRefs: refs, nextAction });
    }
    if (merged.size > 24) throw new Error("A track surface can retain at most 24 attacker paths.");
    const vectorKeys = new Set<string>();
    for (const angle of merged.values()) {
      if (!angle.vector) continue;
      const signature = [angle.vector.attackerControl, angle.vector.trustBoundary,
        angle.vector.failureMode, angle.vector.unauthorizedEffect]
        .map((part) => part.trim().toLowerCase().replace(/\s+/gu, " ")).join("|");
      if (vectorKeys.has(signature)) throw new Error("Track attacker paths must have distinct causal vectors, not renamed variants.");
      vectorKeys.add(signature);
    }
    return [...merged.values()];
  }

  public finishEpoch(sessionId: string, succeeded: boolean, diagnostic?: string,
    options: { countNoProgress?: boolean } = {}): ResearchTrackSnapshot | null {
    const track = this.get();
    if (!track || track.activeSessionId !== sessionId) return track;
    const work = track.surfaces.find((item) => item.surfaceId === track.activeSurfaceId);
    const now = new Date().toISOString();
    const noProgressIncrement = options.countNoProgress === false ? 0 : 1;
    if (work?.status === "active") {
      this.#database.prepare(`UPDATE app_server_research_track_surfaces SET status='deferred', no_progress_epochs=no_progress_epochs+?,
        actionability_score=?, score_updated_at=?, next_action=?, updated_at=? WHERE workspace_id=? AND surface_id=?`)
        .run(noProgressIncrement, Math.max(0, work.actionabilityScore - 20), now,
          "Resume the assigned threat-model investigation and record a durable checkpoint.", now, this.#workspaceId, work.surfaceId);
    }
    const noProgressEpochs = work ? work.noProgressEpochs + (work.status === "active" ? noProgressIncrement : 0) : 0;
    const repeatedNoProgress = work?.status !== "excluded" && noProgressEpochs >= 3;
    const otherSurfaceAvailable = track.surfaces.some((surface) => surface.surfaceId !== work?.surfaceId
      && surface.status !== "complete" && surface.status !== "blocked" && surface.status !== "excluded");
    const pauseForNoProgress = repeatedNoProgress && !otherSurfaceAvailable;
    this.#database.prepare(`UPDATE app_server_research_tracks SET active_session_id=NULL, active_surface_id=NULL,
      status=CASE WHEN ? THEN 'paused' ELSE status END,
      last_error=CASE WHEN status='paused' THEN last_error ELSE ? END,
      updated_at=? WHERE workspace_id=?`)
      .run(pauseForNoProgress ? 1 : 0,
        pauseForNoProgress ? "Three epochs on one surface produced no durable progress. Review its theory or scope before resuming."
          : succeeded ? null : bounded(diagnostic ?? "The research epoch ended before completion.", 1_000), now, this.#workspaceId);
    return this.get();
  }

  #insertSurfaces(surfaces: readonly ResearchTrackSurfaceProjection[], now: string): void {
    const insert = this.#database.prepare(`INSERT OR IGNORE INTO app_server_research_track_surfaces
      (workspace_id, surface_id, name, status, impact, theory, next_action, evidence_refs_json,
       epochs, no_progress_epochs, revisits, revisit_after, last_worked_at, updated_at,
       actionability_score, score_updated_at)
      VALUES (?, ?, ?, 'unexamined', 'unknown', '', 'Map the attacker path and highest-impact plausible effect.', '[]', 0, 0, 0, NULL, NULL, ?, ?, ?)`);
    for (const surface of surfaces) insert.run(this.#workspaceId, surface.id, surface.name, now,
      surface.parentId ? INITIAL_CHILD_ACTIONABILITY : INITIAL_ROOT_ACTIONABILITY, now);
  }

  #validEvidenceRef(ref: string, since?: string | null): boolean {
    const match = /^(finding|memory|runbook|report):([A-Za-z0-9_-]{1,160})$/u.exec(ref);
    if (!match) return false;
    const kind = match[1] as "finding" | "memory" | "runbook" | "report";
    const table = { finding: "app_server_research_claims", memory: "memory_nodes",
      runbook: "app_server_runbooks", report: "app_server_reports" }[kind];
    if (!this.#database.prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?").get(table)) return false;
    if (kind === "memory" && !this.#database.prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='memory_node_workspaces'").get()) return false;
    const query = kind === "memory"
      ? `SELECT node.updated_at FROM memory_nodes node
          JOIN memory_node_workspaces membership ON membership.node_id=node.id
          WHERE node.id=? AND membership.workspace_id=? LIMIT 1`
      : `SELECT updated_at FROM ${table} WHERE id=? AND workspace_id=? LIMIT 1`;
    const row = this.#database.prepare(query)
      .get(match[2]!, this.#workspaceId) as { updated_at?: unknown } | undefined;
    return typeof row?.updated_at === "string" && (!since || row.updated_at >= since);
  }

  #coverageSatisfied(surface: ResearchTrackSurfaceProjection, track: ResearchTrackSnapshot): boolean {
    if (surface.coverageState.covered) return true;
    if (surface.coverageState.unmetRequirements.length === 0) return false;
    return surface.coverageState.unmetRequirements.every((requirement) => {
      if (requirement === "unblocked_findings") {
        const ids = surface.coverageState.unblockedFindingIds ?? [];
        if (ids.length === 0) return false;
        return ids.every((id) => {
          const row = this.#database.prepare(`SELECT status FROM app_server_research_claims
            WHERE id=? AND workspace_id=?`).get(id, this.#workspaceId) as { status?: unknown } | undefined;
          return row?.status === "report_ready" || row?.status === "disclosed";
        });
      }
      if (requirement === "child_surfaces") {
        const children = this.#database.prepare(`SELECT id FROM app_server_surfaces
          WHERE workspace_id=? AND parent_id=?`).all(this.#workspaceId, surface.id) as { id: string }[];
        return children.length > 0 && children.every((child) => track.surfaces.some((work) =>
          work.surfaceId === child.id && (work.status === "complete" || work.status === "excluded")));
      }
      return false;
    });
  }
}

export function createResearchTrackTool(store: ResearchTrackStore, surface: (id: string) => ResearchTrackSurfaceProjection): ResearchExecutableTool {
  const parameters = { type: "object", required: ["surfaceId", "outcome", "impact", "theory", "nextAction", "evidenceRefs", "angles", "validation"],
    additionalProperties: false, properties: {
      surfaceId: { type: "string" }, outcome: { type: "string", enum: ["continue", "defer", "blocked", "complete"] },
      impact: { type: "string", enum: ["unknown", "low", "medium", "high", "critical"] },
      theory: { type: "string", description: "The concrete attacker-control-to-impact theory tested this epoch, including contrary evidence." },
      nextAction: { type: "string", description: "The next positive proof step, revisit trigger, or unblock condition." },
      evidenceRefs: { type: "array", items: { type: "string" }, description: "Existing canonical IDs as finding:id, memory:id, runbook:id, or report:id. A negative result should be recorded in a memory first." },
      angles: { type: "array", minItems: 1, maxItems: 4,
        description: "Update 1 to 4 distinct causal attacker paths. Preserve earlier paths by name; tested outcomes need canonical evidence cited in evidenceRefs.",
        items: { type: "object", required: ["name", "status", "vector", "proofQuestion", "evidenceRefs", "nextAction"],
          additionalProperties: false, properties: {
            name: { type: "string", description: "Distinct attacker-controlled route or trust boundary, not a file or test name." },
            status: { type: "string", enum: ["open", "supported", "ruled_out", "blocked"] },
            vector: { type: "object", required: ["attackerControl", "trustBoundary", "failureMode", "unauthorizedEffect"],
              additionalProperties: false, properties: {
                attackerControl: { type: "string", description: "Concrete control and prerequisite available to the attacker." },
                trustBoundary: { type: "string", description: "The identity, authority, data, or execution boundary that would be crossed." },
                failureMode: { type: "string", description: "Hypothesized causal failure; do not state it as observed before proof." },
                unauthorizedEffect: { type: "string", description: "Protected asset and unauthorized effect if the path succeeds." },
              } },
            proofQuestion: { type: "string", description: "The observation that would distinguish this path from a harmless behavior." },
            evidenceRefs: { type: "array", items: { type: "string" } },
            nextAction: { type: "string", description: "Specific next proof step, or why this path was ruled out or blocked." }
          } } },
      validation: { type: "object", required: ["targetDependent", "environment", "reason"], additionalProperties: false,
        description: "State whether the theory needs target behavior, where it was actually tested, and why. Device availability checks alone count as none. Explain a target-validation gap when only localhost or no test was possible.",
        properties: { targetDependent: { type: "boolean" }, environment: { type: "string", enum: ["device", "vm", "web", "localhost", "none"] }, reason: { type: "string" } } },
    } };
  return { descriptor: { name: "track.checkpoint", transportName: "track_checkpoint",
    description: "Record substantive progress and distinct attacker paths on the assigned surface. Completing high-impact work without a finding requires two evidence-backed tested paths. Use defer when rotation is due; ordinary uncertainty is not a blocker.",
    actionClasses: ["synthesize"], sideEffects: "write", requiredPermissions: ["research-resource:inventory"], inputSchema: parameters },
    parameters, async execute(action: ResearchToolAction, _context?: ResearchToolExecutionContext) {
      const startedAt = new Date().toISOString();
      try {
        const input = action.input;
        const surfaceId = required(input.surfaceId, "surfaceId");
        const outcome = input.outcome;
        const impact = input.impact;
        if (outcome !== "continue" && outcome !== "defer" && outcome !== "blocked" && outcome !== "complete") throw new Error("Invalid track outcome.");
        if (impact !== "unknown" && impact !== "low" && impact !== "medium" && impact !== "high" && impact !== "critical") throw new Error("Invalid impact estimate.");
        if (!Array.isArray(input.evidenceRefs) || !input.evidenceRefs.every((ref) => typeof ref === "string")) throw new Error("evidenceRefs must be strings.");
        if (!Array.isArray(input.angles)) throw new Error("angles must contain 1 to 4 attacker paths.");
        const validation = input.validation;
        if (!validation || typeof validation !== "object" || Array.isArray(validation)) throw new Error("validation is required.");
        const output = store.checkpoint({ surfaceId, outcome, impact,
          theory: required(input.theory, "theory"), nextAction: required(input.nextAction, "nextAction"),
          evidenceRefs: input.evidenceRefs as string[], angles: input.angles as ResearchTrackAngle[],
          validation: validation as ResearchTrackValidation }, surface(surfaceId));
        return { action, status: "complete", startedAt, completedAt: new Date().toISOString(),
          summary: "Research track checkpoint recorded.", output, followUpActions: [] };
      } catch (error) {
        if (error && typeof error === "object" && "code" in error
          && typeof error.code === "string" && error.code.startsWith("ERR_SQLITE_")) {
          try { store.pauseWithError(`Track checkpoint storage failed: ${error instanceof Error ? error.message : String(error)}`); }
          catch { /* The original storage error is still returned to the session. */ }
        }
        return { action, status: "error", startedAt, completedAt: new Date().toISOString(),
          summary: "Research track checkpoint failed.", error: { message: error instanceof Error ? error.message : String(error) }, followUpActions: [] };
      }
    } };
}

export function createResearchTrackHandoffTool(store: ResearchTrackStore): ResearchExecutableTool {
  const parameters = { type: "object", additionalProperties: false, properties: {} };
  return {
    descriptor: { name: "track.handoff", transportName: "track_handoff",
      description: "Read the current epoch assignment and durable progress from the app-server. Call at the start of a research track epoch instead of reconstructing the prior transcript.",
      actionClasses: ["recall"], sideEffects: "read", requiredPermissions: ["research-resource:inventory"], inputSchema: parameters },
    parameters,
    async execute(action: ResearchToolAction) {
      const startedAt = new Date().toISOString();
      try {
        const { track, surface } = store.currentAssignment();
        return { action, status: "complete", startedAt, completedAt: new Date().toISOString(),
          summary: "Current research track handoff loaded.",
          output: { trackId: track.id, pass: track.pass, epochCount: track.epochCount,
            surface, progress: researchTrackProgress(track),
            interrupted: track.lastError === "The research worker ended without a terminal session record." },
          followUpActions: [] };
      } catch (error) {
        return { action, status: "error", startedAt, completedAt: new Date().toISOString(),
          summary: "Research track handoff unavailable.",
          error: { message: error instanceof Error ? error.message : String(error) }, followUpActions: [] };
      }
    },
  };
}

function required(value: unknown, label: string): string {
  if (typeof value !== "string" || !value.trim()) throw new Error(`${label} must be a non-empty string.`);
  return value.trim();
}
function nullable(value: unknown): string | null { return typeof value === "string" && value ? value : null; }
function bounded(value: string, max: number): string { return value.trim().slice(0, max); }
