import { createHash } from "node:crypto";
import { chmodSync, mkdirSync } from "node:fs";
import { dirname } from "node:path";
import type { DatabaseSync } from "node:sqlite";
import { applyDatabaseMigrations } from "./database-migrations.js";
import { openResearchDatabase } from "./database.js";
import { initializeFindingSchema, LEGACY_CLAIM_MEMORY_TYPES, type ResearchClaimStore } from "./findings.js";
import type {
  CampaignExperimentProjectionSummary,
  CampaignObservationProjectionSummary,
  CampaignQuestionProjectionSummary,
  CampaignTrackProjectionSummary,
  FindingStatus,
  FindingSummary,
} from "./knowledge-types.js";
import { createId, nowIso } from "./ids.js";
import {
  MemoryGraphStore,
  type MemoryEvidenceRef,
  type MemoryNode,
  type MemoryNodeStatus,
  type MemoryNodeType,
} from "./memory-graph.js";
import {
  selectMemoryModelContext,
  type ResearchModelMemoryContextNode,
} from "./model-context.js";
import type { ModelAuthor } from "./model-authorship.js";
import type { ResearchMemoryContext } from "./types.js";

export const CAMPAIGN_TRACK_STATUSES = ["active", "blocked", "complete", "archived"] as const;
export type CampaignTrackStatus = (typeof CAMPAIGN_TRACK_STATUSES)[number];
export const CAMPAIGN_TRACK_STAGES = [
  "orienting", "exploring", "testing", "reproducing", "verifying", "reporting", "complete", "blocked",
] as const;
export type CampaignTrackStage = (typeof CAMPAIGN_TRACK_STAGES)[number];
export type CampaignTrackSource = "runtime" | "shadow" | "replay" | "manual";
export type CampaignTrackResourceKind = "session" | "memory" | "evidence" | "finding" | "runbook" | "report";

export interface CampaignTrackRecord {
  id: string;
  workspaceId: string;
  workspaceName: string;
  subjectId: string | null;
  subjectName: string | null;
  title: string;
  objective: string;
  status: CampaignTrackStatus;
  stage: CampaignTrackStage;
  source: CampaignTrackSource;
  originSessionId: string | null;
  sourceRevision: string | null;
  environmentFingerprint: string | null;
  createdAt: string;
  updatedAt: string;
  revision: number;
}

export interface CampaignTrackSummary extends CampaignTrackRecord {
  sessionIds: string[];
  counts: {
    questions: number;
    openQuestions: number;
    experiments: number;
    observations: number;
    openNextActions: number;
    memoryNodes: number;
    evidenceRefs: number;
    findings: number;
    runbooks: number;
    reports: number;
  };
}

export interface InvestigationQuestion {
  id: string;
  investigationId: string;
  text: string;
  status: "open" | "answered" | "blocked" | "superseded";
  priority: "critical" | "high" | "medium" | "low";
  answer: string;
  createdAt: string;
  updatedAt: string;
  revision: number;
}

export interface InvestigationExperiment {
  id: string;
  investigationId: string;
  questionId: string | null;
  hypothesisMemoryId: string | null;
  runbookId: string | null;
  title: string;
  status: "planned" | "running" | "succeeded" | "failed" | "inconclusive" | "blocked";
  expectedOutcomes: Record<string, unknown>;
  resultSummary: string;
  sourceRevision: string | null;
  environmentFingerprint: string | null;
  startedAt: string | null;
  completedAt: string | null;
  createdAt: string;
  updatedAt: string;
  revision: number;
}

export function campaignExperimentProjection(experiment: InvestigationExperiment): CampaignExperimentProjectionSummary {
  return {
    id: experiment.id,
    investigationId: experiment.investigationId,
    questionId: experiment.questionId,
    runbookId: experiment.runbookId,
    title: experiment.title,
    status: experiment.status,
    resultSummary: experiment.resultSummary,
    startedAt: experiment.startedAt,
    completedAt: experiment.completedAt,
    updatedAt: experiment.updatedAt,
    revision: experiment.revision,
  };
}

export function campaignQuestionProjection(question: InvestigationQuestion): CampaignQuestionProjectionSummary {
  return {
    id: question.id,
    investigationId: question.investigationId,
    text: question.text,
    status: question.status,
    priority: question.priority,
    answer: question.answer,
    updatedAt: question.updatedAt,
    revision: question.revision,
  };
}

export function campaignObservationProjection(observation: InvestigationObservation): CampaignObservationProjectionSummary {
  return {
    id: observation.id,
    investigationId: observation.investigationId,
    experimentId: observation.experimentId,
    kind: observation.kind,
    outcome: observation.outcome,
    summary: observation.summary,
    createdAt: observation.createdAt,
  };
}

export interface InvestigationObservation {
  id: string;
  investigationId: string;
  experimentId: string | null;
  memoryNodeId: string | null;
  kind: "source" | "runtime" | "artifact" | "verifier" | "human" | "historical";
  outcome: "supports" | "refutes" | "narrows" | "neutral";
  summary: string;
  evidenceRefIds: string[];
  sourceEventId: string | null;
  createdAt: string;
}

export interface InvestigationNextAction {
  id: string;
  investigationId: string;
  questionId: string | null;
  title: string;
  rationale: string;
  status: "open" | "in_progress" | "completed" | "dismissed";
  priority: "critical" | "high" | "medium" | "low";
  expectedInformationGain: number;
  estimatedCost: number;
  suggestedPrompt: string;
  createdAt: string;
  updatedAt: string;
  revision: number;
}

export interface CampaignTrackDetail extends CampaignTrackSummary {
  questions: InvestigationQuestion[];
  experiments: InvestigationExperiment[];
  observations: InvestigationObservation[];
  nextActions: InvestigationNextAction[];
}

export interface CampaignTrackStatusSnapshot {
  investigation: CampaignTrackSummary;
  revision: number;
  unchanged: boolean;
  questions: InvestigationQuestion[];
  experiments: InvestigationExperiment[];
  observations: InvestigationObservation[];
  nextActions: InvestigationNextAction[];
}

export interface InvestigationReplayMetrics {
  schemaVersion: 1;
  mode: "historical" | "shadow" | "active";
  workspaceId: string;
  sessionCount: number;
  generatedTrackCount: number;
  linkedMemoryNodeCount: number;
  repeatedMemoryCandidateCount: number;
  rejectedHypothesisResurrectionCount: number;
  environmentTaggedNodeRate: number;
  crossSessionReuseRate: number;
  medianMinutesToFirstEvidence: number | null;
}

export interface InvestigationReplayResult {
  id: string;
  persisted: boolean;
  createdAt: string;
  metrics: InvestigationReplayMetrics;
  tracks: Array<{ title: string; sessionIds: string[]; memoryNodeIds: string[] }>;
}

export interface StageAwareRecallResult {
  investigation: CampaignTrackSummary | null;
  stage: CampaignTrackStage;
  nodes: ResearchModelMemoryContextNode[];
  selection: Array<{ id: string; lane: RecallLane; score: number }>;
}

export interface ConsolidationCandidate {
  id: string;
  workspaceId: string;
  proposedType: "procedure" | "invariant" | "trajectory";
  title: string;
  summary: string;
  status: "candidate" | "accepted" | "rejected";
  sourceInvestigationIds: string[];
  sourceMemoryNodeIds: string[];
  promotedMemoryNodeId: string | null;
  createdAt: string;
  reviewedAt: string | null;
}

type RecallLane = "current" | "target" | "claim" | "negative" | "procedure" | "analogy";

interface CampaignTrackStoreOptions {
  databasePath: string;
  context: ResearchMemoryContext;
  memoryGraph?: MemoryGraphStore;
  claimStore?: ResearchClaimStore;
}

interface EnsureCampaignTrackInput {
  sessionId: string;
  title?: string;
  objective: string;
  source: CampaignTrackSource;
  continuationTrackId?: string;
  allowSimilarMatch?: boolean;
  sourceRevision?: string | null;
  environmentFingerprint?: string | null;
}

interface CreateCampaignTrackInput {
  title: string;
  objective: string;
  stage?: CampaignTrackStage;
  source?: CampaignTrackSource;
  originSessionId?: string | null;
  sourceRevision?: string | null;
  environmentFingerprint?: string | null;
}

interface RecallOptions {
  investigationId?: string;
  query: string;
  stage?: CampaignTrackStage;
  maxNodes?: number;
  maxCharacters?: number;
}

const SESSION_TITLE_STOP_WORDS = new Set([
  "apple", "ios", "macos", "research", "security", "analysis", "audit", "testing", "test",
  "boundaries", "boundary", "behavior", "behaviour", "current", "latest", "session", "support",
]);

export class CampaignTrackStore {
  public readonly databasePath: string;
  private readonly database: DatabaseSync;
  private readonly context: ResearchMemoryContext;
  private readonly memoryGraph: MemoryGraphStore | undefined;
  private readonly claimStore: ResearchClaimStore | undefined;

  public constructor(options: CampaignTrackStoreOptions) {
    this.databasePath = options.databasePath;
    this.context = { ...options.context };
    this.memoryGraph = options.memoryGraph;
    this.claimStore = options.claimStore;
    mkdirSync(dirname(options.databasePath), { recursive: true });
    this.database = openResearchDatabase(options.databasePath);
    if (options.databasePath !== ":memory:") chmodSync(options.databasePath, 0o600);
    this.database.exec("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000;");
    MemoryGraphStore.initializeSchema(this.database);
    initializeFindingSchema(this.database);
    CampaignTrackStore.initializeSchema(this.database);
  }

  public close(): void {
    this.database.close();
  }

  public static initializeSchema(database: DatabaseSync): void {
    applyDatabaseMigrations(database, "app_server_campaign_tracks", [{
      version: 1,
      name: "evidence_governed_campaign_tracks",
      up(db) {
        db.exec(`
          CREATE TABLE IF NOT EXISTS campaign_tracks (
            id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL,
            workspace_name TEXT NOT NULL,
            subject_id TEXT,
            subject_name TEXT,
            title TEXT NOT NULL,
            title_norm TEXT NOT NULL,
            objective TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL CHECK (status IN ('active', 'blocked', 'complete', 'archived')),
            stage TEXT NOT NULL CHECK (stage IN ('orienting', 'exploring', 'testing', 'reproducing', 'verifying', 'reporting', 'complete', 'blocked')),
            source TEXT NOT NULL CHECK (source IN ('runtime', 'shadow', 'replay', 'manual')),
            origin_session_id TEXT,
            source_revision TEXT,
            environment_fingerprint TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            revision INTEGER NOT NULL CHECK (revision > 0),
            UNIQUE(workspace_id, title_norm)
          );
          CREATE INDEX IF NOT EXISTS campaign_tracks_workspace_updated_idx ON campaign_tracks(workspace_id, updated_at DESC);

          CREATE TABLE IF NOT EXISTS campaign_track_sessions (
            investigation_id TEXT NOT NULL REFERENCES campaign_tracks(id) ON DELETE CASCADE,
            session_id TEXT NOT NULL,
            linked_at TEXT NOT NULL,
            PRIMARY KEY(investigation_id, session_id)
          );
          CREATE INDEX IF NOT EXISTS campaign_track_sessions_session_idx ON campaign_track_sessions(session_id, investigation_id);

          CREATE TABLE IF NOT EXISTS campaign_track_resources (
            investigation_id TEXT NOT NULL REFERENCES campaign_tracks(id) ON DELETE CASCADE,
            resource_kind TEXT NOT NULL CHECK (resource_kind IN ('session', 'memory', 'evidence', 'finding', 'runbook', 'report')),
            resource_id TEXT NOT NULL,
            role TEXT NOT NULL DEFAULT 'produced',
            linked_at TEXT NOT NULL,
            PRIMARY KEY(investigation_id, resource_kind, resource_id)
          );
          CREATE INDEX IF NOT EXISTS campaign_track_resources_resource_idx ON campaign_track_resources(resource_kind, resource_id);

          CREATE TABLE IF NOT EXISTS campaign_track_questions (
            id TEXT PRIMARY KEY,
            investigation_id TEXT NOT NULL REFERENCES campaign_tracks(id) ON DELETE CASCADE,
            text TEXT NOT NULL,
            text_norm TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('open', 'answered', 'blocked', 'superseded')),
            priority TEXT NOT NULL CHECK (priority IN ('critical', 'high', 'medium', 'low')),
            answer TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            revision INTEGER NOT NULL CHECK (revision > 0),
            UNIQUE(investigation_id, text_norm)
          );

          CREATE TABLE IF NOT EXISTS campaign_track_experiments (
            id TEXT PRIMARY KEY,
            investigation_id TEXT NOT NULL REFERENCES campaign_tracks(id) ON DELETE CASCADE,
            question_id TEXT REFERENCES campaign_track_questions(id) ON DELETE SET NULL,
            hypothesis_memory_id TEXT,
            runbook_id TEXT,
            title TEXT NOT NULL,
            title_norm TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('planned', 'running', 'succeeded', 'failed', 'inconclusive', 'blocked')),
            expected_outcomes_json TEXT NOT NULL DEFAULT '{}',
            result_summary TEXT NOT NULL DEFAULT '',
            source_revision TEXT,
            environment_fingerprint TEXT,
            started_at TEXT,
            completed_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            revision INTEGER NOT NULL CHECK (revision > 0),
            UNIQUE(investigation_id, title_norm)
          );

          CREATE TABLE IF NOT EXISTS campaign_track_observations (
            id TEXT PRIMARY KEY,
            investigation_id TEXT NOT NULL REFERENCES campaign_tracks(id) ON DELETE CASCADE,
            experiment_id TEXT REFERENCES campaign_track_experiments(id) ON DELETE SET NULL,
            memory_node_id TEXT,
            kind TEXT NOT NULL CHECK (kind IN ('source', 'runtime', 'artifact', 'verifier', 'human', 'historical')),
            outcome TEXT NOT NULL CHECK (outcome IN ('supports', 'refutes', 'narrows', 'neutral')),
            summary TEXT NOT NULL,
            source_event_id TEXT,
            created_at TEXT NOT NULL
          );
          CREATE INDEX IF NOT EXISTS campaign_track_observations_investigation_idx ON campaign_track_observations(investigation_id, created_at DESC);

          CREATE TABLE IF NOT EXISTS campaign_track_observation_evidence (
            observation_id TEXT NOT NULL REFERENCES campaign_track_observations(id) ON DELETE CASCADE,
            evidence_ref_id TEXT NOT NULL,
            PRIMARY KEY(observation_id, evidence_ref_id)
          );

          CREATE TABLE IF NOT EXISTS campaign_track_next_actions (
            id TEXT PRIMARY KEY,
            investigation_id TEXT NOT NULL REFERENCES campaign_tracks(id) ON DELETE CASCADE,
            question_id TEXT REFERENCES campaign_track_questions(id) ON DELETE SET NULL,
            title TEXT NOT NULL,
            title_norm TEXT NOT NULL,
            rationale TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL CHECK (status IN ('open', 'in_progress', 'completed', 'dismissed')),
            priority TEXT NOT NULL CHECK (priority IN ('critical', 'high', 'medium', 'low')),
            expected_information_gain REAL NOT NULL DEFAULT 0.5 CHECK (expected_information_gain >= 0 AND expected_information_gain <= 1),
            estimated_cost REAL NOT NULL DEFAULT 0.5 CHECK (estimated_cost >= 0 AND estimated_cost <= 1),
            suggested_prompt TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            revision INTEGER NOT NULL CHECK (revision > 0),
            UNIQUE(investigation_id, title_norm)
          );

          CREATE TABLE IF NOT EXISTS campaign_track_claim_reviews (
            id TEXT PRIMARY KEY,
            investigation_id TEXT NOT NULL REFERENCES campaign_tracks(id) ON DELETE CASCADE,
            source_memory_node_id TEXT NOT NULL,
            source_memory_revision INTEGER NOT NULL CHECK (source_memory_revision > 0),
            promoted_memory_node_id TEXT,
            verdict TEXT NOT NULL CHECK (verdict IN ('accept', 'revise', 'reject')),
            rationale TEXT NOT NULL,
            evidence_ref_ids_json TEXT NOT NULL DEFAULT '[]',
            reviewer_session_id TEXT,
            reviewer_agent_id TEXT,
            independent INTEGER NOT NULL CHECK (independent IN (0, 1)),
            created_at TEXT NOT NULL
          );
          CREATE INDEX IF NOT EXISTS campaign_track_claim_reviews_memory_idx ON campaign_track_claim_reviews(source_memory_node_id, created_at DESC);

          CREATE TABLE IF NOT EXISTS campaign_track_replay_runs (
            id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL,
            persisted INTEGER NOT NULL CHECK (persisted IN (0, 1)),
            metrics_json TEXT NOT NULL,
            created_at TEXT NOT NULL
          );

          CREATE TABLE IF NOT EXISTS campaign_track_consolidations (
            id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL,
            proposed_type TEXT NOT NULL CHECK (proposed_type IN ('procedure', 'invariant', 'trajectory')),
            title TEXT NOT NULL,
            summary TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('candidate', 'accepted', 'rejected')),
            source_investigation_ids_json TEXT NOT NULL,
            source_memory_node_ids_json TEXT NOT NULL,
            promoted_memory_node_id TEXT,
            created_at TEXT NOT NULL,
            reviewed_at TEXT
          );

          CREATE TRIGGER IF NOT EXISTS campaign_track_link_memory_on_session
          AFTER INSERT ON memory_node_sessions
          BEGIN
            INSERT OR IGNORE INTO campaign_track_resources(investigation_id, resource_kind, resource_id, role, linked_at)
            SELECT investigation_id, 'memory', NEW.node_id, 'produced', strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            FROM campaign_track_sessions WHERE session_id = NEW.session_id;
          END;

          CREATE TRIGGER IF NOT EXISTS campaign_track_link_evidence_on_create
          AFTER INSERT ON memory_evidence_refs
          BEGIN
            INSERT OR IGNORE INTO campaign_track_resources(investigation_id, resource_kind, resource_id, role, linked_at)
            SELECT investigation_id, 'evidence', NEW.id, 'supports', NEW.created_at
            FROM campaign_track_resources
            WHERE resource_kind = 'memory' AND resource_id = NEW.node_id;
          END;

          CREATE TRIGGER IF NOT EXISTS campaign_track_unlink_evidence_on_delete
          AFTER DELETE ON memory_evidence_refs
          BEGIN
            DELETE FROM campaign_track_resources WHERE resource_kind = 'evidence' AND resource_id = OLD.id;
          END;

          CREATE TRIGGER IF NOT EXISTS campaign_track_link_runbook_on_create
          AFTER INSERT ON app_server_runbooks WHEN NEW.session_id IS NOT NULL
          BEGIN
            INSERT OR IGNORE INTO campaign_track_resources(investigation_id, resource_kind, resource_id, role, linked_at)
            SELECT investigation_id, 'runbook', NEW.id, 'produced', NEW.created_at
            FROM campaign_track_sessions WHERE session_id = NEW.session_id;
          END;

          CREATE TRIGGER IF NOT EXISTS campaign_track_link_report_on_create
          AFTER INSERT ON app_server_reports WHEN NEW.session_id IS NOT NULL
          BEGIN
            INSERT OR IGNORE INTO campaign_track_resources(investigation_id, resource_kind, resource_id, role, linked_at)
            SELECT investigation_id, 'report', NEW.id, 'produced', NEW.created_at
            FROM campaign_track_sessions WHERE session_id = NEW.session_id;
          END;
        `);
      },
    }, {
      version: 2,
      name: "canonical_research_claim_links",
      up(db) {
        db.exec(`
          DROP TRIGGER IF EXISTS campaign_track_link_finding_on_create;
          CREATE TABLE IF NOT EXISTS campaign_track_research_claim_reviews (
            id TEXT PRIMARY KEY,
            investigation_id TEXT NOT NULL REFERENCES campaign_tracks(id) ON DELETE CASCADE,
            claim_id TEXT NOT NULL REFERENCES app_server_research_claims(id) ON DELETE CASCADE,
            claim_revision INTEGER NOT NULL CHECK (claim_revision > 0),
            resulting_revision INTEGER NOT NULL CHECK (resulting_revision > 0),
            verdict TEXT NOT NULL CHECK (verdict IN ('accept', 'revise', 'reject')),
            rationale TEXT NOT NULL,
            evidence_ids_json TEXT NOT NULL DEFAULT '[]',
            reviewer_session_id TEXT,
            reviewer_agent_id TEXT,
            independent INTEGER NOT NULL CHECK (independent IN (0, 1)),
            created_at TEXT NOT NULL
          );
          CREATE INDEX IF NOT EXISTS campaign_track_research_claim_reviews_claim_idx
            ON campaign_track_research_claim_reviews(claim_id, created_at DESC);
          CREATE TRIGGER IF NOT EXISTS campaign_track_link_claim_on_create
          AFTER INSERT ON app_server_research_claims WHEN NEW.origin_session_id IS NOT NULL
          BEGIN
            INSERT OR IGNORE INTO campaign_track_resources(investigation_id, resource_kind, resource_id, role, linked_at)
            SELECT investigation_id, 'finding', NEW.id, 'produced', NEW.created_at
            FROM campaign_track_sessions WHERE session_id = NEW.origin_session_id;
          END;
        `);
      },
    }]);
  }

  public ensureForSession(input: EnsureCampaignTrackInput): CampaignTrackRecord {
    const linked = this.database.prepare(`
      SELECT t.* FROM campaign_tracks t
      JOIN campaign_track_sessions s ON s.investigation_id = t.id
      WHERE s.session_id = ? AND t.workspace_id = ? AND t.status <> 'archived'
      ORDER BY t.updated_at DESC LIMIT 1
    `).get(input.sessionId, this.context.workspaceId) as SqlRow | undefined;
    if (linked) return trackFromRow(linked);

    const requestedTitle = input.title?.trim();
    const sessionTitle = requestedTitle && !isPlaceholderTitle(requestedTitle)
      ? requestedTitle
      : this.sessionTitle(input.sessionId) ?? titleFromObjective(input.objective);
    const explicitContinuation = input.continuationTrackId
      ? this.get(input.continuationTrackId)
      : null;
    if (input.continuationTrackId && (!explicitContinuation || explicitContinuation.status === "archived")) {
      throw new Error(`Campaign continuation track is unavailable: ${input.continuationTrackId}`);
    }
    const candidates = input.allowSimilarMatch ? this.list({ includeArchived: false }) : [];
    const signature = researchSignature(`${sessionTitle} ${input.objective}`);
    const closest = candidates
      .map((candidate) => ({
        candidate,
        score: signatureSimilarity(signature, researchSignature(`${candidate.title} ${candidate.objective}`)),
        overlap: intersectionSize(signature, researchSignature(`${candidate.title} ${candidate.objective}`)),
      }))
      .sort((left, right) => right.score - left.score || right.overlap - left.overlap || right.candidate.updatedAt.localeCompare(left.candidate.updatedAt))[0];
    const track = explicitContinuation
      ?? (closest && closest.score >= 0.62 && closest.overlap >= 3 ? closest.candidate : null)
      ?? this.create({
          title: sessionTitle,
          objective: input.objective,
          stage: inferStage(input.objective),
          source: input.source,
          originSessionId: input.sessionId,
          sourceRevision: input.sourceRevision ?? null,
          environmentFingerprint: input.environmentFingerprint ?? null,
        });
    this.linkSession(track.id, input.sessionId);
    return this.get(track.id) ?? track;
  }

  public create(input: CreateCampaignTrackInput): CampaignTrackRecord {
    return this.createRecord(input);
  }

  private createRecord(
    input: CreateCampaignTrackInput,
    identity: Pick<CampaignTrackRecord, "workspaceName" | "subjectId" | "subjectName"> = this.context,
  ): CampaignTrackRecord {
    const title = requiredText(input.title, "campaign track title");
    const titleNorm = normalizeText(title);
    const existing = this.database.prepare(
      "SELECT * FROM campaign_tracks WHERE workspace_id = ? AND title_norm = ?",
    ).get(this.context.workspaceId, titleNorm) as SqlRow | undefined;
    if (existing) return trackFromRow(existing);
    const now = nowIso();
    const id = stableId("investigation", `${this.context.workspaceId}\0${titleNorm}`);
    this.database.prepare(`
      INSERT INTO campaign_tracks(
        id, workspace_id, workspace_name, subject_id, subject_name, title, title_norm, objective,
        status, stage, source, origin_session_id, source_revision, environment_fingerprint,
        created_at, updated_at, revision
      ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'active', ?, ?, ?, ?, ?, ?, ?, 1)
    `).run(
      id, this.context.workspaceId, identity.workspaceName,
      identity.subjectId ?? null, identity.subjectName ?? null,
      title, titleNorm, input.objective.trim(), input.stage ?? inferStage(input.objective), input.source ?? "manual",
      input.originSessionId ?? null, input.sourceRevision ?? null, input.environmentFingerprint ?? null, now, now,
    );
    if (input.originSessionId) this.linkSession(id, input.originSessionId);
    return this.get(id)!;
  }

  public get(id: string): CampaignTrackRecord | null {
    const row = this.database.prepare(
      "SELECT * FROM campaign_tracks WHERE id = ? AND workspace_id = ?",
    ).get(id, this.context.workspaceId) as SqlRow | undefined;
    return row ? trackFromRow(row) : null;
  }

  public getForSession(sessionId: string): CampaignTrackRecord | null {
    const row = this.database.prepare(`
      SELECT t.* FROM campaign_tracks t
      JOIN campaign_track_sessions s ON s.investigation_id = t.id
      WHERE s.session_id = ? AND t.workspace_id = ? AND t.status <> 'archived'
      ORDER BY t.updated_at DESC LIMIT 1
    `).get(sessionId, this.context.workspaceId) as SqlRow | undefined;
    return row ? trackFromRow(row) : null;
  }

  public list(options: { includeArchived?: boolean } = {}): CampaignTrackSummary[] {
    const rows = this.database.prepare(`
      SELECT * FROM campaign_tracks WHERE workspace_id = ? ${options.includeArchived ? "" : "AND status <> 'archived'"}
      ORDER BY CASE status WHEN 'active' THEN 0 WHEN 'blocked' THEN 1 WHEN 'complete' THEN 2 ELSE 3 END, updated_at DESC
    `).all(this.context.workspaceId) as SqlRow[];
    return rows.map((row) => this.summary(trackFromRow(row)));
  }

  public detail(id: string): CampaignTrackDetail | null {
    const track = this.get(id);
    if (!track) return null;
    return {
      ...this.summary(track),
      questions: this.questions(id),
      experiments: this.experiments(id),
      observations: this.observations(id),
      nextActions: this.nextActions(id),
    };
  }

  public status(id: string, options: { afterRevision?: number; limit?: number } = {}): CampaignTrackStatusSnapshot | null {
    const track = this.get(id);
    if (!track) return null;
    const investigation = this.summary(track);
    if (options.afterRevision !== undefined && options.afterRevision >= investigation.revision) {
      return {
        investigation,
        revision: investigation.revision,
        unchanged: true,
        questions: [],
        experiments: [],
        observations: [],
        nextActions: [],
      };
    }
    const limit = Math.max(1, Math.min(20, Math.trunc(options.limit ?? 5)));
    return {
      investigation,
      revision: investigation.revision,
      unchanged: false,
      questions: this.questions(id).filter((question) => question.status === "open" || question.status === "blocked").slice(0, limit),
      experiments: this.experiments(id).filter((experiment) => experiment.status === "planned" || experiment.status === "running").slice(0, limit),
      observations: this.observations(id).slice(0, limit),
      nextActions: this.nextActions(id).filter((action) => action.status === "open" || action.status === "in_progress").slice(0, limit),
    };
  }

  public updateTrack(id: string, expectedRevision: number, patch: {
    title?: string;
    objective?: string;
    status?: CampaignTrackStatus;
    stage?: CampaignTrackStage;
  }): CampaignTrackRecord {
    const track = this.requireTrack(id);
    if (track.revision !== expectedRevision) throw new Error(`Campaign track revision conflict for ${id}.`);
    const title = patch.title?.trim() || track.title;
    const status = patch.status ?? track.status;
    const stage = patch.stage ?? (status === "blocked" ? "blocked" : status === "complete" ? "complete" : track.stage);
    this.database.prepare(`
      UPDATE campaign_tracks SET title = ?, title_norm = ?, objective = ?, status = ?, stage = ?, updated_at = ?, revision = revision + 1
      WHERE id = ? AND revision = ?
    `).run(title, normalizeText(title), patch.objective?.trim() ?? track.objective, status, stage, nowIso(), id, expectedRevision);
    return this.requireTrack(id);
  }

  public linkSession(investigationId: string, sessionId: string): void {
    this.requireTrack(investigationId);
    const now = nowIso();
    this.database.prepare(
      "INSERT OR IGNORE INTO campaign_track_sessions(investigation_id, session_id, linked_at) VALUES (?, ?, ?)",
    ).run(investigationId, requiredText(sessionId, "sessionId"), now);
    this.database.prepare(
      "INSERT OR IGNORE INTO campaign_track_resources(investigation_id, resource_kind, resource_id, role, linked_at) VALUES (?, 'session', ?, 'execution', ?)",
    ).run(investigationId, sessionId, now);
    this.backfillSessionResources(investigationId, sessionId);
    this.touch(investigationId);
  }

  public linkResource(investigationId: string, kind: CampaignTrackResourceKind, resourceId: string, role = "produced"): void {
    this.requireTrack(investigationId);
    this.database.prepare(`
      INSERT INTO campaign_track_resources(investigation_id, resource_kind, resource_id, role, linked_at)
      VALUES (?, ?, ?, ?, ?)
      ON CONFLICT(investigation_id, resource_kind, resource_id) DO UPDATE SET role = excluded.role
    `).run(investigationId, kind, requiredText(resourceId, "resourceId"), role.trim() || "produced", nowIso());
    if (kind === "memory" && tableExists(this.database, "memory_evidence_refs")) {
      this.database.prepare(`
        INSERT OR IGNORE INTO campaign_track_resources(investigation_id, resource_kind, resource_id, role, linked_at)
        SELECT ?, 'evidence', id, 'supports', created_at FROM memory_evidence_refs WHERE node_id = ?
      `).run(investigationId, resourceId);
    }
    this.touch(investigationId);
  }

  public upsertQuestion(input: {
    investigationId: string;
    text: string;
    status?: InvestigationQuestion["status"];
    priority?: InvestigationQuestion["priority"];
    answer?: string;
  }): InvestigationQuestion {
    this.requireTrack(input.investigationId);
    const text = requiredText(input.text, "question");
    const norm = normalizeText(text);
    const existing = this.database.prepare(
      "SELECT * FROM campaign_track_questions WHERE investigation_id = ? AND text_norm = ?",
    ).get(input.investigationId, norm) as SqlRow | undefined;
    const now = nowIso();
    if (existing) {
      this.database.prepare(`
        UPDATE campaign_track_questions SET status = ?, priority = ?, answer = ?, updated_at = ?, revision = revision + 1 WHERE id = ?
      `).run(input.status ?? textValue(existing.status) as InvestigationQuestion["status"], input.priority ?? textValue(existing.priority) as InvestigationQuestion["priority"], input.answer?.trim() ?? textValue(existing.answer), now, textValue(existing.id));
    } else {
      const id = stableId("question", `${input.investigationId}\0${norm}`);
      this.database.prepare(`
        INSERT INTO campaign_track_questions(id, investigation_id, text, text_norm, status, priority, answer, created_at, updated_at, revision)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1)
      `).run(id, input.investigationId, text, norm, input.status ?? "open", input.priority ?? "high", input.answer?.trim() ?? "", now, now);
    }
    this.touch(input.investigationId);
    return questionFromRow(this.database.prepare(
      "SELECT * FROM campaign_track_questions WHERE investigation_id = ? AND text_norm = ?",
    ).get(input.investigationId, norm) as SqlRow);
  }

  public upsertExperiment(input: {
    investigationId: string;
    title: string;
    status?: InvestigationExperiment["status"];
    questionId?: string | null;
    hypothesisMemoryId?: string | null;
    runbookId?: string | null;
    expectedOutcomes?: Record<string, unknown>;
    resultSummary?: string;
    sourceRevision?: string | null;
    environmentFingerprint?: string | null;
  }): InvestigationExperiment {
    const track = this.requireTrack(input.investigationId);
    if (input.questionId) this.requireQuestion(input.questionId, input.investigationId);
    const title = requiredText(input.title, "experiment title");
    const norm = normalizeText(title);
    const existing = this.database.prepare(
      "SELECT * FROM campaign_track_experiments WHERE investigation_id = ? AND title_norm = ?",
    ).get(input.investigationId, norm) as SqlRow | undefined;
    const now = nowIso();
    const status = input.status ?? (existing ? textValue(existing.status) as InvestigationExperiment["status"] : "planned");
    const startedAt = status === "running" ? optionalText(existing?.started_at) ?? now : optionalText(existing?.started_at);
    const completedAt = ["succeeded", "failed", "inconclusive", "blocked"].includes(status) ? optionalText(existing?.completed_at) ?? now : null;
    if (existing) {
      this.database.prepare(`
        UPDATE campaign_track_experiments SET question_id = ?, hypothesis_memory_id = ?, runbook_id = ?, status = ?,
          expected_outcomes_json = ?, result_summary = ?, source_revision = ?, environment_fingerprint = ?,
          started_at = ?, completed_at = ?, updated_at = ?, revision = revision + 1 WHERE id = ?
      `).run(
        input.questionId ?? optionalText(existing.question_id), input.hypothesisMemoryId ?? optionalText(existing.hypothesis_memory_id),
        input.runbookId ?? optionalText(existing.runbook_id), status,
        JSON.stringify(input.expectedOutcomes ?? jsonObject(existing.expected_outcomes_json)), input.resultSummary?.trim() ?? textValue(existing.result_summary),
        input.sourceRevision ?? optionalText(existing.source_revision) ?? track.sourceRevision,
        input.environmentFingerprint ?? optionalText(existing.environment_fingerprint) ?? track.environmentFingerprint,
        startedAt, completedAt, now, textValue(existing.id),
      );
    } else {
      const id = stableId("experiment", `${input.investigationId}\0${norm}`);
      this.database.prepare(`
        INSERT INTO campaign_track_experiments(
          id, investigation_id, question_id, hypothesis_memory_id, runbook_id, title, title_norm, status,
          expected_outcomes_json, result_summary, source_revision, environment_fingerprint,
          started_at, completed_at, created_at, updated_at, revision
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1)
      `).run(
        id, input.investigationId, input.questionId ?? null, input.hypothesisMemoryId ?? null, input.runbookId ?? null,
        title, norm, status, JSON.stringify(input.expectedOutcomes ?? {}), input.resultSummary?.trim() ?? "",
        input.sourceRevision ?? track.sourceRevision, input.environmentFingerprint ?? track.environmentFingerprint,
        startedAt, completedAt, now, now,
      );
    }
    if (input.hypothesisMemoryId) this.linkResource(input.investigationId, "memory", input.hypothesisMemoryId, "hypothesis");
    if (input.runbookId) this.linkResource(input.investigationId, "runbook", input.runbookId, "experiment");
    this.setStageForExperiment(input.investigationId, status);
    return experimentFromRow(this.database.prepare(
      "SELECT * FROM campaign_track_experiments WHERE investigation_id = ? AND title_norm = ?",
    ).get(input.investigationId, norm) as SqlRow);
  }

  public addObservation(input: {
    investigationId: string;
    experimentId?: string | null;
    memoryNodeId?: string | null;
    kind: InvestigationObservation["kind"];
    outcome: InvestigationObservation["outcome"];
    summary: string;
    evidenceRefIds?: readonly string[];
    sourceEventId?: string | null;
  }): InvestigationObservation {
    this.requireTrack(input.investigationId);
    if (input.experimentId) this.requireExperiment(input.experimentId, input.investigationId);
    const evidenceRefIds = unique(input.evidenceRefIds ?? []);
    this.validateEvidenceIds(evidenceRefIds, input.memoryNodeId ?? null);
    const now = nowIso();
    const id = createId("observation");
    this.database.prepare(`
      INSERT INTO campaign_track_observations(
        id, investigation_id, experiment_id, memory_node_id, kind, outcome, summary, source_event_id, created_at
      ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
    `).run(id, input.investigationId, input.experimentId ?? null, input.memoryNodeId ?? null, input.kind, input.outcome, requiredText(input.summary, "observation summary"), input.sourceEventId ?? null, now);
    const insertEvidence = this.database.prepare(
      "INSERT OR IGNORE INTO campaign_track_observation_evidence(observation_id, evidence_ref_id) VALUES (?, ?)",
    );
    const linkEvidence = this.database.prepare(
      "INSERT OR IGNORE INTO campaign_track_resources(investigation_id, resource_kind, resource_id, role, linked_at) VALUES (?, 'evidence', ?, 'observation', ?)",
    );
    for (const evidenceId of evidenceRefIds) {
      insertEvidence.run(id, evidenceId);
      linkEvidence.run(input.investigationId, evidenceId, now);
    }
    if (input.memoryNodeId) this.linkResource(input.investigationId, "memory", input.memoryNodeId, "observed");
    this.touch(input.investigationId);
    return this.observation(id)!;
  }

  public upsertNextAction(input: {
    investigationId: string;
    title: string;
    rationale?: string;
    status?: InvestigationNextAction["status"];
    priority?: InvestigationNextAction["priority"];
    questionId?: string | null;
    expectedInformationGain?: number;
    estimatedCost?: number;
    suggestedPrompt?: string;
  }): InvestigationNextAction {
    this.requireTrack(input.investigationId);
    if (input.questionId) this.requireQuestion(input.questionId, input.investigationId);
    const title = requiredText(input.title, "next action title");
    const norm = normalizeText(title);
    const existing = this.database.prepare(
      "SELECT * FROM campaign_track_next_actions WHERE investigation_id = ? AND title_norm = ?",
    ).get(input.investigationId, norm) as SqlRow | undefined;
    const now = nowIso();
    const informationGain = bounded(input.expectedInformationGain ?? numberValue(existing?.expected_information_gain, 0.5));
    const estimatedCost = bounded(input.estimatedCost ?? numberValue(existing?.estimated_cost, 0.5));
    if (existing) {
      this.database.prepare(`
        UPDATE campaign_track_next_actions SET question_id = ?, rationale = ?, status = ?, priority = ?,
          expected_information_gain = ?, estimated_cost = ?, suggested_prompt = ?, updated_at = ?, revision = revision + 1
        WHERE id = ?
      `).run(
        input.questionId ?? optionalText(existing.question_id), input.rationale?.trim() ?? textValue(existing.rationale),
        input.status ?? textValue(existing.status), input.priority ?? textValue(existing.priority), informationGain, estimatedCost,
        input.suggestedPrompt?.trim() ?? textValue(existing.suggested_prompt), now, textValue(existing.id),
      );
    } else {
      const id = stableId("next_action", `${input.investigationId}\0${norm}`);
      this.database.prepare(`
        INSERT INTO campaign_track_next_actions(
          id, investigation_id, question_id, title, title_norm, rationale, status, priority,
          expected_information_gain, estimated_cost, suggested_prompt, created_at, updated_at, revision
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1)
      `).run(
        id, input.investigationId, input.questionId ?? null, title, norm, input.rationale?.trim() ?? "", input.status ?? "open",
        input.priority ?? "high", informationGain, estimatedCost, input.suggestedPrompt?.trim() ?? "", now, now,
      );
    }
    this.touch(input.investigationId);
    return nextActionFromRow(this.database.prepare(
      "SELECT * FROM campaign_track_next_actions WHERE investigation_id = ? AND title_norm = ?",
    ).get(input.investigationId, norm) as SqlRow);
  }

  public recall(options: RecallOptions): StageAwareRecallResult {
    const memoryGraph = this.requireMemoryGraph();
    const investigation = options.investigationId
      ? this.list().find((track) => track.id === options.investigationId) ?? null
      : this.context.sessionId
        ? this.getForSession(this.context.sessionId)
        : null;
    const summary = investigation ? this.summary(investigation) : null;
    const stage = options.stage ?? summary?.stage ?? inferStage(options.query);
    const candidates = new Map<string, MemoryNode>();
    for (const node of memoryGraph.search({ scope: "workspace", limit: 100 })) if (!LEGACY_CLAIM_MEMORY_TYPES.has(node.type)) candidates.set(node.id, node);
    for (const node of memoryGraph.search({ query: options.query, scope: "subject", limit: 100 })) if (!LEGACY_CLAIM_MEMORY_TYPES.has(node.type)) candidates.set(node.id, node);
    const currentIds = new Set(summary ? this.resourceIds(summary.id, "memory") : []);
    const ranked = stageAwareRank([...candidates.values()], currentIds, options.query, stage);
    const maxNodes = Math.max(1, Math.min(25, Math.floor(options.maxNodes ?? 10)));
    const selected = selectWithLaneQuotas(ranked, stage, maxNodes);
    const nodes = selectMemoryModelContext({
      nodes: selected.map((item) => item.node),
      edges: memoryGraph.listEdgesForNodes(selected.map((item) => item.node.id)),
      prompt: options.query,
      maxNodes,
      maxCharacters: options.maxCharacters ?? 8_000,
      workspaceId: this.context.workspaceId,
      profileMemory: memoryGraph.getProfileMemory(),
    });
    const laneById = new Map(selected.map((item) => [item.node.id, item]));
    return {
      investigation: summary,
      stage,
      nodes,
      selection: nodes.map((node) => ({ id: node.id, lane: laneById.get(node.id)?.lane ?? "claim", score: laneById.get(node.id)?.score ?? 0 })),
    };
  }

  public refreshFromLinkedResources(investigationId: string): CampaignTrackDetail {
    this.requireTrack(investigationId);
    const memoryGraph = this.requireMemoryGraph();
    for (const memoryId of this.resourceIds(investigationId, "memory")) {
      const node = memoryGraph.get(memoryId);
      if (!node) continue;
      const evidenceRefIds = node.evidence.map((evidence) => evidence.id);
      if (evidenceRefIds.length > 0 && (node.status === "confirmed" || node.status === "rejected")) {
        const existingObservation = this.database.prepare(
          "SELECT id FROM campaign_track_observations WHERE investigation_id = ? AND memory_node_id = ? LIMIT 1",
        ).get(investigationId, node.id);
        if (!existingObservation) this.addObservation({
          investigationId,
          memoryNodeId: node.id,
          kind: "runtime",
          outcome: node.status === "rejected" ? "refutes" : "supports",
          summary: node.summary || node.title,
          evidenceRefIds,
        });
      }
      if (node.type === "hypothesis" && ["draft", "suspected"].includes(node.status)) {
        const question = this.upsertQuestion({
          investigationId,
          text: `What positive evidence would establish ${node.title}, and what evidence would genuinely narrow or contradict a necessary part of it?`,
          priority: "high",
        });
        this.upsertNextAction({
          investigationId,
          questionId: question.id,
          title: `Prove or narrow: ${node.title}`,
          rationale: node.summary,
          priority: "high",
          expectedInformationGain: 0.8,
          estimatedCost: 0.5,
          suggestedPrompt: `Pursue the next positive proof obligation for ${node.title}, while recording any result that genuinely narrows or contradicts a necessary part. Do not treat an incomplete attempt as refutation.`,
        });
      }
    }
    if (this.claimStore) {
      for (const claimId of this.resourceIds(investigationId, "finding")) {
        const claim = this.claimStore.get(claimId);
        if (!claim || claim.duplicateOfClaimId) continue;
        if (claim.projection === "lead" && claim.maturity !== "refuted") {
          const question = this.upsertQuestion({
            investigationId,
            text: `What positive evidence would directly observe ${claim.title}, and what evidence would genuinely narrow or contradict a necessary link?`,
            priority: "high",
          });
          this.upsertNextAction({
            investigationId,
            questionId: question.id,
            title: `Prove or narrow: ${claim.title}`,
            rationale: claim.summary,
            priority: "high",
            expectedInformationGain: 0.8,
            estimatedCost: 0.5,
            suggestedPrompt: `Pursue the next positive proof obligation for lead ${claim.id}; also record genuinely contrary or narrowing evidence, and update that same claim without treating an incomplete attempt as refutation.`,
          });
        }
        const stage: CampaignTrackStage | null = claim.workflow === "reporting" ? "reporting"
          : claim.maturity === "verified" ? "verifying"
            : claim.maturity === "reproduced" ? "verifying"
              : claim.maturity === "observed" ? "reproducing"
                : claim.projection === "lead" ? "testing" : null;
        if (stage) this.database.prepare(`UPDATE campaign_tracks SET stage = ?, updated_at = ?, revision = revision + 1
          WHERE id = ? AND status = 'active' AND stage NOT IN ('complete', 'blocked', 'reporting')`).run(stage, nowIso(), investigationId);
      }
    }
    for (const runbookId of this.resourceIds(investigationId, "runbook")) {
      const runbook = replayRunbook(this.database, runbookId);
      if (runbook) this.upsertExperiment({
        investigationId,
        title: runbook.title,
        runbookId: runbook.id,
        status: runbook.status,
        resultSummary: runbook.resultSummary,
      });
    }
    return this.detail(investigationId)!;
  }

  public reviewClaim(input: {
    investigationId: string;
    claimId: string;
    expectedRevision: number;
    verdict: "accept" | "revise" | "reject";
    rationale: string;
    evidenceIds: readonly string[];
    targetClassification?: string;
    targetStatus?: FindingStatus;
    confidence?: number;
    reviewer?: ModelAuthor;
    reviewerAgentId?: string;
  }): { reviewId: string; independent: boolean; claim: FindingSummary } {
    this.requireTrack(input.investigationId);
    const claimStore = this.requireClaimStore();
    const source = claimStore.get(input.claimId);
    if (!source) throw new Error(`Research claim not found: ${input.claimId}`);
    if (source.duplicateOfClaimId) {
      throw new Error(`Research claim ${source.id} is a duplicate; review canonical parent ${source.duplicateOfClaimId}.`);
    }
    if (source.revision !== input.expectedRevision) throw new Error(`Research claim revision conflict for ${source.id}.`);
    const evidenceIds = unique(input.evidenceIds);
    if (input.verdict !== "revise" && evidenceIds.length === 0) throw new Error("Claim acceptance or rejection requires evidence.");
    const sourceEvidenceIds = new Set(source.evidence.map((evidence) => evidence.id));
    const unknownEvidence = evidenceIds.filter((id) => !sourceEvidenceIds.has(id));
    if (unknownEvidence.length > 0) throw new Error(`Review evidence does not belong to claim ${source.id}: ${unknownEvidence.join(", ")}.`);
    const reviewerSessionId = this.context.sessionId ?? null;
    const independent = isIndependentClaimReviewer(source, reviewerSessionId, input.reviewerAgentId);
    if (input.verdict === "accept" && !independent) {
      throw new Error("Claim promotion requires an independent reviewer that did not author a claim revision or its evidence; a distinct subagent in the same session or a separate session qualifies.");
    }
    let next = source;
    if (input.verdict === "accept") {
      next = claimStore.transition(source.id, {
        expectedRevision: source.revision,
        toStatus: input.targetStatus ?? "observed",
        reason: requiredText(input.rationale, "review rationale"),
        ...(input.targetClassification ? { classification: input.targetClassification } : {}),
      }, input.reviewer, input.reviewerAgentId);
    } else if (input.verdict === "reject") {
      next = claimStore.transition(source.id, {
        expectedRevision: source.revision,
        toStatus: "rejected",
        reason: requiredText(input.rationale, "review rationale"),
      }, input.reviewer, input.reviewerAgentId);
    } else {
      next = claimStore.revise(source.id, {
        expectedRevision: source.revision,
        reason: requiredText(input.rationale, "review rationale"),
        ...(input.targetClassification ? { classification: input.targetClassification } : {}),
        ...(typeof input.confidence === "number" ? { confidence: bounded(input.confidence) } : {}),
      }, input.reviewer, input.reviewerAgentId);
    }
    const reviewId = createId("claim_review");
    this.database.prepare(`
      INSERT INTO campaign_track_research_claim_reviews(
        id, investigation_id, claim_id, claim_revision, resulting_revision,
        verdict, rationale, evidence_ids_json, reviewer_session_id, reviewer_agent_id, independent, created_at
      ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
    `).run(
      reviewId, input.investigationId, source.id, source.revision, next.revision,
      input.verdict, requiredText(input.rationale, "review rationale"), JSON.stringify(evidenceIds), reviewerSessionId,
      input.reviewerAgentId?.trim() || null, independent ? 1 : 0, nowIso(),
    );
    this.linkResource(input.investigationId, "finding", next.id, input.verdict === "accept" ? "reviewed_claim" : "reviewed");
    return { reviewId, independent, claim: next };
  }

  public replayWorkspace(options: {
    persist?: boolean;
    record?: boolean;
    mode?: InvestigationReplayMetrics["mode"];
    excludeSessionIds?: readonly string[];
  } = {}): InvestigationReplayResult {
    const excludedSessionIds = new Set(options.excludeSessionIds ?? []);
    const sessions = this.replaySessions().filter((session) => !excludedSessionIds.has(session.id));
    const clusters = clusterReplaySessions(sessions);
    const metrics = replayMetrics(
      this.database,
      this.context.workspaceId,
      sessions,
      clusters,
      options.mode ?? "historical",
    );
    const createdAt = nowIso();
    const id = createId("investigation_replay");
    if (options.persist) {
      for (const cluster of clusters) this.persistReplayCluster(cluster);
    }
    if (options.persist || options.record) this.database.prepare(
      "INSERT INTO campaign_track_replay_runs(id, workspace_id, persisted, metrics_json, created_at) VALUES (?, ?, ?, ?, ?)",
    ).run(id, this.context.workspaceId, options.persist ? 1 : 0, JSON.stringify(metrics), createdAt);
    return {
      id,
      persisted: options.persist === true,
      createdAt,
      metrics,
      tracks: clusters.map((cluster) => ({
        title: cluster.title,
        sessionIds: cluster.sessions.map((session) => session.id),
        memoryNodeIds: unique(cluster.sessions.flatMap((session) => session.memoryNodes.map((node) => node.id))),
      })),
    };
  }

  public repairPlaceholderTracks(options: { skipActiveSessions?: boolean } = {}): {
    repaired: boolean;
    archivedTrackIds: string[];
    replacementTrackIds: string[];
  } {
    const placeholders = (this.database.prepare(`
      SELECT t.* FROM campaign_tracks t
      WHERE t.workspace_id = ? AND t.status <> 'archived'
        AND t.title_norm IN ('no title yet', 'untitled research track')
        AND (SELECT COUNT(*) FROM campaign_track_sessions s WHERE s.investigation_id = t.id) > 0
      ORDER BY t.created_at, t.id
    `).all(this.context.workspaceId) as SqlRow[]).map(trackFromRow);
    const replacementTrackIds = new Set<string>();
    const archivedTrackIds: string[] = [];
    for (const placeholder of placeholders) {
      if (options.skipActiveSessions && this.hasActiveSession(placeholder.id)) continue;
      const sessionIds = new Set(this.resourceIds(placeholder.id, "session"));
      if (sessionIds.size === 0) {
        for (const row of this.database.prepare(
          "SELECT session_id FROM campaign_track_sessions WHERE investigation_id = ?",
        ).all(placeholder.id) as SqlRow[]) sessionIds.add(textValue(row.session_id));
      }
      const clusters = clusterReplaySessions(
        this.replaySessions().filter((session) => sessionIds.has(session.id)),
        { includeInactiveWithoutMemory: true },
      );
      if (clusters.length === 0 || clusters.some((cluster) => isPlaceholderTitle(cluster.title))) continue;
      for (const cluster of clusters) {
        this.persistReplayCluster(cluster, placeholder);
        const replacement = this.list({ includeArchived: false }).find((track) =>
          track.title === cluster.title && cluster.sessions.every((session) => track.sessionIds.includes(session.id))
        );
        if (replacement) replacementTrackIds.add(replacement.id);
      }
      this.database.prepare(`
        UPDATE campaign_tracks SET status = 'archived', updated_at = ?, revision = revision + 1
        WHERE id = ? AND status <> 'archived'
      `).run(nowIso(), placeholder.id);
      archivedTrackIds.push(placeholder.id);
    }
    return {
      repaired: archivedTrackIds.length > 0,
      archivedTrackIds,
      replacementTrackIds: [...replacementTrackIds],
    };
  }

  public latestReplayMetrics(): InvestigationReplayMetrics | null {
    return readLatestCampaignTrackReplayMetrics(this.database, this.context.workspaceId);
  }

  public generateConsolidationCandidates(): ConsolidationCandidate[] {
    const memoryGraph = this.requireMemoryGraph();
    const groups = new Map<string, Array<{ trackId: string; node: MemoryNode }>>();
    for (const track of this.list()) {
      for (const memoryId of this.resourceIds(track.id, "memory")) {
        const node = memoryGraph.get(memoryId);
        if (!node || !["procedure", "invariant", "trajectory", "mitigation"].includes(node.type)) continue;
        const rootCauseKey = typeof node.attributes.rootCauseKey === "string" ? node.attributes.rootCauseKey.trim() : "";
        const key = rootCauseKey || consolidationKey(node.title);
        if (!key) continue;
        const entries = groups.get(key) ?? [];
        entries.push({ trackId: track.id, node });
        groups.set(key, entries);
      }
    }
    const now = nowIso();
    for (const [key, entries] of groups) {
      const trackIds = unique(entries.map((entry) => entry.trackId));
      if (trackIds.length < 2) continue;
      const nodeIds = unique(entries.map((entry) => entry.node.id));
      const creatableTypes = new Set(memoryGraph.getProfileMemory().types
        .filter((type) => type.lifecycle === "active" && type.creatable)
        .map((type) => type.id));
      const proposedType: ConsolidationCandidate["proposedType"] | null =
        entries.some((entry) => entry.node.type === "procedure") && creatableTypes.has("procedure")
          ? "procedure"
          : creatableTypes.has("invariant")
            ? "invariant"
            : creatableTypes.has("trajectory")
              ? "trajectory"
              : null;
      if (!proposedType) continue;
      const strongest = [...entries].sort((left, right) => right.node.confidence - left.node.confidence || right.node.updatedAt.localeCompare(left.node.updatedAt))[0]!;
      const id = stableId("consolidation", `${this.context.workspaceId}\0${proposedType}\0${key}`);
      this.database.prepare(`
        INSERT OR IGNORE INTO campaign_track_consolidations(
          id, workspace_id, proposed_type, title, summary, status,
          source_investigation_ids_json, source_memory_node_ids_json, promoted_memory_node_id, created_at, reviewed_at
        ) VALUES (?, ?, ?, ?, ?, 'candidate', ?, ?, NULL, ?, NULL)
      `).run(
        id, this.context.workspaceId, proposedType, `Cross-track ${proposedType}: ${strongest.node.title}`,
        consolidatedSummary(entries.map((entry) => entry.node)), JSON.stringify(trackIds), JSON.stringify(nodeIds), now,
      );
    }
    return this.listConsolidations();
  }

  public reviewConsolidation(input: {
    id: string;
    verdict: "accept" | "reject";
    reviewer?: ModelAuthor;
  }): ConsolidationCandidate {
    const candidate = this.listConsolidations().find((item) => item.id === input.id);
    if (!candidate) throw new Error(`Consolidation candidate not found: ${input.id}`);
    if (candidate.status !== "candidate") throw new Error(`Consolidation candidate is already ${candidate.status}.`);
    let promotedMemoryNodeId: string | null = null;
    if (input.verdict === "accept") {
      const graph = this.requireMemoryGraph();
      const sourceNodes = candidate.sourceMemoryNodeIds.flatMap((id) => {
        const node = graph.get(id);
        return node ? [node] : [];
      });
      if (sourceNodes.length !== candidate.sourceMemoryNodeIds.length) {
        throw new Error("Consolidation acceptance requires every source memory to remain available.");
      }
      if (!this.context.sessionId || sourceNodes.some((node) => node.sessionIds.includes(this.context.sessionId!))) {
        throw new Error("Consolidation acceptance requires an independent review session.");
      }
      const evidence = uniqueEvidence(sourceNodes.flatMap((node) => node.evidence));
      if (evidence.length === 0) throw new Error("Consolidation acceptance requires source evidence.");
      const promoted = graph.save({
        type: candidate.proposedType,
        title: candidate.title,
        summary: candidate.summary,
        status: "confirmed",
        confidence: Math.min(...sourceNodes.map((node) => node.confidence)),
        assetIds: unique(sourceNodes.flatMap((node) => node.assetIds)),
        tags: unique([...sourceNodes.flatMap((node) => node.tags), "cross_investigation"]),
        evidence,
      }, input.reviewer);
      promotedMemoryNodeId = promoted.id;
      for (const node of sourceNodes) graph.link(node.id, promoted.id, "supports", "Source experience for reviewed cross-investigation consolidation.", input.reviewer);
    }
    this.database.prepare(`
      UPDATE campaign_track_consolidations SET status = ?, promoted_memory_node_id = ?, reviewed_at = ? WHERE id = ?
    `).run(input.verdict === "accept" ? "accepted" : "rejected", promotedMemoryNodeId, nowIso(), input.id);
    return this.listConsolidations().find((item) => item.id === input.id)!;
  }

  public listConsolidations(): ConsolidationCandidate[] {
    const rows = this.database.prepare(
      "SELECT * FROM campaign_track_consolidations WHERE workspace_id = ? ORDER BY created_at DESC, id",
    ).all(this.context.workspaceId) as SqlRow[];
    return rows.map(consolidationFromRow);
  }

  private summary(track: CampaignTrackRecord): CampaignTrackSummary {
    const sessions = this.resourceIds(track.id, "session");
    const resourceCounts = new Map<string, number>();
    for (const row of this.database.prepare(`
      SELECT resource_kind, COUNT(*) AS count FROM campaign_track_resources WHERE investigation_id = ? GROUP BY resource_kind
    `).all(track.id) as SqlRow[]) resourceCounts.set(textValue(row.resource_kind), numberValue(row.count));
    const questionCounts = this.database.prepare(`
      SELECT COUNT(*) AS count, SUM(CASE WHEN status = 'open' THEN 1 ELSE 0 END) AS open_count
      FROM campaign_track_questions WHERE investigation_id = ?
    `).get(track.id) as SqlRow;
    const experimentCount = this.database.prepare("SELECT COUNT(*) AS count FROM campaign_track_experiments WHERE investigation_id = ?").get(track.id) as SqlRow;
    const observationCount = this.database.prepare("SELECT COUNT(*) AS count FROM campaign_track_observations WHERE investigation_id = ?").get(track.id) as SqlRow;
    const actionCount = this.database.prepare("SELECT COUNT(*) AS count FROM campaign_track_next_actions WHERE investigation_id = ? AND status IN ('open', 'in_progress')").get(track.id) as SqlRow;
    return {
      ...track,
      sessionIds: sessions,
      counts: {
        questions: numberValue(questionCounts.count),
        openQuestions: numberValue(questionCounts.open_count),
        experiments: numberValue(experimentCount.count),
        observations: numberValue(observationCount.count),
        openNextActions: numberValue(actionCount.count),
        memoryNodes: resourceCounts.get("memory") ?? 0,
        evidenceRefs: resourceCounts.get("evidence") ?? 0,
        findings: resourceCounts.get("finding") ?? 0,
        runbooks: resourceCounts.get("runbook") ?? 0,
        reports: resourceCounts.get("report") ?? 0,
      },
    };
  }

  private questions(investigationId: string): InvestigationQuestion[] {
    return (this.database.prepare(
      "SELECT * FROM campaign_track_questions WHERE investigation_id = ? ORDER BY CASE priority WHEN 'critical' THEN 0 WHEN 'high' THEN 1 WHEN 'medium' THEN 2 ELSE 3 END, updated_at DESC",
    ).all(investigationId) as SqlRow[]).map(questionFromRow);
  }

  private experiments(investigationId: string): InvestigationExperiment[] {
    return (this.database.prepare(
      "SELECT * FROM campaign_track_experiments WHERE investigation_id = ? ORDER BY updated_at DESC",
    ).all(investigationId) as SqlRow[]).map(experimentFromRow);
  }

  private observations(investigationId: string): InvestigationObservation[] {
    return (this.database.prepare(
      "SELECT * FROM campaign_track_observations WHERE investigation_id = ? ORDER BY created_at DESC",
    ).all(investigationId) as SqlRow[]).map((row) => this.observationFromRow(row));
  }

  private observation(id: string): InvestigationObservation | null {
    const row = this.database.prepare("SELECT * FROM campaign_track_observations WHERE id = ?").get(id) as SqlRow | undefined;
    return row ? this.observationFromRow(row) : null;
  }

  private observationFromRow(row: SqlRow): InvestigationObservation {
    const id = textValue(row.id);
    const evidenceRows = this.database.prepare(
      "SELECT evidence_ref_id FROM campaign_track_observation_evidence WHERE observation_id = ? ORDER BY evidence_ref_id",
    ).all(id) as SqlRow[];
    return {
      id,
      investigationId: textValue(row.investigation_id),
      experimentId: optionalText(row.experiment_id),
      memoryNodeId: optionalText(row.memory_node_id),
      kind: textValue(row.kind) as InvestigationObservation["kind"],
      outcome: textValue(row.outcome) as InvestigationObservation["outcome"],
      summary: textValue(row.summary),
      evidenceRefIds: evidenceRows.map((candidate) => textValue(candidate.evidence_ref_id)),
      sourceEventId: optionalText(row.source_event_id),
      createdAt: textValue(row.created_at),
    };
  }

  private nextActions(investigationId: string): InvestigationNextAction[] {
    return (this.database.prepare(`
      SELECT * FROM campaign_track_next_actions WHERE investigation_id = ?
      ORDER BY CASE status WHEN 'in_progress' THEN 0 WHEN 'open' THEN 1 ELSE 2 END,
        CASE priority WHEN 'critical' THEN 0 WHEN 'high' THEN 1 WHEN 'medium' THEN 2 ELSE 3 END,
        (expected_information_gain - estimated_cost) DESC, updated_at DESC
    `).all(investigationId) as SqlRow[]).map(nextActionFromRow);
  }

  private resourceIds(investigationId: string, kind: CampaignTrackResourceKind): string[] {
    return (this.database.prepare(
      "SELECT resource_id FROM campaign_track_resources WHERE investigation_id = ? AND resource_kind = ? ORDER BY linked_at, resource_id",
    ).all(investigationId, kind) as SqlRow[]).map((row) => textValue(row.resource_id));
  }

  private backfillSessionResources(investigationId: string, sessionId: string): void {
    const now = nowIso();
    const inserts: Array<{ kind: CampaignTrackResourceKind; sql: string }> = [
      { kind: "memory", sql: "SELECT node_id AS id FROM memory_node_sessions WHERE session_id = ?" },
      { kind: "evidence", sql: "SELECT e.id FROM memory_evidence_refs e JOIN memory_node_sessions s ON s.node_id = e.node_id WHERE s.session_id = ?" },
      { kind: "finding", sql: "SELECT id FROM app_server_research_claims WHERE origin_session_id = ?" },
      { kind: "runbook", sql: "SELECT id FROM app_server_runbooks WHERE session_id = ?" },
      { kind: "report", sql: "SELECT id FROM app_server_reports WHERE session_id = ?" },
    ];
    const insert = this.database.prepare(
      "INSERT OR IGNORE INTO campaign_track_resources(investigation_id, resource_kind, resource_id, role, linked_at) VALUES (?, ?, ?, 'produced', ?)",
    );
    for (const candidate of inserts) {
      if (!tableExists(this.database, tableNameFromSelect(candidate.sql))) continue;
      for (const row of this.database.prepare(candidate.sql).all(sessionId) as SqlRow[]) {
        insert.run(investigationId, candidate.kind, textValue(row.id), now);
      }
    }
  }

  private persistReplayCluster(
    cluster: ReplayCluster,
    identity?: Pick<CampaignTrackRecord, "workspaceName" | "subjectId" | "subjectName">,
  ): void {
    const objective = cluster.sessions.map((session) => session.summary).filter(Boolean).at(-1) ?? `Continue ${cluster.title}.`;
    const track = this.createRecord(
      { title: cluster.title, objective, source: "replay", stage: clusterStage(cluster) },
      identity,
    );
    for (const session of cluster.sessions) this.linkSession(track.id, session.id);
    const nodes = uniqueById(cluster.sessions.flatMap((session) => session.memoryNodes));
    for (const node of nodes) {
      this.linkResource(track.id, "memory", node.id, "historical");
      const evidenceIds = this.evidenceIdsForMemory(node.id);
      if (evidenceIds.length > 0 && (node.status === "confirmed" || node.status === "rejected")) {
        const existingObservation = this.database.prepare(
          "SELECT id FROM campaign_track_observations WHERE investigation_id = ? AND memory_node_id = ? LIMIT 1",
        ).get(track.id, node.id);
        if (!existingObservation) this.addObservation({
          investigationId: track.id,
          memoryNodeId: node.id,
          kind: "historical",
          outcome: node.status === "rejected" ? "refutes" : "supports",
          summary: node.summary || node.title,
          evidenceRefIds: evidenceIds,
        });
      }
      if (node.type === "hypothesis" && ["draft", "suspected"].includes(node.status)) {
        const question = this.upsertQuestion({
          investigationId: track.id,
          text: `What positive evidence would establish ${node.title}, and what evidence would genuinely narrow or contradict a necessary part of it?`,
          priority: "high",
        });
        this.upsertNextAction({
          investigationId: track.id,
          questionId: question.id,
          title: `Prove or narrow: ${node.title}`,
          rationale: node.summary,
          priority: "high",
          expectedInformationGain: 0.8,
          estimatedCost: 0.5,
          suggestedPrompt: `Pursue the next positive proof obligation for ${node.title}, while recording any result that genuinely narrows or contradicts a necessary part. Do not treat an incomplete attempt as refutation.`,
        });
      }
    }
    for (const runbook of cluster.sessions.flatMap((session) => session.runbooks)) {
      this.upsertExperiment({
        investigationId: track.id,
        title: runbook.title,
        runbookId: runbook.id,
        status: runbook.status,
        resultSummary: runbook.resultSummary,
      });
    }
  }

  private replaySessions(): ReplaySession[] {
    if (!tableExists(this.database, "app_server_sessions")) return [];
    const rows = this.database.prepare(`
      SELECT id, title, summary, document_json, status, created_at, updated_at
      FROM app_server_sessions WHERE workspace_id = ? ORDER BY created_at, id
    `).all(this.context.workspaceId) as SqlRow[];
    return rows.map((row) => {
      const id = textValue(row.id);
      const memoryNodes = tableExists(this.database, "memory_node_sessions")
        ? (this.database.prepare(`
            SELECT n.id, n.type, n.title, n.summary, n.status, n.attributes_json, n.created_at, n.updated_at
            FROM memory_nodes n JOIN memory_node_sessions s ON s.node_id = n.id WHERE s.session_id = ? ORDER BY n.created_at
          `).all(id) as SqlRow[]).map(replayNodeFromRow)
        : [];
      const runbooks = tableExists(this.database, "app_server_runbooks")
        ? replayRunbooks(this.database, id)
        : [];
      return {
        id,
        title: researchSessionTitle(row) ?? "Untitled research track",
        summary: textValue(row.summary),
        status: textValue(row.status),
        createdAt: textValue(row.created_at),
        updatedAt: textValue(row.updated_at),
        memoryNodes,
        runbooks,
      };
    });
  }

  private evidenceIdsForMemory(memoryNodeId: string): string[] {
    if (!tableExists(this.database, "memory_evidence_refs")) return [];
    return (this.database.prepare(
      "SELECT id FROM memory_evidence_refs WHERE node_id = ? ORDER BY created_at, id",
    ).all(memoryNodeId) as SqlRow[]).map((row) => textValue(row.id));
  }

  private validateEvidenceIds(ids: readonly string[], memoryNodeId: string | null): void {
    if (ids.length === 0) return;
    for (const id of ids) {
      const row = this.database.prepare(
        "SELECT node_id FROM memory_evidence_refs WHERE id = ?",
      ).get(id) as SqlRow | undefined;
      if (!row) throw new Error(`Evidence reference not found: ${id}`);
      if (memoryNodeId && textValue(row.node_id) !== memoryNodeId) {
        throw new Error(`Evidence reference ${id} does not belong to memory ${memoryNodeId}.`);
      }
    }
  }

  private setStageForExperiment(investigationId: string, status: InvestigationExperiment["status"]): void {
    const next: CampaignTrackStage = status === "planned" || status === "running" ? "testing" : "exploring";
    const track = this.requireTrack(investigationId);
    if (["complete", "blocked", "reporting", "verifying"].includes(track.stage)) return;
    this.database.prepare("UPDATE campaign_tracks SET stage = ?, updated_at = ?, revision = revision + 1 WHERE id = ?")
      .run(next, nowIso(), investigationId);
  }

  private sessionTitle(sessionId: string): string | null {
    if (!tableExists(this.database, "app_server_sessions")) return null;
    const row = this.database.prepare(
      "SELECT title, document_json FROM app_server_sessions WHERE id = ?",
    ).get(sessionId) as SqlRow | undefined;
    return row ? researchSessionTitle(row) : null;
  }

  private hasActiveSession(investigationId: string): boolean {
    if (!tableExists(this.database, "app_server_sessions")) return false;
    return Boolean(this.database.prepare(`
      SELECT 1 FROM campaign_track_sessions t
      JOIN app_server_sessions s ON s.id = t.session_id
      WHERE t.investigation_id = ? AND s.status = 'active'
      LIMIT 1
    `).get(investigationId));
  }

  private touch(id: string): void {
    this.database.prepare("UPDATE campaign_tracks SET updated_at = ?, revision = revision + 1 WHERE id = ?").run(nowIso(), id);
  }

  private requireTrack(id: string): CampaignTrackRecord {
    const track = this.get(id);
    if (!track) throw new Error(`Campaign track not found: ${id}`);
    return track;
  }

  private requireQuestion(id: string, investigationId: string): void {
    const row = this.database.prepare("SELECT id FROM campaign_track_questions WHERE id = ? AND investigation_id = ?").get(id, investigationId);
    if (!row) throw new Error(`Question does not belong to campaign track ${investigationId}: ${id}`);
  }

  private requireExperiment(id: string, investigationId: string): void {
    const row = this.database.prepare("SELECT id FROM campaign_track_experiments WHERE id = ? AND investigation_id = ?").get(id, investigationId);
    if (!row) throw new Error(`Experiment does not belong to campaign track ${investigationId}: ${id}`);
  }

  private requireMemoryGraph(): MemoryGraphStore {
    if (!this.memoryGraph) throw new Error("This campaign track operation requires an active memory graph.");
    return this.memoryGraph;
  }

  private requireClaimStore(): ResearchClaimStore {
    if (!this.claimStore) throw new Error("Campaign claim review requires the active research claim ledger.");
    return this.claimStore;
  }
}

function isIndependentClaimReviewer(
  source: FindingSummary,
  reviewerSessionId: string | null,
  reviewerAgentId: string | undefined,
): boolean {
  if (!reviewerSessionId) return false;
  const reviewerAgent = optionalText(reviewerAgentId);
  const attributedContributions = [
    ...source.transitions.map((transition) => ({
      sessionId: transition.sessionId,
      actorId: transition.actorId,
    })),
    ...source.evidence.map((evidence) => ({
      sessionId: evidence.sessionId,
      actorId: evidence.actorId,
    })),
  ];
  const sourceSessionIds = new Set([
    source.originSessionId,
    ...attributedContributions.map((contribution) => contribution.sessionId),
  ].filter((id): id is string => Boolean(id)));
  if (!sourceSessionIds.has(reviewerSessionId)) return true;
  if (!reviewerAgent) return false;

  const sameSessionContributions = attributedContributions.filter(
    (contribution) => contribution.sessionId === reviewerSessionId,
  );
  if (sameSessionContributions.length === 0 || sameSessionContributions.some((contribution) => !contribution.actorId)) {
    return false;
  }
  if (sameSessionContributions.some((contribution) => contribution.actorId === reviewerAgent)) return false;

  // Agent IDs for subagents are run-global UUIDs. This guards pre-migration
  // transitions whose session cannot be reconstructed without rejecting a
  // distinct same-session reviewer merely because the root authored the claim.
  return !source.transitions.some(
    (transition) => !transition.sessionId && transition.actorId === reviewerAgent,
  );
}

export function readCampaignTrackSummaries(
  database: DatabaseSync,
  workspaceId: string,
): CampaignTrackProjectionSummary[] {
  if (!tableExists(database, "campaign_tracks")) return [];
  const rows = database.prepare(
    "SELECT * FROM campaign_tracks WHERE workspace_id = ? AND status <> 'archived' ORDER BY updated_at DESC",
  ).all(workspaceId) as SqlRow[];
  return rows.map((row) => {
    const track = trackFromRow(row);
    const resourceCounts = new Map<string, number>();
    for (const countRow of database.prepare(
      "SELECT resource_kind, COUNT(*) AS count FROM campaign_track_resources WHERE investigation_id = ? GROUP BY resource_kind",
    ).all(track.id) as SqlRow[]) resourceCounts.set(textValue(countRow.resource_kind), numberValue(countRow.count));
    const question = database.prepare(
      "SELECT COUNT(*) AS count, SUM(CASE WHEN status = 'open' THEN 1 ELSE 0 END) AS open_count FROM campaign_track_questions WHERE investigation_id = ?",
    ).get(track.id) as SqlRow;
    const experiment = database.prepare("SELECT COUNT(*) AS count FROM campaign_track_experiments WHERE investigation_id = ?").get(track.id) as SqlRow;
    const observation = database.prepare("SELECT COUNT(*) AS count FROM campaign_track_observations WHERE investigation_id = ?").get(track.id) as SqlRow;
    const action = database.prepare("SELECT COUNT(*) AS count FROM campaign_track_next_actions WHERE investigation_id = ? AND status IN ('open', 'in_progress')").get(track.id) as SqlRow;
    const sessionIds = (database.prepare(
      "SELECT session_id FROM campaign_track_sessions WHERE investigation_id = ? ORDER BY linked_at, session_id",
    ).all(track.id) as SqlRow[]).map((session) => textValue(session.session_id));
    const questions = (database.prepare(
      "SELECT * FROM campaign_track_questions WHERE investigation_id = ? ORDER BY CASE priority WHEN 'critical' THEN 0 WHEN 'high' THEN 1 WHEN 'medium' THEN 2 ELSE 3 END, updated_at DESC",
    ).all(track.id) as SqlRow[]).map(questionFromRow).map(campaignQuestionProjection);
    const experiments = (database.prepare(
      "SELECT * FROM campaign_track_experiments WHERE investigation_id = ? ORDER BY updated_at DESC",
    ).all(track.id) as SqlRow[]).map(experimentFromRow).map(campaignExperimentProjection);
    const observations = (database.prepare(
      "SELECT * FROM campaign_track_observations WHERE investigation_id = ? ORDER BY created_at DESC",
    ).all(track.id) as SqlRow[]).map(observationProjectionFromRow);
    return {
      ...track,
      sessionIds,
      questions,
      experiments,
      observations,
      counts: {
        questions: numberValue(question.count), openQuestions: numberValue(question.open_count), experiments: numberValue(experiment.count),
        observations: numberValue(observation.count), openNextActions: numberValue(action.count), memoryNodes: resourceCounts.get("memory") ?? 0,
        evidenceRefs: resourceCounts.get("evidence") ?? 0,
        findings: resourceCounts.get("finding") ?? 0, runbooks: resourceCounts.get("runbook") ?? 0, reports: resourceCounts.get("report") ?? 0,
      },
    };
  });
}

function observationProjectionFromRow(row: SqlRow): CampaignObservationProjectionSummary {
  return {
    id: textValue(row.id),
    investigationId: textValue(row.investigation_id),
    experimentId: optionalText(row.experiment_id),
    kind: textValue(row.kind) as CampaignObservationProjectionSummary["kind"],
    outcome: textValue(row.outcome) as CampaignObservationProjectionSummary["outcome"],
    summary: textValue(row.summary),
    createdAt: textValue(row.created_at),
  };
}

export function readLatestCampaignTrackReplayMetrics(
  database: DatabaseSync,
  workspaceId: string,
): InvestigationReplayMetrics | null {
  if (!tableExists(database, "campaign_track_replay_runs")) return null;
  const row = database.prepare(
    "SELECT metrics_json FROM campaign_track_replay_runs WHERE workspace_id = ? ORDER BY created_at DESC, rowid DESC LIMIT 1",
  ).get(workspaceId) as SqlRow | undefined;
  if (!row) return null;
  try {
    const parsed = JSON.parse(textValue(row.metrics_json)) as unknown;
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return null;
    const metrics = parsed as Record<string, unknown>;
    if (metrics.schemaVersion !== 1 || metrics.workspaceId !== workspaceId
      || !["historical", "shadow", "active"].includes(textValue(metrics.mode))) return null;
    return metrics as unknown as InvestigationReplayMetrics;
  } catch {
    return null;
  }
}

function stageAwareRank(nodes: readonly MemoryNode[], currentIds: ReadonlySet<string>, query: string, stage: CampaignTrackStage): RankedMemory[] {
  const queryTokens = researchSignature(query);
  return nodes.map((node) => {
    const lane = recallLane(node, currentIds);
    const lexical = signatureSimilarity(queryTokens, researchSignature(`${node.title} ${node.summary} ${node.tags.join(" ")}`));
    const stageWeight = laneWeight(stage, lane);
    const currentWeight = currentIds.has(node.id) ? 3 : 0;
    const evidenceWeight = Math.min(2, node.evidence.length / 3);
    const authorityWeight = node.status === "confirmed" ? 1.5 : node.status === "rejected" || node.status === "stale" ? 1.2 : 0.5;
    return { node, lane, score: lexical * 8 + stageWeight + currentWeight + evidenceWeight + authorityWeight };
  }).sort((left, right) => right.score - left.score || right.node.updatedAt.localeCompare(left.node.updatedAt));
}

interface RankedMemory { node: MemoryNode; lane: RecallLane; score: number }

function selectWithLaneQuotas(ranked: readonly RankedMemory[], stage: CampaignTrackStage, limit: number): RankedMemory[] {
  const quotas = stageLaneQuotas(stage, limit);
  const selected: RankedMemory[] = [];
  const selectedIds = new Set<string>();
  for (const [lane, quota] of Object.entries(quotas) as Array<[RecallLane, number]>) {
    for (const item of ranked.filter((candidate) => candidate.lane === lane).slice(0, quota)) {
      selected.push(item); selectedIds.add(item.node.id);
    }
  }
  for (const item of ranked) {
    if (selected.length >= limit) break;
    if (!selectedIds.has(item.node.id)) { selected.push(item); selectedIds.add(item.node.id); }
  }
  return selected.sort((left, right) => right.score - left.score).slice(0, limit);
}

function stageLaneQuotas(stage: CampaignTrackStage, limit: number): Record<RecallLane, number> {
  const base: Record<RecallLane, number> = { current: 2, target: 2, claim: 2, negative: 1, procedure: 1, analogy: 1 };
  if (["testing", "reproducing", "verifying"].includes(stage)) { base.procedure = 2; base.negative = 2; }
  if (stage === "orienting" || stage === "exploring") { base.target = 3; base.analogy = 2; }
  if (stage === "reporting") { base.claim = 3; base.current = 3; }
  const total = Object.values(base).reduce((sum, value) => sum + value, 0);
  if (total <= limit) return base;
  const scale = limit / total;
  return Object.fromEntries(Object.entries(base).map(([lane, value]) => [lane, Math.max(lane === "current" ? 1 : 0, Math.floor(value * scale))])) as Record<RecallLane, number>;
}

function recallLane(node: MemoryNode, currentIds: ReadonlySet<string>): RecallLane {
  if (node.status === "rejected" || node.status === "stale") return "negative";
  if (node.type === "procedure") return "procedure";
  if (node.type === "invariant" || node.type === "trajectory") return "analogy";
  if (["asset", "source", "sink", "flow-endpoint"].includes(node.type)) return "target";
  if (currentIds.has(node.id)) return "current";
  return "claim";
}

function laneWeight(stage: CampaignTrackStage, lane: RecallLane): number {
  const weights: Record<CampaignTrackStage, Partial<Record<RecallLane, number>>> = {
    orienting: { target: 3, current: 3, analogy: 2 }, exploring: { target: 3, current: 3, negative: 2 },
    testing: { current: 4, procedure: 3, negative: 3 }, reproducing: { current: 4, procedure: 4, claim: 3 },
    verifying: { negative: 4, claim: 4, procedure: 3 }, reporting: { claim: 4, current: 4, target: 2 },
    complete: { analogy: 3, procedure: 3, claim: 2 }, blocked: { negative: 4, procedure: 3, current: 3 },
  };
  return weights[stage][lane] ?? 1;
}

interface ReplayMemoryNode {
  id: string; type: string; title: string; summary: string; status: string; attributes: Record<string, unknown>; createdAt: string; updatedAt: string;
}
interface ReplayRunbook { id: string; title: string; status: InvestigationExperiment["status"]; resultSummary: string }
interface ReplaySession {
  id: string; title: string; summary: string; status: string; createdAt: string; updatedAt: string; memoryNodes: ReplayMemoryNode[]; runbooks: ReplayRunbook[];
}
interface ReplayCluster { title: string; signature: Set<string>; sessions: ReplaySession[] }

function clusterReplaySessions(
  sessions: readonly ReplaySession[],
  options: { includeInactiveWithoutMemory?: boolean } = {},
): ReplayCluster[] {
  const clusters: ReplayCluster[] = [];
  for (const session of sessions) {
    if (!options.includeInactiveWithoutMemory
      && session.memoryNodes.length === 0
      && ["failed", "stopped", "paused", "blocked"].includes(session.status)) continue;
    const signature = sessionSignature(session);
    const closest = clusters.map((cluster) => ({ cluster, score: signatureSimilarity(signature, cluster.signature) }))
      .sort((left, right) => right.score - left.score)[0];
    if (closest && closest.score >= 0.24 && intersectionSize(signature, closest.cluster.signature) >= 2) {
      closest.cluster.sessions.push(session);
      closest.cluster.signature = new Set([...closest.cluster.signature, ...signature]);
    } else {
      clusters.push({ title: session.title || titleFromObjective(session.summary), signature, sessions: [session] });
    }
  }
  return clusters;
}

function replayMetrics(
  database: DatabaseSync,
  workspaceId: string,
  sessions: readonly ReplaySession[],
  clusters: readonly ReplayCluster[],
  mode: InvestigationReplayMetrics["mode"],
): InvestigationReplayMetrics {
  const nodes = uniqueById(sessions.flatMap((session) => session.memoryNodes));
  const titleGroups = new Map<string, ReplayMemoryNode[]>();
  for (const node of nodes) {
    const key = consolidationKey(node.title);
    const group = titleGroups.get(key) ?? []; group.push(node); titleGroups.set(key, group);
  }
  const repeatedMemoryCandidateCount = [...titleGroups.values()].filter((group) => group.length > 1).length;
  let rejectedHypothesisResurrectionCount = 0;
  const ordered = [...nodes].sort((left, right) => left.createdAt.localeCompare(right.createdAt));
  for (const [index, node] of ordered.entries()) {
    if (node.type !== "hypothesis" || !["suspected", "draft"].includes(node.status)) continue;
    const signature = researchSignature(node.title);
    if (ordered.slice(0, index).some((earlier) => earlier.type === "hypothesis" && earlier.status === "rejected" && signatureSimilarity(signature, researchSignature(earlier.title)) >= 0.6)) {
      rejectedHypothesisResurrectionCount += 1;
    }
  }
  const nodeSessionCounts = tableExists(database, "memory_node_sessions")
    ? database.prepare(`
        SELECT s.node_id, COUNT(*) AS count FROM memory_node_sessions s
        JOIN memory_node_workspaces w ON w.node_id = s.node_id WHERE w.workspace_id = ? GROUP BY s.node_id
      `).all(workspaceId) as SqlRow[]
    : [];
  const reused = nodeSessionCounts.filter((row) => numberValue(row.count) > 1).length;
  const environmentTagged = nodes.filter((node) =>
    typeof node.attributes.environmentFingerprint === "string" || typeof node.attributes.deviceOs === "string" || typeof node.attributes.sourceRevision === "string").length;
  const firstEvidenceMinutes = sessions.flatMap((session) => {
    if (!tableExists(database, "memory_evidence_refs")) return [];
    const row = database.prepare(`
      SELECT MIN(e.created_at) AS created_at FROM memory_evidence_refs e
      JOIN memory_node_sessions s ON s.node_id = e.node_id WHERE s.session_id = ?
    `).get(session.id) as SqlRow;
    const first = optionalText(row.created_at);
    if (!first) return [];
    return [Math.max(0, (Date.parse(first) - Date.parse(session.createdAt)) / 60_000)];
  });
  return {
    schemaVersion: 1,
    mode,
    workspaceId,
    sessionCount: sessions.length,
    generatedTrackCount: clusters.length,
    linkedMemoryNodeCount: nodes.length,
    repeatedMemoryCandidateCount,
    rejectedHypothesisResurrectionCount,
    environmentTaggedNodeRate: nodes.length ? environmentTagged / nodes.length : 0,
    crossSessionReuseRate: nodeSessionCounts.length ? reused / nodeSessionCounts.length : 0,
    medianMinutesToFirstEvidence: median(firstEvidenceMinutes),
  };
}

function replayRunbooks(database: DatabaseSync, sessionId: string): ReplayRunbook[] {
  const rows = database.prepare(`
    SELECT r.id, r.title,
      COALESCE((SELECT e.status FROM app_server_runbook_executions e WHERE e.runbook_id = r.id ORDER BY e.started_at DESC LIMIT 1), 'planned') AS execution_status,
      COALESCE((SELECT e.error FROM app_server_runbook_executions e WHERE e.runbook_id = r.id ORDER BY e.started_at DESC LIMIT 1), '') AS result_summary
    FROM app_server_runbooks r WHERE r.session_id = ?
  `).all(sessionId) as SqlRow[];
  return rows.map((row) => ({
    id: textValue(row.id), title: textValue(row.title), status: runbookExperimentStatus(textValue(row.execution_status)), resultSummary: textValue(row.result_summary),
  }));
}

function replayRunbook(database: DatabaseSync, runbookId: string): ReplayRunbook | null {
  if (!tableExists(database, "app_server_runbooks")) return null;
  const row = database.prepare(`
    SELECT r.id, r.title,
      COALESCE((SELECT e.status FROM app_server_runbook_executions e WHERE e.runbook_id = r.id ORDER BY e.started_at DESC LIMIT 1), 'planned') AS execution_status,
      COALESCE((SELECT e.error FROM app_server_runbook_executions e WHERE e.runbook_id = r.id ORDER BY e.started_at DESC LIMIT 1), '') AS result_summary
    FROM app_server_runbooks r WHERE r.id = ?
  `).get(runbookId) as SqlRow | undefined;
  return row ? {
    id: textValue(row.id),
    title: textValue(row.title),
    status: runbookExperimentStatus(textValue(row.execution_status)),
    resultSummary: textValue(row.result_summary),
  } : null;
}

function runbookExperimentStatus(status: string): InvestigationExperiment["status"] {
  if (status === "succeeded") return "succeeded";
  if (status === "failed") return "failed";
  if (status === "blocked") return "blocked";
  if (status === "running") return "running";
  return "planned";
}

function sessionSignature(session: ReplaySession): Set<string> {
  return researchSignature(`${session.title} ${session.memoryNodes.map((node) => `${node.type} ${node.title}`).join(" ")}`);
}

function clusterStage(cluster: ReplayCluster): CampaignTrackStage {
  if (cluster.sessions.some((session) => session.memoryNodes.some((node) => node.type === "chain" && node.status === "confirmed"))) return "verifying";
  if (cluster.sessions.some((session) => session.memoryNodes.some((node) => node.type === "primitive" && node.status === "confirmed"))) return "reproducing";
  if (cluster.sessions.some((session) => session.memoryNodes.some((node) => node.type === "hypothesis"))) return "testing";
  return "exploring";
}

function trackFromRow(row: SqlRow): CampaignTrackRecord {
  return {
    id: textValue(row.id), workspaceId: textValue(row.workspace_id), workspaceName: textValue(row.workspace_name),
    subjectId: optionalText(row.subject_id), subjectName: optionalText(row.subject_name), title: textValue(row.title), objective: textValue(row.objective),
    status: textValue(row.status) as CampaignTrackStatus, stage: textValue(row.stage) as CampaignTrackStage, source: textValue(row.source) as CampaignTrackSource,
    originSessionId: optionalText(row.origin_session_id), sourceRevision: optionalText(row.source_revision), environmentFingerprint: optionalText(row.environment_fingerprint),
    createdAt: textValue(row.created_at), updatedAt: textValue(row.updated_at), revision: numberValue(row.revision),
  };
}

function questionFromRow(row: SqlRow): InvestigationQuestion {
  return {
    id: textValue(row.id), investigationId: textValue(row.investigation_id), text: textValue(row.text),
    status: textValue(row.status) as InvestigationQuestion["status"], priority: textValue(row.priority) as InvestigationQuestion["priority"], answer: textValue(row.answer),
    createdAt: textValue(row.created_at), updatedAt: textValue(row.updated_at), revision: numberValue(row.revision),
  };
}

function experimentFromRow(row: SqlRow): InvestigationExperiment {
  return {
    id: textValue(row.id), investigationId: textValue(row.investigation_id), questionId: optionalText(row.question_id), hypothesisMemoryId: optionalText(row.hypothesis_memory_id),
    runbookId: optionalText(row.runbook_id), title: textValue(row.title), status: textValue(row.status) as InvestigationExperiment["status"],
    expectedOutcomes: jsonObject(row.expected_outcomes_json), resultSummary: textValue(row.result_summary), sourceRevision: optionalText(row.source_revision),
    environmentFingerprint: optionalText(row.environment_fingerprint), startedAt: optionalText(row.started_at), completedAt: optionalText(row.completed_at),
    createdAt: textValue(row.created_at), updatedAt: textValue(row.updated_at), revision: numberValue(row.revision),
  };
}

function nextActionFromRow(row: SqlRow): InvestigationNextAction {
  return {
    id: textValue(row.id), investigationId: textValue(row.investigation_id), questionId: optionalText(row.question_id), title: textValue(row.title), rationale: textValue(row.rationale),
    status: textValue(row.status) as InvestigationNextAction["status"], priority: textValue(row.priority) as InvestigationNextAction["priority"],
    expectedInformationGain: numberValue(row.expected_information_gain), estimatedCost: numberValue(row.estimated_cost), suggestedPrompt: textValue(row.suggested_prompt),
    createdAt: textValue(row.created_at), updatedAt: textValue(row.updated_at), revision: numberValue(row.revision),
  };
}

function consolidationFromRow(row: SqlRow): ConsolidationCandidate {
  return {
    id: textValue(row.id), workspaceId: textValue(row.workspace_id), proposedType: textValue(row.proposed_type) as ConsolidationCandidate["proposedType"],
    title: textValue(row.title), summary: textValue(row.summary), status: textValue(row.status) as ConsolidationCandidate["status"],
    sourceInvestigationIds: jsonStrings(row.source_investigation_ids_json), sourceMemoryNodeIds: jsonStrings(row.source_memory_node_ids_json),
    promotedMemoryNodeId: optionalText(row.promoted_memory_node_id), createdAt: textValue(row.created_at), reviewedAt: optionalText(row.reviewed_at),
  };
}

function replayNodeFromRow(row: SqlRow): ReplayMemoryNode {
  return {
    id: textValue(row.id), type: textValue(row.type), title: textValue(row.title), summary: textValue(row.summary), status: textValue(row.status),
    attributes: jsonObject(row.attributes_json), createdAt: textValue(row.created_at), updatedAt: textValue(row.updated_at),
  };
}

function inferStage(text: string): CampaignTrackStage {
  const normalized = text.toLowerCase();
  if (/\b(report|submission|disclosure)\b/u.test(normalized)) return "reporting";
  if (/\b(independent(?:ly)? verify|verification|challenge the finding)\b/u.test(normalized)) return "verifying";
  if (/\b(reproduce|reproduction|proof of concept|poc)\b/u.test(normalized)) return "reproducing";
  if (/\b(test|experiment|validate|refute|discriminat)\w*\b/u.test(normalized)) return "testing";
  if (/\b(map|enumerate|orient|surface)\w*\b/u.test(normalized)) return "orienting";
  return "exploring";
}

function titleFromObjective(objective: string): string {
  const heading = objective.match(/^\s*#*\s*Objective\s*\n+([^\n]+)/imu)?.[1]?.trim();
  const first = heading || objective.replace(/^\s*#+\s*/u, "").split(/[\n.!?]/u).map((part) => part.trim()).find(Boolean) || "Untitled research track";
  return first.length > 100 ? `${first.slice(0, 97).trimEnd()}...` : first;
}

function researchSessionTitle(row: SqlRow): string | null {
  const storedTitle = optionalText(row.title);
  if (storedTitle && !isPlaceholderTitle(storedTitle)) return storedTitle;
  const prompt = sessionPrompt(row.document_json);
  return prompt ? titleFromObjective(prompt) : null;
}

function sessionPrompt(document: unknown): string | null {
  try {
    const parsed = JSON.parse(textValue(document)) as unknown;
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return null;
    return optionalText((parsed as Record<string, unknown>).prompt);
  } catch {
    return null;
  }
}

function isPlaceholderTitle(title: string): boolean {
  const normalized = normalizeText(title);
  return normalized === "no title yet" || normalized === "untitled research track";
}

function researchSignature(value: string): Set<string> {
  return new Set(value.toLowerCase().match(/[a-z0-9][a-z0-9_+.-]{2,}/gu)?.filter((token) => !SESSION_TITLE_STOP_WORDS.has(token)) ?? []);
}

function signatureSimilarity(left: ReadonlySet<string>, right: ReadonlySet<string>): number {
  if (left.size === 0 || right.size === 0) return 0;
  const intersection = intersectionSize(left, right);
  return intersection / (left.size + right.size - intersection);
}

function intersectionSize(left: ReadonlySet<string>, right: ReadonlySet<string>): number {
  let count = 0; for (const value of left) if (right.has(value)) count += 1; return count;
}

function consolidationKey(title: string): string {
  return [...researchSignature(title)].sort().slice(0, 8).join("-");
}

function consolidatedSummary(nodes: readonly MemoryNode[]): string {
  const summaries = unique(nodes.map((node) => node.summary.trim()).filter(Boolean));
  const combined = `Reviewed across ${nodes.length} source memories from multiple campaign tracks. ${summaries.join(" ")}`;
  return combined.length > 1_500 ? `${combined.slice(0, 1_497).trimEnd()}...` : combined;
}

function uniqueEvidence(evidence: readonly MemoryEvidenceRef[]): Array<Omit<MemoryEvidenceRef, "id" | "createdAt">> {
  const seen = new Set<string>();
  return evidence.flatMap((item) => {
    const key = `${item.kind}\0${item.pathBase ?? ""}\0${item.path ?? ""}\0${JSON.stringify(item.locator)}\0${item.summary}`;
    if (seen.has(key)) return [];
    seen.add(key);
    return [{ kind: item.kind, ...(item.pathBase ? { pathBase: item.pathBase } : {}), ...(item.path ? { path: item.path } : {}), locator: item.locator, summary: item.summary }];
  });
}

function normalizeText(value: string): string { return value.trim().toLowerCase().replace(/\s+/gu, " "); }
function stableId(prefix: string, value: string): string { return `${prefix}_${createHash("sha256").update(value).digest("hex").slice(0, 24)}`; }
function requiredText(value: unknown, label: string): string { const result = typeof value === "string" ? value.trim() : ""; if (!result) throw new Error(`${label} is required.`); return result; }
function textValue(value: unknown): string { return typeof value === "string" ? value : value === null || value === undefined ? "" : String(value); }
function optionalText(value: unknown): string | null { const result = textValue(value).trim(); return result || null; }
function numberValue(value: unknown, fallback = 0): number { return typeof value === "number" && Number.isFinite(value) ? value : fallback; }
function bounded(value: number): number { if (!Number.isFinite(value)) throw new Error("Expected a finite score."); return Math.max(0, Math.min(1, value)); }
function unique<T>(values: readonly T[]): T[] { return [...new Set(values)]; }
function uniqueById<T extends { id: string }>(values: readonly T[]): T[] { return [...new Map(values.map((value) => [value.id, value])).values()]; }
function jsonObject(value: unknown): Record<string, unknown> { try { const parsed = typeof value === "string" ? JSON.parse(value) : value; return parsed && typeof parsed === "object" && !Array.isArray(parsed) ? parsed as Record<string, unknown> : {}; } catch { return {}; } }
function jsonStrings(value: unknown): string[] { try { const parsed = typeof value === "string" ? JSON.parse(value) : value; return Array.isArray(parsed) ? parsed.filter((item): item is string => typeof item === "string") : []; } catch { return []; } }
function median(values: readonly number[]): number | null { if (values.length === 0) return null; const sorted = [...values].sort((a, b) => a - b); const middle = Math.floor(sorted.length / 2); return sorted.length % 2 ? sorted[middle]! : (sorted[middle - 1]! + sorted[middle]!) / 2; }
function tableExists(database: DatabaseSync, name: string): boolean { return Boolean(database.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?").get(name)); }
function tableNameFromSelect(sql: string): string { return sql.match(/\bFROM\s+([a-zA-Z0-9_]+)/iu)?.[1] ?? ""; }

type SqlRow = Record<string, unknown>;
