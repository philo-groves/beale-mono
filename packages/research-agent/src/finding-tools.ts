import { createHash } from "node:crypto";
import { nowIso } from "./ids.js";
import {
  candidateCompletionChecklist,
  ResearchClaimStore,
  type CandidateCompletionTarget,
  type FindingEvidenceInput,
  type FindingSecurityTrackingUpdate,
} from "./findings.js";
import type { FindingEvidenceKind, FindingStatus, FindingSummary, ResearchClaimRating } from "./knowledge-types.js";
import type { ResearchExecutableTool, ResearchToolExecutionContext, ResearchToolExecutionResult } from "./tool-registry.js";
import type { ResearchToolAction } from "./types.js";

const EVIDENCE_KINDS: FindingEvidenceKind[] = ["code", "artifact", "command", "url", "calculation", "proof", "publication", "human_review", "runbook_execution", "independent_verification", "report", "disclosure"];
const FINDING_STATUSES: FindingStatus[] = ["hypothesis", "observed", "reproduced", "verified", "report_ready", "disclosed", "stale", "rejected"];
const RESEARCH_CLAIM_RATINGS: ResearchClaimRating[] = ["informational", "low", "medium", "high", "critical"];
const EVIDENCE_SCHEMA = {
  type: "object",
  required: ["kind", "summary"],
  properties: {
    kind: { type: "string", enum: EVIDENCE_KINDS },
    referenceId: { type: "string", description: "Durable evidence identity. For runbook_execution use runId from runbook.run output or execution.latestSuccessfulRunId from runbook.get; for report use the exact reportId; for disclosure use the exact disclosureReference." },
    contentHash: { type: "string" },
    summary: { type: "string" },
    independent: { type: "boolean" },
    metadata: { type: "object" },
  },
};
const SECURITY_TRACKING_SCHEMA = {
  type: "object",
  description: "Evidence-backed security tracking. Risk acceptance is intentionally operator-controlled and is not model-writable.",
  properties: {
    reachability: {
      type: "object",
      required: ["state"],
      properties: {
        state: { type: "string", enum: ["not_assessed", "unreachable", "conditional", "reachable"] },
        conditions: { type: "string" },
        evidenceIds: { type: "array", items: { type: "string" } },
        assessedAt: { type: "string" },
        sourceRevision: { type: "string" },
        environmentFingerprint: { type: "string" },
      },
    },
    cvssAssessment: {
      type: "object",
      required: ["version", "vector", "score", "nomenclature"],
      properties: {
        version: { type: "string", enum: ["4.0", "3.1"] },
        vector: { type: "string" },
        score: { type: "number", minimum: 0, maximum: 10 },
        nomenclature: { type: "string", enum: ["CVSS-B", "CVSS-BT", "CVSS-BE", "CVSS-BTE", "CVSS:3.1"] },
        assessedAt: { type: "string" },
        environmentFingerprint: { type: "string" },
      },
    },
    affectedAssetIds: { type: "array", items: { type: "string" } },
    affectedVersions: {
      type: "array",
      items: {
        type: "object",
        required: ["range"],
        properties: { assetId: { type: "string" }, range: { type: "string" }, fixedVersion: { type: "string" } },
      },
    },
    externalReferences: {
      type: "array",
      items: {
        type: "object",
        required: ["kind", "identifier"],
        properties: { kind: { type: "string" }, identifier: { type: "string" }, url: { type: "string" } },
      },
    },
  },
};

export interface FindingToolDefaults {
  classifications?: readonly string[];
}

export function createFindingTools(store: ResearchClaimStore, defaults: FindingToolDefaults = {}): ResearchExecutableTool[] {
  return [
    findingTool("lead.list", "lead_list", "List proposed or refuted research leads when the full lead catalog or complete claim state is needed. Use history.search for normal workspace-history search. Leads and findings are views of one claim ledger, so a lead keeps the same ID when evidence promotes it to a finding.", "read", {
      type: "object",
      properties: {
        query: { type: "string" },
        statuses: { type: "array", items: { type: "string", enum: FINDING_STATUSES } },
        classifications: { type: "array", items: { type: "string" } },
        limit: { type: "number", minimum: 1, maximum: 100 },
        afterRevision: { type: "string" },
      },
    }, (input) => projectClaimCatalog(store.listLeads(), input, "leads")),
    findingTool("finding.list", "finding_list", "List evidence-backed findings when the full finding catalog or complete claim state is needed. Use history.search for normal workspace-history search. Findings are promoted views of canonical research claims, not duplicate memory records.", "read", {
      type: "object",
      properties: {
        query: { type: "string" },
        statuses: { type: "array", items: { type: "string", enum: FINDING_STATUSES } },
        classifications: { type: "array", items: { type: "string" } },
        limit: { type: "number", minimum: 1, maximum: 100 },
        afterRevision: { type: "string" },
      },
    }, (input) => projectClaimCatalog(store.listFindings(), input, "findings")),
    findingTool("finding.completion_check", "finding_completion_check", "Evaluate a security candidate against the evidence-backed completion checklist for observed, reproduced, verified, or report-ready work. This is read-only and reports every required or recommended gap.", "read", {
      type: "object",
      required: ["id"],
      properties: {
        id: { type: "string" },
        targetStatus: { type: "string", enum: ["observed", "reproduced", "verified", "report_ready"] },
      },
    }, (input) => store.completionChecklist(
      requiredString(input.id, "id"),
      optionalCompletionTarget(input.targetStatus) ?? "verified",
    )),
    findingTool("lead.create", "lead_create", "Record one new research lead. Search claims with history.search first; do not duplicate an existing claim. This creates a proposed claim without asserting observation. The qualitative rating is an untrusted estimate for prioritization and notifications, not an evidence-backed severity assessment.", "write", {
      type: "object",
      required: ["title", "classification", "rating"],
      properties: {
        title: { type: "string" }, classification: { type: "string", ...(defaults.classifications?.length ? { enum: [...defaults.classifications] } : {}), description: "A classification declared by the active research profile." },
        summary: { type: "string" }, impact: { type: "string" },
        rating: { type: "string", enum: RESEARCH_CLAIM_RATINGS, description: "Untrusted qualitative estimate: informational, low, medium, high, or critical." },
        confidence: { type: "number", minimum: 0, maximum: 1 },
        sourceRevision: { type: "string", description: "Exact component identity. Use kind:resource-id:value (for example git:webkit:abc123) when automatic same-resource invalidation is desired." },
        environmentFingerprint: { type: "string", description: "Exact target identity. Use environment:resource-id:value when automatic same-target invalidation is desired." },
        evidence: { type: "array", items: EVIDENCE_SCHEMA },
      },
    }, (input, context) => store.create({
      title: requiredString(input.title, "title"),
      classification: requiredClassification(input.classification, defaults.classifications),
      rating: requiredResearchClaimRating(input.rating),
      ...(string(input.summary) ? { summary: string(input.summary)! } : {}),
      ...(string(input.impact) ? { impact: string(input.impact)! } : {}),
      ...(typeof input.confidence === "number" ? { confidence: input.confidence } : {}),
      ...(string(input.sourceRevision) ? { sourceRevision: string(input.sourceRevision)! } : {}),
      ...(string(input.environmentFingerprint) ? { environmentFingerprint: string(input.environmentFingerprint)! } : {}),
      ...(Array.isArray(input.evidence) ? { evidence: input.evidence.map(parseEvidence) } : {}),
    }, context?.modelAuthor, context?.agentId)),
    findingTool("finding.revise", "finding_revise", "Revise a canonical claim and its security tracking without changing evidence maturity. The qualitative rating remains an untrusted prioritization estimate. Reachability must cite existing finding evidence. CVSS stores the version, vector, assessed score, and nomenclature together. Risk treatment is operator-controlled and cannot be changed by this tool.", "write", {
      type: "object",
      required: ["id", "expectedRevision", "reason"],
      properties: {
        id: { type: "string" }, expectedRevision: { type: "number" }, reason: { type: "string" },
        title: { type: "string" }, summary: { type: "string" }, impact: { type: "string" },
        rating: { type: "string", enum: RESEARCH_CLAIM_RATINGS, description: "Untrusted qualitative estimate for prioritization and notifications." },
        confidence: { type: "number", minimum: 0, maximum: 1 }, classification: { type: "string" },
        componentClaimIds: { type: "array", items: { type: "string" } },
        securityTracking: SECURITY_TRACKING_SCHEMA,
      },
    }, (input, context) => store.revise(requiredString(input.id, "id"), {
      expectedRevision: requiredInteger(input.expectedRevision, "expectedRevision"),
      reason: requiredString(input.reason, "reason"),
      ...(input.title !== undefined ? { title: requiredString(input.title, "title") } : {}),
      ...(input.summary !== undefined ? { summary: string(input.summary) ?? "" } : {}),
      ...(input.impact !== undefined ? { impact: string(input.impact) ?? "" } : {}),
      ...(input.rating !== undefined ? { rating: requiredResearchClaimRating(input.rating) } : {}),
      ...(typeof input.confidence === "number" ? { confidence: input.confidence } : {}),
      ...(input.classification !== undefined ? { classification: requiredClassification(input.classification, defaults.classifications) } : {}),
      ...(Array.isArray(input.componentClaimIds) ? { componentClaimIds: input.componentClaimIds.map((id) => requiredString(id, "componentClaimIds[]")) } : {}),
      ...(input.securityTracking !== undefined ? { securityTracking: parseSecurityTrackingUpdate(input.securityTracking) } : {}),
    }, context?.modelAuthor, context?.agentId)),
    findingTool("finding.transition", "finding_transition", "Update a canonical claim through its evidence-gated lifecycle, or append new evidence by using its current status as toStatus. Same-status evidence appends retain the claim ID and maturity while creating an immutable revision. Direct observation promotes a lead to the finding view without changing its ID. Reproduction requires a successful runbook; verification must be independent; reporting and disclosure require matching durable references.", "write", {
      type: "object",
      required: ["id", "expectedRevision", "toStatus", "reason"],
      properties: {
        id: { type: "string" }, expectedRevision: { type: "number" }, toStatus: { type: "string", enum: FINDING_STATUSES }, reason: { type: "string" },
        evidence: { type: "array", items: EVIDENCE_SCHEMA },
        sourceRevision: { type: "string", description: "New verified component identity for a non-stale transition; ignored when marking stale so the prior baseline is preserved. Use kind:resource-id:value (for example git:webkit:abc123) for same-resource invalidation." },
        environmentFingerprint: { type: "string", description: "New verified target identity for a non-stale transition; ignored when marking stale so the prior baseline is preserved. Use environment:resource-id:value for same-target invalidation." },
        reproductionRunbookId: { type: "string" }, reportId: { type: "string" }, disclosureReference: { type: "string" },
        classification: { type: "string" },
        componentClaimIds: { type: "array", items: { type: "string" }, description: "Ordered component claim IDs for a composite classification such as security.chain." },
      },
    }, (input, context) => store.transition(requiredString(input.id, "id"), {
      expectedRevision: requiredInteger(input.expectedRevision, "expectedRevision"),
      toStatus: requiredStatus(input.toStatus),
      reason: requiredString(input.reason, "reason"),
      ...(Array.isArray(input.evidence) ? { evidence: input.evidence.map(parseEvidence) } : {}),
      ...(string(input.sourceRevision) ? { sourceRevision: string(input.sourceRevision)! } : {}),
      ...(string(input.environmentFingerprint) ? { environmentFingerprint: string(input.environmentFingerprint)! } : {}),
      ...(input.reproductionRunbookId !== undefined ? { reproductionRunbookId: string(input.reproductionRunbookId) } : {}),
      ...(input.reportId !== undefined ? { reportId: string(input.reportId) } : {}),
      ...(input.disclosureReference !== undefined ? { disclosureReference: string(input.disclosureReference) } : {}),
      ...(input.classification !== undefined ? { classification: requiredClassification(input.classification, defaults.classifications) } : {}),
      ...(Array.isArray(input.componentClaimIds) ? { componentClaimIds: input.componentClaimIds.map((id) => requiredString(id, "componentClaimIds[]")) } : {}),
    }, context?.modelAuthor, context?.agentId)),
  ];
}

function projectClaimCatalog(findings: readonly FindingSummary[], input: Record<string, unknown>, key: "leads" | "findings"): Record<string, unknown> {
  const query = string(input.query)?.toLowerCase() ?? "";
  const requestedStatuses = new Set(Array.isArray(input.statuses)
    ? input.statuses.flatMap((status) => typeof status === "string" && FINDING_STATUSES.includes(status as FindingStatus) ? [status] : [])
    : []);
  const requestedClassifications = new Set(Array.isArray(input.classifications)
    ? input.classifications.flatMap((classification) => string(classification) ? [string(classification)!] : [])
    : []);
  const limit = typeof input.limit === "number" && Number.isFinite(input.limit)
    ? Math.max(1, Math.min(100, Math.floor(input.limit)))
    : 25;
  const revision = createHash("sha256")
    .update(JSON.stringify({ query, statuses: [...requestedStatuses].sort(), classifications: [...requestedClassifications].sort(), limit }))
    .update("\n")
    .update(findings.map((finding) => `${finding.id}:${finding.revision}:${finding.updatedAt}`).join("\n"))
    .digest("hex")
    .slice(0, 16);
  if (string(input.afterRevision) === revision) {
    return { revision, unchanged: true, total: findings.length, [key]: [] };
  }
  const matches = findings
    .filter((finding) => requestedStatuses.size === 0 || requestedStatuses.has(finding.status))
    .filter((finding) => requestedClassifications.size === 0 || requestedClassifications.has(finding.classification))
    .filter((finding) => !query || `${finding.title}\n${finding.summary}\n${finding.impact}\n${finding.rating}`.toLowerCase().includes(query));
  return {
    revision,
    unchanged: false,
    total: findings.length,
    matched: matches.length,
    truncated: matches.length > limit,
    [key]: matches.slice(0, limit).map((finding) => ({
      id: finding.id,
      projection: finding.projection,
      maturity: finding.maturity,
      freshness: finding.freshness,
      workflow: finding.workflow,
      rating: finding.rating,
      classification: finding.classification,
      componentClaimIds: finding.componentClaimIds,
      title: finding.title,
      summary: finding.summary,
      impact: finding.impact,
      securityTracking: finding.securityTracking,
      status: finding.status,
      staleFromStatus: finding.staleFromStatus,
      confidence: finding.confidence,
      sourceRevision: finding.sourceRevision,
      environmentFingerprint: finding.environmentFingerprint,
      reproductionRunbookId: finding.reproductionRunbookId,
      reportId: finding.reportId,
      staleReason: finding.staleReason,
      evidenceCount: finding.evidence.length,
      duplicateCount: finding.duplicateClaims.length,
      independentEvidenceCount: finding.evidence.filter((evidence) => evidence.independent).length,
      latestTransition: finding.transitions.at(-1) ?? null,
      completion: (() => {
        const checklist = candidateCompletionChecklist(finding, "verified");
        return {
          targetStatus: checklist.targetStatus,
          ready: checklist.ready,
          completedRequired: checklist.completedRequired,
          requiredCount: checklist.requiredCount,
          missingRequired: checklist.missingRequired,
        };
      })(),
      updatedAt: finding.updatedAt,
      revision: finding.revision,
    })),
  };
}

function findingTool(name: string, transportName: string, description: string, sideEffects: "read" | "write", parameters: Record<string, unknown>, run: (input: Record<string, unknown>, context?: ResearchToolExecutionContext) => unknown): ResearchExecutableTool {
  return {
    descriptor: { name, transportName, description, actionClasses: [sideEffects === "read" ? "recall" : "synthesize"], sideEffects, requiredPermissions: [sideEffects === "read" ? "memory:read" : "memory:write"], inputSchema: parameters },
    parameters: parameters as NonNullable<ResearchExecutableTool["parameters"]>,
    async execute(action: ResearchToolAction, context?: ResearchToolExecutionContext): Promise<ResearchToolExecutionResult> {
      const startedAt = nowIso();
      try { return { action, status: "complete", startedAt, completedAt: nowIso(), summary: `${name} completed.`, output: run(isRecord(action.input) ? action.input : {}, context), followUpActions: [] }; }
      catch (error) { return { action, status: "error", startedAt, completedAt: nowIso(), summary: `${name} failed.`, error: { message: error instanceof Error ? error.message : String(error) }, followUpActions: [] }; }
    },
  };
}
function parseSecurityTrackingUpdate(value: unknown): FindingSecurityTrackingUpdate {
  const input = requiredRecord(value, "securityTracking");
  const reachability = input.reachability === undefined ? undefined : requiredRecord(input.reachability, "securityTracking.reachability");
  const cvss = input.cvssAssessment === undefined ? undefined : requiredRecord(input.cvssAssessment, "securityTracking.cvssAssessment");
  return {
    ...(reachability ? {
      reachability: {
        state: requiredReachabilityState(reachability.state),
        ...(reachability.conditions !== undefined ? { conditions: requiredString(reachability.conditions, "securityTracking.reachability.conditions") } : {}),
        ...(Array.isArray(reachability.evidenceIds) ? { evidenceIds: reachability.evidenceIds.map((id) => requiredString(id, "securityTracking.reachability.evidenceIds[]")) } : {}),
        ...(reachability.assessedAt !== undefined ? { assessedAt: requiredString(reachability.assessedAt, "securityTracking.reachability.assessedAt") } : {}),
        ...(reachability.sourceRevision !== undefined ? { sourceRevision: string(reachability.sourceRevision) } : {}),
        ...(reachability.environmentFingerprint !== undefined ? { environmentFingerprint: string(reachability.environmentFingerprint) } : {}),
      },
    } : {}),
    ...(cvss ? {
      cvssAssessment: {
        version: requiredCvssVersion(cvss.version),
        vector: requiredString(cvss.vector, "securityTracking.cvssAssessment.vector"),
        score: requiredNumber(cvss.score, "securityTracking.cvssAssessment.score"),
        nomenclature: requiredCvssNomenclature(cvss.nomenclature),
        assessedAt: cvss.assessedAt === undefined ? nowIso() : requiredString(cvss.assessedAt, "securityTracking.cvssAssessment.assessedAt"),
        environmentFingerprint: string(cvss.environmentFingerprint),
      },
    } : {}),
    ...(Array.isArray(input.affectedAssetIds) ? { affectedAssetIds: input.affectedAssetIds.map((id) => requiredString(id, "securityTracking.affectedAssetIds[]")) } : {}),
    ...(Array.isArray(input.affectedVersions) ? {
      affectedVersions: input.affectedVersions.map((value) => {
        const version = requiredRecord(value, "securityTracking.affectedVersions[]");
        return {
          assetId: string(version.assetId),
          range: requiredString(version.range, "securityTracking.affectedVersions[].range"),
          fixedVersion: string(version.fixedVersion),
        };
      }),
    } : {}),
    ...(Array.isArray(input.externalReferences) ? {
      externalReferences: input.externalReferences.map((value) => {
        const reference = requiredRecord(value, "securityTracking.externalReferences[]");
        return {
          kind: requiredString(reference.kind, "securityTracking.externalReferences[].kind"),
          identifier: requiredString(reference.identifier, "securityTracking.externalReferences[].identifier"),
          url: string(reference.url),
        };
      }),
    } : {}),
  };
}
function parseEvidence(value: unknown): FindingEvidenceInput { const input = requiredRecord(value, "evidence"); return { kind: requiredEvidenceKind(input.kind), summary: requiredString(input.summary, "evidence.summary"), ...(string(input.referenceId) ? { referenceId: string(input.referenceId) } : {}), ...(string(input.contentHash) ? { contentHash: string(input.contentHash) } : {}), ...(input.independent === true ? { independent: true } : {}), ...(isRecord(input.metadata) ? { metadata: input.metadata } : {}) }; }
function requiredStatus(value: unknown): FindingStatus { if (typeof value === "string" && FINDING_STATUSES.includes(value as FindingStatus)) return value as FindingStatus; throw new Error("toStatus is invalid."); }
function requiredResearchClaimRating(value: unknown): ResearchClaimRating { if (typeof value === "string" && RESEARCH_CLAIM_RATINGS.includes(value as ResearchClaimRating)) return value as ResearchClaimRating; throw new Error("rating is invalid."); }
function requiredEvidenceKind(value: unknown): FindingEvidenceKind { if (typeof value === "string" && EVIDENCE_KINDS.includes(value as FindingEvidenceKind)) return value as FindingEvidenceKind; throw new Error("evidence.kind is invalid."); }
function isRecord(value: unknown): value is Record<string, unknown> { return Boolean(value) && typeof value === "object" && !Array.isArray(value); }
function requiredRecord(value: unknown, field: string): Record<string, unknown> { if (!isRecord(value)) throw new Error(`${field} must be an object.`); return value; }
function string(value: unknown): string | null { return typeof value === "string" && value.trim() ? value.trim() : null; }
function requiredString(value: unknown, field: string): string { const result = string(value); if (!result) throw new Error(`${field} must be a non-empty string.`); return result; }
function requiredInteger(value: unknown, field: string): number { if (typeof value !== "number" || !Number.isSafeInteger(value)) throw new Error(`${field} must be an integer.`); return value; }
function requiredNumber(value: unknown, field: string): number { if (typeof value !== "number" || !Number.isFinite(value)) throw new Error(`${field} must be a number.`); return value; }
function requiredReachabilityState(value: unknown): "not_assessed" | "unreachable" | "conditional" | "reachable" { if (value === "not_assessed" || value === "unreachable" || value === "conditional" || value === "reachable") return value; throw new Error("securityTracking.reachability.state is invalid."); }
function requiredCvssVersion(value: unknown): "4.0" | "3.1" { if (value === "4.0" || value === "3.1") return value; throw new Error("securityTracking.cvssAssessment.version is invalid."); }
function requiredCvssNomenclature(value: unknown): "CVSS-B" | "CVSS-BT" | "CVSS-BE" | "CVSS-BTE" | "CVSS:3.1" { if (value === "CVSS-B" || value === "CVSS-BT" || value === "CVSS-BE" || value === "CVSS-BTE" || value === "CVSS:3.1") return value; throw new Error("securityTracking.cvssAssessment.nomenclature is invalid."); }
function optionalCompletionTarget(value: unknown): CandidateCompletionTarget | null { if (value === "observed" || value === "reproduced" || value === "verified" || value === "report_ready") return value; return null; }
function requiredClassification(value: unknown, allowed: readonly string[] | undefined): string {
  const classification = requiredString(value, "classification");
  if (allowed?.length && !allowed.includes(classification)) throw new Error(`classification is not declared by the active profile: ${classification}.`);
  return classification;
}
