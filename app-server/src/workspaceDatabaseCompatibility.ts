import { normalizeResearchProfile } from '@beale/research-agent';
export { selectRunTarget } from './workspaceRunTarget.js';

const RESEARCH_KIT_IDS = new Set(['general', 'hackerone', 'apple-security-bounty', 'google-oss-vrp', 'msrc']);
const PROFILE_SOURCES = new Set(['explicit', 'workspace-default', 'bundled-default']);
const SUBAGENT_MODES = new Set(['simple', 'advanced']);
const PROVIDERS = new Set(['openai-codex', 'anthropic', 'xai', 'zai', 'openrouter']);
const EFFORTS = new Set(['minimal', 'low', 'medium', 'high', 'xhigh', 'max']);
const ROLES = ['discoverer', 'prover', 'reviewer', 'reporter'] as const;

export function isResearchKitId(value: unknown): boolean {
  return typeof value === 'string' && RESEARCH_KIT_IDS.has(value);
}

export function repositoryClonedDirectory(
  asset: { kind?: unknown; attributes?: Record<string, unknown> },
): string | null {
  const value = asset.kind === 'repo' ? asset.attributes?.clonedDirectory : null;
  return typeof value === 'string' && value.trim() ? value.trim() : null;
}

export function decodeResolvedResearchProfile(value: unknown): {
  profile: ReturnType<typeof normalizeResearchProfile>;
  hash: string;
  source: string;
  path?: string;
} {
  const input = record(value, 'Resolved research profile');
  const hash = nonEmpty(input.hash, 'Resolved research profile hash');
  if (!/^[a-f0-9]{64}$/u.test(hash)) throw new Error('Resolved research profile hash must be a lowercase SHA-256 digest.');
  const source = nonEmpty(input.source, 'Resolved research profile source');
  if (!PROFILE_SOURCES.has(source)) throw new Error('Resolved research profile source is invalid.');
  return {
    profile: validateResearchProfile(input.profile),
    hash,
    source,
    ...(input.path === undefined ? {} : { path: nonEmpty(input.path, 'Resolved research profile path') }),
  };
}

export function decodeResearchProfileJson(value: string): ReturnType<typeof normalizeResearchProfile> {
  return validateResearchProfile(JSON.parse(value) as unknown);
}

export function serializeResearchProfile(value: unknown): string {
  return stableJson(value);
}

function validateResearchProfile(value: unknown): ReturnType<typeof normalizeResearchProfile> {
  normalizeResearchProfile(value);
  // Beale already decoded this versioned DTO before transport. Preserve its
  // exact optional-field representation because that representation is the
  // input to existing project hashes.
  return value as ReturnType<typeof normalizeResearchProfile>;
}

export function normalizeShellSafetyMode(value: unknown): 'manual_approval' | 'auto_review' | 'danger' {
  return value === 'manual_approval' || value === 'danger' ? value : 'auto_review';
}

export function normalizeRepeatSchedule(value: unknown):
  | { type: 'none' }
  | { type: 'minutely' | 'hourly' | 'daily' | 'weekly' | 'monthly'; interval: number } {
  if (!isRecord(value) || !['minutely', 'hourly', 'daily', 'weekly', 'monthly'].includes(String(value.type))) {
    return { type: 'none' };
  }
  const interval = typeof value.interval === 'number' && Number.isFinite(value.interval)
    ? Math.floor(value.interval)
    : 1;
  return {
    type: value.type as 'minutely' | 'hourly' | 'daily' | 'weekly' | 'monthly',
    interval: Math.max(1, Math.min(99, interval)),
  };
}

export function normalizeResearchCollaboration(
  value: unknown,
  fallbackProviders: readonly Record<string, unknown>[] = [],
): Record<string, unknown> {
  const input = isRecord(value) ? value : {};
  const candidates = Array.isArray(input.providers) ? input.providers : fallbackProviders;
  const seen = new Set<string>();
  const providers = candidates.flatMap((candidate) => {
    if (!isRecord(candidate)) return [];
    const provider = typeof candidate.provider === 'string' ? candidate.provider : '';
    const model = typeof candidate.model === 'string' ? candidate.model.trim() : '';
    const reasoningEffort = typeof candidate.reasoningEffort === 'string' ? candidate.reasoningEffort : '';
    const key = `${provider}\u0000${model}`;
    if (!PROVIDERS.has(provider) || !model || !EFFORTS.has(reasoningEffort) || seen.has(key)) return [];
    seen.add(key);
    const requestedRoles = Array.isArray(candidate.roles)
      ? candidate.roles
      : typeof candidate.role === 'string' ? [candidate.role] : ROLES;
    const roles = [...new Set(requestedRoles.filter((role): role is typeof ROLES[number] => (
      typeof role === 'string' && ROLES.includes(role as typeof ROLES[number])
    )))];
    return [{ provider, model, reasoningEffort, enabled: candidate.enabled !== false, roles: roles.length ? roles : [...ROLES] }];
  });
  return {
    mode: 'always',
    subagentMode: SUBAGENT_MODES.has(String(input.subagentMode)) ? input.subagentMode : 'simple',
    intensity: 'balanced',
    providers,
    independentFirstPass: false,
    peerChallengeRounds: 0,
    maxConcurrentRooms: 2,
    maxMembersPerRoom: 3,
  };
}

export function resolvedBreakoutRoomStatus(
  room: { phase: string; status: string },
  members: readonly { status: string }[],
  runStatus?: string,
): string {
  if (room.phase === 'completed' || room.status === 'completed') return 'completed';
  if (runStatus !== undefined && runStatus !== 'active') {
    return room.status === 'errored' || members.some((member) => member.status === 'errored')
      ? 'errored'
      : 'interrupted';
  }
  if (members.some((member) => member.status === 'active' || member.status === 'pending')) return 'active';
  if (room.status === 'errored' || members.some((member) => member.status === 'errored')) return 'errored';
  if (room.status === 'interrupted' || members.some((member) => member.status === 'interrupted')) return 'interrupted';
  return room.status;
}

function stableJson(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(stableJson).join(',')}]`;
  if (!value || typeof value !== 'object') return JSON.stringify(value);
  return `{${Object.entries(value as Record<string, unknown>)
    .filter(([, nested]) => nested !== undefined)
    .sort(([left], [right]) => left < right ? -1 : left > right ? 1 : 0)
    .map(([key, nested]) => `${JSON.stringify(key)}:${stableJson(nested)}`)
    .join(',')}}`;
}

function record(value: unknown, label: string): Record<string, unknown> {
  if (!isRecord(value)) throw new Error(`${label} must be an object.`);
  return value;
}

function nonEmpty(value: unknown, label: string): string {
  if (typeof value !== 'string' || !value.trim()) throw new Error(`${label} must be a non-empty string.`);
  return value.trim();
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}
