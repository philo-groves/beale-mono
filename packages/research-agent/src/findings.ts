import { createHash, randomUUID } from "node:crypto";
import { DatabaseSync } from "node:sqlite";
import { applyDatabaseMigrations } from "./database-migrations.js";
import type {
  FindingEvidenceKind,
  FindingEvidenceSummary,
  FindingAffectedVersion,
  FindingCvssAssessment,
  FindingExternalReference,
  FindingReachabilityAssessment,
  FindingReachabilityState,
  FindingRiskDecision,
  FindingRiskTreatment,
  FindingSecurityTracking,
  FindingStatus,
  FindingSummary,
  FindingTransitionSummary,
  ModelAuthorSummary,
  ResearchClaimDuplicateSummary,
  ResearchClaimRating,
} from "./knowledge-types.js";
import type { MemoryGraphStore, MemoryNode } from "./memory-graph.js";
import type { ModelAuthor } from "./model-authorship.js";

const DIRECT_OBSERVATION_KINDS = new Set<FindingEvidenceKind>(["code", "artifact", "command", "url", "calculation", "proof", "publication"]);
const TERMINAL_FINDING_STATUSES = new Set<FindingStatus>(["disclosed", "rejected"]);

const ALLOWED_TRANSITIONS: Readonly<Record<FindingStatus, readonly FindingStatus[]>> = {
  hypothesis: ["observed", "rejected"],
  observed: ["reproduced", "stale", "rejected"],
  reproduced: ["verified", "stale", "rejected"],
  verified: ["report_ready", "stale", "rejected"],
  report_ready: ["disclosed", "stale", "rejected"],
  disclosed: ["stale"],
  stale: ["observed", "reproduced", "verified", "report_ready", "rejected"],
  rejected: ["hypothesis"],
};

export const LEGACY_CLAIM_MEMORY_TYPES = new Set([
  "hypothesis", "primitive", "chain", "conjecture", "theorem", "counterexample",
]);

export interface FindingEvidenceInput {
  kind: FindingEvidenceKind;
  referenceId?: string | null;
  contentHash?: string | null;
  summary: string;
  sessionId?: string | null;
  actorId?: string | null;
  independent?: boolean;
  metadata?: Record<string, unknown>;
}

export interface CreateFindingInput {
  /** Optional legacy source. New leads are canonical and do not require a memory node. */
  memoryNodeId?: string;
  classification?: string;
  componentClaimIds?: string[];
  title: string;
  summary?: string;
  impact?: string;
  rating?: ResearchClaimRating;
  confidence?: number;
  sourceRevision?: string | null;
  environmentFingerprint?: string | null;
  evidence?: FindingEvidenceInput[];
}

export type CreateLeadInput = CreateFindingInput;

export interface TransitionFindingInput {
  expectedRevision: number;
  toStatus: FindingStatus;
  reason: string;
  evidence?: FindingEvidenceInput[];
  sourceRevision?: string | null;
  environmentFingerprint?: string | null;
  reproductionRunbookId?: string | null;
  reportId?: string | null;
  disclosureReference?: string | null;
  classification?: string;
  componentClaimIds?: string[];
}

export interface ReviseResearchClaimInput {
  expectedRevision: number;
  reason: string;
  title?: string;
  summary?: string;
  impact?: string;
  rating?: ResearchClaimRating;
  confidence?: number;
  classification?: string;
  componentClaimIds?: string[];
  securityTracking?: FindingSecurityTrackingUpdate;
}

export interface MarkResearchClaimDuplicateInput {
  expectedRevision: number;
  parentClaimId: string;
  reason: string;
}

export interface UndoResearchClaimDuplicateInput {
  expectedRevision: number;
  reason: string;
}

export interface FindingSecurityTrackingUpdate {
  reachability?: {
    state: FindingReachabilityState;
    conditions?: string;
    evidenceIds?: string[];
    assessedAt?: string;
    sourceRevision?: string | null;
    environmentFingerprint?: string | null;
  };
  riskTreatment?: FindingRiskTreatment;
  riskDecision?: {
    rationale: string;
    decidedAt?: string;
    expiresAt?: string | null;
  };
  cvssAssessment?: Omit<FindingCvssAssessment, "assessorId"> & { assessorId?: string };
  affectedAssetIds?: string[];
  affectedVersions?: FindingAffectedVersion[];
  externalReferences?: FindingExternalReference[];
}

export type CandidateCompletionTarget = "observed" | "reproduced" | "verified" | "report_ready";

export interface CandidateCompletionChecklistItem {
  key: string;
  label: string;
  required: boolean;
  status: "complete" | "missing" | "recommended_missing" | "not_applicable";
  detail: string;
}

export interface CandidateCompletionChecklist {
  claimId: string;
  targetStatus: CandidateCompletionTarget;
  ready: boolean;
  completedRequired: number;
  requiredCount: number;
  missingRequired: string[];
  items: CandidateCompletionChecklistItem[];
}

export class ResearchClaimStore {
  private readonly database: DatabaseSync;

  public constructor(private readonly memoryGraph: MemoryGraphStore) {
    this.database = new DatabaseSync(memoryGraph.databasePath);
    this.database.exec("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;");
    initializeFindingSchema(this.database);
    migrateLegacyMemoryClaims(this.database, memoryGraph.getContext().workspaceId);
  }

  public close(): void {
    this.database.close();
  }

  public create(input: CreateFindingInput, author?: ModelAuthor, actorId?: string): FindingSummary {
    const context = this.memoryGraph.getContext();
    const memory = input.memoryNodeId ? this.requireWorkspaceMemory(input.memoryNodeId) : null;
    const existing = memory ? this.database.prepare(
      "SELECT id FROM app_server_research_claims WHERE workspace_id = ? AND legacy_memory_node_id = ?",
    ).get(context.workspaceId, memory.id) as { id?: unknown } | undefined : undefined;
    if (typeof existing?.id === "string") return this.get(existing.id)!;

    const now = new Date().toISOString();
    const id = memory ? stableFindingId(context.workspaceId, memory.id) : `claim_${randomUUID()}`;
    const evidence = normalizeEvidenceInputs(input.evidence ?? [], context.sessionId ?? null, actorId ?? null);
    const classification = claimClassification(input.classification ?? legacyClaimClassification(memory?.type));
    const componentClaimIds = uniqueStrings(input.componentClaimIds ?? []);
    const securityTracking = securityClassification(classification) ? emptyFindingSecurityTracking() : null;
    this.validateComponents(id, componentClaimIds);
    this.database.exec("BEGIN IMMEDIATE");
    try {
      this.database.prepare(`INSERT INTO app_server_research_claims (
        id, workspace_id, subject_id, legacy_memory_node_id, origin_session_id, classification,
        title, summary, impact, rating, status, stale_from_status, confidence,
        source_revision, environment_fingerprint, reproduction_runbook_id,
        report_id, disclosure_reference, stale_reason, security_tracking_json,
        created_at, updated_at, revision
      ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'hypothesis', NULL, ?, ?, ?, NULL, NULL, NULL, NULL, ?, ?, ?, 1)`).run(
        id,
        context.workspaceId,
        context.subjectId ?? `subject_workspace:${context.workspaceId}`,
        memory?.id ?? null,
        context.sessionId ?? null,
        classification,
        requiredText(input.title, "Lead title"),
        normalizedText(input.summary) ?? memory?.summary ?? "",
        normalizedText(input.impact) ?? "",
        researchClaimRating(input.rating),
        confidence(input.confidence ?? memory?.confidence ?? 0.5),
        nullableText(input.sourceRevision),
        nullableText(input.environmentFingerprint),
        stableJson(securityTracking),
        now,
        now,
      );
      const evidenceIds = this.insertEvidence(id, evidence, now);
      this.replaceComponents(id, componentClaimIds, now);
      this.insertTransition(id, 1, null, "hypothesis", "Research lead created.", actorId ?? null, evidenceIds, now);
      this.insertAuthor(id, 1, author, now);
      this.database.exec("COMMIT");
    } catch (error) {
      this.database.exec("ROLLBACK");
      throw error;
    }
    return this.get(id)!;
  }

  public transition(id: string, input: TransitionFindingInput, author?: ModelAuthor, actorId?: string): FindingSummary {
    const current = this.get(id);
    if (!current) throw new Error(`Research claim not found: ${id}.`);
    requireCanonicalClaim(current);
    if (current.revision !== input.expectedRevision) {
      throw new Error(`Research claim revision conflict for ${id}: expected ${input.expectedRevision}, found ${current.revision}.`);
    }
    const appendingEvidenceAtCurrentStatus = current.status === input.toStatus;
    if (!appendingEvidenceAtCurrentStatus && !ALLOWED_TRANSITIONS[current.status].includes(input.toStatus)) {
      throw new Error(`Research claim transition ${current.status} -> ${input.toStatus} is not allowed.`);
    }
    if (appendingEvidenceAtCurrentStatus && (input.evidence?.length ?? 0) === 0) {
      throw new Error(`Research claim ${current.status} evidence append requires at least one new evidence item.`);
    }
    const context = this.memoryGraph.getContext();
    if (current.workspaceId !== context.workspaceId) throw new Error("Finding is outside the active workspace.");
    const now = new Date().toISOString();
    const newEvidence = normalizeEvidenceInputs(input.evidence ?? [], context.sessionId ?? null, actorId ?? null);
    const accumulated = [...current.evidence, ...newEvidence.map((item, index) => ({
      id: `pending_${index}`,
      ...item,
      createdAt: now,
    }))];
    validateTransitionEvidence(this.database, current, input, accumulated);
    const classification = claimClassification(input.classification ?? current.classification);
    const componentClaimIds = input.componentClaimIds === undefined
      ? current.componentClaimIds
      : uniqueStrings(input.componentClaimIds);
    this.validateComponents(id, componentClaimIds);
    validateCompositeClaim(classification, input.toStatus, componentClaimIds);

    const nextRevision = current.revision + 1;
    this.database.exec("BEGIN IMMEDIATE");
    try {
      const evidenceIds = this.insertEvidence(id, newEvidence, now);
      const staleFromStatus = input.toStatus === "stale"
        ? (current.status === "stale" ? current.staleFromStatus : current.status)
        : null;
      const result = this.database.prepare(`UPDATE app_server_research_claims SET
        status = ?, stale_from_status = ?, classification = ?, source_revision = ?, environment_fingerprint = ?,
        reproduction_runbook_id = ?, report_id = ?, disclosure_reference = ?, stale_reason = ?,
        updated_at = ?, revision = ? WHERE id = ? AND revision = ?`).run(
        input.toStatus,
        staleFromStatus,
        classification,
        input.toStatus === "stale" || input.sourceRevision === undefined
          ? current.sourceRevision
          : nullableText(input.sourceRevision),
        input.toStatus === "stale" || input.environmentFingerprint === undefined
          ? current.environmentFingerprint
          : nullableText(input.environmentFingerprint),
        input.reproductionRunbookId === undefined ? current.reproductionRunbookId : nullableText(input.reproductionRunbookId),
        input.reportId === undefined ? current.reportId : nullableText(input.reportId),
        input.disclosureReference === undefined ? current.disclosureReference : nullableText(input.disclosureReference),
        input.toStatus === "stale" ? requiredText(input.reason, "Staleness reason") : null,
        now,
        nextRevision,
        id,
        current.revision,
      );
      if (Number(result.changes) !== 1) throw new Error(`Research claim revision conflict for ${id}.`);
      this.replaceComponents(id, componentClaimIds, now);
      this.insertTransition(id, nextRevision, current.status, input.toStatus, requiredText(input.reason, "Transition reason"), actorId ?? null, evidenceIds, now);
      this.insertAuthor(id, nextRevision, author, now);
      this.database.exec("COMMIT");
    } catch (error) {
      this.database.exec("ROLLBACK");
      throw error;
    }
    return this.get(id)!;
  }

  public get(id: string): FindingSummary | null {
    const context = this.memoryGraph.getContext();
    const rows = readFindings(this.database, context.workspaceId, id);
    return rows[0] ?? null;
  }

  public markDuplicate(
    id: string,
    input: MarkResearchClaimDuplicateInput,
    author?: ModelAuthor,
    actorId?: string,
  ): FindingSummary {
    const current = this.get(id);
    if (!current) throw new Error(`Research claim not found: ${id}.`);
    if (current.revision !== input.expectedRevision) {
      throw new Error(`Research claim revision conflict for ${id}: expected ${input.expectedRevision}, found ${current.revision}.`);
    }
    if (current.duplicateOfClaimId) {
      throw new Error(`Research claim ${id} is already marked as a duplicate of ${current.duplicateOfClaimId}.`);
    }
    const parentClaimId = requiredText(input.parentClaimId, "Canonical parent claim id");
    if (parentClaimId === id) throw new Error("A research claim cannot be marked as a duplicate of itself.");
    const parent = this.get(parentClaimId);
    if (!parent) throw new Error(`Canonical parent research claim not found: ${parentClaimId}.`);
    if (parent.workspaceId !== current.workspaceId) throw new Error("Duplicate claims must belong to the same workspace.");
    if (parent.subjectId !== current.subjectId) throw new Error("Duplicate claims must belong to the same research subject.");
    if (parent.duplicateOfClaimId) {
      throw new Error(`Canonical parent ${parentClaimId} is itself a duplicate; choose its canonical parent instead.`);
    }
    if (current.duplicateClaims.length > 0) {
      throw new Error(`Research claim ${id} owns duplicate claims; undo or reassign them before coalescing it.`);
    }

    const now = new Date().toISOString();
    const nextRevision = current.revision + 1;
    const reason = requiredText(input.reason, "Duplicate reason");
    this.database.exec("BEGIN IMMEDIATE");
    try {
      const result = this.database.prepare(`UPDATE app_server_research_claims SET
        duplicate_of_claim_id = ?, duplicate_marked_at = ?, updated_at = ?, revision = ?
        WHERE id = ? AND revision = ? AND duplicate_of_claim_id IS NULL`).run(
        parentClaimId,
        now,
        now,
        nextRevision,
        id,
        current.revision,
      );
      if (Number(result.changes) !== 1) throw new Error(`Research claim revision conflict for ${id}.`);
      this.insertTransition(id, nextRevision, current.status, current.status, reason, actorId ?? null, [], now);
      this.insertAuthor(id, nextRevision, author, now);
      this.database.exec("COMMIT");
    } catch (error) {
      this.database.exec("ROLLBACK");
      throw error;
    }
    return this.get(parentClaimId)!;
  }

  public undoDuplicate(
    id: string,
    input: UndoResearchClaimDuplicateInput,
    author?: ModelAuthor,
    actorId?: string,
  ): FindingSummary {
    const current = this.get(id);
    if (!current) throw new Error(`Research claim not found: ${id}.`);
    if (current.revision !== input.expectedRevision) {
      throw new Error(`Research claim revision conflict for ${id}: expected ${input.expectedRevision}, found ${current.revision}.`);
    }
    if (!current.duplicateOfClaimId) throw new Error(`Research claim ${id} is not marked as a duplicate.`);

    const now = new Date().toISOString();
    const nextRevision = current.revision + 1;
    const reason = requiredText(input.reason, "Duplicate undo reason");
    this.database.exec("BEGIN IMMEDIATE");
    try {
      const result = this.database.prepare(`UPDATE app_server_research_claims SET
        duplicate_of_claim_id = NULL, duplicate_marked_at = NULL, updated_at = ?, revision = ?
        WHERE id = ? AND revision = ? AND duplicate_of_claim_id IS NOT NULL`).run(
        now,
        nextRevision,
        id,
        current.revision,
      );
      if (Number(result.changes) !== 1) throw new Error(`Research claim revision conflict for ${id}.`);
      this.insertTransition(id, nextRevision, current.status, current.status, reason, actorId ?? null, [], now);
      this.insertAuthor(id, nextRevision, author, now);
      this.database.exec("COMMIT");
    } catch (error) {
      this.database.exec("ROLLBACK");
      throw error;
    }
    return this.get(id)!;
  }

  public revise(id: string, input: ReviseResearchClaimInput, author?: ModelAuthor, actorId?: string): FindingSummary {
    const current = this.get(id);
    if (!current) throw new Error(`Research claim not found: ${id}.`);
    requireCanonicalClaim(current);
    if (current.revision !== input.expectedRevision) {
      throw new Error(`Research claim revision conflict for ${id}: expected ${input.expectedRevision}, found ${current.revision}.`);
    }
    const classification = claimClassification(input.classification ?? current.classification);
    const componentClaimIds = input.componentClaimIds === undefined ? current.componentClaimIds : uniqueStrings(input.componentClaimIds);
    this.validateComponents(id, componentClaimIds);
    validateCompositeClaim(classification, current.status, componentClaimIds);
    const nextRevision = current.revision + 1;
    const now = new Date().toISOString();
    const securityTracking = revisedFindingSecurityTracking(
      current,
      classification,
      input.securityTracking,
      author,
      actorId,
      now,
    );
    this.database.exec("BEGIN IMMEDIATE");
    try {
      const result = this.database.prepare(`UPDATE app_server_research_claims SET
        title = ?, summary = ?, impact = ?, rating = ?, confidence = ?, classification = ?, security_tracking_json = ?, updated_at = ?, revision = ?
        WHERE id = ? AND revision = ?`).run(
        input.title === undefined ? current.title : requiredText(input.title, "Claim title"),
        input.summary === undefined ? current.summary : normalizedText(input.summary) ?? "",
        input.impact === undefined ? current.impact : normalizedText(input.impact) ?? "",
        input.rating === undefined ? current.rating : researchClaimRating(input.rating),
        input.confidence === undefined ? current.confidence : confidence(input.confidence),
        classification,
        stableJson(securityTracking),
        now,
        nextRevision,
        id,
        current.revision,
      );
      if (Number(result.changes) !== 1) throw new Error(`Research claim revision conflict for ${id}.`);
      this.replaceComponents(id, componentClaimIds, now);
      this.insertTransition(id, nextRevision, current.status, current.status, requiredText(input.reason, "Revision reason"), actorId ?? null, [], now);
      this.insertAuthor(id, nextRevision, author, now);
      this.database.exec("COMMIT");
    } catch (error) {
      this.database.exec("ROLLBACK");
      throw error;
    }
    return this.get(id)!;
  }

  public list(): FindingSummary[] {
    return readFindings(this.database, this.memoryGraph.getContext().workspaceId)
      .filter((claim) => claim.duplicateOfClaimId === null);
  }

  public listLeads(): FindingSummary[] {
    return this.list().filter((claim) => claim.projection === "lead");
  }

  public listFindings(): FindingSummary[] {
    return this.list().filter((claim) => claim.projection === "finding");
  }

  public completionChecklist(
    id: string,
    targetStatus: CandidateCompletionTarget = "verified",
  ): CandidateCompletionChecklist {
    const claim = this.get(id);
    if (!claim) throw new Error(`Research claim not found: ${id}.`);
    requireCanonicalClaim(claim);
    return candidateCompletionChecklist(claim, targetStatus);
  }

  public refreshStaleness(sourceRevision: string | null, environmentFingerprint: string | null, actorId = "host"): FindingSummary[] {
    const changed: FindingSummary[] = [];
    for (const finding of this.list()) {
      if (finding.status === "hypothesis" || finding.status === "rejected" || finding.status === "stale") continue;
      const reasons = stalenessReasons(finding, sourceRevision, environmentFingerprint);
      if (reasons.length === 0) continue;
      changed.push(this.transition(finding.id, {
        expectedRevision: finding.revision,
        toStatus: "stale",
        reason: reasons.join(" "),
      }, undefined, actorId));
    }
    return changed;
  }

  private requireWorkspaceMemory(id: string): MemoryNode {
    const memory = this.memoryGraph.get(requiredText(id, "Memory node id"));
    if (!memory) throw new Error(`Memory node not found: ${id}.`);
    const context = this.memoryGraph.getContext();
    if (!memory.workspaces.some((workspace) => workspace.id === context.workspaceId)) {
      throw new Error("Finding memory must belong to the active workspace.");
    }
    return memory;
  }

  private insertEvidence(findingId: string, evidence: readonly NormalizedFindingEvidence[], now: string): string[] {
    const insert = this.database.prepare(`INSERT INTO app_server_claim_evidence (
      id, claim_id, kind, reference_id, content_hash, summary, session_id,
      actor_id, independent, metadata_json, created_at
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`);
    return evidence.map((item) => {
      const id = `claim_evidence_${randomUUID()}`;
      insert.run(id, findingId, item.kind, item.referenceId, item.contentHash, item.summary,
        item.sessionId, item.actorId, item.independent ? 1 : 0, stableJson(item.metadata), now);
      return id;
    });
  }

  private insertTransition(
    findingId: string,
    revision: number,
    fromStatus: FindingStatus | null,
    toStatus: FindingStatus,
    reason: string,
    actorId: string | null,
    evidenceIds: readonly string[],
    now: string,
  ): void {
    const sessionId = this.memoryGraph.getContext().sessionId ?? null;
    this.database.prepare(`INSERT INTO app_server_claim_transitions (
      id, claim_id, claim_revision, from_status, to_status, reason, session_id, actor_id, evidence_ids_json, created_at
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`).run(
      `claim_transition_${randomUUID()}`, findingId, revision, fromStatus, toStatus, reason,
      sessionId, actorId, stableJson([...evidenceIds]), now,
    );
  }

  private insertAuthor(findingId: string, revision: number, author: ModelAuthor | undefined, now: string): void {
    if (!author?.provider.trim() || !author.model.trim()) return;
    this.database.prepare(`INSERT OR IGNORE INTO app_server_claim_authorship
      (claim_id, revision, provider, model, created_at) VALUES (?, ?, ?, ?, ?)`)
      .run(findingId, revision, author.provider.trim(), author.model.trim(), now);
  }

  private validateComponents(id: string, componentIds: readonly string[]): void {
    if (componentIds.includes(id)) throw new Error("A research claim cannot compose itself.");
    const context = this.memoryGraph.getContext();
    for (const componentId of componentIds) {
      const row = this.database.prepare(
        "SELECT workspace_id, duplicate_of_claim_id FROM app_server_research_claims WHERE id = ?",
      ).get(componentId) as { workspace_id?: unknown; duplicate_of_claim_id?: unknown } | undefined;
      if (!row || row.workspace_id !== context.workspaceId) {
        throw new Error(`Component research claim is unavailable in this workspace: ${componentId}.`);
      }
      if (typeof row.duplicate_of_claim_id === "string" && row.duplicate_of_claim_id.length > 0) {
        throw new Error(`Component research claim ${componentId} is a duplicate; use canonical parent ${row.duplicate_of_claim_id}.`);
      }
    }
  }

  private replaceComponents(id: string, componentIds: readonly string[], now: string): void {
    this.database.prepare("DELETE FROM app_server_claim_components WHERE claim_id = ?").run(id);
    const insert = this.database.prepare(
      "INSERT INTO app_server_claim_components(claim_id, component_claim_id, position, created_at) VALUES (?, ?, ?, ?)",
    );
    componentIds.forEach((componentId, index) => insert.run(id, componentId, index, now));
  }
}

/** @deprecated Use ResearchClaimStore. */
export const FindingStore = ResearchClaimStore;
/** @deprecated Use ResearchClaimStore. */
export type FindingStore = ResearchClaimStore;

export function requireCanonicalClaim(claim: FindingSummary): void {
  if (claim.duplicateOfClaimId) {
    throw new Error(`Research claim ${claim.id} is a duplicate; use canonical parent ${claim.duplicateOfClaimId}.`);
  }
}

interface NormalizedFindingEvidence {
  kind: FindingEvidenceKind;
  referenceId: string | null;
  contentHash: string | null;
  summary: string;
  sessionId: string | null;
  actorId: string | null;
  independent: boolean;
  metadata: Record<string, unknown>;
}

export function initializeFindingSchema(database: DatabaseSync): void {
  // The old finding tables are migration input only. Creating them in a fresh
  // database would preserve their foreign key to the retired memory_nodes
  // schema and can make an otherwise knowledge-free workspace impossible to
  // initialize. Only advance that schema when an installation actually has it.
  if (tableExists(database, "app_server_findings")) applyDatabaseMigrations(database, "app_server_findings", [{
    version: 1,
    name: "evidence_gated_finding_lifecycle",
    up(db) {
      db.exec(`
        CREATE TABLE IF NOT EXISTS app_server_findings (
          id TEXT PRIMARY KEY,
          workspace_id TEXT NOT NULL,
          subject_id TEXT NOT NULL,
          memory_node_id TEXT NOT NULL REFERENCES memory_nodes(id) ON DELETE RESTRICT ON UPDATE CASCADE,
          origin_session_id TEXT,
          title TEXT NOT NULL,
          summary TEXT NOT NULL,
          impact TEXT NOT NULL DEFAULT '',
          status TEXT NOT NULL CHECK (status IN ('hypothesis','observed','reproduced','verified','report_ready','disclosed','stale','rejected')),
          stale_from_status TEXT,
          confidence REAL NOT NULL CHECK (confidence >= 0 AND confidence <= 1),
          source_revision TEXT,
          environment_fingerprint TEXT,
          reproduction_runbook_id TEXT,
          report_id TEXT,
          disclosure_reference TEXT,
          stale_reason TEXT,
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL,
          revision INTEGER NOT NULL CHECK (revision > 0),
          UNIQUE(workspace_id, memory_node_id)
        );
        CREATE INDEX IF NOT EXISTS app_server_findings_workspace_status_idx
          ON app_server_findings(workspace_id, status, updated_at);
        CREATE TABLE IF NOT EXISTS app_server_finding_evidence (
          id TEXT PRIMARY KEY,
          finding_id TEXT NOT NULL REFERENCES app_server_findings(id) ON DELETE CASCADE,
          kind TEXT NOT NULL CHECK (kind IN ('code','artifact','command','url','runbook_execution','independent_verification','report','disclosure')),
          reference_id TEXT,
          content_hash TEXT,
          summary TEXT NOT NULL,
          session_id TEXT,
          actor_id TEXT,
          independent INTEGER NOT NULL CHECK (independent IN (0,1)),
          metadata_json TEXT NOT NULL DEFAULT '{}',
          created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS app_server_finding_evidence_finding_idx
          ON app_server_finding_evidence(finding_id, created_at);
        CREATE TABLE IF NOT EXISTS app_server_finding_transitions (
          id TEXT PRIMARY KEY,
          finding_id TEXT NOT NULL REFERENCES app_server_findings(id) ON DELETE CASCADE,
          finding_revision INTEGER NOT NULL,
          from_status TEXT,
          to_status TEXT NOT NULL,
          reason TEXT NOT NULL,
          actor_id TEXT,
          evidence_ids_json TEXT NOT NULL DEFAULT '[]',
          created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS app_server_finding_transitions_finding_idx
          ON app_server_finding_transitions(finding_id, created_at);
        CREATE TABLE IF NOT EXISTS app_server_finding_authorship (
          finding_id TEXT NOT NULL REFERENCES app_server_findings(id) ON DELETE CASCADE,
          revision INTEGER NOT NULL,
          provider TEXT NOT NULL,
          model TEXT NOT NULL,
          created_at TEXT NOT NULL,
          PRIMARY KEY(finding_id, revision, provider, model)
        );
      `);
    },
  }, {
    version: 2,
    name: "ordered_finding_transitions",
    up(db) {
      if (!tableHasColumn(db, "app_server_finding_transitions", "finding_revision")) {
        db.exec("ALTER TABLE app_server_finding_transitions ADD COLUMN finding_revision INTEGER;");
        db.exec(`UPDATE app_server_finding_transitions AS transition_row SET finding_revision = (
          SELECT COUNT(*) FROM app_server_finding_transitions AS earlier
          WHERE earlier.finding_id = transition_row.finding_id
            AND (earlier.created_at < transition_row.created_at
              OR (earlier.created_at = transition_row.created_at AND earlier.rowid <= transition_row.rowid))
        );`);
      }
      db.exec(`CREATE UNIQUE INDEX IF NOT EXISTS app_server_finding_transitions_revision_idx
        ON app_server_finding_transitions(finding_id, finding_revision);`);
    },
  }]);
  applyDatabaseMigrations(database, "app_server_research_claims", [{
    version: 1,
    name: "canonical_research_claim_ledger",
    up(db) {
      db.exec(`
        CREATE TABLE IF NOT EXISTS app_server_research_claims (
          id TEXT PRIMARY KEY,
          workspace_id TEXT NOT NULL,
          subject_id TEXT NOT NULL,
          legacy_memory_node_id TEXT,
          origin_session_id TEXT,
          classification TEXT NOT NULL,
          title TEXT NOT NULL,
          summary TEXT NOT NULL,
          impact TEXT NOT NULL DEFAULT '',
          status TEXT NOT NULL CHECK (status IN ('hypothesis','observed','reproduced','verified','report_ready','disclosed','stale','rejected')),
          stale_from_status TEXT,
          confidence REAL NOT NULL CHECK (confidence >= 0 AND confidence <= 1),
          source_revision TEXT,
          environment_fingerprint TEXT,
          reproduction_runbook_id TEXT,
          report_id TEXT,
          disclosure_reference TEXT,
          stale_reason TEXT,
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL,
          revision INTEGER NOT NULL CHECK (revision > 0)
        );
        CREATE INDEX IF NOT EXISTS app_server_research_claims_workspace_status_idx
          ON app_server_research_claims(workspace_id, status, updated_at);
        CREATE UNIQUE INDEX IF NOT EXISTS app_server_research_claims_legacy_memory_idx
          ON app_server_research_claims(workspace_id, legacy_memory_node_id)
          WHERE legacy_memory_node_id IS NOT NULL;
        CREATE TABLE IF NOT EXISTS app_server_claim_evidence (
          id TEXT PRIMARY KEY,
          claim_id TEXT NOT NULL REFERENCES app_server_research_claims(id) ON DELETE CASCADE,
          kind TEXT NOT NULL,
          reference_id TEXT,
          content_hash TEXT,
          summary TEXT NOT NULL,
          session_id TEXT,
          actor_id TEXT,
          independent INTEGER NOT NULL CHECK (independent IN (0,1)),
          metadata_json TEXT NOT NULL DEFAULT '{}',
          created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS app_server_claim_evidence_claim_idx
          ON app_server_claim_evidence(claim_id, created_at);
        CREATE TABLE IF NOT EXISTS app_server_claim_transitions (
          id TEXT PRIMARY KEY,
          claim_id TEXT NOT NULL REFERENCES app_server_research_claims(id) ON DELETE CASCADE,
          claim_revision INTEGER NOT NULL,
          from_status TEXT,
          to_status TEXT NOT NULL,
          reason TEXT NOT NULL,
          actor_id TEXT,
          evidence_ids_json TEXT NOT NULL DEFAULT '[]',
          created_at TEXT NOT NULL,
          UNIQUE(claim_id, claim_revision)
        );
        CREATE TABLE IF NOT EXISTS app_server_claim_authorship (
          claim_id TEXT NOT NULL REFERENCES app_server_research_claims(id) ON DELETE CASCADE,
          revision INTEGER NOT NULL,
          provider TEXT NOT NULL,
          model TEXT NOT NULL,
          created_at TEXT NOT NULL,
          PRIMARY KEY(claim_id, revision, provider, model)
        );
        CREATE TABLE IF NOT EXISTS app_server_claim_components (
          claim_id TEXT NOT NULL REFERENCES app_server_research_claims(id) ON DELETE CASCADE,
          component_claim_id TEXT NOT NULL REFERENCES app_server_research_claims(id) ON DELETE RESTRICT,
          position INTEGER NOT NULL CHECK (position >= 0),
          created_at TEXT NOT NULL,
          PRIMARY KEY(claim_id, component_claim_id),
          UNIQUE(claim_id, position),
          CHECK(claim_id <> component_claim_id)
        );
      `);
      if (tableExists(db, "app_server_findings") && tableExists(db, "memory_nodes")) {
        db.exec(`
        INSERT OR IGNORE INTO app_server_research_claims(
          id, workspace_id, subject_id, legacy_memory_node_id, origin_session_id, classification,
          title, summary, impact, status, stale_from_status, confidence, source_revision,
          environment_fingerprint, reproduction_runbook_id, report_id, disclosure_reference,
          stale_reason, created_at, updated_at, revision
        )
        SELECT f.id, f.workspace_id, f.subject_id, f.memory_node_id, f.origin_session_id,
          CASE COALESCE(n.type, '')
            WHEN 'hypothesis' THEN 'security.vulnerability'
            WHEN 'primitive' THEN 'security.primitive'
            WHEN 'chain' THEN 'security.chain'
            WHEN 'conjecture' THEN 'mathematics.conjecture'
            WHEN 'theorem' THEN 'mathematics.theorem'
            WHEN 'counterexample' THEN 'mathematics.counterexample'
            ELSE 'general.result'
          END,
          f.title, f.summary, f.impact, f.status, f.stale_from_status, f.confidence,
          f.source_revision, f.environment_fingerprint, f.reproduction_runbook_id,
          f.report_id, f.disclosure_reference, f.stale_reason, f.created_at, f.updated_at, f.revision
        FROM app_server_findings f LEFT JOIN memory_nodes n ON n.id = f.memory_node_id;
        `);
      } else if (tableExists(db, "app_server_findings")) {
        db.exec(`
        INSERT OR IGNORE INTO app_server_research_claims(
          id, workspace_id, subject_id, legacy_memory_node_id, origin_session_id, classification,
          title, summary, impact, status, stale_from_status, confidence, source_revision,
          environment_fingerprint, reproduction_runbook_id, report_id, disclosure_reference,
          stale_reason, created_at, updated_at, revision
        )
        SELECT f.id, f.workspace_id, f.subject_id, NULL, f.origin_session_id, 'general.result',
          f.title, f.summary, f.impact, f.status, f.stale_from_status, f.confidence,
          f.source_revision, f.environment_fingerprint, f.reproduction_runbook_id,
          f.report_id, f.disclosure_reference, f.stale_reason, f.created_at, f.updated_at, f.revision
        FROM app_server_findings f;
        `);
      }
      if (tableExists(db, "app_server_finding_evidence")) db.exec(`
        INSERT OR IGNORE INTO app_server_claim_evidence
          SELECT id, finding_id, kind, reference_id, content_hash, summary, session_id,
            actor_id, independent, metadata_json, created_at FROM app_server_finding_evidence;
      `);
      if (tableExists(db, "app_server_finding_transitions")) db.exec(`
        INSERT OR IGNORE INTO app_server_claim_transitions
          SELECT id, finding_id, finding_revision, from_status, to_status, reason, actor_id,
            evidence_ids_json, created_at FROM app_server_finding_transitions;
      `);
      if (tableExists(db, "app_server_finding_authorship")) db.exec(`
        INSERT OR IGNORE INTO app_server_claim_authorship
          SELECT finding_id, revision, provider, model, created_at FROM app_server_finding_authorship;
      `);
    },
  }, {
    version: 2,
    name: "security_finding_tracking",
    up(db) {
      if (!tableHasColumn(db, "app_server_research_claims", "security_tracking_json")) {
        db.exec("ALTER TABLE app_server_research_claims ADD COLUMN security_tracking_json TEXT NOT NULL DEFAULT 'null';");
      }
    },
  }, {
    version: 3,
    name: "untrusted_qualitative_claim_rating",
    up(db) {
      if (!tableHasColumn(db, "app_server_research_claims", "rating")) {
        db.exec(`ALTER TABLE app_server_research_claims
          ADD COLUMN rating TEXT NOT NULL DEFAULT 'informational'
          CHECK (rating IN ('informational','low','medium','high','critical'));`);
      }
    },
  }, {
    version: 4,
    name: "repair_incomparable_workspace_staleness",
    up(db) {
      repairIncomparableWorkspaceStaleness(db);
    },
  }, {
    version: 5,
    name: "claim_transition_session_attribution",
    up(db) {
      if (!tableHasColumn(db, "app_server_claim_transitions", "session_id")) {
        db.exec("ALTER TABLE app_server_claim_transitions ADD COLUMN session_id TEXT;");
      }
      db.exec(`UPDATE app_server_claim_transitions AS transition_row SET session_id = (
        SELECT claim.origin_session_id FROM app_server_research_claims claim
        WHERE claim.id = transition_row.claim_id
      ) WHERE transition_row.claim_revision = 1 AND transition_row.session_id IS NULL;`);
    },
  }, {
    version: 6,
    name: "claim_duplicate_relationship",
    up(db) {
      if (!tableHasColumn(db, "app_server_research_claims", "duplicate_of_claim_id")) {
        db.exec(`ALTER TABLE app_server_research_claims
          ADD COLUMN duplicate_of_claim_id TEXT REFERENCES app_server_research_claims(id) ON DELETE RESTRICT;`);
      }
      if (!tableHasColumn(db, "app_server_research_claims", "duplicate_marked_at")) {
        db.exec("ALTER TABLE app_server_research_claims ADD COLUMN duplicate_marked_at TEXT;");
      }
      db.exec(`CREATE INDEX IF NOT EXISTS app_server_research_claims_duplicate_parent_idx
        ON app_server_research_claims(workspace_id, duplicate_of_claim_id, duplicate_marked_at);`);
    },
  }]);
}

export function readFindings(database: DatabaseSync, workspaceId: string, findingId?: string): FindingSummary[] {
  if (!tableExists(database, "app_server_research_claims")) return [];
  const rows = database.prepare(`SELECT * FROM app_server_research_claims
    WHERE workspace_id = ?${findingId ? " AND id = ?" : ""}
    ORDER BY updated_at DESC, id`).all(...(findingId ? [workspaceId, findingId] : [workspaceId])) as SqlRow[];
  const evidence = groupedEvidence(database, new Set(rows.map((row) => requiredSqlText(row.id))));
  const transitions = groupedTransitions(database, new Set(rows.map((row) => requiredSqlText(row.id))));
  const authors = groupedAuthors(database, new Set(rows.map((row) => requiredSqlText(row.id))));
  const components = groupedComponents(database, new Set(rows.map((row) => requiredSqlText(row.id))));
  const duplicates = groupedDuplicateClaims(database, workspaceId, new Set(rows.map((row) => requiredSqlText(row.id))));
  return rows.map((row) => {
    const id = requiredSqlText(row.id);
    const status = findingStatus(row.status);
    const staleFromStatus = row.stale_from_status === null ? null : findingStatus(row.stale_from_status);
    const classification = claimClassification(row.classification);
    return {
    id,
    workspaceId: requiredSqlText(row.workspace_id),
    subjectId: requiredSqlText(row.subject_id),
    memoryNodeId: optionalSqlText(row.legacy_memory_node_id),
    originSessionId: optionalSqlText(row.origin_session_id),
    ...claimProjection(status, staleFromStatus, (evidence.get(id) ?? []).length),
    rating: researchClaimRating(row.rating),
    classification,
    componentClaimIds: components.get(id) ?? [],
    duplicateOfClaimId: optionalSqlText(row.duplicate_of_claim_id),
    duplicateMarkedAt: optionalSqlText(row.duplicate_marked_at),
    duplicateClaims: duplicates.get(id) ?? [],
    title: requiredSqlText(row.title),
    summary: requiredSqlText(row.summary),
    impact: requiredSqlText(row.impact),
    securityTracking: findingSecurityTracking(row.security_tracking_json, classification),
    status,
    staleFromStatus,
    confidence: requiredSqlNumber(row.confidence),
    sourceRevision: optionalSqlText(row.source_revision),
    environmentFingerprint: optionalSqlText(row.environment_fingerprint),
    reproductionRunbookId: optionalSqlText(row.reproduction_runbook_id),
    reportId: optionalSqlText(row.report_id),
    disclosureReference: optionalSqlText(row.disclosure_reference),
    staleReason: optionalSqlText(row.stale_reason),
    evidence: evidence.get(id) ?? [],
    transitions: transitions.get(id) ?? [],
    authors: authors.get(id) ?? [],
    createdAt: requiredSqlText(row.created_at),
    updatedAt: requiredSqlText(row.updated_at),
    revision: requiredSqlNumber(row.revision),
  }; });
}

export function refreshFindingStaleness(input: {
  databasePath: string;
  workspaceId: string;
  sourceRevision?: string | null;
  environmentFingerprint?: string | null;
  actorId?: string;
}): FindingSummary[] {
  const database = new DatabaseSync(input.databasePath);
  database.exec("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;");
  try {
    initializeFindingSchema(database);
    const changed: FindingSummary[] = [];
    for (const finding of readFindings(database, input.workspaceId).filter((claim) => claim.duplicateOfClaimId === null)) {
      if (finding.status === "hypothesis" || finding.status === "rejected" || finding.status === "stale") continue;
      const reasons = stalenessReasons(
        finding,
        nullableText(input.sourceRevision),
        nullableText(input.environmentFingerprint),
      );
      if (reasons.length === 0) continue;
      const now = new Date().toISOString();
      const nextRevision = finding.revision + 1;
      database.exec("BEGIN IMMEDIATE");
      try {
        const result = database.prepare(`UPDATE app_server_research_claims SET
          status = 'stale', stale_from_status = ?, stale_reason = ?, updated_at = ?, revision = ?
          WHERE id = ? AND revision = ?`).run(
          finding.status,
          reasons.join(" "),
          now,
          nextRevision,
          finding.id,
          finding.revision,
        );
        if (Number(result.changes) !== 1) throw new Error(`Finding revision conflict for ${finding.id}.`);
        database.prepare(`INSERT INTO app_server_claim_transitions (
          id, claim_id, claim_revision, from_status, to_status, reason, actor_id, evidence_ids_json, created_at
        ) VALUES (?, ?, ?, ?, 'stale', ?, ?, '[]', ?)`).run(
          `claim_transition_${randomUUID()}`,
          finding.id,
          nextRevision,
          finding.status,
          reasons.join(" "),
          input.actorId?.trim() || "host",
          now,
        );
        database.exec("COMMIT");
      } catch (error) {
        database.exec("ROLLBACK");
        throw error;
      }
      changed.push(readFindings(database, input.workspaceId, finding.id)[0]!);
    }
    return changed;
  } finally {
    database.close();
  }
}

export function migrateWorkspaceResearchClaims(databasePath: string, workspaceId: string): void {
  const database = new DatabaseSync(databasePath);
  database.exec("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;");
  try {
    initializeFindingSchema(database);
    migrateLegacyMemoryClaims(database, workspaceId);
  } finally {
    database.close();
  }
}

function repairIncomparableWorkspaceStaleness(database: DatabaseSync): void {
  const rows = database.prepare(`SELECT claim.*, transition.to_status AS latest_to_status,
      transition.reason AS latest_reason, transition.actor_id AS latest_actor_id,
      transition.claim_revision AS latest_transition_revision
    FROM app_server_research_claims claim
    LEFT JOIN app_server_claim_transitions transition
      ON transition.claim_id = claim.id AND transition.claim_revision = claim.revision
    WHERE claim.status = 'stale' AND claim.stale_from_status IS NOT NULL`).all() as SqlRow[];
  const update = database.prepare(`UPDATE app_server_research_claims SET
    status = ?, stale_from_status = NULL, source_revision = ?, environment_fingerprint = ?,
    stale_reason = NULL, updated_at = ?, revision = ? WHERE id = ? AND revision = ?`);
  const insertTransition = database.prepare(`INSERT INTO app_server_claim_transitions (
    id, claim_id, claim_revision, from_status, to_status, reason, actor_id, evidence_ids_json, created_at
  ) VALUES (?, ?, ?, 'stale', ?, ?, 'host:migration', '[]', ?)`);

  for (const row of rows) {
    if (row.latest_to_status !== "stale" || row.latest_actor_id !== "host"
      || row.latest_transition_revision !== row.revision || typeof row.latest_reason !== "string") continue;
    const legacy = parseIncomparableWorkspaceStaleReason(row.latest_reason);
    if (!legacy) continue;
    const currentSource = optionalSqlText(row.source_revision);
    const currentEnvironment = optionalSqlText(row.environment_fingerprint);
    if (legacy.currentSource && currentSource !== legacy.currentSource) continue;
    if (legacy.environmentChanged && !isWorkspaceEnvironmentFingerprint(currentEnvironment)) continue;
    const restoredStatus = findingStatus(row.stale_from_status);
    if (restoredStatus === "stale" || restoredStatus === "hypothesis" || restoredStatus === "rejected") continue;

    const restored = restoredFindingIdentities(row, legacy);
    const now = new Date().toISOString();
    const currentRevision = requiredSqlNumber(row.revision);
    const nextRevision = currentRevision + 1;
    const id = requiredSqlText(row.id);
    const result = update.run(
      restoredStatus,
      restored.sourceRevision,
      restored.environmentFingerprint,
      now,
      nextRevision,
      id,
      currentRevision,
    );
    if (Number(result.changes) !== 1) {
      throw new Error(`Research claim revision conflict while repairing stale claim ${id}.`);
    }
    insertTransition.run(
      `claim_transition_${randomUUID()}`,
      id,
      nextRevision,
      restoredStatus,
      "Restored after repairing an incomparable workspace-wide staleness transition; the prior host transition remains in the audit history.",
      now,
    );
  }

  const activeRows = database.prepare(`SELECT * FROM app_server_research_claims
    WHERE status NOT IN ('hypothesis', 'stale', 'rejected')
      AND (source_revision GLOB 'source:[0-9a-f]*' OR environment_fingerprint GLOB 'environment:[0-9a-f]*')`).all() as SqlRow[];
  const priorHostTransition = database.prepare(`SELECT reason FROM app_server_claim_transitions
    WHERE claim_id = ? AND to_status = 'stale' AND actor_id = 'host'
    ORDER BY claim_revision DESC LIMIT 1`);
  const revise = database.prepare(`UPDATE app_server_research_claims SET
    source_revision = ?, environment_fingerprint = ?, updated_at = ?, revision = ?
    WHERE id = ? AND revision = ?`);
  for (const row of activeRows) {
    const id = requiredSqlText(row.id);
    const transition = priorHostTransition.get(id) as SqlRow | undefined;
    const legacy = typeof transition?.reason === "string"
      ? parseIncomparableWorkspaceStaleReason(transition.reason)
      : null;
    if (!legacy) continue;
    const currentSource = optionalSqlText(row.source_revision);
    const currentEnvironment = optionalSqlText(row.environment_fingerprint);
    const restored = restoredFindingIdentities(row, legacy);
    if (restored.sourceRevision === currentSource
      && restored.environmentFingerprint === currentEnvironment) continue;
    const status = findingStatus(row.status);
    const currentRevision = requiredSqlNumber(row.revision);
    const nextRevision = currentRevision + 1;
    const now = new Date().toISOString();
    const result = revise.run(
      restored.sourceRevision,
      restored.environmentFingerprint,
      now,
      nextRevision,
      id,
      currentRevision,
    );
    if (Number(result.changes) !== 1) {
      throw new Error(`Research claim revision conflict while repairing active claim ${id}.`);
    }
    database.prepare(`INSERT INTO app_server_claim_transitions (
      id, claim_id, claim_revision, from_status, to_status, reason, actor_id, evidence_ids_json, created_at
    ) VALUES (?, ?, ?, ?, ?, ?, 'host:migration', '[]', ?)`).run(
      `claim_transition_${randomUUID()}`,
      id,
      nextRevision,
      status,
      status,
      "Restored finding-specific verification identities after repairing a prior incomparable workspace-wide staleness transition.",
      now,
    );
  }
}

function restoredFindingIdentities(
  row: SqlRow,
  legacy: IncomparableWorkspaceStaleReason,
): { sourceRevision: string | null; environmentFingerprint: string | null } {
  const currentSource = optionalSqlText(row.source_revision);
  const currentEnvironment = optionalSqlText(row.environment_fingerprint);
  const tracking = parseJsonObject(row.security_tracking_json);
  const reachability = isRecord(tracking.reachability) ? tracking.reachability : {};
  const trackedSource = nullableText(reachability.sourceRevision);
  const trackedEnvironment = nullableText(reachability.environmentFingerprint);
  return {
    sourceRevision: isWorkspaceSourceFingerprint(currentSource)
      ? legacy.recordedSource ?? (isWorkspaceSourceFingerprint(trackedSource) ? null : trackedSource)
      : currentSource,
    environmentFingerprint: isWorkspaceEnvironmentFingerprint(currentEnvironment)
      ? (isWorkspaceEnvironmentFingerprint(trackedEnvironment) ? null : trackedEnvironment)
      : currentEnvironment,
  };
}

interface IncomparableWorkspaceStaleReason {
  recordedSource: string | null;
  currentSource: string | null;
  environmentChanged: boolean;
}

function parseIncomparableWorkspaceStaleReason(reason: string): IncomparableWorkspaceStaleReason | null {
  const environmentSentence = "The current execution environment differs from the verified environment fingerprint.";
  if (reason === environmentSentence) {
    return { recordedSource: null, currentSource: null, environmentChanged: true };
  }
  const match = /^Recorded source revision (.+) differs from current revision (source:[a-f0-9]{32})\.(?: The current execution environment differs from the verified environment fingerprint\.)?$/u.exec(reason);
  if (!match || !match[1] || !match[2]) return null;
  if (parseComparableFindingIdentity(match[1], "source")
    && parseComparableFindingIdentity(match[2], "source")) return null;
  return {
    recordedSource: match[1],
    currentSource: match[2],
    environmentChanged: reason.endsWith(environmentSentence),
  };
}

function isWorkspaceEnvironmentFingerprint(value: string | null): boolean {
  return value !== null && /^environment:[a-f0-9]{32}$/u.test(value);
}

function isWorkspaceSourceFingerprint(value: string | null): boolean {
  return value !== null && /^source:[a-f0-9]{32}$/u.test(value);
}

function validateTransitionEvidence(
  database: DatabaseSync,
  current: FindingSummary,
  input: TransitionFindingInput,
  evidence: readonly FindingEvidenceSummary[],
): void {
  if (input.toStatus === "observed" && !evidence.some((item) =>
    DIRECT_OBSERVATION_KINDS.has(item.kind) && Boolean(item.referenceId || item.contentHash))) {
    throw new Error("Observed findings require direct code, artifact, command, URL, calculation, proof, or publication evidence.");
  }
  if (input.toStatus === "reproduced") {
    const runbookId = nullableText(input.reproductionRunbookId) ?? current.reproductionRunbookId;
    const execution = evidence.find((item) => item.kind === "runbook_execution" && item.referenceId
      && successfulRunbookExecutionExists(database, current.workspaceId, runbookId, item.referenceId));
    if (!runbookId || !execution) {
      throw new Error("Reproduced findings require a successful runbook execution and reproductionRunbookId.");
    }
  }
  if (input.toStatus === "verified") {
    const verification = evidence.find((item) => ["independent_verification", "human_review", "proof"].includes(item.kind) && item.independent);
    if (!verification || !verification.referenceId) {
      throw new Error("Verified findings require durable evidence from an independent reviewer.");
    }
  }
  if (input.toStatus === "report_ready") {
    const reportId = nullableText(input.reportId) ?? current.reportId;
    if (!reportId || !workspaceResourceExists(database, "app_server_reports", reportId, current.workspaceId)
      || !evidence.some((item) => item.kind === "report" && item.referenceId === reportId)) {
      throw new Error("Report-ready findings require a report reference and report evidence.");
    }
  }
  if (input.toStatus === "disclosed") {
    const disclosure = nullableText(input.disclosureReference) ?? current.disclosureReference;
    if (!disclosure || !evidence.some((item) => item.kind === "disclosure" && item.referenceId === disclosure)) {
      throw new Error("Disclosed findings require a disclosure reference and disclosure evidence.");
    }
  }
}

export function candidateCompletionChecklist(
  claim: FindingSummary,
  targetStatus: CandidateCompletionTarget = "verified",
): CandidateCompletionChecklist {
  const targetRank = ({ observed: 1, reproduced: 2, verified: 3, report_ready: 4 } as const)[targetStatus];
  const tracking = claim.securityTracking;
  const hasDirectEvidence = claim.evidence.some((evidence) =>
    DIRECT_OBSERVATION_KINDS.has(evidence.kind) && Boolean(evidence.referenceId || evidence.contentHash));
  const hasReproduction = Boolean(claim.reproductionRunbookId)
    && claim.evidence.some((evidence) => evidence.kind === "runbook_execution" && Boolean(evidence.referenceId));
  const hasIndependentVerification = claim.evidence.some((evidence) =>
    evidence.independent
    && ["independent_verification", "human_review", "proof"].includes(evidence.kind)
    && Boolean(evidence.referenceId));
  const hasPositiveControl = claim.evidence.some((evidence) => evidenceControl(evidence) === "positive");
  const hasNegativeControl = claim.evidence.some((evidence) => evidenceControl(evidence) === "negative");
  const priorArtReferences = tracking?.externalReferences.filter((reference) =>
    /(?:cve|ghsa|osv|advis|bulletin|prior[_ -]?art|security[_ -]?history)/i.test(reference.kind)) ?? [];
  const composite = claim.classification === "security.chain"
    || claim.classification.endsWith(".chain")
    || claim.classification.endsWith(".composite");
  const item = (
    key: string,
    label: string,
    complete: boolean,
    required: boolean,
    completeDetail: string,
    missingDetail: string,
  ): CandidateCompletionChecklistItem => ({
    key,
    label,
    required,
    status: complete ? "complete" : required ? "missing" : "recommended_missing",
    detail: complete ? completeDetail : missingDetail,
  });
  const items: CandidateCompletionChecklistItem[] = [
    item("summary", "Candidate summary", Boolean(claim.summary.trim()), true, "A concise claim summary is recorded.", "Record the violated property and concrete observed behavior."),
    item("direct_evidence", "Direct observation evidence", hasDirectEvidence, targetRank >= 1, "Direct source, artifact, command, URL, calculation, proof, or publication evidence is linked.", "Link durable direct evidence before treating the lead as observed."),
    item("source_revision", "Source revision", Boolean(claim.sourceRevision), targetRank >= 1, `Bound to ${claim.sourceRevision ?? ""}.`, "Record the exact inspected source revision."),
    item("environment", "Environment fingerprint", Boolean(claim.environmentFingerprint), targetRank >= 2, "The reproduction environment is fingerprinted.", "Record the runtime or verifier environment used for reproduction."),
    item("impact", "Assessed impact", Boolean(claim.impact.trim()), targetRank >= 2, "Impact is recorded.", "Record demonstrated impact separately from plausible downstream impact."),
    item("reproduction", "Reproduction runbook", hasReproduction, targetRank >= 2, "A reproduction runbook and execution evidence are linked.", "Link a successful runbook execution and reproductionRunbookId."),
    item("positive_control", "Positive control", hasPositiveControl, false, "Evidence identifies a positive control.", "Recommended: mark evidence metadata.control=positive for the triggering case."),
    item("negative_control", "Negative control", hasNegativeControl, false, "Evidence identifies a negative control.", "Recommended: mark evidence metadata.control=negative for the nearest non-triggering case."),
    item("reachability", "Reachability assessment", Boolean(tracking && tracking.reachability.state !== "not_assessed"), targetRank >= 3, tracking ? `Reachability is ${tracking.reachability.state}.` : "Reachability is assessed.", "Record reachable, conditional, or unreachable with evidence and conditions."),
    item("affected_assets", "Affected assets", Boolean(tracking?.affectedAssetIds.length), targetRank >= 3, "Affected assets are identified.", "Identify the affected shipping components or assets."),
    item("affected_versions", "Affected versions", Boolean(tracking?.affectedVersions.length), targetRank >= 3, "Affected or fixed version ranges are recorded.", "Record the assessed affected range and fixed version when known."),
    item("cvss", "Assessed CVSS", Boolean(tracking?.cvssAssessments.length), targetRank >= 3, "A versioned CVSS assessment is recorded.", "Record an assessed CVSS vector, score, version, and nomenclature."),
    item("prior_art", "Prior-art disposition", priorArtReferences.length > 0, targetRank >= 3, "A public prior-art match or explicit no-match search is recorded.", "Record matched advisories or a prior_art_search external reference documenting the no-match query and date."),
    item("independent_verification", "Independent verification", hasIndependentVerification, targetRank >= 3, "Independent evidence is linked.", "Link durable evidence from an independent reviewer; a distinct reviewer in the same session qualifies."),
    composite
      ? item("components", "Composite components", claim.componentClaimIds.length > 0, targetRank >= 3, "Component claims are linked.", "Link every component claim used by the composite finding.")
      : { key: "components", label: "Composite components", required: false, status: "not_applicable", detail: "This is not a composite claim." },
    item("risk_treatment", "Risk treatment", Boolean(tracking && tracking.riskTreatment !== "unreviewed"), false, tracking ? `Operator risk treatment is ${tracking.riskTreatment}.` : "Risk treatment is recorded.", "Operator-controlled; leave unreviewed until a human records a disposition."),
  ];
  const required = items.filter((candidate) => candidate.required);
  const missingRequired = required.filter((candidate) => candidate.status !== "complete").map((candidate) => candidate.key);
  return {
    claimId: claim.id,
    targetStatus,
    ready: missingRequired.length === 0,
    completedRequired: required.length - missingRequired.length,
    requiredCount: required.length,
    missingRequired,
    items,
  };
}

function evidenceControl(evidence: FindingEvidenceSummary): "positive" | "negative" | null {
  const control = normalizedText(evidence.metadata.control)?.toLowerCase();
  if (control === "positive" || control === "negative") return control;
  const summary = evidence.summary.toLowerCase();
  if (summary.includes("positive control")) return "positive";
  if (summary.includes("negative control")) return "negative";
  return null;
}

function stalenessReasons(finding: FindingSummary, sourceRevision: string | null, environmentFingerprint: string | null): string[] {
  const reasons: string[] = [];
  if (comparableIdentityChanged(finding.sourceRevision, sourceRevision, "source")) {
    reasons.push(`Recorded source revision ${finding.sourceRevision} differs from current revision ${sourceRevision}.`);
  }
  if (comparableIdentityChanged(finding.environmentFingerprint, environmentFingerprint, "environment")) {
    reasons.push("The current execution environment differs from the verified environment fingerprint.");
  }
  return reasons;
}

type FindingIdentityDomain = "source" | "environment";

interface ComparableFindingIdentity {
  kind: string;
  resourceId: string;
  value: string;
}

function comparableIdentityChanged(
  recorded: string | null,
  current: string | null,
  domain: FindingIdentityDomain,
): boolean {
  const recordedIdentity = parseComparableFindingIdentity(recorded, domain);
  const currentIdentity = parseComparableFindingIdentity(current, domain);
  return Boolean(recordedIdentity && currentIdentity
    && recordedIdentity.kind === currentIdentity.kind
    && recordedIdentity.resourceId === currentIdentity.resourceId
    && recordedIdentity.value !== currentIdentity.value);
}

function parseComparableFindingIdentity(
  value: string | null,
  domain: FindingIdentityDomain,
): ComparableFindingIdentity | null {
  if (!value) return null;
  const match = /^([a-z][a-z0-9-]*):([A-Za-z0-9._/-]+):(.+)$/u.exec(value);
  if (!match) return null;
  const [, kind, resourceId, identityValue] = match;
  const acceptedKinds = domain === "source"
    ? new Set(["git", "source-drop", "product-build", "binary-sha256"])
    : new Set(["environment", "runtime"]);
  if (!acceptedKinds.has(kind!) || !resourceId || !identityValue?.trim()) return null;
  return { kind: kind!, resourceId, value: identityValue.trim() };
}

function normalizeEvidenceInputs(items: readonly FindingEvidenceInput[], defaultSessionId: string | null, defaultActorId: string | null): NormalizedFindingEvidence[] {
  return items.map((item) => ({
    kind: findingEvidenceKind(item.kind),
    referenceId: nullableText(item.referenceId),
    contentHash: nullableText(item.contentHash),
    summary: requiredText(item.summary, "Finding evidence summary"),
    sessionId: item.sessionId === undefined ? defaultSessionId : nullableText(item.sessionId),
    actorId: item.actorId === undefined ? defaultActorId : nullableText(item.actorId),
    independent: item.independent === true,
    metadata: isRecord(item.metadata) ? item.metadata : {},
  }));
}

function groupedEvidence(database: DatabaseSync, findingIds: ReadonlySet<string>): Map<string, FindingEvidenceSummary[]> {
  const grouped = new Map<string, FindingEvidenceSummary[]>();
  if (findingIds.size === 0 || !tableExists(database, "app_server_claim_evidence")) return grouped;
  for (const row of database.prepare("SELECT * FROM app_server_claim_evidence ORDER BY created_at, id").all() as SqlRow[]) {
    const findingId = requiredSqlText(row.claim_id);
    if (!findingIds.has(findingId)) continue;
    grouped.set(findingId, [...(grouped.get(findingId) ?? []), {
      id: requiredSqlText(row.id),
      kind: findingEvidenceKind(row.kind),
      referenceId: optionalSqlText(row.reference_id),
      contentHash: optionalSqlText(row.content_hash),
      summary: requiredSqlText(row.summary),
      sessionId: optionalSqlText(row.session_id),
      actorId: optionalSqlText(row.actor_id),
      independent: row.independent === 1,
      metadata: parseJsonObject(row.metadata_json),
      createdAt: requiredSqlText(row.created_at),
    }]);
  }
  return grouped;
}

function groupedTransitions(database: DatabaseSync, findingIds: ReadonlySet<string>): Map<string, FindingTransitionSummary[]> {
  const grouped = new Map<string, FindingTransitionSummary[]>();
  if (findingIds.size === 0 || !tableExists(database, "app_server_claim_transitions")) return grouped;
  for (const row of database.prepare("SELECT * FROM app_server_claim_transitions ORDER BY claim_id, claim_revision").all() as SqlRow[]) {
    const findingId = requiredSqlText(row.claim_id);
    if (!findingIds.has(findingId)) continue;
    grouped.set(findingId, [...(grouped.get(findingId) ?? []), {
      id: requiredSqlText(row.id),
      revision: requiredSqlNumber(row.claim_revision),
      fromStatus: row.from_status === null ? null : findingStatus(row.from_status),
      toStatus: findingStatus(row.to_status),
      reason: requiredSqlText(row.reason),
      sessionId: optionalSqlText(row.session_id),
      actorId: optionalSqlText(row.actor_id),
      evidenceIds: parseJsonStringArray(row.evidence_ids_json),
      createdAt: requiredSqlText(row.created_at),
    }]);
  }
  return grouped;
}

function groupedAuthors(database: DatabaseSync, findingIds: ReadonlySet<string>): Map<string, ModelAuthorSummary[]> {
  const grouped = new Map<string, ModelAuthorSummary[]>();
  if (findingIds.size === 0 || !tableExists(database, "app_server_claim_authorship")) return grouped;
  for (const row of database.prepare(`SELECT claim_id, provider, model
    FROM app_server_claim_authorship ORDER BY claim_id, revision, provider, model`).all() as SqlRow[]) {
    const findingId = requiredSqlText(row.claim_id);
    if (!findingIds.has(findingId)) continue;
    const author = { provider: requiredSqlText(row.provider), model: requiredSqlText(row.model) };
    const current = grouped.get(findingId) ?? [];
    if (!current.some((item) => item.provider === author.provider && item.model === author.model)) {
      grouped.set(findingId, [...current, author]);
    }
  }
  return grouped;
}

function groupedComponents(database: DatabaseSync, claimIds: ReadonlySet<string>): Map<string, string[]> {
  const grouped = new Map<string, string[]>();
  if (claimIds.size === 0 || !tableExists(database, "app_server_claim_components")) return grouped;
  for (const row of database.prepare(
    "SELECT claim_id, component_claim_id FROM app_server_claim_components ORDER BY claim_id, position",
  ).all() as SqlRow[]) {
    const claimId = requiredSqlText(row.claim_id);
    if (!claimIds.has(claimId)) continue;
    grouped.set(claimId, [...(grouped.get(claimId) ?? []), requiredSqlText(row.component_claim_id)]);
  }
  return grouped;
}

function groupedDuplicateClaims(
  database: DatabaseSync,
  workspaceId: string,
  parentClaimIds: ReadonlySet<string>,
): Map<string, ResearchClaimDuplicateSummary[]> {
  const grouped = new Map<string, ResearchClaimDuplicateSummary[]>();
  if (parentClaimIds.size === 0 || !tableHasColumn(database, "app_server_research_claims", "duplicate_of_claim_id")) {
    return grouped;
  }
  const rows = database.prepare(`SELECT claim.*,
      (SELECT COUNT(*) FROM app_server_claim_evidence evidence WHERE evidence.claim_id = claim.id) AS evidence_count
    FROM app_server_research_claims claim
    WHERE claim.workspace_id = ? AND claim.duplicate_of_claim_id IS NOT NULL
    ORDER BY claim.duplicate_marked_at DESC, claim.id`).all(workspaceId) as SqlRow[];
  for (const row of rows) {
    const parentClaimId = requiredSqlText(row.duplicate_of_claim_id);
    if (!parentClaimIds.has(parentClaimId)) continue;
    const status = findingStatus(row.status);
    const staleFromStatus = row.stale_from_status === null ? null : findingStatus(row.stale_from_status);
    const projection = claimProjection(status, staleFromStatus, requiredSqlNumber(row.evidence_count));
    grouped.set(parentClaimId, [...(grouped.get(parentClaimId) ?? []), {
      id: requiredSqlText(row.id),
      projection: projection.projection,
      maturity: projection.maturity,
      rating: researchClaimRating(row.rating),
      classification: claimClassification(row.classification),
      title: requiredSqlText(row.title),
      status,
      revision: requiredSqlNumber(row.revision),
      markedAt: requiredSqlText(row.duplicate_marked_at),
    }]);
  }
  return grouped;
}

export function migrateLegacyMemoryClaims(database: DatabaseSync, workspaceId: string): void {
  if (!tableExists(database, "memory_nodes") || !tableExists(database, "memory_node_workspaces")) return;
  const rows = database.prepare(`
    SELECT n.* FROM memory_nodes n
    JOIN memory_node_workspaces w ON w.node_id = n.id
    WHERE w.workspace_id = ? AND n.type IN ('hypothesis','primitive','chain','conjecture','theorem','counterexample')
    ORDER BY n.created_at, n.id
  `).all(workspaceId) as SqlRow[];
  if (rows.length === 0) return;
  const insertClaim = database.prepare(`INSERT OR IGNORE INTO app_server_research_claims(
    id, workspace_id, subject_id, legacy_memory_node_id, origin_session_id, classification,
    title, summary, impact, status, stale_from_status, confidence, source_revision,
    environment_fingerprint, reproduction_runbook_id, report_id, disclosure_reference,
    stale_reason, created_at, updated_at, revision
  ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, NULL, NULL, NULL, ?, ?, ?, 1)`);
  const insertTransition = database.prepare(`INSERT OR IGNORE INTO app_server_claim_transitions(
    id, claim_id, claim_revision, from_status, to_status, reason, actor_id, evidence_ids_json, created_at
  ) VALUES (?, ?, 1, NULL, ?, ?, 'migration', ?, ?)`);
  const insertEvidence = database.prepare(`INSERT OR IGNORE INTO app_server_claim_evidence(
    id, claim_id, kind, reference_id, content_hash, summary, session_id, actor_id,
    independent, metadata_json, created_at
  ) VALUES (?, ?, ?, ?, NULL, ?, ?, 'migration', 0, ?, ?)`);
  database.exec("BEGIN IMMEDIATE");
  try {
    for (const row of rows) {
      const memoryId = requiredSqlText(row.id);
      const claimId = stableFindingId(workspaceId, memoryId);
      const type = requiredSqlText(row.type);
      const mapped = legacyMemoryClaimStatus(type, requiredSqlText(row.status));
      const originSession = tableExists(database, "memory_node_sessions")
        ? optionalSqlText((database.prepare(
            "SELECT session_id FROM memory_node_sessions WHERE node_id = ? ORDER BY session_id LIMIT 1",
          ).get(memoryId) as SqlRow | undefined)?.session_id)
        : null;
      const attributes = parseJsonObject(row.attributes_json);
      insertClaim.run(
        claimId,
        workspaceId,
        requiredSqlText(row.subject_id),
        memoryId,
        originSession,
        legacyClaimClassification(type),
        requiredSqlText(row.title),
        requiredSqlText(row.summary),
        normalizedText(attributes.impact) ?? "",
        mapped.status,
        mapped.staleFromStatus,
        requiredSqlNumber(row.confidence),
        mapped.staleReason,
        requiredSqlText(row.created_at),
        requiredSqlText(row.updated_at),
      );
      const evidenceIds: string[] = [];
      if (tableExists(database, "memory_evidence_refs")) {
        for (const evidence of database.prepare(
          "SELECT * FROM memory_evidence_refs WHERE node_id = ? ORDER BY created_at, id",
        ).all(memoryId) as SqlRow[]) {
          const sourceEvidenceId = requiredSqlText(evidence.id);
          const evidenceId = `claim_evidence_legacy_${createHash("sha256").update(sourceEvidenceId).digest("hex").slice(0, 20)}`;
          insertEvidence.run(
            evidenceId,
            claimId,
            legacyEvidenceKind(evidence.kind),
            sourceEvidenceId,
            requiredSqlText(evidence.summary),
            originSession,
            stableJson({
              legacyMemoryNodeId: memoryId,
              pathBase: optionalSqlText(evidence.path_base),
              path: optionalSqlText(evidence.path),
              locator: parseJsonObject(evidence.locator_json),
            }),
            requiredSqlText(evidence.created_at),
          );
          evidenceIds.push(evidenceId);
        }
      }
      insertTransition.run(
        `claim_transition_legacy_${createHash("sha256").update(claimId).digest("hex").slice(0, 20)}`,
        claimId,
        mapped.status,
        "Migrated from the legacy claim-shaped memory taxonomy.",
        stableJson(evidenceIds),
        requiredSqlText(row.created_at),
      );
    }
    database.exec("COMMIT");
  } catch (error) {
    database.exec("ROLLBACK");
    throw error;
  }

  if (!tableExists(database, "memory_edges")) return;
  const componentRows = database.prepare(`
    SELECT parent.id AS claim_id, component.id AS component_claim_id
    FROM app_server_research_claims parent
    JOIN memory_nodes parent_memory ON parent_memory.id = parent.legacy_memory_node_id
    JOIN memory_edges edge ON edge.from_id = parent_memory.id OR edge.to_id = parent_memory.id
    JOIN memory_nodes component_memory ON component_memory.id = CASE
      WHEN edge.from_id = parent_memory.id THEN edge.to_id ELSE edge.from_id END
    JOIN app_server_research_claims component ON component.legacy_memory_node_id = component_memory.id
    WHERE parent.workspace_id = ? AND parent_memory.type = 'chain'
      AND component.workspace_id = parent.workspace_id AND component_memory.type = 'primitive'
    ORDER BY parent.id, component.id
  `).all(workspaceId) as SqlRow[];
  const positions = new Map<string, number>();
  const insertComponent = database.prepare(
    "INSERT OR IGNORE INTO app_server_claim_components(claim_id, component_claim_id, position, created_at) VALUES (?, ?, ?, ?)",
  );
  for (const row of componentRows) {
    const claimId = requiredSqlText(row.claim_id);
    const position = positions.get(claimId) ?? 0;
    insertComponent.run(claimId, requiredSqlText(row.component_claim_id), position, new Date().toISOString());
    positions.set(claimId, position + 1);
  }
}

function claimProjection(
  status: FindingStatus,
  staleFromStatus: FindingStatus | null,
  evidenceCount: number,
): Pick<FindingSummary, "projection" | "maturity" | "freshness" | "workflow"> {
  const effective = status === "stale" ? staleFromStatus ?? "observed" : status;
  const projection = effective === "hypothesis" || (effective === "rejected" && evidenceCount === 0)
    ? "lead" as const
    : "finding" as const;
  const maturity = effective === "hypothesis"
    ? "proposed" as const
    : effective === "observed"
      ? "observed" as const
      : effective === "reproduced"
        ? "reproduced" as const
        : effective === "rejected"
          ? "refuted" as const
          : "verified" as const;
  const workflow = effective === "report_ready"
    ? "reporting" as const
    : effective === "disclosed"
      ? "published" as const
      : effective === "rejected"
        ? "closed" as const
        : projection === "lead"
          ? "open" as const
          : "active" as const;
  return { projection, maturity, freshness: status === "stale" ? "stale" : "current", workflow };
}

function legacyMemoryClaimStatus(type: string, status: string): { status: FindingStatus; staleFromStatus: FindingStatus | null; staleReason: string | null } {
  if (status === "rejected" || status === "refuted") return { status: "rejected", staleFromStatus: null, staleReason: null };
  if (status === "stale" || status === "superseded") {
    const prior: FindingStatus = type === "hypothesis" || type === "conjecture" ? "hypothesis" : "observed";
    return { status: "stale", staleFromStatus: prior, staleReason: "Migrated legacy memory was already marked stale or superseded." };
  }
  if ((type === "chain" || type === "theorem" || type === "counterexample") && (status === "confirmed" || status === "verified")) {
    return { status: "verified", staleFromStatus: null, staleReason: null };
  }
  if (type === "primitive" && status === "confirmed") return { status: "observed", staleFromStatus: null, staleReason: null };
  return { status: "hypothesis", staleFromStatus: null, staleReason: null };
}

function legacyClaimClassification(type: string | undefined): string {
  if (type === "hypothesis") return "security.vulnerability";
  if (type === "primitive") return "security.primitive";
  if (type === "chain") return "security.chain";
  if (type === "conjecture") return "mathematics.conjecture";
  if (type === "theorem") return "mathematics.theorem";
  if (type === "counterexample") return "mathematics.counterexample";
  return "general.result";
}

function legacyEvidenceKind(value: unknown): FindingEvidenceKind {
  const normalized = normalizedText(value) ?? "artifact";
  if (["code", "artifact", "command", "url", "calculation", "proof", "publication", "human_review"].includes(normalized)) {
    return normalized as FindingEvidenceKind;
  }
  return "artifact";
}

function validateCompositeClaim(classification: string, status: FindingStatus, componentClaimIds: readonly string[]): void {
  const composite = classification === "security.chain" || classification.endsWith(".chain") || classification.endsWith(".composite");
  if (composite && ["verified", "report_ready", "disclosed"].includes(status) && componentClaimIds.length === 0) {
    throw new Error("A verified composite finding requires at least one component finding.");
  }
}

function securityClassification(classification: string): boolean {
  return classification === "security" || classification.startsWith("security.");
}

function emptyFindingSecurityTracking(): FindingSecurityTracking {
  return {
    reachability: {
      state: "not_assessed",
      conditions: "",
      evidenceIds: [],
      assessorId: null,
      assessedAt: null,
      sourceRevision: null,
      environmentFingerprint: null,
    },
    riskTreatment: "unreviewed",
    riskDecisions: [],
    cvssAssessments: [],
    affectedAssetIds: [],
    affectedVersions: [],
    externalReferences: [],
  };
}

function findingSecurityTracking(value: unknown, classification: string): FindingSecurityTracking | null {
  if (!securityClassification(classification)) return null;
  const parsed = parseJsonValue(value);
  if (parsed === null) return emptyFindingSecurityTracking();
  if (!isRecord(parsed)) throw new Error("Stored security finding tracking must be an object.");
  const empty = emptyFindingSecurityTracking();
  const riskTreatment = findingRiskTreatment(parsed.riskTreatment ?? empty.riskTreatment);
  const riskDecisions = storedArray(parsed.riskDecisions, storedRiskDecision);
  if (riskTreatment === "accepted" && riskDecisions.at(-1)?.treatment !== "accepted") {
    throw new Error("Accepted risk requires a matching audited risk decision.");
  }
  return {
    reachability: storedReachability(parsed.reachability ?? empty.reachability),
    riskTreatment,
    riskDecisions,
    cvssAssessments: storedArray(parsed.cvssAssessments, storedCvssAssessment),
    affectedAssetIds: uniqueTrackingStrings(storedStringArray(parsed.affectedAssetIds), "Affected asset id"),
    affectedVersions: storedArray(parsed.affectedVersions, storedAffectedVersion),
    externalReferences: storedArray(parsed.externalReferences, storedExternalReference),
  };
}

function revisedFindingSecurityTracking(
  current: FindingSummary,
  classification: string,
  update: FindingSecurityTrackingUpdate | undefined,
  author: ModelAuthor | undefined,
  actorId: string | undefined,
  now: string,
): FindingSecurityTracking | null {
  if (!securityClassification(classification)) {
    if (update !== undefined) throw new Error("Security tracking is only valid for security claim classifications.");
    return null;
  }
  const base = current.securityTracking ?? emptyFindingSecurityTracking();
  if (!update) return base;
  const resolvedActorId = normalizedText(actorId)
    ?? (author ? `model:${author.provider}:${author.model}` : "host");
  const reachability = update.reachability
    ? revisedReachability(current, update.reachability, resolvedActorId, now)
    : base.reachability;
  const riskTreatment = update.riskTreatment ?? base.riskTreatment;
  let riskDecisions = base.riskDecisions;
  if (update.riskTreatment !== undefined && update.riskTreatment !== base.riskTreatment && !update.riskDecision) {
    throw new Error("Changing risk treatment requires an audited risk decision.");
  }
  if (update.riskDecision) {
    if (riskTreatment === "accepted" && author) {
      throw new Error("Risk acceptance must be recorded by a human operator, not a model-authored revision.");
    }
    const decision = storedRiskDecision({
      treatment: riskTreatment,
      actorId: resolvedActorId,
      rationale: requiredText(update.riskDecision.rationale, "Risk decision rationale"),
      decidedAt: isoTimestamp(update.riskDecision.decidedAt ?? now, "Risk decision timestamp"),
      expiresAt: nullableIsoTimestamp(update.riskDecision.expiresAt, "Risk decision expiry"),
    });
    riskDecisions = [...base.riskDecisions, decision];
  }
  const cvssAssessments = update.cvssAssessment
    ? uniqueRecords([...base.cvssAssessments, revisedCvssAssessment(update.cvssAssessment, resolvedActorId, now)])
    : base.cvssAssessments;
  return {
    reachability,
    riskTreatment,
    riskDecisions,
    cvssAssessments,
    affectedAssetIds: update.affectedAssetIds === undefined
      ? base.affectedAssetIds
      : uniqueTrackingStrings(update.affectedAssetIds, "Affected asset id"),
    affectedVersions: update.affectedVersions === undefined
      ? base.affectedVersions
      : uniqueRecords(update.affectedVersions.map(storedAffectedVersion)),
    externalReferences: update.externalReferences === undefined
      ? base.externalReferences
      : uniqueRecords(update.externalReferences.map(storedExternalReference)),
  };
}

function revisedReachability(
  current: FindingSummary,
  input: NonNullable<FindingSecurityTrackingUpdate["reachability"]>,
  assessorId: string,
  now: string,
): FindingReachabilityAssessment {
  const state = findingReachabilityState(input.state);
  if (state === "not_assessed") return emptyFindingSecurityTracking().reachability;
  const evidenceIds = uniqueTrackingStrings(input.evidenceIds ?? [], "Reachability evidence id");
  if (evidenceIds.length === 0) throw new Error("Assessed reachability requires at least one finding evidence id.");
  const availableEvidenceIds = new Set(current.evidence.map((evidence) => evidence.id));
  const missing = evidenceIds.filter((evidenceId) => !availableEvidenceIds.has(evidenceId));
  if (missing.length > 0) throw new Error(`Reachability references unknown finding evidence: ${missing.join(", ")}.`);
  return {
    state,
    conditions: requiredText(input.conditions, "Reachability conditions"),
    evidenceIds,
    assessorId,
    assessedAt: isoTimestamp(input.assessedAt ?? now, "Reachability assessment timestamp"),
    sourceRevision: input.sourceRevision === undefined ? current.sourceRevision : nullableText(input.sourceRevision),
    environmentFingerprint: input.environmentFingerprint === undefined
      ? current.environmentFingerprint
      : nullableText(input.environmentFingerprint),
  };
}

function revisedCvssAssessment(
  input: NonNullable<FindingSecurityTrackingUpdate["cvssAssessment"]>,
  assessorId: string,
  now: string,
): FindingCvssAssessment {
  return storedCvssAssessment({
    ...input,
    assessorId,
    assessedAt: input.assessedAt ?? now,
  });
}

function storedReachability(value: unknown): FindingReachabilityAssessment {
  if (!isRecord(value)) throw new Error("Stored reachability assessment must be an object.");
  const state = findingReachabilityState(value.state);
  if (state === "not_assessed") return emptyFindingSecurityTracking().reachability;
  const evidenceIds = uniqueTrackingStrings(storedStringArray(value.evidenceIds), "Reachability evidence id");
  if (evidenceIds.length === 0) throw new Error("Assessed reachability requires at least one finding evidence id.");
  return {
    state,
    conditions: requiredText(value.conditions, "Reachability conditions"),
    evidenceIds,
    assessorId: requiredText(value.assessorId, "Reachability assessor"),
    assessedAt: isoTimestamp(value.assessedAt, "Reachability assessment timestamp"),
    sourceRevision: nullableText(value.sourceRevision),
    environmentFingerprint: nullableText(value.environmentFingerprint),
  };
}

function storedRiskDecision(value: unknown): FindingRiskDecision {
  if (!isRecord(value)) throw new Error("Stored risk decision must be an object.");
  const decidedAt = isoTimestamp(value.decidedAt, "Risk decision timestamp");
  const expiresAt = nullableIsoTimestamp(value.expiresAt, "Risk decision expiry");
  if (expiresAt && Date.parse(expiresAt) <= Date.parse(decidedAt)) {
    throw new Error("Risk decision expiry must be later than its decision timestamp.");
  }
  return {
    treatment: findingRiskTreatment(value.treatment),
    actorId: requiredText(value.actorId, "Risk decision actor"),
    rationale: requiredText(value.rationale, "Risk decision rationale"),
    decidedAt,
    expiresAt,
  };
}

function storedCvssAssessment(value: unknown): FindingCvssAssessment {
  if (!isRecord(value)) throw new Error("Stored CVSS assessment must be an object.");
  const version = value.version === "4.0" || value.version === "3.1" ? value.version : null;
  if (!version) throw new Error("CVSS version must be 4.0 or 3.1.");
  const vector = requiredText(value.vector, "CVSS vector");
  if (!vector.startsWith(`CVSS:${version}/`)) throw new Error(`CVSS ${version} vector must start with CVSS:${version}/.`);
  validateCvssVector(version, vector);
  const score = requiredSqlNumber(value.score);
  if (score < 0 || score > 10) throw new Error("CVSS score must be between 0 and 10.");
  const nomenclature = findingCvssNomenclature(value.nomenclature);
  if (version === "3.1" && nomenclature !== "CVSS:3.1") throw new Error("CVSS 3.1 assessments require CVSS:3.1 nomenclature.");
  if (version === "4.0" && nomenclature === "CVSS:3.1") throw new Error("CVSS 4.0 assessments require CVSS-B, CVSS-BT, CVSS-BE, or CVSS-BTE nomenclature.");
  return {
    version,
    vector,
    score,
    nomenclature,
    assessorId: requiredText(value.assessorId, "CVSS assessor"),
    assessedAt: isoTimestamp(value.assessedAt, "CVSS assessment timestamp"),
    environmentFingerprint: nullableText(value.environmentFingerprint),
  };
}

function validateCvssVector(version: "4.0" | "3.1", vector: string): void {
  const requiredMetrics: Readonly<Record<string, readonly string[]>> = version === "4.0" ? {
    AV: ["N", "A", "L", "P"], AC: ["L", "H"], AT: ["N", "P"], PR: ["N", "L", "H"], UI: ["N", "P", "A"],
    VC: ["H", "L", "N"], VI: ["H", "L", "N"], VA: ["H", "L", "N"],
    SC: ["H", "L", "N"], SI: ["H", "L", "N"], SA: ["H", "L", "N"],
  } : {
    AV: ["N", "A", "L", "P"], AC: ["L", "H"], PR: ["N", "L", "H"], UI: ["N", "R"],
    S: ["U", "C"], C: ["H", "L", "N"], I: ["H", "L", "N"], A: ["H", "L", "N"],
  };
  const metrics = new Map<string, string>();
  for (const segment of vector.split("/").slice(1)) {
    const [key, metricValue, ...extra] = segment.split(":");
    if (!key || !metricValue || extra.length > 0 || metrics.has(key)) throw new Error(`CVSS vector contains an invalid or duplicate metric: ${segment}.`);
    metrics.set(key, metricValue);
  }
  for (const [key, allowed] of Object.entries(requiredMetrics)) {
    const metricValue = metrics.get(key);
    if (!metricValue || !allowed.includes(metricValue)) throw new Error(`CVSS ${version} vector requires a valid ${key} metric.`);
  }
}

function storedAffectedVersion(value: unknown): FindingAffectedVersion {
  if (!isRecord(value)) throw new Error("Affected version must be an object.");
  return {
    assetId: nullableText(value.assetId),
    range: requiredText(value.range, "Affected version range"),
    fixedVersion: nullableText(value.fixedVersion),
  };
}

function storedExternalReference(value: unknown): FindingExternalReference {
  if (!isRecord(value)) throw new Error("External reference must be an object.");
  const kind = requiredText(value.kind, "External reference kind");
  if (!/^[a-z0-9]+(?:[._-][a-z0-9]+)*$/u.test(kind)) throw new Error("External reference kind must be a lowercase stable identifier.");
  const url = nullableText(value.url);
  if (url && !/^https?:\/\//u.test(url)) throw new Error("External reference URL must use http or https.");
  return { kind, identifier: requiredText(value.identifier, "External reference identifier"), url };
}

function findingReachabilityState(value: unknown): FindingReachabilityState {
  if (value === "not_assessed" || value === "unreachable" || value === "conditional" || value === "reachable") return value;
  throw new Error(`Invalid reachability state: ${String(value)}.`);
}

function findingRiskTreatment(value: unknown): FindingRiskTreatment {
  if (value === "unreviewed" || value === "remediate" || value === "mitigated" || value === "accepted" || value === "transferred") return value;
  throw new Error(`Invalid risk treatment: ${String(value)}.`);
}

function findingCvssNomenclature(value: unknown): FindingCvssAssessment["nomenclature"] {
  if (value === "CVSS-B" || value === "CVSS-BT" || value === "CVSS-BE" || value === "CVSS-BTE" || value === "CVSS:3.1") return value;
  throw new Error(`Invalid CVSS nomenclature: ${String(value)}.`);
}

function isoTimestamp(value: unknown, label: string): string {
  const timestamp = requiredText(value, label);
  if (!Number.isFinite(Date.parse(timestamp))) throw new Error(`${label} must be an ISO timestamp.`);
  return timestamp;
}

function nullableIsoTimestamp(value: unknown, label: string): string | null {
  return value === null || value === undefined ? null : isoTimestamp(value, label);
}

function parseJsonValue(value: unknown): unknown {
  if (typeof value !== "string") return null;
  return JSON.parse(value) as unknown;
}

function storedStringArray(value: unknown): string[] {
  if (value === undefined) return [];
  if (!Array.isArray(value)) throw new Error("Stored security tracking list must be an array.");
  return value.map((item) => requiredText(item, "Security tracking value"));
}

function storedArray<T>(value: unknown, parse: (item: unknown) => T): T[] {
  if (value === undefined) return [];
  if (!Array.isArray(value)) throw new Error("Stored security tracking collection must be an array.");
  return uniqueRecords(value.map(parse));
}

function uniqueTrackingStrings(values: readonly string[], label: string): string[] {
  return [...new Set(values.map((value) => requiredText(value, label)))];
}

function uniqueRecords<T>(values: readonly T[]): T[] {
  const seen = new Set<string>();
  return values.filter((value) => {
    const key = stableJson(value);
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

function stableFindingId(workspaceId: string, memoryNodeId: string): string {
  return `finding_${createHash("sha256").update(`${workspaceId}\0${memoryNodeId}`).digest("hex").slice(0, 20)}`;
}

function workspaceResourceExists(database: DatabaseSync, table: "app_server_runbooks" | "app_server_reports", id: string, workspaceId: string): boolean {
  return tableExists(database, table)
    && Boolean(database.prepare(`SELECT 1 FROM ${table} WHERE id = ? AND workspace_id = ?`).get(id, workspaceId));
}

function successfulRunbookExecutionExists(
  database: DatabaseSync,
  workspaceId: string,
  runbookId: string | null,
  runId: string,
): boolean {
  return Boolean(runbookId)
    && tableExists(database, "app_server_runbook_executions")
    && Boolean(database.prepare(`SELECT 1 FROM app_server_runbook_executions
      WHERE workspace_id = ? AND runbook_id = ? AND run_id = ? AND status = 'succeeded'`)
      .get(workspaceId, runbookId, runId));
}

type SqlRow = Record<string, unknown>;
function confidence(value: number): number { if (!Number.isFinite(value) || value < 0 || value > 1) throw new Error("Finding confidence must be between 0 and 1."); return value; }
function requiredText(value: unknown, label: string): string { const text = normalizedText(value); if (!text) throw new Error(`${label} must be a non-empty string.`); return text; }
function normalizedText(value: unknown): string | null { return typeof value === "string" && value.trim() ? value.trim() : null; }
function nullableText(value: unknown): string | null { return normalizedText(value); }
function requiredSqlText(value: unknown): string { if (typeof value !== "string") throw new Error("Expected SQLite text value."); return value; }
function optionalSqlText(value: unknown): string | null { return typeof value === "string" ? value : null; }
function requiredSqlNumber(value: unknown): number { if (typeof value !== "number" || !Number.isFinite(value)) throw new Error("Expected SQLite numeric value."); return value; }
function isRecord(value: unknown): value is Record<string, unknown> { return Boolean(value) && typeof value === "object" && !Array.isArray(value); }
function parseJsonObject(value: unknown): Record<string, unknown> { if (typeof value !== "string") return {}; const parsed = JSON.parse(value) as unknown; return isRecord(parsed) ? parsed : {}; }
function parseJsonStringArray(value: unknown): string[] { if (typeof value !== "string") return []; const parsed = JSON.parse(value) as unknown; return Array.isArray(parsed) ? parsed.filter((item): item is string => typeof item === "string") : []; }
function stableJson(value: unknown): string { if (Array.isArray(value)) return `[${value.map(stableJson).join(",")}]`; if (!value || typeof value !== "object") return JSON.stringify(value); return `{${Object.entries(value as Record<string, unknown>).filter(([, nested]) => nested !== undefined).sort(([left], [right]) => left.localeCompare(right)).map(([key, nested]) => `${JSON.stringify(key)}:${stableJson(nested)}`).join(",")}}`; }
function tableExists(database: DatabaseSync, table: string): boolean { return Boolean(database.prepare("SELECT 1 AS present FROM sqlite_master WHERE type = 'table' AND name = ?").get(table)); }
function tableHasColumn(database: DatabaseSync, table: string, column: string): boolean { return (database.prepare(`PRAGMA table_info(${table})`).all() as Array<{ name?: unknown }>).some((row) => row.name === column); }
function findingStatus(value: unknown): FindingStatus { if (value === "hypothesis" || value === "observed" || value === "reproduced" || value === "verified" || value === "report_ready" || value === "disclosed" || value === "stale" || value === "rejected") return value; throw new Error(`Invalid finding status: ${String(value)}.`); }
function researchClaimRating(value: unknown): ResearchClaimRating {
  if (value === undefined || value === null) return "informational";
  if (value === "informational" || value === "low" || value === "medium" || value === "high" || value === "critical") return value;
  throw new Error(`Invalid research claim rating: ${String(value)}.`);
}
function findingEvidenceKind(value: unknown): FindingEvidenceKind {
  const kind = normalizedText(value);
  if (kind && [
    "code", "artifact", "command", "url", "calculation", "proof", "publication", "human_review",
    "runbook_execution", "independent_verification", "report", "disclosure",
  ].includes(kind)) return kind as FindingEvidenceKind;
  throw new Error(`Invalid research-claim evidence kind: ${String(value)}.`);
}
function claimClassification(value: unknown): string {
  const classification = normalizedText(value);
  if (!classification || !/^[a-z0-9]+(?:[._-][a-z0-9]+)*$/u.test(classification)) {
    throw new Error("Research claim classification must be a lowercase stable identifier.");
  }
  return classification;
}
function uniqueStrings(values: readonly string[]): string[] {
  return [...new Set(values.map((value) => requiredText(value, "Component claim id")))];
}

export function findingIsTerminal(status: FindingStatus): boolean {
  return TERMINAL_FINDING_STATUSES.has(status);
}
