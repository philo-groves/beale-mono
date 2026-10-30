import { RESEARCH_MODEL_PROVIDER_IDS, type ResearchModelProviderId } from './auth-routing.js';
import { readCustomProviderDefinitions } from './custom-providers.js';
import type { AuxiliaryModelProvider } from './auxiliary-model-job.js';

export const RESEARCH_PROVIDER_IDS = RESEARCH_MODEL_PROVIDER_IDS;
export type ResearchProviderId = ResearchModelProviderId;

export const DEFAULT_SMALL_MODELS: Readonly<Record<ResearchProviderId, string>> = Object.freeze({
  'openai-codex': 'gpt-6-luna',
  anthropic: 'claude-haiku-4-5',
  xai: 'grok-4.3',
  zai: 'glm-5-turbo',
  openrouter: 'auto',
});

export interface ProviderSemanticsDescriptor {
  providers: readonly AuxiliaryModelProvider[];
  aliases: Readonly<Record<string, AuxiliaryModelProvider>>;
  defaultSmallModels: Readonly<Record<string, string>>;
  auxiliaryEfforts: readonly ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'];
  sessionTitleEffort: 'medium';
  shellReviewEffort: 'low';
}

export function providerSemanticsDescriptor(): ProviderSemanticsDescriptor {
  return {
    providers: [...RESEARCH_PROVIDER_IDS, ...configuredCustomProviderIds()],
    aliases: { openai: 'openai-codex' },
    defaultSmallModels: DEFAULT_SMALL_MODELS,
    auxiliaryEfforts: ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'],
    sessionTitleEffort: 'medium',
    shellReviewEffort: 'low',
  };
}

function configuredCustomProviderIds(): AuxiliaryModelProvider[] {
  try {
    return readCustomProviderDefinitions().map((provider) => provider.id as AuxiliaryModelProvider);
  } catch {
    return [];
  }
}
