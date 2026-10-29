import { createHash } from "node:crypto";
import type { DatabaseSync } from "node:sqlite";
import { openResearchDatabase } from "./database.js";
import { completeAuxiliaryText, type CompleteAuxiliaryTextOptions } from "./auxiliary-completion.js";
import { resolveAuxiliaryModelRoute, type AuxiliaryModelEffort } from "./auxiliary-model-job.js";
import { providerSemanticsDescriptor } from "./provider-semantics.js";
import { AppServerSessionStore } from "./session-store.js";
import {
  resolveStoredResearchProfile,
  resolveStoredResearchWorkspaceBinding,
  type StoredResearchWorkspaceBinding,
} from "./workspace-binding.js";

export interface ResearchGoalSuggestionPromptSuggestion {
  title: string;
  promptMarkdown: string;
  rationale?: string;
}

export interface GeneratedResearchGoalSuggestions {
  phase: string;
  suggestions: string[];
  promptSuggestions?: ResearchGoalSuggestionPromptSuggestion[];
  cacheStatus?: "stale";
}

export interface ResearchGoalSuggestionInput {
  workspaceId: string;
  phase?: string;
  requestId?: string;
  refresh?: boolean;
  sourceRunId?: string;
  /** Generate and persist one additional suggestion per request. */
  incremental?: boolean;
}

export interface ResearchGoalSuggestionSelectionInput {
  workspaceId: string;
  scopeId: string;
  profileHash: string;
  phase: string;
  suggestion: string;
}

export interface HostedResearchGoalSuggestionInput extends ResearchGoalSuggestionInput {
  workspaceRoot: string;
  databasePath: string;
  artifactDirectoryPath: string;
  researchProfileId: string;
  memoryEnabled: boolean;
  provider: {
    id: string;
    smallModel?: string;
    reasoningEffort?: string;
    authenticationPreferences?: CompleteAuxiliaryTextOptions["authenticationPreferences"];
    codexAuthFile?: string;
  };
}

export interface HostedResearchGoalSuggestionSelectionInput extends ResearchGoalSuggestionSelectionInput {
  databasePath: string;
}

export interface ResearchGoalSuggestionDependencies {
  completeText?: (options: CompleteAuxiliaryTextOptions) => Promise<{ text: string }>;
  signal?: AbortSignal;
}

const MAX_SUGGESTIONS = 12;
const SUGGESTION_GENERATOR_VERSION = "goal-suggestions-v3";

export async function generateStoredResearchGoalSuggestions(
  input: HostedResearchGoalSuggestionInput,
  dependencies: ResearchGoalSuggestionDependencies = {},
): Promise<GeneratedResearchGoalSuggestions> {
  const workspaceId = requiredText(input.workspaceId, "workspaceId", 256);
  const sourceRunId = optionalText(input.sourceRunId, 128);
  const profile = await resolveStoredResearchProfile({
    workspaceRoot: input.workspaceRoot,
    databasePath: input.databasePath,
    researchProfileId: input.researchProfileId,
  });
  const requestedPhase = optionalText(input.phase, 128);
  const workflow = requestedPhase
    ? profile.profile.workflows.find((candidate) => candidate.id === requestedPhase)
    : profile.profile.workflows.find((candidate) => candidate.default) ?? profile.profile.workflows[0];
  if (!workflow) {
    throw new Error(requestedPhase
      ? `Research suggestion lane ${requestedPhase} is not defined by profile ${profile.profile.id}@${profile.profile.version}.`
      : `Research profile ${profile.profile.id}@${profile.profile.version} has no suggestion lane.`);
  }
  const phase = workflow.id;
  if (workflow.goalSuggestionCount < 1 || workflow.goalSuggestionCount > MAX_SUGGESTIONS) {
    throw new Error(`Research suggestion lane ${phase} exceeds the host maximum of ${MAX_SUGGESTIONS}.`);
  }

  const sessions = new AppServerSessionStore({ databasePath: input.databasePath });
  try {
    if (sourceRunId) {
      const source = sessions.getSummary(sourceRunId);
      if (!source || source.workspaceId !== workspaceId) throw new Error(`Session not found: ${sourceRunId}`);
      if (source.status === "active" || source.status === "paused") {
        throw new Error("Next-step suggestions are only available after the source session has ended.");
      }
      if (source.workflowId && source.workflowId !== phase) {
        throw new Error(`Source session suggestion lane mismatch: expected ${source.workflowId}, received ${phase}.`);
      }
      const rawPromptSuggestions = source.finalDisposition?.nextPromptSuggestions;
      const promptSuggestions = Array.isArray(rawPromptSuggestions)
        ? rawPromptSuggestions.flatMap((suggestion) => {
            if (!isRecord(suggestion)) return [];
            const title = optionalText(suggestion.title, 300);
            const promptMarkdown = optionalText(suggestion.promptMarkdown, 8_000);
            const rationale = optionalText(suggestion.rationale, 2_000);
            return title && promptMarkdown
              ? [{ title, promptMarkdown, ...(rationale ? { rationale } : {}) }]
              : [];
          })
        : [];
      if (promptSuggestions.length > 0) {
        return { phase, suggestions: promptSuggestions.map((suggestion) => suggestion.title), promptSuggestions };
      }
    }

    const database = openSuggestionDatabase(input.databasePath);
    try {
      const context = readSuggestionContext(database, workspaceId);
      const sessionSummaries = sessions.listSummaries(workspaceId, 4)
        .filter((session) => session.status !== "active")
        .map((session) => ({
          id: session.id,
          title: boundedText(session.title, 120),
          summary: boundedText(session.summary, 300),
          status: session.status,
          endedAt: session.endedAt,
        }));
      const contextRevision = suggestionContextRevision(database, workspaceId, context, sessionSummaries);
      const cached = readSuggestionCache(database, workspaceId, context.scopeId, profile.hash, phase);
      const currentCache = cached?.contextRevision.startsWith(`${SUGGESTION_GENERATOR_VERSION}::`) ? cached : null;
      if (input.refresh !== true) {
        if (currentCache && (currentCache.suggestions.length >= workflow.goalSuggestionCount
          || currentCache.contextRevision !== contextRevision)) {
          return {
            phase,
            suggestions: currentCache.suggestions,
            ...(currentCache.contextRevision === contextRevision ? {} : { cacheStatus: "stale" as const }),
          };
        }
      }

      const binding = resolveStoredResearchWorkspaceBinding({
        workspaceRoot: input.workspaceRoot,
        databasePath: input.databasePath,
        researchProfileId: input.researchProfileId,
      });
      const providerSemantics = providerSemanticsDescriptor();
      const providerId = requiredText(input.provider.id, "provider", 100);
      const provider = providerSemantics.providers.find((candidate) => candidate === providerId)
        ?? providerSemantics.aliases[providerId];
      if (!provider) throw new Error(`Unsupported research model provider: ${providerId}.`);
      const route = resolveAuxiliaryModelRoute({
        jobName: "goalSuggestions",
        job: profile.profile.modelJobs.goalSuggestions ?? null,
        provider,
        configuredModel: optionalText(input.provider.smallModel, 200),
        configuredEffort: optionalText(input.provider.reasoningEffort, 30),
        fallbackModel: providerSemantics.defaultSmallModels[provider] ?? null,
        fallbackEffort: "low",
      });
      const suggestions = input.refresh === true || !currentCache || currentCache.contextRevision !== contextRevision
        ? [] : [...currentCache.suggestions];
      const contextPayload = {
        workspace: {
          name: context.workspaceName,
          scope: boundedText(context.description, 2_000),
          rules: context.rules,
          subject: binding.memoryContext.subjectName,
          resources: binding.resources.filter((resource) => resource.direction === "in_scope").slice(0, 20).map((resource) => ({
            id: resource.id,
            kind: resource.kind,
            label: boundedText(resource.name ?? resource.locator, 120),
            ...(resource.instruction ? { instruction: boundedText(resource.instruction, 240) } : {}),
          })),
          excludedResources: binding.resources.filter((resource) => resource.direction === "out_of_scope").slice(0, 12)
            .map((resource) => boundedText(resource.name ?? resource.locator, 120)),
        },
        surfaces: readSuggestionSurfaces(database, workspaceId),
        securityFindings: readSuggestionFindings(database, workspaceId),
        previousResearch: sessionSummaries,
      };
      const baseSystemPrompt = [
        `You are ${profile.profile.agent.role}.`,
        `Suggest one security research goal for the ${workflow.name} category: ${workflow.description}`,
        ...workflow.goalSuggestionInstructions,
        "Stay within the recorded authorization boundary and workspace rules.",
        "Focus on attacker-controlled entry points, trust boundaries, security-relevant sinks, and plausible unauthorized effects. Keep reachability and impact evidence-gated.",
        "Use the supplied scope and surface map when useful. A broadly reusable security direction is welcome; do not force it into one recorded claim or a narrow reference.",
        "Suggest a distinct goal from those already shown. Do not just rephrase one. Do not center harmless correctness bugs without a plausible security consequence.",
        "Write one concise, self-contained sentence. Do not prescribe commands or mention launch-time controls. Return the sentence as plain text; JSON with a single goal field is also accepted.",
      ].join("\n");
      while (suggestions.length < workflow.goalSuggestionCount) {
        let goal: string | null = null;
        let feedback: string | null = null;
        for (let attempt = 0; attempt < 3; attempt += 1) {
          const completion = await (dependencies.completeText ?? completeAuxiliaryText)({
            provider: route.provider,
            model: route.model,
            effort: route.effort as AuxiliaryModelEffort,
            systemPrompt: feedback ? `${baseSystemPrompt}\nPrevious response issue: ${boundedText(feedback, 180)}. Write a different valid sentence.` : baseSystemPrompt,
            prompt: JSON.stringify({ ...contextPayload, alreadyShown: suggestions }),
            maxTokens: 600,
            cwd: input.workspaceRoot,
            ...(dependencies.signal ? { signal: dependencies.signal } : {}),
            ...(input.provider.authenticationPreferences
              ? { authenticationPreferences: input.provider.authenticationPreferences }
              : {}),
            ...(input.provider.codexAuthFile ? { codexAuthFile: input.provider.codexAuthFile } : {}),
          });
          try {
            const candidate = parseSingleSuggestion(completion.text);
            if (suggestions.some((shown) => suggestionKey(shown) === suggestionKey(candidate))) {
              throw new Error("This goal is already shown.");
            }
            goal = candidate;
            break;
          } catch (error) {
            feedback = error instanceof Error ? error.message : String(error);
          }
        }
        if (!goal) throw new Error(`Could not generate the next ${workflow.name.toLowerCase()} goal after three attempts: ${feedback ?? "invalid response"}`);
        suggestions.push(goal);
        saveSuggestionCache(database, {
          workspaceId,
          scopeId: context.scopeId,
          profileHash: profile.hash,
          phase,
          contextRevision,
          suggestions,
          recordSuggestions: [goal],
        });
        if (input.incremental) break;
      }
      return { phase, suggestions };
    } finally {
      database.close();
    }
  } finally {
    sessions.close();
  }
}

export function selectStoredResearchGoalSuggestion(input: HostedResearchGoalSuggestionSelectionInput): void {
  const workspaceId = requiredText(input.workspaceId, "workspaceId", 256);
  const scopeId = requiredText(input.scopeId, "scopeId", 256);
  const profileHash = requiredText(input.profileHash, "profileHash", 256);
  const phase = requiredText(input.phase, "phase", 128);
  const suggestion = normalizedSuggestion(input.suggestion);
  if (!suggestion) throw new Error("Selecting a research goal suggestion requires a suggestion.");
  const database = openSuggestionDatabase(input.databasePath);
  try {
    const scope = database.prepare("SELECT 1 AS present FROM scope_versions WHERE id = ? AND workspace_id = ?")
      .get(scopeId, workspaceId) as { present?: unknown } | undefined;
    if (scope?.present !== 1) throw new Error("Research goal suggestion selection no longer matches the workspace scope.");
    const now = new Date().toISOString();
    const key = suggestionKey(suggestion);
    database.exec("BEGIN IMMEDIATE;");
    try {
      database.prepare(`
        INSERT INTO research_goal_suggestion_history(
          workspace_id, scope_version_id, profile_hash, phase, suggestion_key, suggestion_text,
          first_generated_at, last_generated_at, selected_at, generation_count, selection_count
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 0, 1)
        ON CONFLICT(workspace_id, scope_version_id, profile_hash, phase, suggestion_key) DO UPDATE SET
          suggestion_text = excluded.suggestion_text,
          selected_at = excluded.selected_at,
          selection_count = research_goal_suggestion_history.selection_count + 1
      `).run(workspaceId, scopeId, profileHash, phase, key, suggestion, now, now, now);
      const cached = readSuggestionCache(database, workspaceId, scopeId, profileHash, phase);
      if (cached) {
        const remaining = cached.suggestions.filter((candidate) => suggestionKey(candidate) !== key);
        if (remaining.length === 0) {
          database.prepare(`DELETE FROM research_goal_suggestion_cache
            WHERE workspace_id = ? AND scope_version_id = ? AND profile_hash = ? AND phase = ?`)
            .run(workspaceId, scopeId, profileHash, phase);
        } else {
          database.prepare(`UPDATE research_goal_suggestion_cache SET suggestions_json = ?, updated_at = ?
            WHERE workspace_id = ? AND scope_version_id = ? AND profile_hash = ? AND phase = ?`)
            .run(JSON.stringify(remaining), now, workspaceId, scopeId, profileHash, phase);
        }
      }
      database.exec("COMMIT;");
    } catch (error) {
      database.exec("ROLLBACK;");
      throw error;
    }
  } finally {
    database.close();
  }
}

function openSuggestionDatabase(path: string): DatabaseSync {
  const database = openResearchDatabase(path);
  database.exec("PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;");
  for (const table of ["workspaces", "scope_versions", "research_goal_suggestion_cache", "research_goal_suggestion_history"]) {
    const row = database.prepare("SELECT 1 AS present FROM sqlite_master WHERE type = 'table' AND name = ?").get(table) as { present?: unknown } | undefined;
    if (row?.present !== 1) {
      database.close();
      throw new Error(`Workspace suggestion storage is missing required table ${table}.`);
    }
  }
  return database;
}

function readSuggestionContext(database: DatabaseSync, workspaceId: string): {
  scopeId: string;
  workspaceName: string;
  scopeOwner: string;
  description: string;
  rules: string[];
  ruleRevision: string;
} {
  const scope = database.prepare(`SELECT id, workspace_name, scope_owner, description_markdown
    FROM scope_versions WHERE workspace_id = ? AND status = 'active' ORDER BY version DESC LIMIT 1`)
    .get(workspaceId) as Record<string, unknown> | undefined;
  if (!scope) throw new Error(`Workspace ${workspaceId} has no active scope version.`);
  const rules = database.prepare(`SELECT text, created_at FROM workspace_rules
    WHERE workspace_id = ? ORDER BY created_at ASC, id ASC`).all(workspaceId) as Array<Record<string, unknown>>;
  return {
    scopeId: requiredText(scope.id, "scope id", 256),
    workspaceName: requiredText(scope.workspace_name, "workspace name", 500),
    scopeOwner: optionalText(scope.scope_owner, 500) ?? "",
    description: optionalText(scope.description_markdown, 8_000) ?? "",
    rules: rules.flatMap((row) => optionalText(row.text, 1_000) ? [optionalText(row.text, 1_000)!] : []),
    ruleRevision: rules.map((row) => `${optionalText(row.created_at, 100) ?? ""}:${optionalText(row.text, 1_000) ?? ""}`).join("|"),
  };
}

function suggestionContextRevision(database: DatabaseSync, workspaceId: string, context: { ruleRevision: string }, sessions: Array<{ id: string; endedAt: string | null }>): string {
  const latest = sessions.find((session) => session.endedAt) ?? null;
  const latestSurface = suggestionTableExists(database, "app_server_surfaces")
    ? database.prepare("SELECT MAX(updated_at) AS updated_at FROM app_server_surfaces WHERE workspace_id = ?").get(workspaceId) as { updated_at: string | null }
    : null;
  const latestRoot = suggestionTableExists(database, "app_server_surface_roots")
    ? database.prepare("SELECT MAX(updated_at) AS updated_at FROM app_server_surface_roots WHERE workspace_id = ?").get(workspaceId) as { updated_at: string | null }
    : null;
  const latestFinding = suggestionTableExists(database, "app_server_research_claims")
    ? database.prepare("SELECT MAX(updated_at) AS updated_at FROM app_server_research_claims WHERE workspace_id = ?").get(workspaceId) as { updated_at: string | null }
    : null;
  return `${SUGGESTION_GENERATOR_VERSION}::${latest?.endedAt ?? "initial"}::${latest?.id ?? ""}`
    + `::rules:${createHash("sha256").update(context.ruleRevision).digest("hex")}`
    + `::surfaces:${latestSurface?.updated_at ?? ""}:${latestRoot?.updated_at ?? ""}::findings:${latestFinding?.updated_at ?? ""}`;
}

function readSuggestionCache(database: DatabaseSync, workspaceId: string, scopeId: string, profileHash: string, phase: string): {
  contextRevision: string;
  suggestions: string[];
} | null {
  const row = database.prepare(`SELECT context_revision, suggestions_json FROM research_goal_suggestion_cache
    WHERE workspace_id = ? AND scope_version_id = ? AND profile_hash = ? AND phase = ?`)
    .get(workspaceId, scopeId, profileHash, phase) as Record<string, unknown> | undefined;
  if (!row || typeof row.context_revision !== "string" || typeof row.suggestions_json !== "string") return null;
  try {
    const parsed = JSON.parse(row.suggestions_json) as unknown;
    if (!Array.isArray(parsed)) return null;
    const suggestions = parsed.map(normalizedResearchGoalSuggestion);
    return suggestions.length > 0 && suggestions.every((suggestion): suggestion is string => Boolean(suggestion))
      ? { contextRevision: row.context_revision, suggestions }
      : null;
  } catch {
    return null;
  }
}

function saveSuggestionCache(database: DatabaseSync, input: {
  workspaceId: string;
  scopeId: string;
  profileHash: string;
  phase: string;
  contextRevision: string;
  suggestions: string[];
  recordSuggestions: string[];
}): void {
  const now = new Date().toISOString();
  database.exec("BEGIN IMMEDIATE;");
  try {
    database.prepare(`INSERT INTO research_goal_suggestion_cache(
      workspace_id, scope_version_id, profile_hash, phase, context_revision, suggestions_json, generated_at, updated_at
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
    ON CONFLICT(workspace_id, scope_version_id, profile_hash, phase) DO UPDATE SET
      context_revision = excluded.context_revision, suggestions_json = excluded.suggestions_json,
      generated_at = excluded.generated_at, updated_at = excluded.updated_at`)
      .run(input.workspaceId, input.scopeId, input.profileHash, input.phase, input.contextRevision, JSON.stringify(input.suggestions), now, now);
    const record = database.prepare(`INSERT INTO research_goal_suggestion_history(
      workspace_id, scope_version_id, profile_hash, phase, suggestion_key, suggestion_text,
      first_generated_at, last_generated_at, selected_at, generation_count, selection_count
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL, 1, 0)
    ON CONFLICT(workspace_id, scope_version_id, profile_hash, phase, suggestion_key) DO UPDATE SET
      suggestion_text = excluded.suggestion_text, last_generated_at = excluded.last_generated_at,
      generation_count = research_goal_suggestion_history.generation_count + 1`);
    for (const suggestion of input.recordSuggestions) {
      record.run(input.workspaceId, input.scopeId, input.profileHash, input.phase, suggestionKey(suggestion), suggestion, now, now);
    }
    database.exec("COMMIT;");
  } catch (error) {
    database.exec("ROLLBACK;");
    throw error;
  }
}

function readSuggestionSurfaces(database: DatabaseSync, workspaceId: string): Array<Record<string, unknown>> {
  const roots = suggestionTableExists(database, "app_server_surface_roots")
    ? database.prepare(`SELECT resource_id AS id, resource_id AS name, '' AS description, foundation_json
        FROM app_server_surface_roots WHERE workspace_id = ? ORDER BY updated_at DESC LIMIT 4`).all(workspaceId) as Array<Record<string, unknown>>
    : [];
  const children = suggestionTableExists(database, "app_server_surfaces")
    ? database.prepare(`SELECT id, name, description, foundation_json FROM app_server_surfaces
        WHERE workspace_id = ? ORDER BY updated_at DESC LIMIT 12`).all(workspaceId) as Array<Record<string, unknown>>
    : [];
  const rows = [...roots, ...children].slice(0, 12);
  return rows.map((row) => {
    let foundation: Record<string, unknown> | null = null;
    try {
      const parsed: unknown = JSON.parse(String(row.foundation_json ?? "null"));
      foundation = isRecord(parsed) ? parsed : null;
    } catch { /* Unknown foundation. */ }
    const threatModel = isRecord(foundation?.threatModel) ? foundation.threatModel : null;
    return {
      id: String(row.id),
      name: boundedText(String(row.name), 100),
      description: boundedText(String(row.description ?? ""), 180),
      ...(threatModel ? { attackerControl: boundedText(String(threatModel.attackerControl ?? ""), 180),
        trustBoundaries: boundedText(String(threatModel.trustBoundaries ?? ""), 180),
        securityObjectives: boundedText(threatModelSummary(threatModel.securityObjectives), 180),
        invariants: boundedText(String(threatModel.invariants ?? ""), 180),
        sinks: boundedText(String(threatModel.sinks ?? ""), 180),
        plausibleImpacts: boundedText(threatModelSummary(threatModel.risks ?? threatModel.riskImpact ?? threatModel.plausibleImpacts), 180) } : {}),
    };
  });
}

function threatModelSummary(value: unknown): string {
  return Array.isArray(value) ? value.filter((item): item is string => typeof item === "string").join("; ")
    : typeof value === "string" ? value : "";
}

function readSuggestionFindings(database: DatabaseSync, workspaceId: string): Array<Record<string, string>> {
  if (!suggestionTableExists(database, "app_server_research_claims")) return [];
  const rows = database.prepare(`SELECT id, title, summary, status FROM app_server_research_claims
    WHERE workspace_id = ? AND classification LIKE 'security.%' AND status != 'rejected'
    ORDER BY updated_at DESC LIMIT 8`).all(workspaceId) as Array<Record<string, unknown>>;
  return rows.map((row) => ({ id: String(row.id), title: boundedText(String(row.title), 120),
    summary: boundedText(String(row.summary), 240), status: String(row.status) }));
}

function suggestionTableExists(database: DatabaseSync, name: string): boolean {
  return Boolean(database.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?").get(name));
}

function parseSingleSuggestion(output: string): string {
  let value = output.trim().replace(/^```(?:json|text)?\s*/iu, "").replace(/\s*```$/u, "").trim();
  if (value.startsWith("{")) {
    let parsed: unknown;
    try { parsed = JSON.parse(value) as unknown; } catch { throw new Error("Return a plain sentence or a JSON object with one goal field."); }
    value = isRecord(parsed) && typeof parsed.goal === "string" ? parsed.goal : "";
  }
  value = value.replace(/^(?:goal\s*:\s*|[-*]\s*|\d+[.)]\s*)/iu, "").replace(/^[']|[']$/gu, "").replace(/^["]|["]$/gu, "");
  const normalized = normalizedResearchGoalSuggestion(value);
  if (!normalized || normalized.length < 20 || normalized.length > 500) {
    throw new Error("Return one security research goal between 20 and 500 characters.");
  }
  return /[.!?]$/u.test(normalized) ? normalized : `${normalized}.`;
}

function suggestionKey(value: string): string {
  return createHash("sha256").update(normalizedSuggestion(value)?.toLocaleLowerCase() ?? "").digest("hex");
}

function normalizedSuggestion(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const normalized = value.replace(/\s+/gu, " ").trim();
  return normalized && normalized.length <= 2_000 ? normalized : null;
}

const LEADING_SHELL_SAFETY_CLAUSE = /^(?:after|using|with)\s+(?:the\s+)?(?:auto[- ]review(?:ed)?|manual approval|danger mode)\b[^,]*,\s*/iu;
const SHELL_SAFETY_REFERENCE = /\b(?:auto[- ]review(?:ed)?|manual approval|danger mode|shell[- ]safety mode)\b/iu;

function normalizedResearchGoalSuggestion(value: unknown): string | null {
  const normalized = normalizedSuggestion(value);
  if (!normalized) return null;
  const objective = normalized.replace(LEADING_SHELL_SAFETY_CLAUSE, "").trim();
  return objective && !SHELL_SAFETY_REFERENCE.test(objective) ? objective : null;
}

function requiredText(value: unknown, name: string, maxLength: number): string {
  const text = optionalText(value, maxLength);
  if (!text) throw new Error(`${name} is required.`);
  return text;
}

function optionalText(value: unknown, maxLength: number): string | null {
  if (typeof value !== "string") return null;
  const text = value.trim();
  return text ? text.slice(0, maxLength) : null;
}

function boundedText(value: string, maxLength: number): string {
  return value.length <= maxLength ? value : `${value.slice(0, Math.max(0, maxLength - 1))}…`;
}

function nonNegativeInteger(value: unknown): number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : 0;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
