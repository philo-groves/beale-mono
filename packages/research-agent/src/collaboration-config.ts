import type { ResearchCollaborationConfig } from "./types.js";

const SUBAGENT_MODES = new Set(["simple", "advanced"]);
const SUBAGENT_ROLES = new Set(["discoverer", "prover", "reviewer", "reporter"]);
const EFFORTS = new Set(["minimal", "low", "medium", "high", "xhigh", "max"]);

export function decodeResearchCollaborationConfig(value: unknown): ResearchCollaborationConfig {
  if (!isRecord(value)) throw new Error("Collaboration config must be a JSON object.");
  const subagentMode = value.subagentMode === undefined
    ? "simple"
    : requiredEnum(value.subagentMode, SUBAGENT_MODES, "subagentMode") as ResearchCollaborationConfig["subagentMode"];
  if (!Array.isArray(value.providers)) throw new Error("Collaboration config providers must be an array.");
  const seen = new Set<string>();
  const providers = value.providers.map((entry, index) => {
    if (!isRecord(entry)) throw new Error(`Collaboration provider ${index + 1} must be an object.`);
    const provider = requiredString(entry.provider, `providers[${index}].provider`);
    const model = requiredString(entry.model, `providers[${index}].model`);
    const key = providerModelKey(provider, model);
    if (seen.has(key)) throw new Error(`Collaboration provider/model ${provider}/${model} is configured more than once.`);
    seen.add(key);
    const reasoningEffort = requiredEnum(entry.reasoningEffort, EFFORTS, `providers[${index}].reasoningEffort`);
    const roles = decodeRoles(entry.roles, entry.role, index);
    return { provider, model, reasoningEffort, enabled: entry.enabled !== false, roles };
  });
  boundedInteger(value.maxConcurrentRooms, 1, 5, "maxConcurrentRooms");
  boundedInteger(value.maxMembersPerRoom, 2, 5, "maxMembersPerRoom");
  return {
    mode: "always",
    subagentMode,
    intensity: "balanced",
    providers,
    independentFirstPass: false,
    peerChallengeRounds: 0,
    maxConcurrentRooms: 2,
    maxMembersPerRoom: 3,
  };
}

function decodeRoles(
  value: unknown,
  legacyRole: unknown,
  providerIndex: number,
): ResearchCollaborationConfig["providers"][number]["roles"] {
  let candidates: unknown[];
  if (value === undefined) candidates = legacyRole === undefined ? [...SUBAGENT_ROLES] : [legacyRole];
  else if (Array.isArray(value)) candidates = value;
  else throw new Error(`Collaboration config providers[${providerIndex}].roles must be an array.`);
  const roles = [...new Set(candidates.map((role, roleIndex) => requiredEnum(
    role,
    SUBAGENT_ROLES,
    `providers[${providerIndex}].roles[${roleIndex}]`,
  ) as ResearchCollaborationConfig["providers"][number]["roles"][number]))];
  if (roles.length === 0) throw new Error(`Collaboration config providers[${providerIndex}].roles must contain at least one role.`);
  return roles;
}

function providerModelKey(provider: string, model: string): string {
  return `${provider}\u0000${model}`;
}

function requiredString(value: unknown, field: string): string {
  if (typeof value !== "string" || !value.trim()) throw new Error(`Collaboration config ${field} must be a non-empty string.`);
  return value.trim();
}

function requiredEnum(value: unknown, values: ReadonlySet<string>, field: string): string {
  const normalized = requiredString(value, field);
  if (!values.has(normalized)) throw new Error(`Unsupported collaboration config ${field}: ${normalized}.`);
  return normalized;
}

function boundedInteger(value: unknown, minimum: number, maximum: number, field: string): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < minimum || value > maximum) {
    throw new Error(`Collaboration config ${field} must be an integer from ${minimum} to ${maximum}.`);
  }
  return value;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}
