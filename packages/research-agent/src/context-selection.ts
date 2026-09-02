import { statSync } from "node:fs";
import { isAbsolute, relative, resolve } from "node:path";
import type {
  CampaignGraphSummary,
  CampaignTrackProjectionSummary,
} from "./knowledge-types.js";
import {
  createModelWorkspaceContext,
  type ResearchModelWorkspaceContext,
} from "./model-context.js";
import type {
  ResearchAgentInstructions,
  ResearchWorkspaceContext,
  ResearchWorkspaceResourceContext,
} from "./types.js";
import { PRE_BEALE_DATA_DIRECTORY_NAME } from "./legacy-compatibility.js";

const MAX_SELECTION_TEXT = 1_200;
const MAX_SELECTION_ITEM_TEXT = 700;
const MAX_INDEX_NOTE_TEXT = 400;
const MAX_SELECTED_RESOURCES = 12;
const MAX_SELECTED_REPOSITORIES = 12;
const MAX_SELECTED_MEMORIES = 12;
const MAX_SELECTED_CLAIMS = 12;
const MAX_SELECTED_ARTIFACTS = 10;
const MAX_SELECTED_TRACKS = 8;
const MAX_SELECTED_NOTES = 16;
const MAX_SELECTED_PATHS = 16;
const MAX_KEY_FACTS = 12;
const MAX_OPEN_QUESTIONS = 10;
const MAX_CONSTRAINTS = 12;
const MAX_REFERENCES_PER_FACT = 8;

export interface ResearchContextSelectionFact {
  summary: string;
  references: readonly string[];
}

export interface ResearchContextSelection {
  schemaVersion: 1;
  summary: string;
  rationale: string;
  selectedResourceIds: readonly string[];
  selectedRepositoryRoots: readonly string[];
  selectedMemoryIds: readonly string[];
  selectedClaimIds: readonly string[];
  selectedRunbookIds: readonly string[];
  selectedReportIds: readonly string[];
  selectedTrackIds: readonly string[];
  selectedProjectNoteIndexes: readonly number[];
  selectedPaths: readonly string[];
  keyFacts: readonly ResearchContextSelectionFact[];
  openQuestions: readonly string[];
  constraints: readonly string[];
}

export interface ResearchContextSelectionCatalog {
  resourceIds: ReadonlySet<string>;
  repositoryRoots: ReadonlySet<string>;
  memoryIds: ReadonlySet<string>;
  claimIds: ReadonlySet<string>;
  runbookIds: ReadonlySet<string>;
  reportIds: ReadonlySet<string>;
  trackIds: ReadonlySet<string>;
  projectNoteCount: number;
  inspectionRoots: readonly string[];
}

export interface ResearchInitialContextPacket {
  schemaVersion: 1;
  source: "model-preflight" | "deterministic-fallback";
  summary: string;
  rationale: string;
  resources: readonly ResearchWorkspaceResourceContext[];
  repositoryRoots: readonly string[];
  paths: readonly string[];
  research: {
    memoryIds: readonly string[];
    claimIds: readonly string[];
    runbookIds: readonly string[];
    reportIds: readonly string[];
    trackIds: readonly string[];
  };
  keyFacts: readonly ResearchContextSelectionFact[];
  openQuestions: readonly string[];
  constraints: readonly string[];
  omitted: {
    resources: number;
    repositories: number;
    projectNotes: number;
    tracks: number;
  };
}

export interface ResearchContextPreflightIndex {
  schemaVersion: 1;
  workspace: {
    authorization: ResearchWorkspaceContext["authorization"] | null;
    identity: ResearchModelWorkspaceContext["memory"] | null;
    resources: readonly Pick<
      ResearchWorkspaceResourceContext,
      "id" | "direction" | "kind" | "locator" | "name" | "sensitivity"
    >[];
    repositories: readonly Pick<
      ResearchWorkspaceContext["knownRepositories"][number],
      "rootPath" | "label" | "role" | "source" | "repositoryUrl"
    >[];
    materializedSourcePaths: readonly string[];
    projectNotes: readonly { index: number; text: string }[];
    instructionDirectoryHints: readonly string[];
  };
  research: {
    counts: CampaignGraphSummary["counts"];
    activeTrackId: string | null;
    tracks: readonly {
      id: string;
      title: string;
      status: string;
      stage: string;
      updatedAt: string;
      revision: number;
      counts: CampaignTrackProjectionSummary["counts"];
    }[];
  };
}

export function createResearchContextPreflightIndex(input: {
  workspaceContext: ResearchWorkspaceContext;
  campaign: CampaignGraphSummary;
  agentInstructions: ResearchAgentInstructions;
}): ResearchContextPreflightIndex {
  const workspace = createModelWorkspaceContext(input.workspaceContext);
  return {
    schemaVersion: 1,
    workspace: {
      authorization: input.workspaceContext.authorization ?? null,
      identity: workspace.memory ?? null,
      resources: (input.workspaceContext.resources ?? []).map((resource) => ({
        id: resource.id,
        direction: resource.direction,
        kind: resource.kind,
        locator: truncate(resource.locator, MAX_SELECTION_ITEM_TEXT),
        ...(resource.name ? { name: truncate(resource.name, MAX_SELECTION_ITEM_TEXT) } : {}),
        ...(resource.sensitivity ? { sensitivity: resource.sensitivity } : {}),
      })),
      repositories: input.workspaceContext.knownRepositories.map((repository) => ({
        rootPath: repository.rootPath,
        role: repository.role,
        ...(repository.label ? { label: truncate(repository.label, MAX_SELECTION_ITEM_TEXT) } : {}),
        ...(repository.source ? { source: repository.source } : {}),
        ...(repository.repositoryUrl
          ? { repositoryUrl: truncate(repository.repositoryUrl, MAX_SELECTION_ITEM_TEXT) }
          : {}),
      })),
      materializedSourcePaths: input.workspaceContext.materializedSourcePaths,
      projectNotes: input.workspaceContext.projectNotes.map((text, index) => ({
        index,
        text: truncate(text, MAX_INDEX_NOTE_TEXT),
      })),
      instructionDirectoryHints: discoverInstructionDirectoryHints(
        input.agentInstructions,
        input.workspaceContext.workspaceRoot,
      ),
    },
    research: {
      counts: { ...input.campaign.counts },
      activeTrackId: input.campaign.activeTrackId ?? null,
      tracks: [...(input.campaign.tracks ?? [])]
        .sort((left, right) => right.updatedAt.localeCompare(left.updatedAt))
        .map((track) => ({
          id: track.id,
          title: truncate(track.title, MAX_SELECTION_ITEM_TEXT),
          status: track.status,
          stage: track.stage,
          updatedAt: track.updatedAt,
          revision: track.revision,
          counts: { ...track.counts },
        })),
    },
  };
}

export function createResearchContextSelectionCatalog(input: {
  workspaceContext: ResearchWorkspaceContext;
  campaign: CampaignGraphSummary;
  memoryIds: readonly string[];
  inspectionRoots: readonly string[];
}): ResearchContextSelectionCatalog {
  return {
    resourceIds: new Set((input.workspaceContext.resources ?? []).map((resource) => resource.id)),
    repositoryRoots: new Set(input.workspaceContext.knownRepositories.map((repository) => resolve(repository.rootPath))),
    memoryIds: new Set(input.memoryIds),
    claimIds: new Set(input.campaign.nodes.flatMap((node) => node.claimId ? [node.claimId] : [])),
    runbookIds: new Set(input.campaign.nodes.filter((node) => node.kind === "runbook").map((node) => node.id.replace(/^runbook:/u, ""))),
    reportIds: new Set(input.campaign.nodes.filter((node) => node.kind === "report").map((node) => node.id.replace(/^report:/u, ""))),
    trackIds: new Set((input.campaign.tracks ?? []).map((track) => track.id)),
    projectNoteCount: input.workspaceContext.projectNotes.length,
    inspectionRoots: uniqueResolved(input.inspectionRoots),
  };
}

export function createResearchContextSelectionPrompt(request: string): string {
  return [
    "You are the context preflight for a research session. Build the smallest useful starting context for the lead agent; do not conduct the research itself.",
    "",
    "Inspect the compact workspace and research indexes, then use the available read-only tools to retrieve only material directly relevant to the request. Search existing claims, memories, and runbooks together with history.search, and inspect reports before scanning files. You may inspect the workspace and instruction-declared directory hints when that improves target orientation. Do not modify files, memory, claims, runbooks, reports, resources, or external state.",
    "",
    "Prefer exact canonical IDs, repository roots, evidence references, established negative results, current target/build facts, and unresolved proof obligations. Exclude merely recent or globally important research that is unrelated to this request. The lead agent retains all normal tools and may depart from your selection.",
    "",
    "Return one JSON object inside <context_selection> and </context_selection> tags. Do not put Markdown fences around the JSON.",
    "",
    "Required schema:",
    JSON.stringify({
      schemaVersion: 1,
      summary: "one concise orientation paragraph",
      rationale: "why this subset is sufficient",
      selectedResourceIds: ["scope resource IDs"],
      selectedRepositoryRoots: ["absolute configured repository roots"],
      selectedMemoryIds: ["memory IDs"],
      selectedClaimIds: ["lead or finding IDs"],
      selectedRunbookIds: ["runbook IDs"],
      selectedReportIds: ["report IDs"],
      selectedTrackIds: ["investigation track IDs"],
      selectedProjectNoteIndexes: [0],
      selectedPaths: ["absolute files or directories inspected or worth opening first"],
      keyFacts: [{ summary: "canonical fact or material negative", references: ["durable ID or path"] }],
      openQuestions: ["unresolved question that affects the first action"],
      constraints: ["request-specific limitation or environment fact"],
    }, null, 2),
    "",
    "Research request:",
    request,
  ].join("\n");
}

export function parseResearchContextSelection(
  output: string,
  catalog: ResearchContextSelectionCatalog,
): ResearchContextSelection {
  const tagged = output.match(/<context_selection>\s*([\s\S]*?)\s*<\/context_selection>/iu)?.[1];
  if (!tagged) throw new Error("Context preflight did not return a tagged selection packet.");
  let parsed: unknown;
  try {
    parsed = JSON.parse(tagged);
  } catch (error) {
    throw new Error(`Context preflight returned invalid JSON: ${error instanceof Error ? error.message : String(error)}`);
  }
  if (!isRecord(parsed)) throw new Error("Context preflight selection must be a JSON object.");
  const selection = normalizeSelection(parsed, catalog);
  if (!selection.summary || !selection.rationale) {
    throw new Error("Context preflight selection requires a non-empty summary and rationale.");
  }
  return selection;
}

export function createFallbackResearchContextSelection(input: {
  prompt: string;
  workspaceContext: ResearchWorkspaceContext;
  campaign: CampaignGraphSummary;
  memoryIds: readonly string[];
  inspectionRoots: readonly string[];
}): ResearchContextSelection {
  const terms = queryTerms(input.prompt);
  const resources = (input.workspaceContext.resources ?? [])
    .filter((resource) => terms.some((term) => `${resource.name ?? ""} ${resource.locator}`.toLowerCase().includes(term)))
    .slice(0, MAX_SELECTED_RESOURCES);
  const repositories = input.workspaceContext.knownRepositories
    .filter((repository) => terms.some((term) => `${repository.label ?? ""} ${repository.rootPath}`.toLowerCase().includes(term)))
    .slice(0, MAX_SELECTED_REPOSITORIES);
  const activeTrackId = input.campaign.activeTrackId && input.campaign.tracks?.some((track) => track.id === input.campaign.activeTrackId)
    ? input.campaign.activeTrackId
    : null;
  return {
    schemaVersion: 1,
    summary: "Deterministic startup context selected from request-matching workspace assets and current-session memory.",
    rationale: "The model preflight was unavailable or invalid, so app-server retained a compact canonical fallback instead of restoring the full campaign dump.",
    selectedResourceIds: resources.map((resource) => resource.id),
    selectedRepositoryRoots: repositories.map((repository) => resolve(repository.rootPath)),
    selectedMemoryIds: input.memoryIds.slice(0, MAX_SELECTED_MEMORIES),
    selectedClaimIds: [],
    selectedRunbookIds: [],
    selectedReportIds: [],
    selectedTrackIds: activeTrackId ? [activeTrackId] : [],
    selectedProjectNoteIndexes: [],
    selectedPaths: repositories.map((repository) => resolve(repository.rootPath)).slice(0, MAX_SELECTED_PATHS),
    keyFacts: [],
    openQuestions: [],
    constraints: [],
  };
}

export function createResearchInitialContextPacket(input: {
  source: ResearchInitialContextPacket["source"];
  selection: ResearchContextSelection;
  workspaceContext: ResearchWorkspaceContext;
  campaign: CampaignGraphSummary;
}): ResearchInitialContextPacket {
  const resources = (input.workspaceContext.resources ?? []).filter((resource) =>
    input.selection.selectedResourceIds.includes(resource.id),
  );
  return {
    schemaVersion: 1,
    source: input.source,
    summary: input.selection.summary,
    rationale: input.selection.rationale,
    resources,
    repositoryRoots: input.selection.selectedRepositoryRoots,
    paths: input.selection.selectedPaths,
    research: {
      memoryIds: input.selection.selectedMemoryIds,
      claimIds: input.selection.selectedClaimIds,
      runbookIds: input.selection.selectedRunbookIds,
      reportIds: input.selection.selectedReportIds,
      trackIds: input.selection.selectedTrackIds,
    },
    keyFacts: input.selection.keyFacts,
    openQuestions: input.selection.openQuestions,
    constraints: input.selection.constraints,
    omitted: {
      resources: Math.max(0, (input.workspaceContext.resources?.length ?? 0) - resources.length),
      repositories: Math.max(0, input.workspaceContext.knownRepositories.length - input.selection.selectedRepositoryRoots.length),
      projectNotes: Math.max(0, input.workspaceContext.projectNotes.length - input.selection.selectedProjectNoteIndexes.length),
      tracks: Math.max(0, (input.campaign.tracks?.length ?? 0) - input.selection.selectedTrackIds.length),
    },
  };
}

export function projectSelectedModelWorkspaceContext(
  workspaceContext: ResearchWorkspaceContext,
  selection: ResearchContextSelection,
): ResearchModelWorkspaceContext {
  const base = createModelWorkspaceContext(workspaceContext);
  const roots = new Set(selection.selectedRepositoryRoots.map((root) => resolve(root)));
  const materialized = new Set(selection.selectedPaths.map((path) => resolve(path)));
  const selectedNotes = new Set(selection.selectedProjectNoteIndexes);
  return {
    ...base,
    knownRepositories: base.knownRepositories.filter((repository) => roots.has(resolve(repository.rootPath))),
    materializedSourcePaths: base.materializedSourcePaths.filter((path) =>
      roots.has(resolve(path)) || materialized.has(resolve(path)),
    ),
    projectNotes: base.projectNotes.filter((note, index) =>
      selectedNotes.has(index) || isAlwaysProjectNote(note),
    ),
  };
}

export function discoverInstructionDirectoryHints(
  instructions: ResearchAgentInstructions,
  workspaceRoot: string,
): string[] {
  const hints = [resolve(workspaceRoot)];
  for (const line of instructions.content.split(/\r?\n/u)) {
    const separator = line.indexOf(":");
    if (separator < 0) continue;
    const label = line.slice(0, separator).replace(/^\s*[-*]\s*/u, "").trim().toLowerCase();
    if (!/(?:directory|prior research|resources?|source|artifacts?)/u.test(label)) continue;
    const remainder = line.slice(separator + 1);
    for (const match of remainder.matchAll(/(?:^|\s)((?:[A-Za-z]:[\\/]|\/)[A-Za-z0-9_.,+@%:=~\\/-]+)/gu)) {
      const candidate = match[1]?.replace(/[),.;:]+$/u, "");
      if (!candidate || sensitiveInstructionPath(candidate)) continue;
      const path = resolve(candidate);
      try {
        if (statSync(path).isDirectory()) hints.push(path);
      } catch {
        // Instruction-declared paths may be temporarily unavailable.
      }
    }
  }
  return uniqueResolved(hints);
}

function normalizeSelection(
  input: Record<string, unknown>,
  catalog: ResearchContextSelectionCatalog,
): ResearchContextSelection {
  return {
    schemaVersion: 1,
    summary: boundedString(input.summary, MAX_SELECTION_TEXT),
    rationale: boundedString(input.rationale, MAX_SELECTION_TEXT),
    selectedResourceIds: selectedStrings(input.selectedResourceIds, catalog.resourceIds, MAX_SELECTED_RESOURCES),
    selectedRepositoryRoots: selectedResolvedPaths(input.selectedRepositoryRoots, catalog.repositoryRoots, MAX_SELECTED_REPOSITORIES),
    selectedMemoryIds: selectedStrings(input.selectedMemoryIds, catalog.memoryIds, MAX_SELECTED_MEMORIES),
    selectedClaimIds: selectedStrings(input.selectedClaimIds, catalog.claimIds, MAX_SELECTED_CLAIMS),
    selectedRunbookIds: selectedStrings(input.selectedRunbookIds, catalog.runbookIds, MAX_SELECTED_ARTIFACTS),
    selectedReportIds: selectedStrings(input.selectedReportIds, catalog.reportIds, MAX_SELECTED_ARTIFACTS),
    selectedTrackIds: selectedStrings(input.selectedTrackIds, catalog.trackIds, MAX_SELECTED_TRACKS),
    selectedProjectNoteIndexes: selectedIndexes(input.selectedProjectNoteIndexes, catalog.projectNoteCount, MAX_SELECTED_NOTES),
    selectedPaths: selectedInspectionPaths(input.selectedPaths, catalog.inspectionRoots, MAX_SELECTED_PATHS),
    keyFacts: selectedFacts(input.keyFacts, catalog),
    openQuestions: boundedStringArray(input.openQuestions, MAX_OPEN_QUESTIONS),
    constraints: boundedStringArray(input.constraints, MAX_CONSTRAINTS),
  };
}

function selectedStrings(value: unknown, allowed: ReadonlySet<string>, limit: number): string[] {
  if (!Array.isArray(value)) return [];
  return unique(value.flatMap((item) => typeof item === "string" && allowed.has(item.trim()) ? [item.trim()] : [])).slice(0, limit);
}

function selectedResolvedPaths(value: unknown, allowed: ReadonlySet<string>, limit: number): string[] {
  if (!Array.isArray(value)) return [];
  return unique(value.flatMap((item) => {
    if (typeof item !== "string" || !item.trim()) return [];
    const path = resolve(item.trim());
    return allowed.has(path) ? [path] : [];
  })).slice(0, limit);
}

function selectedInspectionPaths(value: unknown, roots: readonly string[], limit: number): string[] {
  if (!Array.isArray(value)) return [];
  return unique(value.flatMap((item) => {
    if (typeof item !== "string" || !item.trim() || !isAbsolute(item.trim())) return [];
    const path = resolve(item.trim());
    return roots.some((root) => isWithin(path, root)) && !sensitiveInstructionPath(path) ? [path] : [];
  })).slice(0, limit);
}

function selectedIndexes(value: unknown, count: number, limit: number): number[] {
  if (!Array.isArray(value)) return [];
  return unique(value.flatMap((item) => Number.isSafeInteger(item) && Number(item) >= 0 && Number(item) < count ? [Number(item)] : [])).slice(0, limit);
}

function selectedFacts(
  value: unknown,
  catalog: ResearchContextSelectionCatalog,
): ResearchContextSelectionFact[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((item) => {
    if (!isRecord(item)) return [];
    const summary = boundedString(item.summary, MAX_SELECTION_ITEM_TEXT);
    if (!summary) return [];
    const references = canonicalFactReferences(item.references, catalog);
    if (references.length === 0) return [];
    return [{
      summary,
      references,
    }];
  }).slice(0, MAX_KEY_FACTS);
}

function canonicalFactReferences(
  value: unknown,
  catalog: ResearchContextSelectionCatalog,
): string[] {
  if (!Array.isArray(value)) return [];
  const canonicalIds = new Set([
    ...catalog.resourceIds,
    ...catalog.memoryIds,
    ...catalog.claimIds,
    ...catalog.runbookIds,
    ...catalog.reportIds,
    ...catalog.trackIds,
    ...catalog.repositoryRoots,
  ]);
  return unique(value.flatMap((item) => {
    const reference = boundedString(item, MAX_SELECTION_ITEM_TEXT);
    if (!reference) return [];
    if (canonicalIds.has(reference)) return [reference];
    if (!isAbsolute(reference)) return [];
    const path = resolve(reference);
    return catalog.inspectionRoots.some((root) => isWithin(path, root))
      && !sensitiveInstructionPath(path)
      ? [path]
      : [];
  })).slice(0, MAX_REFERENCES_PER_FACT);
}

function boundedStringArray(value: unknown, limit: number): string[] {
  if (!Array.isArray(value)) return [];
  return unique(value.flatMap((item) => {
    const text = boundedString(item, MAX_SELECTION_ITEM_TEXT);
    return text ? [text] : [];
  })).slice(0, limit);
}

function boundedString(value: unknown, limit: number): string {
  return typeof value === "string" ? truncate(value.trim(), limit) : "";
}

function truncate(value: string, limit: number): string {
  return value.length <= limit ? value : `${value.slice(0, Math.max(0, limit - 1))}…`;
}

function isAlwaysProjectNote(note: string): boolean {
  return !/^\s*included\s+in\s+authorized\s+scope\b/iu.test(note);
}

function sensitiveInstructionPath(path: string): boolean {
  const normalized = path.replaceAll("\\", "/");
  const segments = normalized.split("/");
  return segments.some((segment) => segment === PRE_BEALE_DATA_DIRECTORY_NAME)
    || /(?:^|\/)(?:\.ssh|\.beale|\.codex)(?:\/|$)/u.test(normalized)
    || /(?:^|\/)(?:id_[^/]+|credentials?|tokens?)(?:\.[^/]*)?$/iu.test(normalized);
}

function isWithin(path: string, root: string): boolean {
  const offset = relative(resolve(root), resolve(path));
  return offset === "" || (offset.length > 0 && !offset.startsWith("..") && !isAbsolute(offset));
}

function queryTerms(value: string): string[] {
  const stopWords = new Set(["and", "the", "this", "that", "with", "from", "research", "objective"]);
  return unique((value.toLowerCase().match(/[a-z0-9][a-z0-9_.+-]{2,}/gu) ?? [])
    .filter((term) => !stopWords.has(term)))
    .slice(0, 24);
}

function unique<T>(values: readonly T[]): T[] {
  return [...new Set(values)];
}

function uniqueResolved(values: readonly string[]): string[] {
  return unique(values.map((value) => resolve(value)));
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}
