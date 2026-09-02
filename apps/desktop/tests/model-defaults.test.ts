import { describe, expect, it } from 'vitest';
import { getAppServerProviderSemantics } from '../src/main/appServerCliClient';

describe('research session title models', () => {
  it('uses the designated small model for each supported provider', () => {
    const semantics = getAppServerProviderSemantics();
    expect(semantics.defaultSmallModels).toEqual({
      'openai-codex': 'gpt-5.6-luna',
      anthropic: 'claude-haiku-4-5',
      xai: 'grok-4.3',
      zai: 'glm-5-turbo',
      openrouter: 'auto'
    });
    expect(semantics.sessionTitleEffort).toBe('medium');
    expect(semantics.shellReviewEffort).toBe('medium');
  });

  it('does not invent a title model for unknown providers', () => {
    expect(getAppServerProviderSemantics().defaultSmallModels).not.toHaveProperty('other');
  });
});
