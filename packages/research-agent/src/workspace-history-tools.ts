import { createHash } from "node:crypto";
import type { ResearchClaimStore } from "./findings.js";
import { nowIso } from "./ids.js";
import type { MemoryGraphStore, MemoryNodeStatus, MemoryNodeType } from "./memory-graph.js";
import type { RunbookStore } from "./runbooks.js";
import type {
  ResearchExecutableTool,
  ResearchToolExecutionContext,
  ResearchToolExecutionResult,
} from "./tool-registry.js";
import type { ResearchToolAction } from "./types.js";

export const WORKSPACE_HISTORY_TYPES = ["claims", "memories", "runbooks"] as const;
export type WorkspaceHistoryType = (typeof WORKSPACE_HISTORY_TYPES)[number];

const DEFAULT_HISTORY_SEARCH_LIMIT = 12;
const MAX_HISTORY_SEARCH_LIMIT = 40;
const SEARCH_CANDIDATE_LIMIT = 100;
const MAX_SUMMARY_CHARACTERS = 700;
const MAX_TAGS = 6;
const MAX_EVIDENCE_REFS = 3;

export interface WorkspaceHistorySearchToolOptions {
  memoryStore?: MemoryGraphStore;
  claimStore?: ResearchClaimStore;
  runbookStore?: RunbookStore;
}

export type WorkspaceHistoryRecordType = "claim" | "memory" | "runbook";

interface HistoryCandidate {
  key: string;
  revision: number;
  updatedAt: string;
  searchText: string;
  summary: Record<string, unknown>;
  type: WorkspaceHistoryType;
}

export function createWorkspaceHistorySearchTool(
  options: WorkspaceHistorySearchToolOptions,
): ResearchExecutableTool {
  const availableTypes = WORKSPACE_HISTORY_TYPES.filter((type) =>
    type === "memories" ? Boolean(options.memoryStore)
      : type === "claims" ? Boolean(options.claimStore)
        : Boolean(options.runbookStore));
  if (availableTypes.length === 0) throw new Error("Workspace history search requires at least one history store.");
  const memory = options.memoryStore?.getProfileMemory();
  const readableMemoryTypes = memory
    ? catalogIdsAndAliases(memory.types.filter((type) => type.lifecycle === "active"))
    : [];
  const memoryStatuses = memory?.statuses.map((status) => status.id) ?? [];
  const schema = {
    type: "object",
    properties: {
      query: { type: "string", description: "Text to find across titles, summaries, classifications, statuses, and reusable procedure descriptions." },
      types: {
        type: "array",
        uniqueItems: true,
        items: { type: "string", enum: availableTypes },
        description: "Record categories to search together. Defaults to every available category.",
      },
      ...(memory ? {
        memoryTypes: { type: "array", items: { type: "string", enum: readableMemoryTypes } },
        memoryStatuses: { type: "array", items: { type: "string", enum: memoryStatuses } },
        assetIds: { type: "array", items: { type: "string" }, description: "Optional memory-only asset filter." },
        tags: { type: "array", items: { type: "string" }, description: "Optional memory-only tag filter." },
      } : {}),
      ...(options.claimStore ? {
        claimStatuses: { type: "array", items: { type: "string", enum: ["hypothesis", "observed", "reproduced", "verified", "report_ready", "disclosed", "stale", "rejected"] } },
        claimClassifications: { type: "array", items: { type: "string" } },
      } : {}),
      limit: { type: "number", minimum: 1, maximum: MAX_HISTORY_SEARCH_LIMIT },
      afterRevision: { type: "string", description: "Return an unchanged response when this exact filtered catalog revision is still current." },
    },
  };
  return tool(
    "history.search",
    "history_search",
    "Search the current workspace's canonical claims, knowledge memories, and runbooks through one history index. Use the multi-value types filter to narrow categories. Results are compact typed cards; duplicates are excluded with the canonical claim catalog. Use the corresponding get or list tool only when more detail is needed, and search before repeating prior work.",
    schema,
    (input) => searchWorkspaceHistory(options, input),
  );
}

export function createWorkspaceHistoryDuplicateTools(
  options: WorkspaceHistorySearchToolOptions,
): ResearchExecutableTool[] {
  const availableTypes = [
    ...(options.claimStore ? ["claim" as const] : []),
    ...(options.memoryStore ? ["memory" as const] : []),
    ...(options.runbookStore ? ["runbook" as const] : []),
  ];
  if (availableTypes.length === 0) throw new Error("Workspace history duplicate tools require at least one history store.");
  const common = {
    type: "object",
    required: ["type", "id", "expectedRevision", "reason"],
    properties: {
      type: { type: "string", enum: availableTypes, description: "The workspace-history record type." },
      id: { type: "string", description: "The duplicate record." },
      expectedRevision: { type: "number", minimum: 1 },
      reason: { type: "string", description: "Why the records are or are not duplicates." },
    },
  };
  return [
    duplicateTool(
      "history.mark_duplicate",
      "history_mark_duplicate",
      "Coalesce a duplicate claim, memory, or runbook under the strongest canonical record of the same type. The duplicate disappears from normal catalogs and history.search, remains visible at the bottom of the canonical record's details, and can be restored. Coalesce only records with the same underlying meaning or procedure; related components and useful variants are not duplicates.",
      {
        ...common,
        required: [...common.required, "parentId"],
        properties: {
          ...common.properties,
          parentId: { type: "string", description: "The canonical record that should remain visible." },
        },
      },
      (input, context) => mutateDuplicate(options, input, false, context),
    ),
    duplicateTool(
      "history.undo_duplicate",
      "history_undo_duplicate",
      "Restore a claim, memory, or runbook that was incorrectly marked as a duplicate. The record becomes visible in normal catalogs and history.search again.",
      common,
      (input, context) => mutateDuplicate(options, input, true, context),
    ),
  ];
}

function mutateDuplicate(
  options: WorkspaceHistorySearchToolOptions,
  input: Record<string, unknown>,
  undo: boolean,
  context?: ResearchToolExecutionContext,
): unknown {
  const type = historyRecordType(input.type);
  const id = requiredText(input.id, "id");
  const expectedRevision = requiredPositiveInteger(input.expectedRevision, "expectedRevision");
  const reason = requiredText(input.reason, "reason");
  const parentId = undo ? null : requiredText(input.parentId, "parentId");
  if (type === "claim") {
    if (!options.claimStore) throw new Error("Claim history is unavailable in this research profile.");
    return undo
      ? options.claimStore.undoDuplicate(id, { expectedRevision, reason }, context?.modelAuthor, context?.agentId)
      : options.claimStore.markDuplicate(id, { expectedRevision, parentClaimId: parentId!, reason }, context?.modelAuthor, context?.agentId);
  }
  if (type === "memory") {
    if (!options.memoryStore) throw new Error("Memory history is unavailable in this research profile.");
    return undo
      ? options.memoryStore.undoDuplicate(id, { expectedRevision, reason }, context?.modelAuthor)
      : options.memoryStore.markDuplicate(id, { expectedRevision, parentMemoryId: parentId!, reason }, context?.modelAuthor);
  }
  if (!options.runbookStore) throw new Error("Runbook history is unavailable in this research profile.");
  return undo
    ? options.runbookStore.undoDuplicate(id, { expectedRevision, reason }, context?.modelAuthor)
    : options.runbookStore.markDuplicate(id, { expectedRevision, parentRunbookId: parentId!, reason }, context?.modelAuthor);
}

function searchWorkspaceHistory(
  options: WorkspaceHistorySearchToolOptions,
  input: Record<string, unknown>,
): Record<string, unknown> {
  const query = text(input.query)?.toLowerCase() ?? "";
  const requestedTypes = historyTypes(input.types);
  const selectedTypes = new Set<WorkspaceHistoryType>(requestedTypes.length
    ? requestedTypes
    : WORKSPACE_HISTORY_TYPES.filter((type) =>
      type === "memories" ? Boolean(options.memoryStore)
        : type === "claims" ? Boolean(options.claimStore)
          : Boolean(options.runbookStore)));
  const limit = clampLimit(input.limit);
  const candidates: HistoryCandidate[] = [];

  if (selectedTypes.has("memories") && options.memoryStore) {
    const nodes = options.memoryStore.search({
      ...(query ? { query } : {}),
      scope: "workspace",
      ...(strings(input.memoryTypes).length ? { types: strings(input.memoryTypes) as MemoryNodeType[] } : {}),
      ...(strings(input.memoryStatuses).length ? { statuses: strings(input.memoryStatuses) as MemoryNodeStatus[] } : {}),
      ...(strings(input.assetIds).length ? { assetIds: strings(input.assetIds) } : {}),
      ...(strings(input.tags).length ? { tags: strings(input.tags) } : {}),
      limit: SEARCH_CANDIDATE_LIMIT,
    });
    const relationshipCounts = relationshipCountByNode(options.memoryStore, nodes.map((node) => node.id));
    for (const node of nodes) {
      candidates.push({
        key: `memory:${node.id}`,
        revision: node.revision,
        updatedAt: node.updatedAt,
        type: "memories",
        searchText: `${node.id}\n${node.type}\n${node.title}\n${node.summary}\n${node.status}\n${node.tags.join(" ")}`,
        summary: {
          detail: "summary", type: "memory", id: node.id, memoryType: node.type, title: node.title,
          summary: truncateText(node.summary), status: node.status, confidence: node.confidence,
          ...(node.tags.length ? { tags: node.tags.slice(0, MAX_TAGS) } : {}),
          evidenceCount: node.evidence.length,
          duplicateCount: node.duplicateMemories.length,
          ...(node.evidence.length ? { evidenceRefs: node.evidence.slice(0, MAX_EVIDENCE_REFS).map((evidence) => ({
            id: evidence.id, kind: evidence.kind,
            ...(evidence.pathBase ? { pathBase: evidence.pathBase } : {}),
            ...(evidence.path ? { path: evidence.path } : {}),
          })) } : {}),
          relationshipCount: relationshipCounts.get(node.id) ?? 0,
          updatedAt: node.updatedAt, revision: node.revision,
        },
      });
    }
  }

  if (selectedTypes.has("claims") && options.claimStore) {
    const statuses = new Set(strings(input.claimStatuses));
    const classifications = new Set(strings(input.claimClassifications));
    for (const claim of options.claimStore.list()) {
      if (statuses.size && !statuses.has(claim.status)) continue;
      if (classifications.size && !classifications.has(claim.classification)) continue;
      const searchText = `${claim.id}\n${claim.projection}\n${claim.maturity}\n${claim.status}\n${claim.rating}\n${claim.classification}\n${claim.title}\n${claim.summary}\n${claim.impact}\n${claim.componentClaimIds.join(" ")}\n${claim.evidence.map((evidence) => `${evidence.kind} ${evidence.referenceId ?? ""} ${evidence.summary}`).join("\n")}`;
      if (query && !matchesQuery(searchText, query)) continue;
      candidates.push({
        key: `claim:${claim.id}`,
        revision: claim.revision,
        updatedAt: claim.updatedAt,
        type: "claims",
        searchText,
        summary: {
          detail: "summary", type: "claim", id: claim.id, projection: claim.projection,
          maturity: claim.maturity, freshness: claim.freshness, workflow: claim.workflow,
          rating: claim.rating, classification: claim.classification, title: claim.title,
          summary: truncateText(claim.summary), impact: truncateText(claim.impact), status: claim.status,
          confidence: claim.confidence, componentClaimIds: claim.componentClaimIds,
          evidenceCount: claim.evidence.length, duplicateCount: claim.duplicateClaims.length,
          independentEvidenceCount: claim.evidence.filter((evidence) => evidence.independent).length,
          ...(claim.evidence.length ? { evidenceRefs: claim.evidence.slice(0, MAX_EVIDENCE_REFS).map((evidence) => ({
            id: evidence.id, kind: evidence.kind, referenceId: evidence.referenceId,
            summary: truncateText(evidence.summary), independent: evidence.independent,
          })) } : {}),
          sourceRevision: claim.sourceRevision, environmentFingerprint: claim.environmentFingerprint,
          reproductionRunbookId: claim.reproductionRunbookId, reportId: claim.reportId,
          updatedAt: claim.updatedAt, revision: claim.revision,
        },
      });
    }
  }

  if (selectedTypes.has("runbooks") && options.runbookStore) {
    for (const runbook of options.runbookStore.list({ limit: 200 })) {
      const searchText = `${runbook.id}\n${runbook.title}\n${runbook.purpose}`;
      if (query && !matchesQuery(searchText, query)) continue;
      candidates.push({
        key: `runbook:${runbook.id}`,
        revision: runbook.revision,
        updatedAt: runbook.updatedAt,
        type: "runbooks",
        searchText,
        summary: {
          detail: "summary", type: "runbook", id: runbook.id, title: runbook.title,
          purpose: truncateText(runbook.purpose), cellCount: runbook.cellCount,
          contentRevision: runbook.contentRevision, execution: runbook.execution,
          duplicateCount: runbook.duplicateRunbooks.length,
          updatedAt: runbook.updatedAt, revision: runbook.revision,
        },
      });
    }
  }

  candidates.sort((left, right) =>
    (query ? historySearchScore(right.searchText, query) - historySearchScore(left.searchText, query) : 0)
    || right.updatedAt.localeCompare(left.updatedAt)
    || left.key.localeCompare(right.key));
  const revision = createHash("sha256")
    .update(JSON.stringify({ query, types: [...selectedTypes].sort(), limit,
      memoryTypes: strings(input.memoryTypes).sort(), memoryStatuses: strings(input.memoryStatuses).sort(),
      claimStatuses: strings(input.claimStatuses).sort(), claimClassifications: strings(input.claimClassifications).sort(),
      assetIds: strings(input.assetIds).sort(), tags: strings(input.tags).sort() }))
    .update("\n")
    .update(candidates.map((candidate) => `${candidate.key}:${candidate.revision}:${candidate.updatedAt}`).join("\n"))
    .digest("hex").slice(0, 16);
  const counts = Object.fromEntries(WORKSPACE_HISTORY_TYPES.map((type) => [type, candidates.filter((candidate) => candidate.type === type).length]));
  if (text(input.afterRevision) === revision) {
    return { revision, unchanged: true, matched: candidates.length, resultCount: 0, counts, results: [] };
  }
  const results = candidates.slice(0, limit).map((candidate) => candidate.summary);
  return {
    revision, unchanged: false, matched: candidates.length, truncated: candidates.length > limit,
    resultCount: results.length, counts, detail: "summary", results,
    recall: "Use memory.get or runbook.get for full records. Use lead.list or finding.list for complete canonical claim state.",
  };
}

function tool(
  name: string,
  transportName: string,
  description: string,
  parameters: Record<string, unknown>,
  run: (input: Record<string, unknown>, context?: ResearchToolExecutionContext) => unknown,
): ResearchExecutableTool {
  return {
    descriptor: { name, transportName, description, actionClasses: ["recall"], sideEffects: "read", requiredPermissions: ["memory:read"], inputSchema: parameters },
    parameters: parameters as NonNullable<ResearchExecutableTool["parameters"]>,
    async execute(action: ResearchToolAction, context?: ResearchToolExecutionContext): Promise<ResearchToolExecutionResult> {
      const startedAt = nowIso();
      try {
        return { action, status: "complete", startedAt, completedAt: nowIso(), summary: `${name} completed.`, output: run(isRecord(action.input) ? action.input : {}, context), followUpActions: [] };
      } catch (error) {
        return { action, status: "error", startedAt, completedAt: nowIso(), summary: `${name} failed.`, error: { message: error instanceof Error ? error.message : String(error) }, followUpActions: [] };
      }
    },
  };
}

function duplicateTool(
  name: string,
  transportName: string,
  description: string,
  parameters: Record<string, unknown>,
  run: (input: Record<string, unknown>, context?: ResearchToolExecutionContext) => unknown,
): ResearchExecutableTool {
  return {
    descriptor: { name, transportName, description, actionClasses: ["synthesize"], sideEffects: "write", requiredPermissions: ["memory:write"], inputSchema: parameters },
    parameters: parameters as NonNullable<ResearchExecutableTool["parameters"]>,
    async execute(action: ResearchToolAction, context?: ResearchToolExecutionContext): Promise<ResearchToolExecutionResult> {
      const startedAt = nowIso();
      try {
        return { action, status: "complete", startedAt, completedAt: nowIso(), summary: `${name} completed.`, output: run(isRecord(action.input) ? action.input : {}, context), followUpActions: [] };
      } catch (error) {
        return { action, status: "error", startedAt, completedAt: nowIso(), summary: `${name} failed.`, error: { message: error instanceof Error ? error.message : String(error) }, followUpActions: [] };
      }
    },
  };
}

function relationshipCountByNode(store: MemoryGraphStore, ids: readonly string[]): Map<string, number> {
  const counts = new Map<string, number>();
  for (const edge of store.listEdgesForNodes(ids)) {
    counts.set(edge.fromId, (counts.get(edge.fromId) ?? 0) + 1);
    counts.set(edge.toId, (counts.get(edge.toId) ?? 0) + 1);
  }
  return counts;
}

function historyTypes(value: unknown): WorkspaceHistoryType[] {
  return strings(value).filter((item): item is WorkspaceHistoryType => WORKSPACE_HISTORY_TYPES.includes(item as WorkspaceHistoryType));
}

function historySearchScore(searchText: string, query: string): number {
  const haystack = searchText.toLowerCase();
  const terms = query.split(/\s+/u).filter(Boolean);
  return (haystack.includes(query) ? 20 : 0)
    + terms.reduce((score, term) => score + (haystack.includes(term) ? 2 : 0), 0);
}

function matchesQuery(searchText: string, query: string): boolean {
  const haystack = searchText.toLowerCase();
  return query.split(/\s+/u).filter(Boolean).some((term) => haystack.includes(term));
}

function catalogIdsAndAliases(types: readonly { id: string; aliases?: readonly string[] }[]): string[] {
  return [...new Set(types.flatMap((type) => [type.id, ...(type.aliases ?? [])]))];
}

function clampLimit(value: unknown): number {
  const requested = typeof value === "number" && Number.isFinite(value) ? Math.floor(value) : DEFAULT_HISTORY_SEARCH_LIMIT;
  return Math.max(1, Math.min(MAX_HISTORY_SEARCH_LIMIT, requested));
}

function truncateText(value: string): string {
  return value.length <= MAX_SUMMARY_CHARACTERS ? value : `${value.slice(0, MAX_SUMMARY_CHARACTERS - 1)}…`;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function text(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function requiredText(value: unknown, label: string): string {
  const result = text(value);
  if (!result) throw new Error(`${label} must be a non-empty string.`);
  return result;
}

function requiredPositiveInteger(value: unknown, label: string): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 1) {
    throw new Error(`${label} must be a positive integer.`);
  }
  return value;
}

function historyRecordType(value: unknown): WorkspaceHistoryRecordType {
  if (value === "claim" || value === "memory" || value === "runbook") return value;
  throw new Error("type must be claim, memory, or runbook.");
}

function strings(value: unknown): string[] {
  return Array.isArray(value) ? value.flatMap((item) => typeof item === "string" && item.trim() ? [item.trim()] : []) : [];
}
