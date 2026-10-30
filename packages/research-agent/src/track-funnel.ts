import type { DatabaseSync } from "node:sqlite";
import { openResearchDatabase } from "./database.js";
import { claimVerificationHash, readFindings } from "./findings.js";
import type { FindingSummary } from "./knowledge-types.js";

export type TrackWorkerRole = "investigator" | "chain_analyst" | "triager" | "reporter";
export interface TrackWorkerAssignment {
  role: TrackWorkerRole;
  claimId: string;
  claimRevision: number;
  surfaceId: string | null;
}

interface AttemptRow {
  role: TrackWorkerRole;
  claim_id: string;
  claim_revision: number;
  attempts_on_revision: number;
  next_eligible_at: string | null;
  active_session_id: string | null;
  assigned_epoch: number;
  assigned_at: string;
}

export interface TrackWorkerCard {
  role: TrackWorkerRole;
  claimId: string;
  title: string;
  assignedAt: string;
  queued: Array<{ claimId: string; title: string }>;
}

/** Durable, claim-revision-aware queue for track-owned subagent work. */
export class TrackFunnelStore {
  readonly #database: DatabaseSync;
  readonly #workspaceId: string;

  public constructor(input: { databasePath: string; workspaceId: string }) {
    this.#database = openResearchDatabase(input.databasePath);
    this.#workspaceId = input.workspaceId;
    this.#database.exec(`CREATE TABLE IF NOT EXISTS app_server_track_worker_attempts (
      workspace_id TEXT NOT NULL,
      role TEXT NOT NULL,
      claim_id TEXT NOT NULL,
      claim_revision INTEGER NOT NULL,
      attempts_on_revision INTEGER NOT NULL,
      next_eligible_at TEXT,
      active_session_id TEXT,
      assigned_epoch INTEGER NOT NULL DEFAULT 0,
      worker_runtime_version INTEGER NOT NULL DEFAULT 1,
      assigned_at TEXT NOT NULL,
      PRIMARY KEY (workspace_id, role, claim_id)
    );
    CREATE INDEX IF NOT EXISTS app_server_track_worker_session_idx
      ON app_server_track_worker_attempts(workspace_id, active_session_id);`);
    const columns = this.#database.prepare("PRAGMA table_info(app_server_track_worker_attempts)").all();
    if (!columns.some((column) => column.name === "assigned_epoch")) {
      this.#database.exec("ALTER TABLE app_server_track_worker_attempts ADD COLUMN assigned_epoch INTEGER NOT NULL DEFAULT 0");
    }
    if (!columns.some((column) => column.name === "worker_runtime_version")) {
      this.#database.exec("ALTER TABLE app_server_track_worker_attempts ADD COLUMN worker_runtime_version INTEGER NOT NULL DEFAULT 0");
    }
    // Earlier track releases could reserve workers before their model runner
    // existed. These reservations are not completed attempts.
    this.#database.prepare(`UPDATE app_server_track_worker_attempts SET attempts_on_revision=0,
      next_eligible_at=NULL, active_session_id=NULL, assigned_epoch=0, worker_runtime_version=1
      WHERE workspace_id=? AND worker_runtime_version<1`).run(this.#workspaceId);
  }

  public close(): void { this.#database.close(); }

  public activeForSession(sessionId: string): TrackWorkerAssignment[] {
    const rows = this.#database.prepare(`SELECT role, claim_id, claim_revision FROM app_server_track_worker_attempts
      WHERE workspace_id=? AND active_session_id=? ORDER BY role, claim_id`).all(this.#workspaceId, sessionId) as unknown as AttemptRow[];
    const claims = new Map(readFindings(this.#database, this.#workspaceId).map((claim) => [claim.id, claim]));
    return rows.flatMap((row) => {
      const claim = claims.get(row.claim_id);
      return claim ? [{ role: row.role, claimId: claim.id, claimRevision: row.claim_revision,
        surfaceId: claim.surfaceId }] : [];
    });
  }

  public cardsForSession(sessionId: string): TrackWorkerCard[] {
    const active = this.#database.prepare(`SELECT role, claim_id, assigned_at FROM app_server_track_worker_attempts
      WHERE workspace_id=? AND active_session_id=? ORDER BY assigned_at, role, claim_id`)
      .all(this.#workspaceId, sessionId) as unknown as AttemptRow[];
    if (active.length === 0) return [];
    if (!this.#database.prepare(`SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_server_session_events'`).get()) return [];
    const assignedAtByKey = new Map(active.map((row) => [`${row.role}:${row.claim_id}`, row.assigned_at]));
    const latestStatus = new Map<string, string>();
    const events = this.#database.prepare(`SELECT
        json_extract(event_json, '$.timestamp') AS timestamp,
        json_extract(event_json, '$.payload.role') AS role,
        json_extract(event_json, '$.payload.trackClaimId') AS claim_id,
        json_extract(event_json, '$.payload.status') AS status
      FROM app_server_session_events
      WHERE session_id=? AND json_extract(event_json, '$.kind')='agent.event'
        AND json_extract(event_json, '$.payload.type')='subagent.activity'
        AND json_extract(event_json, '$.payload.trackClaimId') IS NOT NULL
        AND json_extract(event_json, '$.timestamp')>=?
      ORDER BY event_offset`).all(sessionId, active[0]!.assigned_at) as Array<{
        timestamp: string; role: string; claim_id: string; status: string;
      }>;
    for (const event of events) {
      const key = `${event.role}:${event.claim_id}`;
      const assignedAt = assignedAtByKey.get(key);
      if (assignedAt && event.timestamp >= assignedAt) latestStatus.set(key, event.status);
    }
    const running = active.filter((row) => ["pending", "running"].includes(latestStatus.get(`${row.role}:${row.claim_id}`) ?? ""));
    if (running.length === 0) return [];
    const claims = readFindings(this.#database, this.#workspaceId).filter((claim) => claim.duplicateOfClaimId === null);
    const byId = new Map(claims.map((claim) => [claim.id, claim]));
    const pools = this.eligiblePools(claims);
    return running.flatMap((row) => {
      const claim = byId.get(row.claim_id);
      return claim ? [{ role: row.role, claimId: claim.id, title: claim.title,
        assignedAt: row.assigned_at,
        queued: pools[row.role].map((candidate) => ({ claimId: candidate.id, title: candidate.title })) }] : [];
    });
  }

  public reserveForSession(sessionId: string, epoch: number, maxWorkers = 2): TrackWorkerAssignment[] {
    if (!sessionId.trim()) throw new Error("Track worker session id is required.");
    // A new track epoch supersedes an orphaned reservation from an interrupted session.
    this.#database.prepare(`UPDATE app_server_track_worker_attempts SET active_session_id=NULL
      WHERE workspace_id=? AND active_session_id IS NOT NULL
        AND (active_session_id<>? OR assigned_epoch<>?)`).run(this.#workspaceId, sessionId, epoch);
    const claims = readFindings(this.#database, this.#workspaceId)
      .filter((claim) => claim.duplicateOfClaimId === null);
    const attempts = this.attempts();
    const pools = this.eligiblePools(claims, attempts);
    const selected: Array<{ role: TrackWorkerRole; claim: FindingSummary }> = [];
    if (maxWorkers > 0 && pools.investigator.length > 0) selected.push({ role: "investigator", claim: pools.investigator.shift()! });
    const downstream: TrackWorkerRole[] = ["triager", "reporter", "chain_analyst"];
    const rotated = [...downstream.slice(epoch % downstream.length), ...downstream.slice(0, epoch % downstream.length)];
    while (selected.length < Math.max(0, Math.min(3, maxWorkers))) {
      const role = rotated.find((candidate) => pools[candidate].length > 0)
        ?? (pools.investigator.length > 0 ? "investigator" : null);
      if (!role) break;
      selected.push({ role, claim: pools[role].shift()! });
    }
    const now = new Date().toISOString();
    this.#database.exec("BEGIN IMMEDIATE");
    try {
      const insert = this.#database.prepare(`INSERT INTO app_server_track_worker_attempts
        (workspace_id, role, claim_id, claim_revision, attempts_on_revision, next_eligible_at, active_session_id, assigned_epoch, worker_runtime_version, assigned_at)
        VALUES (?, ?, ?, ?, ?, NULL, ?, ?, 1, ?)
        ON CONFLICT(workspace_id, role, claim_id) DO UPDATE SET claim_revision=excluded.claim_revision,
          attempts_on_revision=excluded.attempts_on_revision, next_eligible_at=NULL,
          active_session_id=excluded.active_session_id, assigned_epoch=excluded.assigned_epoch,
          worker_runtime_version=excluded.worker_runtime_version, assigned_at=excluded.assigned_at`);
      for (const { role, claim } of selected) {
        const prior = attempts.get(`${role}:${claim.id}`);
        insert.run(this.#workspaceId, role, claim.id, claim.revision,
          prior?.claim_revision === claim.revision ? prior.attempts_on_revision + 1 : 1,
          sessionId, epoch, now);
      }
      this.#database.exec("COMMIT");
    } catch (error) { this.#database.exec("ROLLBACK"); throw error; }
    return selected.map(({ role, claim }) => ({ role, claimId: claim.id,
      claimRevision: claim.revision, surfaceId: claim.surfaceId }));
  }

  private attempts(): Map<string, AttemptRow> {
    return new Map((this.#database.prepare(`SELECT role, claim_id, claim_revision, attempts_on_revision,
      next_eligible_at, active_session_id, assigned_epoch, assigned_at FROM app_server_track_worker_attempts WHERE workspace_id=?`)
      .all(this.#workspaceId) as unknown as AttemptRow[]).map((row) => [`${row.role}:${row.claim_id}`, row]));
  }

  private eligiblePools(claims: FindingSummary[], attempts = this.attempts()): Record<TrackWorkerRole, FindingSummary[]> {
    const byId = new Map(claims.map((claim) => [claim.id, claim]));
    const now = new Date().toISOString();
    const eligible = (role: TrackWorkerRole, claim: FindingSummary): boolean => {
      const attempt = attempts.get(`${role}:${claim.id}`);
      if (!attempt) return true;
      if (attempt.active_session_id) return false;
      if (attempt.claim_revision !== claim.revision) return true;
      if (role === "chain_analyst" && attempt.attempts_on_revision >= 2) return false;
      return !attempt.next_eligible_at || attempt.next_eligible_at <= now;
    };
    const security = (claim: FindingSummary): boolean => claim.classification === "security"
      || claim.classification.startsWith("security.");
    const unresolved = (claim: FindingSummary): boolean => ["hypothesis", "observed", "reproduced", "stale"].includes(claim.status);
    const unblocked = (claim: FindingSummary): boolean => !claim.securityTracking?.progressBlocker;
    const investigation = claims.filter((claim) => security(claim) && claim.classification !== "security.chain"
      && claim.projection === "finding" && unresolved(claim) && unblocked(claim)
      && eligible("investigator", claim));
    const chaining = claims.filter((claim) => claim.classification === "security.chain"
      && claim.componentClaimIds.length > 0 && unresolved(claim) && unblocked(claim)
      && claim.componentClaimIds.some((id) => byId.get(id)?.evidence.length)
      && eligible("chain_analyst", claim));
    const triage = claims.filter((claim) => security(claim) && claim.status === "verified"
      && (claim.securityTracking?.reportability === null
        || claim.securityTracking?.reportability?.decision === "reportable"
          && !claim.securityTracking.triage.packet && !claim.securityTracking.triage.blocker)
      && eligible("triager", claim));
    const reporting = claims.filter((claim) => security(claim) && claim.status === "verified" && !claim.reportId
      && claim.securityTracking?.reportability?.decision === "reportable"
      && !claim.securityTracking.triage.blocker
      && claim.securityTracking.triage.packet?.claimBindingHash === claimVerificationHash(claim)
      && eligible("reporter", claim));
    const age = (role: TrackWorkerRole, claim: FindingSummary): string =>
      attempts.get(`${role}:${claim.id}`)?.next_eligible_at ?? claim.updatedAt;
    const order = (role: TrackWorkerRole, items: FindingSummary[]): FindingSummary[] => [...items].sort((left, right) =>
      (role === "investigator" ? maturityOrder(left) - maturityOrder(right) : 0)
      || age(role, left).localeCompare(age(role, right))
      || left.id.localeCompare(right.id));
    const pools: Record<TrackWorkerRole, FindingSummary[]> = {
      investigator: order("investigator", investigation),
      chain_analyst: order("chain_analyst", chaining),
      triager: order("triager", triage),
      reporter: order("reporter", reporting),
    };
    return pools;
  }

  public finishSession(sessionId: string): void {
    const active = this.#database.prepare(`SELECT role, claim_id, claim_revision
      FROM app_server_track_worker_attempts WHERE workspace_id=? AND active_session_id=?`)
      .all(this.#workspaceId, sessionId) as unknown as AttemptRow[];
    const now = new Date();
    this.#database.exec("BEGIN IMMEDIATE");
    try {
      for (const assignment of active) {
        const current = this.#database.prepare(`SELECT revision FROM app_server_research_claims
          WHERE workspace_id=? AND id=?`).get(this.#workspaceId, assignment.claim_id) as { revision?: number } | undefined;
        const progressed = current?.revision !== assignment.claim_revision;
        const delay = assignment.role === "chain_analyst" ? 24 * 60 * 60_000 : 30 * 60_000;
        this.#database.prepare(`UPDATE app_server_track_worker_attempts SET active_session_id=NULL,
          next_eligible_at=? WHERE workspace_id=? AND role=? AND claim_id=? AND active_session_id=?`).run(
            progressed ? null : new Date(now.getTime() + delay).toISOString(),
            this.#workspaceId, assignment.role, assignment.claim_id, sessionId);
      }
      this.#database.exec("COMMIT");
    } catch (error) { this.#database.exec("ROLLBACK"); throw error; }
  }

  public cancelSession(sessionId: string): void {
    this.#database.prepare(`UPDATE app_server_track_worker_attempts SET active_session_id=NULL,
      attempts_on_revision=MAX(0, attempts_on_revision-1), next_eligible_at=NULL
      WHERE workspace_id=? AND active_session_id=?`).run(this.#workspaceId, sessionId);
  }
}

function maturityOrder(claim: FindingSummary): number {
  if (claim.status === "reproduced") return 0;
  if (claim.status === "observed") return 1;
  return 2;
}
