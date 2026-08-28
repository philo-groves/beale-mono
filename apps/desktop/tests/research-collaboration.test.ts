import { describe, expect, it } from 'vitest';
import { ensureDefaultResearchCollaborator, normalizeResearchCollaboration } from '../src/shared/collaboration';
import {
  collaborationRequiresCyberPolicyAcknowledgement,
  selectedSessionProviderIds,
  selectNextAvailableCollaborator
} from '../src/renderer/features/sessions/StartRunForm';

describe('research collaboration normalization', () => {
  it('normalizes legacy room protocol controls to lax channel semantics', () => {
    expect(normalizeResearchCollaboration({ independentFirstPass: false, peerChallengeRounds: 3 })).toMatchObject({
      independentFirstPass: false,
      peerChallengeRounds: 0
    });
    expect(normalizeResearchCollaboration({ independentFirstPass: true, peerChallengeRounds: 1 })).toMatchObject({
      independentFirstPass: false,
      peerChallengeRounds: 0
    });
    expect(normalizeResearchCollaboration(undefined).independentFirstPass).toBe(false);
    expect(normalizeResearchCollaboration(undefined)).toMatchObject({ mode: 'always', intensity: 'balanced' });
    expect(normalizeResearchCollaboration({ mode: 'solo', intensity: 'deep' })).toMatchObject({
      mode: 'always',
      intensity: 'balanced'
    });
    expect(normalizeResearchCollaboration(undefined).subagentMode).toBe('simple');
    expect(normalizeResearchCollaboration({ subagentMode: 'advanced' }).subagentMode).toBe('advanced');
  });

  it('drops legacy invocation budgets and uses balanced room limits', () => {
    const collaboration = normalizeResearchCollaboration({
      intensity: 'deep',
      maxActiveInvocations: 7,
      maxTotalInvocations: 6
    });
    expect(collaboration).not.toHaveProperty('maxActiveInvocations');
    expect(collaboration).not.toHaveProperty('maxTotalInvocations');
    expect(collaboration).toMatchObject({ maxConcurrentRooms: 2, maxMembersPerRoom: 3 });
  });

  it('requires cybersecurity acknowledgement only for the security profile', () => {
    expect(collaborationRequiresCyberPolicyAcknowledgement('security-research')).toBe(true);
    expect(collaborationRequiresCyberPolicyAcknowledgement('mathematics')).toBe(false);
    expect(collaborationRequiresCyberPolicyAcknowledgement('custom-domain')).toBe(false);
  });

  it('requests credentials only for providers selected for the session', () => {
    const input = {
      provider: 'openai-codex',
      collaboration: {
        mode: 'adaptive',
        intensity: 'balanced',
        providers: [
          { provider: 'anthropic', model: 'claude-opus-5', reasoningEffort: 'high', enabled: true },
          { provider: 'xai', model: 'grok-4.6', reasoningEffort: 'high', enabled: false }
        ]
      }
    } as Parameters<typeof selectedSessionProviderIds>[0];

    expect(selectedSessionProviderIds(input)).toEqual(['openai-codex', 'anthropic']);
    expect(selectedSessionProviderIds({
      ...input,
      collaboration: { ...normalizeResearchCollaboration(input.collaboration), mode: 'solo' }
    })).toEqual(['openai-codex', 'anthropic']);
  });

  it('allows distinct models from one provider while preventing exact duplicates', () => {
    const collaboration = normalizeResearchCollaboration({
      mode: 'adaptive',
      intensity: 'balanced',
      providers: [
        { provider: 'anthropic', model: 'claude-opus-5', reasoningEffort: 'high', enabled: true },
        { provider: 'anthropic', model: 'claude-sonnet-5', reasoningEffort: 'high', enabled: true },
        { provider: 'anthropic', model: 'claude-opus-5', reasoningEffort: 'medium', enabled: false }
      ]
    });

    expect(collaboration.providers).toEqual([
      { provider: 'anthropic', model: 'claude-opus-5', reasoningEffort: 'high', enabled: true, roles: ['discoverer', 'prover', 'reviewer', 'reporter'] },
      { provider: 'anthropic', model: 'claude-sonnet-5', reasoningEffort: 'high', enabled: true, roles: ['discoverer', 'prover', 'reviewer', 'reporter'] }
    ]);
  });

  it('normalizes Advanced compatible roles, including legacy single-role preferences', () => {
    const collaboration = normalizeResearchCollaboration({
      subagentMode: 'advanced',
      providers: [
        { provider: 'anthropic', model: 'claude-opus-5', reasoningEffort: 'high', enabled: true, role: 'reviewer' },
        { provider: 'xai', model: 'grok-4.6', reasoningEffort: 'high', enabled: true, roles: ['discoverer', 'reporter', 'unknown'] },
        { provider: 'openai-codex', model: 'gpt-5.6-sol', reasoningEffort: 'high', enabled: true, roles: [] }
      ]
    });

    expect(collaboration.providers[0]).toMatchObject({ roles: ['reviewer'] });
    expect(collaboration.providers[1]).toMatchObject({ roles: ['discoverer', 'reporter'] });
    expect(collaboration.providers[2]).toMatchObject({ roles: ['discoverer', 'prover', 'reviewer', 'reporter'] });
  });

  it('fills unused providers before stacking another model from a represented provider', () => {
    const anthropicOpus = { provider: 'anthropic' as const, model: 'claude-opus-5' };
    const anthropicSonnet = { provider: 'anthropic' as const, model: 'claude-sonnet-5' };
    const xaiGrok = { provider: 'xai' as const, model: 'grok-4.6' };
    const openAiLuna = { provider: 'openai-codex' as const, model: 'gpt-5.6-luna' };

    expect(selectNextAvailableCollaborator(
      [anthropicOpus, anthropicSonnet, xaiGrok, openAiLuna],
      [],
      'openai-codex'
    )).toBe(anthropicOpus);
    expect(selectNextAvailableCollaborator(
      [anthropicSonnet, xaiGrok, openAiLuna],
      [anthropicOpus],
      'openai-codex'
    )).toBe(xaiGrok);
    expect(selectNextAvailableCollaborator(
      [anthropicSonnet, openAiLuna],
      [anthropicOpus, xaiGrok],
      'openai-codex'
    )).toBe(anthropicSonnet);
  });

  it('enables the lead model only when no collaborator has been selected', () => {
    const collaboration = normalizeResearchCollaboration({
      mode: 'adaptive',
      intensity: 'balanced',
      providers: [
        { provider: 'openai-codex', model: 'gpt-5.6-sol', reasoningEffort: 'high', enabled: false },
        { provider: 'anthropic', model: 'claude-opus-5', reasoningEffort: 'high', enabled: false }
      ]
    });
    const withDefault = ensureDefaultResearchCollaborator(collaboration, {
      provider: 'openai-codex',
      model: 'gpt-5.6-sol',
      reasoningEffort: 'xhigh',
      enabled: true
    });

    expect(withDefault.providers).toEqual([
      { provider: 'openai-codex', model: 'gpt-5.6-sol', reasoningEffort: 'xhigh', enabled: true, roles: ['discoverer', 'prover', 'reviewer', 'reporter'] },
      { provider: 'anthropic', model: 'claude-opus-5', reasoningEffort: 'high', enabled: false, roles: ['discoverer', 'prover', 'reviewer', 'reporter'] }
    ]);
    expect(ensureDefaultResearchCollaborator({
      ...withDefault,
      providers: withDefault.providers.map((preference) => ({
        ...preference,
        enabled: preference.provider === 'anthropic'
      }))
    }, {
      provider: 'openai-codex',
      model: 'gpt-5.6-sol',
      reasoningEffort: 'high',
      enabled: true
    }).providers).toEqual([
      { provider: 'openai-codex', model: 'gpt-5.6-sol', reasoningEffort: 'xhigh', enabled: false, roles: ['discoverer', 'prover', 'reviewer', 'reporter'] },
      { provider: 'anthropic', model: 'claude-opus-5', reasoningEffort: 'high', enabled: true, roles: ['discoverer', 'prover', 'reviewer', 'reporter'] }
    ]);
  });
});
