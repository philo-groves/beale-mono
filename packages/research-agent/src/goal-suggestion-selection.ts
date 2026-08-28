import type { ResearchProfileWorkflow } from "./research-profile.js";

const MAX_CANDIDATE_POOL_SIZE = 12;
const MAX_GROUNDING_REFS = 4;
const PRIOR_DUPLICATE_THRESHOLD = 0.49;
const CANDIDATE_DUPLICATE_THRESHOLD = 0.58;
const MINIMUM_SEMANTIC_TOKENS = 8;
const LONGSHOT_CLOSE_ENDED_PATTERN = /\bwhether\b|^(?:verify|confirm)\b|^(?:determine|establish|test|check|assess|resolve)\s+if\b/iu;
const LONGSHOT_HIGH_UPSIDE_PATTERN = /\b(?:critical|high[- ]impact|high[- ]severity|systemic|platform[- ]wide|cross[- ]tenant|remote code execution|system code execution|arbitrary code execution|sandbox escape|privilege escalation|supply[- ]chain|account takeover|major breakthrough|foundational|unifying|general theorem|new framework|broad problem class)\b/iu;
const GENERIC_SURVEY_PATTERN = /\b(?:broad\s+attack\s+surface|potential\s+(?:security\s+)?(?:issues|vulnerabilities|flaws)|vulnerability\s+(?:classes|families)|security\s+weaknesses)\b/iu;

export interface ResearchGoalSuggestionGrounding {
  id: string;
  kind: "resource" | "session" | "memory" | "claim" | "runbook" | "report" | "track";
  title: string;
  summary: string;
  status?: string;
}

export interface ResearchGoalSuggestionCandidate {
  goal: string;
  groundingRefs: readonly string[];
  rationale: string;
  noveltyAxis: string;
}

export interface ResearchGoalSuggestionCandidateSelection {
  selected: readonly ResearchGoalSuggestionCandidate[];
  validCandidates: readonly ResearchGoalSuggestionCandidate[];
  rejectedInvalidCandidates: number;
  rejectedPriorSuggestions: number;
  rejectedSemanticDuplicates: number;
}

export function researchGoalSuggestionCandidateCount(suggestionCount: number): number {
  return Math.min(
    MAX_CANDIDATE_POOL_SIZE,
    Math.max(suggestionCount + 2, suggestionCount * 2),
  );
}

export function parseAndSelectResearchGoalSuggestionCandidates(input: {
  output: string;
  workflow: ResearchProfileWorkflow;
  suggestionCount: number;
  candidateCount: number;
  grounding: readonly ResearchGoalSuggestionGrounding[];
  priorSuggestions: readonly string[];
  previousResearchTexts: readonly string[];
}): ResearchGoalSuggestionCandidateSelection {
  const record = parseOutputRecord(input.output);
  const rawCandidates = record.candidates;
  if (!Array.isArray(rawCandidates) || rawCandidates.length !== input.candidateCount) {
    throw new Error(
      `${input.workflow.name} suggestions must contain exactly ${input.candidateCount} candidates.`,
    );
  }
  const grounding = new Map(input.grounding.map((item) => [item.id, item]));
  const validCandidates: ResearchGoalSuggestionCandidate[] = [];
  const invalidErrors: Error[] = [];
  let rejectedPriorSuggestions = 0;
  rawCandidates.forEach((raw, index) => {
    try {
      const candidate = parseCandidate(raw, index, input.workflow, grounding);
      const priorSimilarity = maximumSimilarity(candidate.goal, input.priorSuggestions);
      if (priorSimilarity >= PRIOR_DUPLICATE_THRESHOLD) {
        rejectedPriorSuggestions += 1;
      } else {
        validCandidates.push(candidate);
      }
    } catch (error) {
      invalidErrors.push(error instanceof Error ? error : new Error(String(error)));
    }
  });
  if (validCandidates.length < input.suggestionCount) {
    if (rejectedPriorSuggestions > 0) {
      throw new Error(`${input.workflow.name} suggestions repeated or closely paraphrased prior suggestions.`);
    }
    throw invalidErrors[0]
      ?? new Error(`${input.workflow.name} suggestions did not contain enough grounded candidates.`);
  }

  const ranked = validCandidates.map((candidate, index) => ({
    candidate,
    index,
    score: scoreCandidate(candidate, grounding, input.previousResearchTexts, input.priorSuggestions),
  })).sort((left, right) => right.score - left.score || left.index - right.index);
  const selected: ResearchGoalSuggestionCandidate[] = [];
  const selectedAxes: string[] = [];
  let rejectedSemanticDuplicates = 0;
  for (const item of ranked) {
    const goalSimilarity = maximumSimilarity(
      item.candidate.goal,
      selected.map((candidate) => candidate.goal),
    );
    const axisSimilarity = maximumSimilarity(item.candidate.noveltyAxis, selectedAxes);
    if (goalSimilarity >= CANDIDATE_DUPLICATE_THRESHOLD || axisSimilarity >= CANDIDATE_DUPLICATE_THRESHOLD) {
      rejectedSemanticDuplicates += 1;
      continue;
    }
    selected.push(item.candidate);
    selectedAxes.push(item.candidate.noveltyAxis);
    if (selected.length === input.suggestionCount) break;
  }
  if (selected.length !== input.suggestionCount) {
    throw new Error(
      `${input.workflow.name} suggestions did not contain ${input.suggestionCount} materially distinct goals and novelty axes.`,
    );
  }
  return {
    selected,
    validCandidates,
    rejectedInvalidCandidates: invalidErrors.length,
    rejectedPriorSuggestions,
    rejectedSemanticDuplicates,
  };
}

export function researchGoalSemanticSimilarity(left: string, right: string): number {
  const leftTokens = semanticTokens(left);
  const rightTokens = semanticTokens(right);
  if (leftTokens.size === 0 || rightTokens.size === 0) return 0;
  let intersection = 0;
  for (const token of leftTokens) {
    if (rightTokens.has(token)) intersection += 1;
  }
  const union = leftTokens.size + rightTokens.size - intersection;
  const jaccard = union > 0 ? intersection / union : 0;
  const overlap = intersection / Math.min(leftTokens.size, rightTokens.size);
  return Math.max(jaccard, overlap * 0.9);
}

function parseCandidate(
  value: unknown,
  index: number,
  workflow: ResearchProfileWorkflow,
  grounding: ReadonlyMap<string, ResearchGoalSuggestionGrounding>,
): ResearchGoalSuggestionCandidate {
  if (!isRecord(value)) {
    throw new Error(`${workflow.name} suggestion candidate ${index + 1} must be an object.`);
  }
  const goal = normalizeGoal(value.goal);
  validateWorkflowGoal(goal, workflow);
  if (GENERIC_SURVEY_PATTERN.test(goal)) {
    throw new Error(`${workflow.name} suggestion candidate ${index + 1} is a generic vulnerability survey.`);
  }
  const rationale = boundedString(value.rationale, 12, 500, "rationale");
  const noveltyAxis = boundedString(value.noveltyAxis, 3, 120, "noveltyAxis");
  if (!Array.isArray(value.groundingRefs)
    || value.groundingRefs.length < 1
    || value.groundingRefs.length > MAX_GROUNDING_REFS) {
    throw new Error(
      `${workflow.name} suggestion candidate ${index + 1} must cite one to ${MAX_GROUNDING_REFS} grounding references.`,
    );
  }
  const groundingRefs = value.groundingRefs.map((reference) => {
    if (typeof reference !== "string" || !reference.trim()) {
      throw new Error(`${workflow.name} suggestion candidate ${index + 1} has an invalid grounding reference.`);
    }
    return reference.trim();
  });
  if (new Set(groundingRefs).size !== groundingRefs.length) {
    throw new Error(`${workflow.name} suggestion candidate ${index + 1} repeats a grounding reference.`);
  }
  const unknownReference = groundingRefs.find((reference) => !grounding.has(reference));
  if (unknownReference) {
    throw new Error(`${workflow.name} suggestion candidate ${index + 1} cites unknown grounding reference ${unknownReference}.`);
  }
  const referencedText = groundingRefs.map((reference) => {
    const item = grounding.get(reference)!;
    return `${item.title} ${item.summary}`;
  });
  if (maximumSimilarity(goal, referencedText) < 0.12) {
    throw new Error(`${workflow.name} suggestion candidate ${index + 1} does not name a concrete cited anchor.`);
  }
  if (semanticTokens(goal).size < MINIMUM_SEMANTIC_TOKENS) {
    throw new Error(`${workflow.name} suggestion candidate ${index + 1} is too generic to be actionable.`);
  }
  return { goal, groundingRefs, rationale, noveltyAxis };
}

function validateWorkflowGoal(goal: string, workflow: ResearchProfileWorkflow): void {
  if (workflow.id !== "longshot") return;
  if (LONGSHOT_CLOSE_ENDED_PATTERN.test(goal)) {
    throw new Error("Longshot suggestions must open a broad research direction rather than ask a binary question.");
  }
  if (!LONGSHOT_HIGH_UPSIDE_PATTERN.test(goal)) {
    throw new Error("Longshot suggestions must state a systemic-impact or major-breakthrough ceiling.");
  }
}

function scoreCandidate(
  candidate: ResearchGoalSuggestionCandidate,
  grounding: ReadonlyMap<string, ResearchGoalSuggestionGrounding>,
  previousResearchTexts: readonly string[],
  priorSuggestions: readonly string[],
): number {
  const referencedText = candidate.groundingRefs.map((reference) => {
    const item = grounding.get(reference)!;
    return `${item.title} ${item.summary}`;
  });
  const groundingSimilarity = maximumSimilarity(candidate.goal, referencedText);
  const previousSimilarity = maximumSimilarity(candidate.goal, previousResearchTexts);
  const priorSimilarity = maximumSimilarity(candidate.goal, priorSuggestions);
  const concreteTokens = Math.min(18, semanticTokens(candidate.goal).size);
  const noveltyTokens = Math.min(8, semanticTokens(candidate.noveltyAxis).size);
  return candidate.groundingRefs.length * 5
    + groundingSimilarity * 30
    + concreteTokens
    + noveltyTokens
    - previousSimilarity * 24
    - priorSimilarity * 38;
}

function maximumSimilarity(value: string, candidates: readonly string[]): number {
  return candidates.reduce(
    (maximum, candidate) => Math.max(maximum, researchGoalSemanticSimilarity(value, candidate)),
    0,
  );
}

function parseOutputRecord(output: string): Record<string, unknown> {
  const trimmed = output.trim();
  const first = trimmed.indexOf("{");
  const last = trimmed.lastIndexOf("}");
  if (first < 0 || last <= first) throw new Error("Research goal suggestions must be a JSON object.");
  try {
    const parsed = JSON.parse(trimmed.slice(first, last + 1)) as unknown;
    if (!isRecord(parsed)) throw new Error("not an object");
    return parsed;
  } catch {
    throw new Error("Research goal suggestions must be a JSON object.");
  }
}

function normalizeGoal(value: unknown): string {
  if (typeof value !== "string") throw new Error("Each research goal suggestion must be a string.");
  let goal = value.replace(/\s+/gu, " ").trim().replace(/^["']|["']$/gu, "");
  goal = goal.replace(LEADING_SHELL_SAFETY_CLAUSE, "").trim();
  if (SHELL_SAFETY_REFERENCE.test(goal)) {
    throw new Error("Research goal suggestions must not mention launch-time safety controls.");
  }
  if (!/[.!?]$/u.test(goal)) goal = `${goal}.`;
  if (goal.length < 40 || goal.length > 360) {
    throw new Error("Each research goal suggestion must be between 40 and 360 characters.");
  }
  if (/[.!?](?:["')\]]*)\s+\S/u.test(goal.slice(0, -1))) {
    throw new Error("Each research goal suggestion must be exactly one sentence.");
  }
  return goal;
}

function boundedString(value: unknown, minimum: number, maximum: number, label: string): string {
  if (typeof value !== "string") throw new Error(`Each research goal ${label} must be a string.`);
  const text = value.replace(/\s+/gu, " ").trim();
  if (text.length < minimum || text.length > maximum) {
    throw new Error(`Each research goal ${label} must be between ${minimum} and ${maximum} characters.`);
  }
  return text;
}

const STOP_WORDS = new Set([
  "a", "an", "and", "as", "at", "be", "by", "for", "from", "in", "into", "its", "of", "on", "or",
  "the", "their", "this", "to", "using", "with", "without", "research", "explore", "examine", "investigate",
  "analyze", "audit", "review", "trace", "test", "target", "targeting", "seek", "seeking", "across",
]);

const SEMANTIC_ALIASES: Readonly<Record<string, string>> = {
  auth: "authorization",
  authentication: "authorization",
  authorized: "authorization",
  access: "authorization",
  lifetime: "lifecycle",
  teardown: "lifecycle",
  reconnect: "lifecycle",
  reattach: "lifecycle",
  overflow: "integer-overflow",
  traversal: "path-traversal",
  directory: "path-traversal",
  filesystem: "path-traversal",
  poc: "proof",
  reproduction: "proof",
  reproduce: "proof",
  exploit: "chain",
  reportable: "chain",
  privileged: "privilege",
};

function semanticTokens(value: string): Set<string> {
  const tokens = value.toLocaleLowerCase().match(/[a-z0-9]+(?:-[a-z0-9]+)*/gu) ?? [];
  return new Set(tokens.flatMap((raw) => {
    if (STOP_WORDS.has(raw)) return [];
    const singular = raw.length > 5 && raw.endsWith("s") ? raw.slice(0, -1) : raw;
    return [SEMANTIC_ALIASES[singular] ?? singular];
  }));
}

const LEADING_SHELL_SAFETY_CLAUSE = /^(?:after|using|with)\s+(?:the\s+)?(?:auto[- ]review(?:ed)?|manual approval|danger mode)\b[^,]*,\s*/iu;
const SHELL_SAFETY_REFERENCE = /\b(?:auto[- ]review(?:ed)?|manual approval|danger mode|shell[- ]safety mode)\b/iu;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
