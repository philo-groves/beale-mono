import { describe, expect, it } from 'vitest';
import { getAppServerProviderSemantics } from '../src/main/appServerCliClient';
import { DEFAULT_RESEARCH_MODEL } from '../src/shared/modelDefaults';

describe('research session title models', () => {
  it('defaults OpenAI large requests to GPT-6 Sol', () => {
    expect(DEFAULT_RESEARCH_MODEL).toBe('gpt-6-sol');
  });

  it('uses the designated small model for each supported provider', () => {
    const semantics = getAppServerProviderSemantics();
    expect(semantics.defaultSmallModels).toEqual({
      'openai-codex': 'gpt-6-luna',
      anthropic: 'claude-haiku-4-5',
      xai: 'grok-4.3',
      zai: 'glm-5-turbo',
      openrouter: 'auto'
    });
    expect(semantics.sessionTitleEffort).toBe('medium');
    expect(semantics.shellReviewEffort).toBe('low');
  });

  it('does not invent a title model for unknown providers', () => {
    expect(getAppServerProviderSemantics().defaultSmallModels).not.toHaveProperty('other');
  });
});
