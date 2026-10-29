import { createProvider, type Model, type MutableModels } from "@earendil-works/pi-ai";
import { openAICompletionsApi } from "@earendil-works/pi-ai/api/openai-completions.lazy";

export interface CustomProviderDefinition {
  id: string;
  name: string;
  baseUrl: string;
  contextWindow: number;
  maxTokens: number;
  models: { id: string; name: string }[];
  /** Enable OpenAI-compatible reasoning controls, with DeepSeek wire semantics for DeepSeek model IDs. */
  reasoning?: boolean;
}

const ID_PATTERN = /^custom-[a-z0-9][a-z0-9-]{0,63}$/u;

export function customProviderKeyEnvironmentVariable(id: string): string {
  if (!ID_PATTERN.test(id)) throw new Error("Invalid custom provider ID.");
  return `BEALE_CUSTOM_PROVIDER_KEY_${id.slice(7).replaceAll("-", "_").toUpperCase()}`;
}

export function normalizeCustomProviderDefinition(value: unknown): CustomProviderDefinition {
  if (!isRecord(value) || typeof value.id !== "string" || !ID_PATTERN.test(value.id)
    || typeof value.name !== "string" || !value.name.trim() || value.name.length > 100
    || typeof value.baseUrl !== "string" || !Array.isArray(value.models) || value.models.length > 500) {
    throw new Error("Invalid custom provider definition.");
  }
  let url: URL;
  try { url = new URL(value.baseUrl); } catch { throw new Error("Invalid custom provider base URL."); }
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash
    || value.baseUrl.length > 2048) throw new Error("Invalid custom provider base URL.");
  const models = value.models.map((model) => {
    if (!isRecord(model) || typeof model.id !== "string" || !model.id.trim() || model.id.length > 200
      || typeof model.name !== "string" || !model.name.trim() || model.name.length > 200) {
      throw new Error("Invalid custom provider model.");
    }
    return { id: model.id.trim(), name: model.name.trim() };
  });
  if (new Set(models.map((model) => model.id)).size !== models.length) {
    throw new Error("Duplicate custom provider model ID.");
  }
  const contextWindow = value.contextWindow ?? 128_000;
  const maxTokens = value.maxTokens ?? 16_000;
  if (!Number.isInteger(contextWindow) || Number(contextWindow) < 1_024 || Number(contextWindow) > 10_000_000
    || !Number.isInteger(maxTokens) || Number(maxTokens) < 256 || Number(maxTokens) > Number(contextWindow)) {
    throw new Error("Invalid custom provider token limits.");
  }
  return { id: value.id, name: value.name.trim(), baseUrl: url.href.replace(/\/$/u, ''),
    contextWindow: Number(contextWindow), maxTokens: Number(maxTokens), models,
    ...(value.reasoning === true ? { reasoning: true } : {}) };
}

export function readCustomProviderDefinitions(value = process.env.APP_SERVER_CUSTOM_PROVIDERS): CustomProviderDefinition[] {
  if (!value) return [];
  const parsed: unknown = JSON.parse(value);
  if (!Array.isArray(parsed) || parsed.length > 32) throw new Error("Invalid custom provider configuration.");
  const definitions = parsed.map(normalizeCustomProviderDefinition);
  if (new Set(definitions.map((provider) => provider.id)).size !== definitions.length) {
    throw new Error("Duplicate custom provider ID.");
  }
  return definitions;
}

export async function discoverCustomProviderModels(
  baseUrl: string,
  apiKey?: string,
  fetcher: typeof fetch = fetch,
): Promise<CustomProviderDefinition['models']> {
  const normalized = normalizeCustomProviderDefinition({ id: 'custom-discovery', name: 'Discovery', baseUrl, models: [] });
  const response = await fetcher(`${normalized.baseUrl}/models`, {
    method: 'GET',
    ...(apiKey ? { headers: { Authorization: `Bearer ${apiKey}` } } : {}),
    redirect: 'error',
    signal: AbortSignal.timeout(15_000),
  });
  if (!response.ok) throw new Error(`Model discovery failed with HTTP ${response.status}.`);
  const length = Number(response.headers.get('content-length') ?? '0');
  if (length > 2_000_000) throw new Error('Provider model list is too large.');
  const raw = await response.text();
  if (raw.length > 2_000_000) throw new Error('Provider model list is too large.');
  const body: unknown = JSON.parse(raw);
  if (!isRecord(body) || !Array.isArray(body.data)) throw new Error('Provider did not return an OpenAI-compatible model list.');
  const entries = body.data.flatMap((entry): { id: string; name: string }[] => {
    if (!isRecord(entry) || typeof entry.id !== 'string' || !entry.id.trim() || entry.id.length > 200) return [];
    return [{ id: entry.id.trim(), name: typeof entry.name === 'string' && entry.name.trim() ? entry.name.trim() : entry.id.trim() }];
  }).slice(0, 500);
  if (!entries.length) throw new Error('Provider returned no usable models.');
  const seen = new Set<string>();
  return entries.filter((entry) => {
    if (seen.has(entry.id)) return false;
    seen.add(entry.id);
    return true;
  });
}

export function createCustomProviderModel(
  definition: CustomProviderDefinition,
  model: { id: string; name: string },
): Model<'openai-completions'> {
  const reasoning = definition.reasoning === true;
  const deepSeek = reasoning && model.id.toLowerCase().includes('deepseek');
  return {
    ...model,
    provider: definition.id,
    api: 'openai-completions',
    baseUrl: definition.baseUrl,
    reasoning,
    input: ['text'],
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    contextWindow: definition.contextWindow,
    maxTokens: definition.maxTokens,
    ...(deepSeek ? {
      thinkingLevelMap: { minimal: null, low: 'low', medium: null, high: 'high', max: 'max' } as const,
    } : {}),
    compat: {
      supportsStore: false,
      supportsDeveloperRole: false,
      supportsReasoningEffort: reasoning,
      ...(deepSeek ? {
        maxTokensField: 'max_tokens' as const,
        requiresReasoningContentOnAssistantMessages: true,
        thinkingFormat: 'deepseek' as const,
      } : {}),
    },
  };
}

export function registerCustomProviders(models: MutableModels, definitions = readCustomProviderDefinitions()): void {
  for (const definition of definitions) {
    const keyVariable = customProviderKeyEnvironmentVariable(definition.id);
    const providerModels = definition.models.map((model) => createCustomProviderModel(definition, model));
    models.setProvider(createProvider({
      id: definition.id,
      name: definition.name,
      baseUrl: definition.baseUrl,
      auth: { apiKey: { name: `${definition.name} API key`, resolve: async ({ ctx }) => {
        const apiKey = await ctx.env(keyVariable);
        return { auth: apiKey ? { apiKey } : {} };
      } } },
      models: providerModels,
      api: openAICompletionsApi(),
    }));
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}
