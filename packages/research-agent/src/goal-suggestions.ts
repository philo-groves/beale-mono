import { createHash } from "node:crypto";
import { DatabaseSync } from "node:sqlite";
import { completeAuxiliaryText, type CompleteAuxiliaryTextOptions } from "./auxiliary-completion.js";
import { resolveAuxiliaryModelRoute, type AuxiliaryModelEffort } from "./auxiliary-model-job.js";
import { discoverResearchAgentInstructions } from "./agent-instructions.js";
import {
  parseAndSelectResearchGoalSuggestionCandidates,
  researchGoalSuggestionCandidateCount,
  type ResearchGoalSuggestionGrounding,
} from "./goal-suggestion-selection.js";
import { getHoneycrispMemorySummary } from "./memory-summary.js";
import type { MemorySummary as HoneycrispMemorySummary } from "./knowledge-types.js";
import { providerSemanticsDescriptor } from "./provider-semantics.js";
import { HoneycrispSessionStore } from "./session-store.js";
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
const MAX_HISTORY = 256;
const MAX_MODEL_HISTORY = 96;
const SUGGESTION_GENERATOR_VERSION = "goal-suggestions-v2";

interface StoredResearchGoalSuggestionHistory {
  suggestion: string;
  lastGeneratedAt: string;
  selectedAt: string | null;
  generationCount: number;
  selectionCount: number;
}

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

  const sessions = new HoneycrispSessionStore({ databasePath: input.databasePath });
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
      const sessionSummaries = sessions.listSummaries(workspaceId, 12)
        .filter((session) => session.status !== "active")
        .map((session) => ({
          id: session.id,
          title: boundedText(session.title, 300),
          prompt: boundedText(session.prompt, 2_000),
          summary: boundedText(session.summary, 2_000),
          status: session.status,
          workflowId: session.workflowId,
          endedAt: session.endedAt,
          disposition: session.finalDisposition
            ? {
                outcome: session.finalDisposition.outcome,
                summary: boundedText(session.finalDisposition.summary, 2_000),
                externalStateRequired: session.finalDisposition.externalStateRequired,
              }
            : null,
        }));
      const contextRevision = suggestionContextRevision(context, sessionSummaries);
      if (input.refresh !== true) {
        const cached = readSuggestionCache(database, workspaceId, context.scopeId, profile.hash, phase);
        if (cached?.contextRevision.startsWith(`${SUGGESTION_GENERATOR_VERSION}::`)) {
          return {
            phase,
            suggestions: cached.suggestions,
            ...(cached.contextRevision === contextRevision ? {} : { cacheStatus: "stale" as const }),
          };
        }
      }

      const binding = resolveStoredResearchWorkspaceBinding({
        workspaceRoot: input.workspaceRoot,
        databasePath: input.databasePath,
        researchProfileId: input.researchProfileId,
      });
      const instructions = discoverResearchAgentInstructions({ workingDirectory: input.workspaceRoot });
      const priorSuggestionHistory = readSuggestionHistory(database, workspaceId, phase);
      const priorSuggestions = priorSuggestionHistory.map((entry) => entry.suggestion);
      const memory = input.memoryEnabled && profile.profile.capabilities.memoryEnabled
        ? getHoneycrispMemorySummary({
            databasePath: input.databasePath,
            artifactDirectoryPath: input.artifactDirectoryPath,
            workspaceId,
            subjectId: binding.memoryContext.subjectId,
            assetIds: binding.authorizedAssetIds,
          })
        : null;
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
      const grounding = createSuggestionGrounding(binding, sessionSummaries, memory, workflow.id);
      const candidateCount = researchGoalSuggestionCandidateCount(workflow.goalSuggestionCount);
      const payload = JSON.stringify({
        task: "suggest_next_research_goals",
        workspace: {
          id: workspaceId,
          name: context.workspaceName,
          scopeOwner: context.scopeOwner,
          description: boundedText(context.description, 4_000),
          rules: context.rules,
          researchSubject: binding.memoryContext.subjectName,
          resources: binding.resources.slice(0, 80).map((resource) => ({
            id: resource.id,
            direction: resource.direction,
            kind: resource.kind,
            locator: boundedText(resource.locator, 500),
            ...(resource.name ? { name: boundedText(resource.name, 300) } : {}),
            ...(resource.instruction ? { instruction: boundedText(resource.instruction, 500) } : {}),
          })),
          projectNotes: binding.projectNotes,
          agentInstructions: instructions.content ? boundedText(instructions.content, 12_000) : null,
        },
        researchProfile: {
          id: profile.profile.id,
          version: profile.profile.version,
          hash: profile.hash,
          lane: { id: workflow.id, name: workflow.name, description: workflow.description },
        },
        previousResearch: sessionSummaries,
        grounding,
        priorSuggestions: priorSuggestionHistory.slice(0, MAX_MODEL_HISTORY),
        suggestionCount: workflow.goalSuggestionCount,
        candidateCount,
      });
      const baseSystemPrompt = [
        `You are ${profile.profile.agent.role}.`,
        ...profile.profile.agent.posture,
        ...profile.profile.agent.style,
        `Generate exactly ${candidateCount} candidates so the host can select the strongest ${workflow.goalSuggestionCount} next research goals for the ${workflow.name} lane.`,
        ...workflow.goalSuggestionInstructions,
        "Ground every candidate in one to four exact IDs from the supplied grounding catalog, and name at least one cited component, artifact, claim, session, or investigation directly in the goal.",
        "Do not repeat, closely paraphrase, or merely rotate the opening verb of a prior suggestion. Treat selected prior suggestions as researcher direction to advance with a different boundary, state transition, evidence gap, or composition opportunity.",
        "Avoid bland category surveys. Prefer named functions, protocol fields, object identities, lifecycle transitions, build-specific behavior, concrete consumers, or unresolved proof obligations supported by the payload.",
        "Make every noveltyAxis concise and materially different from every other candidate; candidates that share the same primary mechanism or proof question are duplicates even when they name different impact ceilings.",
        "Do not propose completed work. A useful goal should make clear what discriminating evidence would change the current research state without prescribing commands or an ordered procedure.",
        "These suggestions are shown before the researcher chooses launch-time controls. Auto-Review is only a default, not an active setting at this stage.",
        "Describe research objectives without mentioning or depending on Auto-Review, Manual Approval, Danger Mode, shell-safety mode, provider selection, or other launch-time controls. Honeycrisp applies relevant execution policy after launch.",
        "Return JSON only as {\"candidates\":[{\"goal\":\"one self-contained action sentence\",\"groundingRefs\":[\"exact-id\"],\"rationale\":\"why this follows from current evidence\",\"noveltyAxis\":\"specific new mechanism or proof question\"}] }.",
      ].join("\n");
      let suggestions: string[] | undefined;
      let validationFeedback: string | undefined;
      for (let attempt = 0; attempt < 2; attempt += 1) {
        const systemPrompt = attempt === 0
          ? baseSystemPrompt
          : `${baseSystemPrompt}\nThe previous response was rejected by the host validator: ${boundedText(validationFeedback ?? "invalid candidate output", 300)} Return a corrected full candidate pool.`;
        const completion = await (dependencies.completeText ?? completeAuxiliaryText)({
          provider: route.provider,
          model: route.model,
          effort: route.effort as AuxiliaryModelEffort,
          systemPrompt,
          prompt: payload,
          maxTokens: 8_192,
          cwd: input.workspaceRoot,
          ...(dependencies.signal ? { signal: dependencies.signal } : {}),
          ...(input.provider.authenticationPreferences
            ? { authenticationPreferences: input.provider.authenticationPreferences }
            : {}),
          ...(input.provider.codexAuthFile ? { codexAuthFile: input.provider.codexAuthFile } : {}),
        });
        try {
          const selection = parseAndSelectResearchGoalSuggestionCandidates({
            output: completion.text,
            workflow,
            suggestionCount: workflow.goalSuggestionCount,
            candidateCount,
            grounding,
            priorSuggestions,
            previousResearchTexts: sessionSummaries.flatMap((session) => [
              session.prompt,
              session.summary,
              session.disposition?.summary ?? "",
            ]).filter(Boolean),
          });
          suggestions = selection.selected.map((candidate) => candidate.goal);
          break;
        } catch (error) {
          validationFeedback = error instanceof Error ? error.message : String(error);
          if (attempt > 0) throw error;
        }
      }
      if (!suggestions) throw new Error("Research goal suggestions did not satisfy the host novelty contract.");
      saveSuggestionCache(database, {
        workspaceId,
        scopeId: context.scopeId,
        profileHash: profile.hash,
        phase,
        contextRevision,
        suggestions,
      });
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
  const database = new DatabaseSync(path);
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

function suggestionContextRevision(context: { ruleRevision: string }, sessions: Array<{ id: string; endedAt: string | null }>): string {
  const latest = sessions.find((session) => session.endedAt) ?? null;
  return `${SUGGESTION_GENERATOR_VERSION}::${latest?.endedAt ?? "initial"}::${latest?.id ?? ""}::rules:${createHash("sha256").update(context.ruleRevision).digest("hex")}`;
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

function readSuggestionHistory(
  database: DatabaseSync,
  workspaceId: string,
  phase: string,
): StoredResearchGoalSuggestionHistory[] {
  const rows = database.prepare(`SELECT suggestion_text, last_generated_at, selected_at,
      generation_count, selection_count FROM research_goal_suggestion_history
    WHERE workspace_id = ? AND phase = ?
    ORDER BY COALESCE(selected_at, last_generated_at) DESC, suggestion_key ASC LIMIT ?`)
    .all(workspaceId, phase, MAX_HISTORY * 4) as Array<Record<string, unknown>>;
  const history = new Map<string, StoredResearchGoalSuggestionHistory>();
  for (const row of rows) {
    const suggestion = normalizedResearchGoalSuggestion(row.suggestion_text);
    if (!suggestion) continue;
    const key = suggestionKey(suggestion);
    const existing = history.get(key);
    const selectedAt = optionalText(row.selected_at, 100);
    if (existing) {
      existing.generationCount += nonNegativeInteger(row.generation_count);
      existing.selectionCount += nonNegativeInteger(row.selection_count);
      if (!existing.selectedAt && selectedAt) existing.selectedAt = selectedAt;
      continue;
    }
    history.set(key, {
      suggestion,
      lastGeneratedAt: optionalText(row.last_generated_at, 100) ?? "",
      selectedAt,
      generationCount: nonNegativeInteger(row.generation_count),
      selectionCount: nonNegativeInteger(row.selection_count),
    });
  }
  return [...history.values()].slice(0, MAX_HISTORY);
}

function saveSuggestionCache(database: DatabaseSync, input: {
  workspaceId: string;
  scopeId: string;
  profileHash: string;
  phase: string;
  contextRevision: string;
  suggestions: string[];
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
    for (const suggestion of input.suggestions) {
      record.run(input.workspaceId, input.scopeId, input.profileHash, input.phase, suggestionKey(suggestion), suggestion, now, now);
    }
    database.exec("COMMIT;");
  } catch (error) {
    database.exec("ROLLBACK;");
    throw error;
  }
}

function createSuggestionGrounding(
  binding: StoredResearchWorkspaceBinding,
  sessions: readonly {
    id: string;
    title: string;
    prompt: string;
    summary: string;
    status: string;
    disposition: { summary: string } | null;
  }[],
  memory: HoneycrispMemorySummary | null,
  workflowId: string,
): ResearchGoalSuggestionGrounding[] {
  const limits = workflowId === "chaining" || workflowId === "reporting"
    ? { resources: 40, sessions: 10, memories: 12, findings: 24, leads: 6, runbooks: 8, reports: 8, tracks: 12 }
    : workflowId === "discovery"
      ? { resources: 40, sessions: 10, memories: 24, findings: 12, leads: 8, runbooks: 4, reports: 4, tracks: 8 }
      : { resources: 40, sessions: 10, memories: 20, findings: 16, leads: 8, runbooks: 5, reports: 5, tracks: 10 };
  const grounding = new Map<string, ResearchGoalSuggestionGrounding>();
  const add = (
    kind: ResearchGoalSuggestionGrounding["kind"],
    rawId: string,
    title: string,
    summary: string,
    status?: string,
  ) => {
    const id = `${kind}:${rawId}`;
    if (!rawId || grounding.has(id)) return;
    grounding.set(id, {
      id,
      kind,
      title: boundedText(title || rawId, 300),
      summary: boundedText(summary, 1_000),
      ...(status ? { status: boundedText(status, 100) } : {}),
    });
  };
  for (const resource of binding.resources.filter((resource) => resource.direction === "in_scope").slice(0, limits.resources)) {
    add(
      "resource",
      resource.id,
      resource.name ?? resource.locator,
      [resource.direction, resource.kind, resource.instruction ?? ""].filter(Boolean).join("; "),
      resource.direction,
    );
  }
  for (const session of sessions.slice(0, limits.sessions)) {
    add(
      "session",
      session.id,
      session.title || session.prompt,
      session.disposition?.summary || session.summary || session.prompt,
      session.status,
    );
  }
  for (const node of memory?.nodes.slice(0, limits.memories) ?? []) {
    add("memory", node.id, node.title, node.summary, node.status);
  }
  for (const claim of [
    ...(memory?.findings.slice(0, limits.findings) ?? []),
    ...(memory?.leads.slice(0, limits.leads) ?? []),
  ]) {
    add(
      "claim",
      claim.id,
      claim.title,
      [claim.summary, claim.impact].filter(Boolean).join(" "),
      `${claim.projection}:${claim.status}`,
    );
  }
  for (const runbook of memory?.runbooks.slice(0, limits.runbooks) ?? []) {
    add("runbook", runbook.id, runbook.title, runbook.purpose, runbook.execution.latest?.status ?? "not_run");
  }
  for (const report of memory?.reports.slice(0, limits.reports) ?? []) {
    add("report", report.id, report.title, report.summary, `${report.status}:${report.triageStatus}`);
  }
  for (const track of memory?.campaign.tracks?.slice(0, limits.tracks) ?? []) {
    add("track", track.id, track.title, track.objective, `${track.status}:${track.stage}`);
  }
  return [...grounding.values()];
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
