import type {
  ResearchCollaborationPreferences,
  ResearchCollaborationProviderPreference,
  ResearchModelEffortLevel,
  ResearchModelProviderId,
  ResearchSubagentMode,
  ResearchSubagentRole
} from './types';

export const DEFAULT_RESEARCH_COLLABORATION = Object.freeze({
  mode: 'always',
  subagentMode: 'simple',
  intensity: 'balanced',
  providers: [],
  independentFirstPass: false,
  peerChallengeRounds: 0,
  maxConcurrentRooms: 2,
  maxMembersPerRoom: 3
} satisfies ResearchCollaborationPreferences);

const SUBAGENT_MODES = new Set<ResearchSubagentMode>(['simple', 'advanced']);
export const RESEARCH_SUBAGENT_ROLES = ['discoverer', 'prover', 'reviewer', 'reporter'] as const satisfies readonly ResearchSubagentRole[];
const SUBAGENT_ROLES = new Set<ResearchSubagentRole>(RESEARCH_SUBAGENT_ROLES);
const PROVIDERS = new Set<ResearchModelProviderId>(['openai-codex', 'anthropic', 'xai', 'zai', 'openrouter']);
const EFFORTS = new Set<ResearchModelEffortLevel>(['minimal', 'low', 'medium', 'high', 'xhigh', 'max']);

export function normalizeResearchCollaboration(
  value: unknown,
  fallbackProviders: readonly ResearchCollaborationProviderPreference[] = []
): ResearchCollaborationPreferences {
  const record = isRecord(value) ? value : {};
  const subagentMode = SUBAGENT_MODES.has(record.subagentMode as ResearchSubagentMode)
    ? record.subagentMode as ResearchSubagentMode
    : DEFAULT_RESEARCH_COLLABORATION.subagentMode;
  const providers = normalizeProviders(record.providers, fallbackProviders);
  return {
    mode: DEFAULT_RESEARCH_COLLABORATION.mode,
    subagentMode,
    intensity: DEFAULT_RESEARCH_COLLABORATION.intensity,
    providers,
    independentFirstPass: false,
    peerChallengeRounds: 0,
    maxConcurrentRooms: DEFAULT_RESEARCH_COLLABORATION.maxConcurrentRooms,
    maxMembersPerRoom: DEFAULT_RESEARCH_COLLABORATION.maxMembersPerRoom
  };
}

export function ensureDefaultResearchCollaborator(
  collaboration: ResearchCollaborationPreferences,
  lead: ResearchCollaborationProviderPreference
): ResearchCollaborationPreferences {
  if (collaboration.providers.some((preference) => preference.enabled)) {
    return collaboration;
  }
  const leadExists = collaboration.providers.some((preference) => (
    preference.provider === lead.provider && preference.model === lead.model
  ));
  if (!leadExists) return collaboration;
  return {
    ...collaboration,
    providers: collaboration.providers.map((preference) => (
      preference.provider === lead.provider && preference.model === lead.model
        ? { ...preference, reasoningEffort: lead.reasoningEffort, enabled: true }
        : preference
    ))
  };
}

function normalizeProviders(
  value: unknown,
  fallback: readonly ResearchCollaborationProviderPreference[]
): ResearchCollaborationProviderPreference[] {
  const candidates = Array.isArray(value) ? value : fallback;
  const seen = new Set<string>();
  return candidates.flatMap((candidate) => {
    if (!isRecord(candidate)) return [];
    const provider = candidate.provider as ResearchModelProviderId;
    const model = typeof candidate.model === 'string' ? candidate.model.trim() : '';
    const reasoningEffort = candidate.reasoningEffort as ResearchModelEffortLevel;
    const key = collaborationProviderModelKey(provider, model);
    if (!PROVIDERS.has(provider) || !model || !EFFORTS.has(reasoningEffort) || seen.has(key)) return [];
    seen.add(key);
    return [{
      provider,
      model,
      reasoningEffort,
      enabled: candidate.enabled !== false,
      roles: normalizeRoles(candidate.roles, candidate.role)
    }];
  });
}

function normalizeRoles(value: unknown, legacyRole: unknown): ResearchSubagentRole[] {
  const candidates = Array.isArray(value)
    ? value
    : SUBAGENT_ROLES.has(legacyRole as ResearchSubagentRole) ? [legacyRole] : RESEARCH_SUBAGENT_ROLES;
  const roles = [...new Set(candidates.filter((role): role is ResearchSubagentRole => (
    SUBAGENT_ROLES.has(role as ResearchSubagentRole)
  )))];
  return roles.length > 0 ? roles : [...RESEARCH_SUBAGENT_ROLES];
}

function collaborationProviderModelKey(provider: ResearchModelProviderId, model: string): string {
  return `${provider}\u0000${model}`;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}
