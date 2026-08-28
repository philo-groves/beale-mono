import { nowIso } from "./ids.js";
import {
  CampaignTrackStore,
  type CampaignTrackStage,
  type InvestigationExperiment,
  type InvestigationNextAction,
  type InvestigationObservation,
  type InvestigationQuestion,
} from "./campaign-tracks.js";
import type { FindingStatus } from "./knowledge-types.js";
import type {
  ResearchExecutableTool,
  ResearchToolExecutionContext,
  ResearchToolExecutionResult,
} from "./tool-registry.js";
import type { ResearchToolAction } from "./types.js";

const PRIORITIES = ["critical", "high", "medium", "low"] as const;

export function createCampaignTrackTools(
  store: CampaignTrackStore,
  activeInvestigationId: string,
): ResearchExecutableTool[] {
  return [
    tool(
      "investigation.status",
      "investigation_status",
      "Read a compact current campaign status. Pass afterRevision from the previous result to receive an unchanged marker instead of repeated data. Increase limit only when the bounded open items are needed.",
      "read",
      {
        type: "object",
        properties: {
          afterRevision: { type: "number", minimum: 1 },
          limit: { type: "number", minimum: 1, maximum: 20 },
        },
      },
      (input) => store.status(activeInvestigationId, {
        ...(typeof input.afterRevision === "number" ? { afterRevision: Math.floor(input.afterRevision) } : {}),
        ...(typeof input.limit === "number" ? { limit: Math.floor(input.limit) } : {}),
      }),
    ),
    tool(
      "investigation.recall",
      "investigation_recall",
      "Recall a stage-balanced evidence set for the current campaign track. It deliberately includes current claims, target facts, rejected or stale paths, procedures, and cross-track principles rather than returning only the nearest text matches.",
      "read",
      {
        type: "object",
        required: ["query"],
        properties: {
          query: { type: "string" },
          stage: { type: "string", enum: ["orienting", "exploring", "testing", "reproducing", "verifying", "reporting", "complete", "blocked"] },
          limit: { type: "number", minimum: 1, maximum: 25 },
        },
      },
      (input) => store.recall({
        investigationId: activeInvestigationId,
        query: requiredString(input.query, "query"),
        ...(string(input.stage) ? { stage: string(input.stage) as CampaignTrackStage } : {}),
        ...(typeof input.limit === "number" ? { maxNodes: Math.floor(input.limit) } : {}),
      }),
    ),
    tool(
      "investigation.question",
      "investigation_question",
      "Create or update one bounded uncertainty for the current campaign track. Questions should identify evidence that would change the investigation, not restate the overall objective.",
      "write",
      {
        type: "object",
        required: ["text"],
        properties: {
          text: { type: "string" },
          status: { type: "string", enum: ["open", "answered", "blocked", "superseded"] },
          priority: { type: "string", enum: PRIORITIES },
          answer: { type: "string" },
        },
      },
      (input) => store.upsertQuestion({
        investigationId: activeInvestigationId,
        text: requiredString(input.text, "text"),
        ...(string(input.status) ? { status: string(input.status) as InvestigationQuestion["status"] } : {}),
        ...(string(input.priority) ? { priority: string(input.priority) as InvestigationQuestion["priority"] } : {}),
        ...(typeof input.answer === "string" ? { answer: input.answer } : {}),
      }),
    ),
    tool(
      "investigation.experiment",
      "investigation_experiment",
      "Plan or update an evidence-producing experiment. Record the positive result that would support the candidate and the result that would genuinely contradict or narrow it before execution, plus the exact source revision and environment that bound the result. A failed setup or inconclusive run is not refutation.",
      "write",
      {
        type: "object",
        required: ["title"],
        properties: {
          title: { type: "string" },
          status: { type: "string", enum: ["planned", "running", "succeeded", "failed", "inconclusive", "blocked"] },
          questionId: { type: "string" },
          hypothesisMemoryId: { type: "string" },
          runbookId: { type: "string" },
          expectedOutcomes: { type: "object" },
          resultSummary: { type: "string" },
          sourceRevision: { type: "string" },
          environmentFingerprint: { type: "string" },
        },
      },
      (input) => store.upsertExperiment({
        investigationId: activeInvestigationId,
        title: requiredString(input.title, "title"),
        ...(string(input.status) ? { status: string(input.status) as InvestigationExperiment["status"] } : {}),
        ...(string(input.questionId) ? { questionId: string(input.questionId) } : {}),
        ...(string(input.hypothesisMemoryId) ? { hypothesisMemoryId: string(input.hypothesisMemoryId) } : {}),
        ...(string(input.runbookId) ? { runbookId: string(input.runbookId) } : {}),
        ...(record(input.expectedOutcomes) ? { expectedOutcomes: record(input.expectedOutcomes)! } : {}),
        ...(typeof input.resultSummary === "string" ? { resultSummary: input.resultSummary } : {}),
        ...(typeof input.sourceRevision === "string" ? { sourceRevision: input.sourceRevision } : {}),
        ...(typeof input.environmentFingerprint === "string" ? { environmentFingerprint: input.environmentFingerprint } : {}),
      }),
    ),
    tool(
      "investigation.observe",
      "investigation_observe",
      "Append an immutable observation produced by an experiment or direct inspection. Link the durable memory node and exact evidence references when available; interpretation belongs in claims, not in the observation.",
      "write",
      {
        type: "object",
        required: ["kind", "outcome", "summary"],
        properties: {
          experimentId: { type: "string" },
          memoryNodeId: { type: "string" },
          kind: { type: "string", enum: ["source", "runtime", "artifact", "verifier", "human", "historical"] },
          outcome: { type: "string", enum: ["supports", "refutes", "narrows", "neutral"] },
          summary: { type: "string" },
          evidenceRefIds: { type: "array", items: { type: "string" } },
          sourceEventId: { type: "string" },
        },
      },
      (input) => store.addObservation({
        investigationId: activeInvestigationId,
        ...(string(input.experimentId) ? { experimentId: string(input.experimentId) } : {}),
        ...(string(input.memoryNodeId) ? { memoryNodeId: string(input.memoryNodeId) } : {}),
        kind: requiredString(input.kind, "kind") as InvestigationObservation["kind"],
        outcome: requiredString(input.outcome, "outcome") as InvestigationObservation["outcome"],
        summary: requiredString(input.summary, "summary"),
        evidenceRefIds: strings(input.evidenceRefIds),
        ...(string(input.sourceEventId) ? { sourceEventId: string(input.sourceEventId) } : {}),
      }),
    ),
    tool(
      "investigation.next_action",
      "investigation_next_action",
      "Create or update a next action, including expected information gain and estimated cost. Prefer the best evidence gain, balancing actions that can establish the next positive proof obligation with actions that can genuinely contradict or narrow a necessary link; do not default to the cheapest falsifier.",
      "write",
      {
        type: "object",
        required: ["title"],
        properties: {
          title: { type: "string" },
          rationale: { type: "string" },
          status: { type: "string", enum: ["open", "in_progress", "completed", "dismissed"] },
          priority: { type: "string", enum: PRIORITIES },
          questionId: { type: "string" },
          expectedInformationGain: { type: "number", minimum: 0, maximum: 1 },
          estimatedCost: { type: "number", minimum: 0, maximum: 1 },
          suggestedPrompt: { type: "string" },
        },
      },
      (input) => store.upsertNextAction({
        investigationId: activeInvestigationId,
        title: requiredString(input.title, "title"),
        ...(typeof input.rationale === "string" ? { rationale: input.rationale } : {}),
        ...(string(input.status) ? { status: string(input.status) as InvestigationNextAction["status"] } : {}),
        ...(string(input.priority) ? { priority: string(input.priority) as InvestigationNextAction["priority"] } : {}),
        ...(string(input.questionId) ? { questionId: string(input.questionId) } : {}),
        ...(typeof input.expectedInformationGain === "number" ? { expectedInformationGain: input.expectedInformationGain } : {}),
        ...(typeof input.estimatedCost === "number" ? { estimatedCost: input.estimatedCost } : {}),
        ...(typeof input.suggestedPrompt === "string" ? { suggestedPrompt: input.suggestedPrompt } : {}),
      }),
    ),
    tool(
      "investigation.review_claim",
      "investigation_review_claim",
      "Independently review a canonical research claim for both supporting proof and genuinely contrary evidence. Acceptance promotes the same stable claim ID and requires evidence plus a reviewer identity distinct from every authoring agent; a separate subagent or separate session qualifies when it did not author the claim. Missing support alone calls for revision or more work, not automatic rejection.",
      "write",
      {
        type: "object",
        required: ["claimId", "expectedRevision", "verdict", "rationale", "evidenceIds"],
        properties: {
          claimId: { type: "string" },
          expectedRevision: { type: "number" },
          verdict: { type: "string", enum: ["accept", "revise", "reject"] },
          rationale: { type: "string" },
          evidenceIds: { type: "array", items: { type: "string" } },
          targetClassification: { type: "string" },
          targetStatus: { type: "string", enum: ["observed", "reproduced", "verified", "report_ready", "disclosed"] },
          confidence: { type: "number", minimum: 0, maximum: 1 },
        },
      },
      (input, context) => store.reviewClaim({
        investigationId: activeInvestigationId,
        claimId: requiredString(input.claimId, "claimId"),
        expectedRevision: requiredInteger(input.expectedRevision, "expectedRevision"),
        verdict: requiredString(input.verdict, "verdict") as "accept" | "revise" | "reject",
        rationale: requiredString(input.rationale, "rationale"),
        evidenceIds: strings(input.evidenceIds),
        ...(string(input.targetClassification) ? { targetClassification: string(input.targetClassification)! } : {}),
        ...(string(input.targetStatus) ? { targetStatus: string(input.targetStatus) as FindingStatus } : {}),
        ...(typeof input.confidence === "number" ? { confidence: input.confidence } : {}),
        ...(context?.modelAuthor ? { reviewer: context.modelAuthor } : {}),
        ...(context?.agentId ? { reviewerAgentId: context.agentId } : {}),
      }),
    ),
    tool(
      "investigation.consolidate",
      "investigation_consolidate",
      "Generate cross-track procedure and invariant candidates. This never promotes them automatically; use investigation.review_consolidation from an independent session after inspecting their provenance.",
      "write",
      { type: "object", properties: {} },
      () => store.generateConsolidationCandidates(),
    ),
    tool(
      "investigation.review_consolidation",
      "investigation_review_consolidation",
      "Accept or reject one cross-track consolidation candidate. Acceptance requires an independent review session and preserves links to every source memory.",
      "write",
      {
        type: "object",
        required: ["id", "verdict"],
        properties: { id: { type: "string" }, verdict: { type: "string", enum: ["accept", "reject"] } },
      },
      (input, context) => store.reviewConsolidation({
        id: requiredString(input.id, "id"),
        verdict: requiredString(input.verdict, "verdict") as "accept" | "reject",
        ...(context?.modelAuthor ? { reviewer: context.modelAuthor } : {}),
      }),
    ),
  ];
}

function tool(
  name: string,
  transportName: string,
  description: string,
  sideEffects: "read" | "write",
  parameters: Record<string, unknown>,
  run: (input: Record<string, unknown>, context?: ResearchToolExecutionContext) => unknown,
): ResearchExecutableTool {
  return {
    descriptor: {
      name,
      transportName,
      description,
      actionClasses: [sideEffects === "read" ? "recall" : "synthesize"],
      sideEffects,
      requiredPermissions: [sideEffects === "read" ? "memory:read" : "memory:write"],
      inputSchema: parameters,
    },
    parameters: parameters as NonNullable<ResearchExecutableTool["parameters"]>,
    async execute(action: ResearchToolAction, context?: ResearchToolExecutionContext): Promise<ResearchToolExecutionResult> {
      const startedAt = nowIso();
      try {
        const output = run(record(action.input) ?? {}, context);
        return { action, status: "complete", startedAt, completedAt: nowIso(), summary: `${name} completed.`, output, followUpActions: [] };
      } catch (error) {
        return {
          action,
          status: "error",
          startedAt,
          completedAt: nowIso(),
          summary: `${name} failed.`,
          error: { message: error instanceof Error ? error.message : String(error) },
          followUpActions: [],
        };
      }
    },
  };
}

function record(value: unknown): Record<string, unknown> | null { return typeof value === "object" && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : null; }
function string(value: unknown): string | null { return typeof value === "string" && value.trim() ? value.trim() : null; }
function strings(value: unknown): string[] { return Array.isArray(value) ? value.flatMap((item) => typeof item === "string" && item.trim() ? [item.trim()] : []) : []; }
function requiredString(value: unknown, field: string): string { const result = string(value); if (!result) throw new Error(`${field} must be a non-empty string.`); return result; }
function requiredInteger(value: unknown, field: string): number { if (typeof value !== "number" || !Number.isSafeInteger(value)) throw new Error(`${field} must be an integer.`); return value; }
