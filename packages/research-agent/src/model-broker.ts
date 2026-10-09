import { randomUUID } from "node:crypto";
import {
  createAssistantMessageEventStream,
  type AssistantMessage,
  type Context,
  type Model,
  type Models,
  type ModelsSimpleStreamOptions,
  type Api,
} from "@earendil-works/pi-ai";
import { ProviderAuthenticationRouter, type ProviderAuthenticationPreferences } from "./auth-routing.js";

export interface ModelBrokerRequest {
  id: string;
  model: { provider: string; id: string };
  context: Context;
  options: Omit<ModelsSimpleStreamOptions, "signal" | "apiKey">;
  brokerFlags?: { nativeCompaction: boolean; fastMode: boolean; daybreakBlue: boolean; contextWindow: number };
}

export interface ModelBrokerEndpoint { url: string; token: string }

const REQUEST_LIMIT_BYTES = 16 * 1024 * 1024;
class BrokerRejected extends Error {}

/** The guest sends no provider credential; the primary resolves auth for each call. */
export function brokeredModels(models: Models, endpoint: ModelBrokerEndpoint): Models {
  return new Proxy(models, {
    get(target, property, receiver) {
      if (property === "streamSimple") return (model: Model<Api>, context: Context, options?: ModelsSimpleStreamOptions) => {
        const stream = createAssistantMessageEventStream();
        const partial: AssistantMessage = {
          role: "assistant", content: [], api: model.api, provider: model.provider,
          model: model.id, usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0,
            cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
          stopReason: "stop", timestamp: Date.now(),
        };
        stream.push({ type: "start", partial });
        void requestBrokerCompletion(endpoint, model, context, options).then((message) => {
          if (message.stopReason === "error" || message.stopReason === "aborted") {
            stream.push({ type: "error", reason: message.stopReason, error: message });
          } else {
            stream.push({ type: "done", reason: message.stopReason, message });
          }
        }).catch((error: unknown) => {
          stream.push({ type: "error", reason: options?.signal?.aborted ? "aborted" : "error",
            error: { ...partial, stopReason: options?.signal?.aborted ? "aborted" : "error",
              errorMessage: error instanceof Error ? error.message : String(error) } });
        });
        return stream;
      };
      if (property === "completeSimple") return async (model: Model<Api>, context: Context, options?: ModelsSimpleStreamOptions) =>
        requestBrokerCompletion(endpoint, model, context, options);
      const value: unknown = Reflect.get(target, property, receiver);
      return typeof value === "function" ? value.bind(target) : value;
    },
  });
}

async function requestBrokerCompletion(endpoint: ModelBrokerEndpoint, model: Model<Api>, context: Context,
  options?: ModelsSimpleStreamOptions): Promise<AssistantMessage> {
  const id = randomUUID();
  const flags = options as (ModelsSimpleStreamOptions & {
    brokerNativeCompaction?: boolean; brokerFastMode?: boolean; brokerDaybreakBlue?: boolean; brokerContextWindow?: number;
  }) | undefined;
  const signal = options?.signal;
  const safeOptions: ModelBrokerRequest["options"] = {
    ...(typeof options?.temperature === "number" ? { temperature: options.temperature } : {}),
    ...(typeof options?.maxTokens === "number" ? { maxTokens: options.maxTokens } : {}),
    ...(typeof options?.reasoning === "string" ? { reasoning: options.reasoning } : {}),
    ...(options?.thinkingBudgets ? { thinkingBudgets: options.thinkingBudgets } : {}),
    ...(typeof options?.sessionId === "string" ? { sessionId: options.sessionId } : {}),
    ...(options?.transport ? { transport: options.transport } : {}),
    ...(options?.cacheRetention ? { cacheRetention: options.cacheRetention } : {}),
    ...(typeof options?.timeoutMs === "number" ? { timeoutMs: options.timeoutMs } : {}),
    ...(typeof options?.websocketConnectTimeoutMs === "number" ? { websocketConnectTimeoutMs: options.websocketConnectTimeoutMs } : {}),
    ...(typeof options?.maxRetries === "number" ? { maxRetries: options.maxRetries } : {}),
    ...(typeof options?.maxRetryDelayMs === "number" ? { maxRetryDelayMs: options.maxRetryDelayMs } : {}),
    ...(options?.metadata ? { metadata: options.metadata } : {}),
  };
  const brokerFlags = flags?.brokerNativeCompaction || flags?.brokerFastMode || flags?.brokerDaybreakBlue
    ? { nativeCompaction: flags.brokerNativeCompaction === true, fastMode: flags.brokerFastMode === true,
        daybreakBlue: flags.brokerDaybreakBlue === true, contextWindow: flags.brokerContextWindow ?? model.contextWindow }
    : undefined;
  const request: ModelBrokerRequest = { id, model: { provider: model.provider, id: model.id }, context,
    options: safeOptions, ...(brokerFlags ? { brokerFlags } : {}) };
  const body = JSON.stringify(request);
  if (Buffer.byteLength(body) > REQUEST_LIMIT_BYTES) throw new Error("Model broker request exceeds its size limit.");
  const headers = { authorization: `Bearer ${endpoint.token}`, "content-type": "application/json" };
  for (;;) {
    if (signal?.aborted) throw new Error("Model broker request was aborted.");
    try {
      const response = await fetch(endpoint.url, { method: "POST", headers, body, signal: AbortSignal.any([
        signal ?? new AbortController().signal, AbortSignal.timeout(30_000),
      ]) });
      if (!response.ok) throw new BrokerRejected(`Model broker submission failed (HTTP ${response.status}).`);
      break;
    } catch (error) {
      if (signal?.aborted || error instanceof BrokerRejected) throw error;
      await delay(500);
    }
  }
  for (;;) {
    if (signal?.aborted) throw new Error("Model broker request was aborted.");
    try {
      const response = await fetch(`${endpoint.url}/${encodeURIComponent(id)}`, { headers: { authorization: headers.authorization },
        signal: AbortSignal.any([signal ?? new AbortController().signal, AbortSignal.timeout(15_000)]) });
      if (!response.ok) throw new BrokerRejected(`Model broker result failed (HTTP ${response.status}).`);
      const result: unknown = await response.json();
      if (isRecord(result) && result.state === "done" && isAssistantMessage(result.message)) return result.message;
      if (isRecord(result) && result.state === "error") throw new BrokerRejected(typeof result.error === "string" ? result.error : "Model broker failed.");
    } catch (error) {
      if (signal?.aborted || error instanceof BrokerRejected) throw error;
      // The guest app-server or SSH tunnel may restart; the request ID remains stable.
    }
    await delay(500);
  }
}

/** Called by the workspace-owning primary with its own provider credentials. */
export async function completeBrokerModelRequest(request: ModelBrokerRequest,
  authenticationPreferences: ProviderAuthenticationPreferences, codexAuthFile?: string): Promise<AssistantMessage> {
  const router = new ProviderAuthenticationRouter(authenticationPreferences);
  const { createAuthenticatedModels } = await import("./auth.js");
  const models = createAuthenticatedModels({ authContext: router.authContext(), ...(codexAuthFile ? { codexAuthFile } : {}) });
  const providerId = request.model.provider === "openai" ? "openai-codex" : request.model.provider;
  const initialModel = router.routePiModel(models, providerId, request.model.id);
  if (!initialModel || initialModel.provider === "anthropic") throw new Error("This provider cannot use the Fleet model broker.");
  let model: Model<Api> = initialModel;
  const { apiKey: _untrustedKey, headers: _untrustedHeaders, env: _untrustedEnv,
    onPayload: _untrustedPayload, onResponse: _untrustedResponse, signal: _untrustedSignal,
    ...safeOptions } = request.options as ModelsSimpleStreamOptions;
  const flags = request.brokerFlags;
  const onPayload = flags && (flags.nativeCompaction || flags.fastMode || flags.daybreakBlue)
    ? async (payload: unknown) => {
        const { applyNativeOpenAiCompaction, applyOpenAiFastMode, applyOpenAiDaybreakBlue } = await import("./agent-executor.js");
        const brokerModel = { ...model, contextWindow: Number.isSafeInteger(flags.contextWindow)
          && flags.contextWindow > 0 && flags.contextWindow <= 1_000_000 ? flags.contextWindow : model.contextWindow };
        let transformed = flags.nativeCompaction ? applyNativeOpenAiCompaction(payload, brokerModel) : payload;
        transformed = applyOpenAiFastMode(transformed, brokerModel, flags.fastMode);
        return applyOpenAiDaybreakBlue(transformed, brokerModel, flags.daybreakBlue);
      }
    : undefined;
  for (let attempt = 0; attempt < 2; attempt += 1) {
    const apiKey = router.requestApiKey(providerId);
    let response: AssistantMessage;
    try {
      response = await models.completeSimple(model, request.context, { ...safeOptions, ...(apiKey ? { apiKey } : {}),
        ...(onPayload ? { onPayload } : {}) });
    } catch (error) {
      if (attempt === 0 && router.tryFallback(providerId, error instanceof Error ? error.message : String(error))) {
        model = router.routePiModel(models, providerId, request.model.id) ?? model;
        continue;
      }
      throw error;
    }
    if (attempt === 0 && response.stopReason === "error" && router.tryFallback(providerId, response.errorMessage ?? "")) {
      model = router.routePiModel(models, providerId, request.model.id) ?? model;
      continue;
    }
    return response.stopReason === "error"
      ? { ...response, errorMessage: "The model provider request failed on the workspace primary." }
      : response;
  }
  throw new Error("Model broker authentication fallback did not complete.");
}

export async function verifyBrokerModelAccess(providerId: string, modelId: string,
  authenticationPreferences: ProviderAuthenticationPreferences, codexAuthFile?: string): Promise<void> {
  const router = new ProviderAuthenticationRouter(authenticationPreferences);
  if (providerId === "anthropic" || providerId === "zai" && router.method("zai") === "subscription") {
    throw new Error("This provider-specific subscription SDK cannot yet run through the Fleet model broker.");
  }
  const { createAuthenticatedModels } = await import("./auth.js");
  const models = createAuthenticatedModels({ authContext: router.authContext(), ...(codexAuthFile ? { codexAuthFile } : {}) });
  const model = router.routePiModel(models, providerId, modelId);
  if (!model) throw new Error(`The primary has no brokered model ${providerId}/${modelId}.`);
  if (router.method(providerId) === "api_key") {
    if (!router.apiKey(providerId)) throw new Error(`The primary has no API key for ${providerId}.`);
    return;
  }
  if (!await models.getAuth(model)) throw new Error(`The primary is not authenticated for ${providerId}.`);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isAssistantMessage(value: unknown): value is AssistantMessage {
  return isRecord(value) && value.role === "assistant" && Array.isArray(value.content)
    && typeof value.model === "string" && typeof value.stopReason === "string";
}

function delay(ms: number): Promise<void> { return new Promise((resolve) => setTimeout(resolve, ms)); }
