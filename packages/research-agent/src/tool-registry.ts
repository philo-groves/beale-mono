import type {
  Tool,
  ToolCall,
  ToolResultMessage,
} from "@earendil-works/pi-ai";
import { createResearchEventId, nowIso } from "./ids.js";
import type { ManagedToolPluginOption } from "./managed-tool-plugins.js";
import {
  MAX_MCP_CALL_TIMEOUT_MS,
  MCP_CALL_TIMEOUT_INPUT_KEY,
  MIN_MCP_CALL_TIMEOUT_MS,
} from "./mcp-tools.js";
import type { ModelAuthor } from "./model-authorship.js";
import {
  redactShellArguments,
  sanitizeShellActionInput,
} from "./shell-safety.js";
import type {
  ResearchActionClass,
  ResearchArtifactRef,
  ResearchEvent,
  ResearchGovernancePolicy,
  ResearchToolAction,
  ResearchToolDescriptor,
  ResearchToolSideEffect,
} from "./types.js";

export type ResearchToolExecutionStatus = "complete" | "error" | "blocked";

export interface ResearchToolExecutionContext {
  signal?: AbortSignal;
  agentId?: string;
  modelAuthor?: ModelAuthor;
  /** True only for a child agent spawned without parent or channel transcript inheritance. */
  freshSubagentContext?: boolean;
  runbookContext?: {
    runbookId: string;
    runId: string;
    cellId: string;
  };
}

export interface ResearchToolValidationHookInput {
  phase: "before" | "after";
  tool: ResearchExecutableTool;
  action: ResearchToolAction;
  context: ExecuteToolCallOptions;
  result?: ResearchToolExecutionResult;
}

export type ResearchToolValidationHook = (
  input: ResearchToolValidationHookInput,
) => string | void | Promise<string | void>;

export interface ResearchToolExecutionResult {
  action: ResearchToolAction;
  status: ResearchToolExecutionStatus;
  startedAt: string;
  completedAt: string;
  summary: string;
  output?: unknown;
  /**
   * Optional compact projection used only for model-visible tool content.
   * The canonical output remains unchanged for audit events and host consumers.
   */
  modelOutput?: unknown;
  modelContent?: ToolResultMessage["content"];
  pagination?: {
    toolActionId: string;
    totalCharacters: number;
    omittedStart: number;
    omittedEnd: number;
  };
  rawOutputRef?: string;
  artifactRefs?: readonly ResearchArtifactRef[];
  followUpActions: readonly string[];
  error?: {
    message: string;
  };
}

export interface ModelToolResultDetails {
  status: ResearchToolExecutionStatus;
  summary: string;
  rawOutputRef?: string;
  artifactRefs?: readonly ResearchArtifactRef[];
  followUpActions?: readonly string[];
  error?: {
    message: string;
  };
}

export interface ModelToolResultProjection {
  content: ToolResultMessage["content"];
  details: ModelToolResultDetails;
  isError: boolean;
}

const DEFAULT_MODEL_TOOL_RESULT_MAX_CHARS = 32_000;
const RESULT_PAGE_MAX_CHARACTERS = 8_000;
const RESULT_PAGE_STORE_MAX_ENTRY_CHARACTERS = 8_000_000;
const RESULT_PAGE_STORE_MAX_CHARACTERS = 24_000_000;
const RESULT_PAGE_STORE_MAX_ENTRIES = 64;
const DEFAULT_TOOL_RUNTIME_BUDGET_MS = 120_000;
const TOOL_RUNTIME_BUDGET_MS_BY_TOOL = new Map<string, number>([
  ["code.detect", 30_000],
  ["code.references", 30_000],
  ["code.call_candidates", 30_000],
]);
const MODEL_TOOL_RESULT_MAX_CHARS_BY_TOOL = new Map<string, number>([
  ["tool_result.page", 64_000],
  ["history.search", 12_000],
  ["history.mark_duplicate", 12_000],
  ["history.undo_duplicate", 12_000],
  ["finding.list", 8_000],
  ["investigation.status", 8_000],
  ["investigation.recall", 10_000],
  ["runbook.get", 12_000],
  ["runbook.list", 8_000],
  ["repository.search", 16_000],
  ["repository.history", 24_000],
  ["prior_art.search", 32_000],
  ["prior_art.fetch", 24_000],
  ["resource.catalog", 24_000],
  ["file.read", 24_000],
  ["shell.run", 24_000],
]);

export interface ResearchExecutableTool {
  descriptor: ResearchToolDescriptor;
  parameters?: Tool["parameters"];
  execute(
    action: ResearchToolAction,
    context?: ResearchToolExecutionContext,
  ): Promise<ResearchToolExecutionResult>;
}

export interface ResearchToolExecutionRecord {
  action: ResearchToolAction;
  result: ResearchToolExecutionResult;
  events: readonly ResearchEvent[];
}

export interface ExecuteToolCallOptions extends ResearchToolExecutionContext {
  permittedActionClasses?: readonly ResearchActionClass[];
  defaultActionClass?: ResearchActionClass;
  toolCallId?: string;
  governance?: ResearchGovernancePolicy;
  toolCallCount?: number;
  excludedPaths?: readonly string[];
}

export interface ResearchToolRegistryOptions {
  managedPlugins?: readonly ManagedToolPluginOption[];
  validationHooks?: ReadonlyMap<string, ResearchToolValidationHook> | Record<string, ResearchToolValidationHook>;
  paginateResults?: boolean;
  resultPageStore?: ToolResultPageStore;
}

interface StoredToolResultPage {
  toolActionId: string;
  toolName: string;
  text: string;
}

/** Ephemeral, bounded pages of the same host-safe projection shown to the model. */
export class ToolResultPageStore {
  readonly #entries = new Map<string, StoredToolResultPage>();
  #characters = 0;

  retain(result: ResearchToolExecutionResult, agentId: string | undefined): ResearchToolExecutionResult {
    if (!agentId || result.action.toolName === "tool_result.page") return result;
    const projected = projectToolResult(result);
    const output = "modelOutput" in projected ? projected.modelOutput : projected.output;
    const text = serializeModelToolResult(projected, output);
    const maxCharacters = modelToolResultMaxCharacters(projected.action.toolName);
    if (text.length <= maxCharacters || text.length > RESULT_PAGE_STORE_MAX_ENTRY_CHARACTERS) return result;
    const key = this.key(agentId, projected.action.id);
    const previous = this.#entries.get(key);
    if (previous) this.#characters -= previous.text.length;
    this.#entries.delete(key);
    this.#entries.set(key, { toolActionId: projected.action.id, toolName: projected.action.toolName, text });
    this.#characters += text.length;
    while (this.#entries.size > RESULT_PAGE_STORE_MAX_ENTRIES || this.#characters > RESULT_PAGE_STORE_MAX_CHARACTERS) {
      const oldestKey = this.#entries.keys().next().value;
      if (oldestKey === undefined) break;
      this.#characters -= this.#entries.get(oldestKey)!.text.length;
      this.#entries.delete(oldestKey);
    }
    const half = Math.floor(maxCharacters / 2);
    return {
      ...result,
      pagination: {
        toolActionId: projected.action.id,
        totalCharacters: text.length,
        omittedStart: half,
        omittedEnd: text.length - half,
      },
    };
  }

  page(agentId: string | undefined, toolActionId: string, offset: number, maxCharacters: number): Record<string, unknown> | null {
    if (!agentId) return null;
    const entry = this.#entries.get(this.key(agentId, toolActionId));
    if (!entry || !Number.isSafeInteger(offset) || offset < 0 || offset >= entry.text.length
      || !Number.isSafeInteger(maxCharacters) || maxCharacters < 1 || maxCharacters > RESULT_PAGE_MAX_CHARACTERS) return null;
    const end = Math.min(entry.text.length, offset + maxCharacters);
    return {
      toolActionId: entry.toolActionId,
      toolName: entry.toolName,
      offset,
      totalCharacters: entry.text.length,
      text: entry.text.slice(offset, end),
      ...(end < entry.text.length ? { nextOffset: end } : {}),
    };
  }

  private key(agentId: string, toolActionId: string): string {
    return `${agentId}\0${toolActionId}`;
  }
}

function createToolResultPageTool(store: ToolResultPageStore): ResearchExecutableTool {
  const parameters = {
    type: "object",
    required: ["toolActionId", "offset"],
    additionalProperties: false,
    properties: {
      toolActionId: { type: "string", minLength: 1 },
      offset: { type: "integer", minimum: 0 },
      maxCharacters: { type: "integer", minimum: 1, maximum: RESULT_PAGE_MAX_CHARACTERS },
    },
  };
  return {
    descriptor: {
      name: "tool_result.page",
      transportName: "tool_result_page",
      description: "Read a bounded page of a truncated tool result using its toolActionId and a character offset. Pages contain the same sanitized model-visible output as the initial preview. Use nextOffset to continue; a missing page means the result expired and the original tool should be rerun narrowly.",
      actionClasses: ["inspect"],
      sideEffects: "none",
      requiredPermissions: [],
      inputSchema: parameters,
    },
    parameters: parameters as NonNullable<ResearchExecutableTool["parameters"]>,
    async execute(action, context) {
      const startedAt = nowIso();
      const toolActionId = action.input.toolActionId;
      const offset = action.input.offset;
      const maxCharacters = action.input.maxCharacters ?? RESULT_PAGE_MAX_CHARACTERS;
      const page = typeof toolActionId === "string" && typeof offset === "number" && typeof maxCharacters === "number"
        ? store.page(context?.agentId, toolActionId, offset, maxCharacters)
        : null;
      return page
        ? { action, status: "complete", startedAt, completedAt: nowIso(), summary: "Read a page of the prior tool result.", output: page, followUpActions: [] }
        : { action, status: "error", startedAt, completedAt: nowIso(), summary: "Tool result page unavailable or offset invalid. Re-run the original tool with narrower input if needed.", followUpActions: [] };
    },
  };
}

export class ResearchToolRegistry {
  readonly managedPlugins: readonly ManagedToolPluginOption[] | undefined;
  readonly #toolsByName = new Map<string, ResearchExecutableTool>();
  readonly #toolsByTransportName = new Map<string, ResearchExecutableTool>();
  readonly #validationHooks = new Map<string, ResearchToolValidationHook>();
  readonly #resultPageStore: ToolResultPageStore | undefined;

  constructor(
    tools: readonly ResearchExecutableTool[] = [],
    options: ResearchToolRegistryOptions = {},
  ) {
    this.managedPlugins = options.managedPlugins;
    this.#resultPageStore = options.resultPageStore ?? (options.paginateResults ? new ToolResultPageStore() : undefined);
    for (const [name, hook] of readValidationHookEntries(options.validationHooks)) {
      this.#validationHooks.set(name, hook);
    }
    for (const tool of tools) {
      this.register(tool);
    }
    if (this.#resultPageStore) this.register(createToolResultPageTool(this.#resultPageStore));
  }

  register(tool: ResearchExecutableTool): void {
    this.#toolsByName.set(tool.descriptor.name, tool);
    this.#toolsByTransportName.set(getToolTransportName(tool), tool);
  }

  listDescriptors(): ResearchToolDescriptor[] {
    return [...this.#toolsByName.values()].map((tool) => tool.descriptor);
  }

  listTools(): ResearchExecutableTool[] {
    return [...this.#toolsByName.values()];
  }

  fork(additionalTools: readonly ResearchExecutableTool[] = []): ResearchToolRegistry {
    return new ResearchToolRegistry([...this.listTools(), ...additionalTools], {
      validationHooks: this.#validationHooks,
      ...(this.managedPlugins ? { managedPlugins: this.managedPlugins } : {}),
      ...(this.#resultPageStore ? { resultPageStore: this.#resultPageStore } : {}),
    });
  }

  toPiTools(): Tool[] {
    return [...this.#toolsByName.values()]
      .filter((tool) => tool.parameters)
      .map((tool) => ({
        name: getToolTransportName(tool),
        description: tool.descriptor.description,
        parameters: tool.parameters!,
      }));
  }

  get size(): number {
    return this.#toolsByName.size;
  }

  find(name: string): ResearchExecutableTool | undefined {
    return this.#toolsByName.get(name) ?? this.#toolsByTransportName.get(name);
  }

  async execute(
    action: ResearchToolAction,
    options: ExecuteToolCallOptions = {},
  ): Promise<ResearchToolExecutionRecord> {
    const tool = this.find(action.toolName);
    if (!tool) {
      const result = createBlockedToolResult(
        action,
        `Unknown tool: ${action.toolName}`,
      );
      return createExecutionRecord(result, options);
    }

    const normalizedAction = applyBudgetDefaults(action, options.governance);
    const validationError = validateToolAction(tool, normalizedAction, options);
    if (validationError) {
      const result = createBlockedToolResult(normalizedAction, validationError);
      return createExecutionRecord(result, options);
    }

    const beforeHookError = await runValidationHooks(
      tool,
      normalizedAction,
      options,
      this.#validationHooks,
      "before",
    );
    if (beforeHookError) {
      const result = createBlockedToolResult(normalizedAction, beforeHookError);
      return createExecutionRecord(result, options);
    }

    const result = await executeWithRuntimeBudget(
      (signal) => tool.execute(
      {
        ...normalizedAction,
        toolName: tool.descriptor.name,
      },
      {
        ...options,
        ...(signal ? { signal } : {}),
      },
      ),
      resolveResearchToolRuntimeBudgetMs(normalizedAction, options.governance),
      normalizedAction,
      options.signal,
    );
    const outputValidationError = validateToolOutput(tool, result);
    if (outputValidationError) {
      const blocked = createBlockedToolResult(
        result.action,
        outputValidationError,
      );
      return createExecutionRecord(blocked, options);
    }

    const afterHookError = await runValidationHooks(
      tool,
      result.action,
      options,
      this.#validationHooks,
      "after",
      result,
    );
    if (afterHookError) {
      const blocked = createBlockedToolResult(result.action, afterHookError);
      return createExecutionRecord(blocked, options);
    }

    return createExecutionRecord(this.#resultPageStore?.retain(result, options.agentId) ?? result, options);
  }

  async executeToolCall(
    toolCall: Pick<ToolCall, "id" | "name" | "arguments">,
    options: ExecuteToolCallOptions = {},
  ): Promise<ResearchToolExecutionRecord> {
    const action = this.createActionFromToolCall(toolCall, options);
    return this.execute(action, {
      ...options,
      toolCallId: toolCall.id,
    });
  }

  createActionFromToolCall(
    toolCall: Pick<ToolCall, "id" | "name" | "arguments">,
    options: ExecuteToolCallOptions = {},
  ): ResearchToolAction {
    return createToolActionFromCall(toolCall, this.find(toolCall.name), options);
  }

  preflight(
    action: ResearchToolAction,
    options: ExecuteToolCallOptions = {},
  ): ResearchToolExecutionRecord | undefined {
    const tool = this.find(action.toolName);
    if (!tool) {
      return createExecutionRecord(
        createBlockedToolResult(action, `Unknown tool: ${action.toolName}`),
        options,
      );
    }

    const normalizedAction = applyBudgetDefaults(action, options.governance);
    const validationError = validateToolAction(tool, normalizedAction, options);
    return validationError
      ? createExecutionRecord(
          createBlockedToolResult(normalizedAction, validationError),
          options,
        )
      : undefined;
  }

  preflightToolCall(
    toolCall: Pick<ToolCall, "id" | "name" | "arguments">,
    options: ExecuteToolCallOptions = {},
  ): ResearchToolExecutionRecord | undefined {
    const tool = this.find(toolCall.name);
    const action = createToolActionFromCall(toolCall, tool, options);
    return this.preflight(action, {
      ...options,
      toolCallId: toolCall.id,
    });
  }
}

export function createResearchToolRegistry(
  tools: readonly ResearchExecutableTool[] = [],
  options: ResearchToolRegistryOptions = {},
): ResearchToolRegistry {
  return new ResearchToolRegistry(tools, options);
}

export function createToolRequestedEvent(
  action: ResearchToolAction,
  options: ResearchToolExecutionContext = {},
): ResearchEvent {
  return {
    id: createResearchEventId(),
    kind: "tool.requested",
    timestamp: nowIso(),
    payload: {
      toolActionId: action.id,
      toolName: action.toolName,
      actionClass: action.actionClass,
      normalizedInputs: projectToolActionInput(action),
      expectedOutputs: action.expectedOutputs ?? [],
      budgetLimits: action.budget ?? {},
      summary: `Requested ${action.toolName} for ${action.actionClass}.`,
    },
  };
}

export function createToolObservedEvent(
  result: ResearchToolExecutionResult,
  options: ResearchToolExecutionContext = {},
): ResearchEvent {
  const projectedResult = projectToolResult(result);
  const fullResultCharacters = serializedContentCharacters(
    createModelToolResultContent(projectedResult, projectedResult.output, false),
  );
  const modelVisibleResultCharacters = serializedContentCharacters(
    projectModelToolResult(projectedResult).content,
  );
  return {
    id: createResearchEventId(),
    kind: "tool.observed",
    timestamp: nowIso(),
    ...(projectedResult.artifactRefs?.length
      ? { artifactRefs: projectedResult.artifactRefs }
      : {}),
    payload: {
      toolActionId: projectedResult.action.id,
      toolName: projectedResult.action.toolName,
      actionClass: projectedResult.action.actionClass,
      normalizedInputs: projectToolActionInput(projectedResult.action),
      generatedArtifactRefs: projectedResult.artifactRefs ?? [],
      status: projectedResult.status,
      followUpActionsProposed: projectedResult.followUpActions,
      summary: projectedResult.summary,
      fullResultCharacters,
      modelVisibleResultCharacters,
      modelResultCharactersRemoved: Math.max(0, fullResultCharacters - modelVisibleResultCharacters),
      ...(projectedResult.rawOutputRef ? { rawOutputRef: projectedResult.rawOutputRef } : {}),
      ...(projectedResult.error ? { error: projectedResult.error } : {}),
      ...(projectedResult.output !== undefined ? { result: projectedResult.output } : {}),
    },
  };
}

export function createToolResultMessage(
  result: ResearchToolExecutionResult,
  toolCallId: string,
  toolName: string,
): ToolResultMessage {
  const projection = projectModelToolResult(result);
  return {
    role: "toolResult",
    toolCallId,
    toolName,
    timestamp: Date.now(),
    ...projection,
  };
}

/**
 * Builds the bounded, host-safe result projection passed back to a model.
 * Provider adapters must use content for readable evidence and details only
 * for compact audit metadata.
 */
export function projectModelToolResult(
  result: ResearchToolExecutionResult,
): ModelToolResultProjection {
  const projectedResult = projectToolResult(result);
  return {
    content: createModelToolResultContent(
      projectedResult,
      "modelOutput" in projectedResult
        ? projectedResult.modelOutput
        : projectedResult.output,
      true,
    ),
    details: modelToolResultDetails(projectedResult),
    isError: projectedResult.status !== "complete",
  };
}

function createModelToolResultContent(
  result: ResearchToolExecutionResult,
  output: unknown,
  bounded: boolean,
): ToolResultMessage["content"] {
  const serialized = serializeModelToolResult(result, output);
  const text = bounded
    ? truncateModelToolResult(serialized, result.action.toolName, result.pagination)
    : serialized;
  return result.modelContent?.length
    ? [{ type: "text", text }, ...result.modelContent]
    : [{ type: "text", text }];
}

function serializeModelToolResult(result: ResearchToolExecutionResult, output: unknown): string {
  return JSON.stringify({
    status: result.status,
    summary: result.summary,
    output,
    error: result.error,
    followUpActions: result.followUpActions,
  }, null, 2);
}

function serializedContentCharacters(content: ToolResultMessage["content"]): number {
  return JSON.stringify(content).length;
}

export function modelToolResultDetails(
  result: ResearchToolExecutionResult,
): ModelToolResultDetails {
  return {
    status: result.status,
    summary: result.summary,
    ...(result.rawOutputRef ? { rawOutputRef: result.rawOutputRef } : {}),
    ...(result.artifactRefs?.length
      ? { artifactRefs: result.artifactRefs }
      : {}),
    ...(result.followUpActions.length
      ? { followUpActions: result.followUpActions }
      : {}),
    ...(result.error ? { error: result.error } : {}),
  };
}

export function getToolTransportName(tool: ResearchExecutableTool): string {
  return tool.descriptor.transportName ?? tool.descriptor.name;
}

function createToolActionFromCall(
  toolCall: Pick<ToolCall, "id" | "name" | "arguments">,
  tool: ResearchExecutableTool | undefined,
  options: ExecuteToolCallOptions,
): ResearchToolAction {
  const input = isRecord(toolCall.arguments) ? toolCall.arguments : {};
  const requestedClass =
    typeof input.actionClass === "string" ? input.actionClass : input.action;
  const actionClass = normalizeActionClass(
    requestedClass,
    tool,
    options.defaultActionClass,
    options.permittedActionClasses,
  );

  return {
    id: toolCall.id,
    actionClass,
    toolName: tool?.descriptor.name ?? toolCall.name,
    input,
  };
}

function normalizeActionClass(
  value: unknown,
  tool: ResearchExecutableTool | undefined,
  defaultActionClass: ResearchActionClass | undefined,
  permittedActionClasses: readonly ResearchActionClass[] | undefined,
): ResearchActionClass {
  if (isResearchActionClass(value)) {
    return value;
  }

  const firstPermitted = tool?.descriptor.actionClasses.find(
    (actionClass) =>
      !permittedActionClasses || permittedActionClasses.includes(actionClass),
  );
  return firstPermitted ?? defaultActionClass ?? tool?.descriptor.actionClasses[0] ?? "synthesize";
}

function validateToolAction(
  tool: ResearchExecutableTool,
  action: ResearchToolAction,
  options: ExecuteToolCallOptions,
): string | undefined {
  if (!tool.descriptor.actionClasses.includes(action.actionClass)) {
    return `${tool.descriptor.name} does not support action class ${action.actionClass}.`;
  }

  if (
    options.permittedActionClasses &&
    !options.permittedActionClasses.includes(action.actionClass)
  ) {
      return `Action class ${action.actionClass} is not permitted for this loop.`;
  }

  if (options.governance?.deniedActionClasses?.includes(action.actionClass)) {
    return `Action class ${action.actionClass} is denied by governance policy.`;
  }

  if (
    options.governance?.allowedActionClasses &&
    !options.governance.allowedActionClasses.includes(action.actionClass)
  ) {
    return `Action class ${action.actionClass} is not allowed by governance policy.`;
  }

  const sideEffectError = validateSideEffects(
    tool.descriptor.sideEffects,
    options.governance,
  );
  if (sideEffectError) {
    return sideEffectError;
  }

  const permissionError = validatePermissions(
    tool.descriptor.requiredPermissions,
    options.governance,
  );
  if (permissionError) {
    return permissionError;
  }

  const callBudgetError = validateToolCallBudget(options);
  if (callBudgetError) {
    return callBudgetError;
  }

  const fileBudgetError = validateFileBudgets(action, options.governance);
  if (fileBudgetError) {
    return fileBudgetError;
  }

  const excludedPathError = validateExcludedPaths(action, options.excludedPaths);
  if (excludedPathError) {
    return excludedPathError;
  }

  const schemaErrors = validateJsonSchema(action.input, tool.descriptor.inputSchema);
  if (schemaErrors.length > 0) {
    return `Tool input failed schema validation: ${schemaErrors.join("; ")}`;
  }

  return undefined;
}

function validateToolOutput(
  tool: ResearchExecutableTool,
  result: ResearchToolExecutionResult,
): string | undefined {
  if (!tool.descriptor.outputSchema || result.status !== "complete") {
    return undefined;
  }

  const schemaErrors = validateJsonSchema(result.output, tool.descriptor.outputSchema);
  if (schemaErrors.length > 0) {
    return `Tool output failed schema validation: ${schemaErrors.join("; ")}`;
  }

  return undefined;
}

function validateSideEffects(
  sideEffect: ResearchToolSideEffect,
  governance: ResearchGovernancePolicy | undefined,
): string | undefined {
  if (governance?.deniedSideEffects?.includes(sideEffect)) {
    return `Tool side effect ${sideEffect} is denied by governance policy.`;
  }

  if (
    governance?.allowedSideEffects &&
    !governance.allowedSideEffects.includes(sideEffect)
  ) {
    return `Tool side effect ${sideEffect} is not allowed by governance policy.`;
  }

  return undefined;
}

function validatePermissions(
  requiredPermissions: readonly string[],
  governance: ResearchGovernancePolicy | undefined,
): string | undefined {
  const deniedPermission = requiredPermissions.find((permission) =>
    governance?.deniedPermissions?.includes(permission),
  );
  if (deniedPermission) {
    return `Tool permission ${deniedPermission} is denied by governance policy.`;
  }

  const missingAllowedPermission = requiredPermissions.find(
    (permission) =>
      governance?.allowedPermissions &&
      !governance.allowedPermissions.includes(permission),
  );
  if (missingAllowedPermission) {
    return `Tool permission ${missingAllowedPermission} is not allowed by governance policy.`;
  }

  return undefined;
}

function validateToolCallBudget(
  options: ExecuteToolCallOptions,
): string | undefined {
  const maxToolCalls = options.governance?.maxToolCalls;
  if (
    typeof maxToolCalls === "number" &&
    typeof options.toolCallCount === "number" &&
    options.toolCallCount >= maxToolCalls
  ) {
    return `Tool call budget exhausted: ${options.toolCallCount}/${maxToolCalls} call(s) already used.`;
  }

  return undefined;
}

function validateFileBudgets(
  action: ResearchToolAction,
  governance: ResearchGovernancePolicy | undefined,
): string | undefined {
  if (!governance) {
    return undefined;
  }

  const hasPathInput = typeof action.input.path === "string";
  if (
    hasPathInput &&
    typeof governance.maxFiles === "number" &&
    governance.maxFiles < 1
  ) {
    return `File budget exhausted: action requires 1 file but maxFiles is ${governance.maxFiles}.`;
  }

  const requestedBytes = readNumericInput(action.input, "maxBytes");
  if (
    typeof requestedBytes === "number" &&
    typeof governance.maxBytes === "number" &&
    requestedBytes > governance.maxBytes
  ) {
    return `Byte budget exceeded: requested ${requestedBytes} byte(s), maxBytes is ${governance.maxBytes}.`;
  }

  const requestedTokens = readNumericInput(action.input, "maxTokens");
  if (
    typeof requestedTokens === "number" &&
    typeof governance.maxTokens === "number" &&
    requestedTokens > governance.maxTokens
  ) {
    return `Token budget exceeded: requested ${requestedTokens} token(s), maxTokens is ${governance.maxTokens}.`;
  }

  return undefined;
}

function validateExcludedPaths(
  action: ResearchToolAction,
  excludedPaths: readonly string[] | undefined,
): string | undefined {
  if (!excludedPaths || excludedPaths.length === 0) {
    return undefined;
  }
  if (typeof action.input.path !== "string") {
    return undefined;
  }

  const requestedPath = normalizeComparablePath(action.input.path);
  const excludedPath = excludedPaths.find((path) =>
    pathsMatch(requestedPath, normalizeComparablePath(path)),
  );
  if (!excludedPath) {
    return undefined;
  }

  return `Path ${action.input.path} is listed in avoid_repeated_targets for this fresh loop; choose a different source path unless the user explicitly asks to revisit it.`;
}

function pathsMatch(left: string, right: string): boolean {
  return (
    left === right ||
    left.endsWith(`/${right}`) ||
    right.endsWith(`/${left}`)
  );
}

function normalizeComparablePath(path: string): string {
  return path.trim().replace(/\\/g, "/").replace(/^\.\/+/u, "").replace(/\/+$/u, "");
}

function applyBudgetDefaults(
  action: ResearchToolAction,
  governance: ResearchGovernancePolicy | undefined,
): ResearchToolAction {
  if (
    !governance ||
    typeof governance.maxBytes !== "number" ||
    typeof action.input.path !== "string" ||
    typeof action.input.maxBytes === "number"
  ) {
    return action;
  }

  return {
    ...action,
    input: {
      ...action.input,
      maxBytes: governance.maxBytes,
    },
  };
}

export function resolveResearchToolRuntimeBudgetMs(
  action: ResearchToolAction,
  governance: ResearchGovernancePolicy | undefined,
): number {
  const actionBudget = action.budget?.maxRuntimeMs;
  if (actionBudget !== undefined) return actionBudget;
  // A human Auto-Review override can wait for the researcher without consuming
  // the command's own execution timeout. Shell execution is bounded separately.
  if (action.toolName === "shell.run") return governance?.maxRuntimeMs ?? 0;
  // A runbook is a container of independently bounded cells. Applying the
  // general per-tool governance deadline to the container can abandon a cell
  // before its declared executor timeout and leave external work in flight.
  if (action.toolName === "runbook.run") return 0;
  return governance?.maxRuntimeMs
    ?? getMcpCallRuntimeBudgetMs(action)
    ?? TOOL_RUNTIME_BUDGET_MS_BY_TOOL.get(action.toolName)
    ?? DEFAULT_TOOL_RUNTIME_BUDGET_MS;
}

function getMcpCallRuntimeBudgetMs(action: ResearchToolAction): number | undefined {
  if (!action.toolName.startsWith("mcp.")) return undefined;
  const requestedTimeout = action.input[MCP_CALL_TIMEOUT_INPUT_KEY];
  if (
    typeof requestedTimeout !== "number"
    || !Number.isInteger(requestedTimeout)
    || requestedTimeout < MIN_MCP_CALL_TIMEOUT_MS
    || requestedTimeout > MAX_MCP_CALL_TIMEOUT_MS
  ) {
    return undefined;
  }
  return requestedTimeout + 1_000;
}

async function executeWithRuntimeBudget(
  execute: (signal?: AbortSignal) => Promise<ResearchToolExecutionResult>,
  timeoutMs: number | undefined,
  action: ResearchToolAction,
  outerSignal?: AbortSignal,
): Promise<ResearchToolExecutionResult> {
  if (!timeoutMs || timeoutMs <= 0) {
    return execute(outerSignal);
  }

  const controller = new AbortController();
  const signal = outerSignal
    ? AbortSignal.any([outerSignal, controller.signal])
    : controller.signal;
  let timeout: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      execute(signal),
      new Promise<ResearchToolExecutionResult>((resolve) => {
        timeout = setTimeout(() => {
          controller.abort(new Error(`Tool runtime budget exceeded after ${timeoutMs}ms.`));
          resolve(
            createBlockedToolResult(
              action,
              `Tool runtime budget exceeded after ${timeoutMs}ms.`,
            ),
          );
        }, timeoutMs);
      }),
    ]);
  } finally {
    if (timeout) {
      clearTimeout(timeout);
    }
  }
}

async function runValidationHooks(
  tool: ResearchExecutableTool,
  action: ResearchToolAction,
  options: ExecuteToolCallOptions,
  hooks: ReadonlyMap<string, ResearchToolValidationHook>,
  phase: "before" | "after",
  result?: ResearchToolExecutionResult,
): Promise<string | undefined> {
  for (const hookName of tool.descriptor.validationHooks ?? []) {
    const hook = hooks.get(hookName);
    if (!hook) {
      return `Validation hook ${hookName} is not registered.`;
    }

    const message = await hook({
      phase,
      tool,
      action,
      context: options,
      ...(result ? { result } : {}),
    });
    if (typeof message === "string" && message.trim().length > 0) {
      return message.trim();
    }
  }

  return undefined;
}

function readValidationHookEntries(
  hooks: ResearchToolRegistryOptions["validationHooks"],
): [string, ResearchToolValidationHook][] {
  if (!hooks) {
    return [];
  }

  if (hooks instanceof Map) {
    return [...hooks.entries()];
  }

  return Object.entries(hooks);
}

function validateJsonSchema(value: unknown, schema: unknown): string[] {
  if (!schema || !isRecord(schema)) {
    return [];
  }

  return validateJsonSchemaAt(value, schema, "$");
}

function validateJsonSchemaAt(
  value: unknown,
  schema: Record<string, unknown>,
  path: string,
): string[] {
  const errors: string[] = [];

  if (Array.isArray(schema.anyOf)) {
    const branchErrors = schema.anyOf.map((branch) =>
      isRecord(branch) ? validateJsonSchemaAt(value, branch, path) : [`${path} has invalid schema branch`],
    );
    if (branchErrors.some((branch) => branch.length === 0)) {
      return [];
    }

    return [
      `${path} did not match any allowed schema (${branchErrors
        .map((branch) => branch.join(", "))
        .join(" | ")})`,
    ];
  }

  if ("const" in schema && value !== schema.const) {
    errors.push(`${path} must equal ${JSON.stringify(schema.const)}`);
  }

  const type = typeof schema.type === "string" ? schema.type : undefined;
  if (type) {
    const typeError = validateJsonSchemaType(value, type, path);
    if (typeError) {
      errors.push(typeError);
      return errors;
    }
  }

  if (type === "object" && isRecord(value)) {
    const required = Array.isArray(schema.required)
      ? schema.required.filter((item): item is string => typeof item === "string")
      : [];
    for (const key of required) {
      if (!(key in value)) {
        errors.push(`${path}.${key} is required`);
      }
    }

    if (isRecord(schema.properties)) {
      for (const [key, propertySchema] of Object.entries(schema.properties)) {
        if (key in value && isRecord(propertySchema)) {
          errors.push(
            ...validateJsonSchemaAt(value[key], propertySchema, `${path}.${key}`),
          );
        }
      }
    }
  }

  if (type === "array" && Array.isArray(value) && isRecord(schema.items)) {
    value.forEach((item, index) => {
      errors.push(...validateJsonSchemaAt(item, schema.items as Record<string, unknown>, `${path}[${index}]`));
    });
  }

  return errors;
}

function validateJsonSchemaType(
  value: unknown,
  type: string,
  path: string,
): string | undefined {
  if (type === "string" && typeof value !== "string") {
    return `${path} must be a string`;
  }
  if (type === "number" && typeof value !== "number") {
    return `${path} must be a number`;
  }
  if (type === "integer" && !Number.isInteger(value)) {
    return `${path} must be an integer`;
  }
  if (type === "boolean" && typeof value !== "boolean") {
    return `${path} must be a boolean`;
  }
  if (type === "object" && !isRecord(value)) {
    return `${path} must be an object`;
  }
  if (type === "array" && !Array.isArray(value)) {
    return `${path} must be an array`;
  }
  if (type === "null" && value !== null) {
    return `${path} must be null`;
  }

  return undefined;
}

function readNumericInput(
  input: Record<string, unknown>,
  key: string,
): number | undefined {
  const value = input[key];
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function createBlockedToolResult(
  action: ResearchToolAction,
  reason: string,
): ResearchToolExecutionResult {
  const timestamp = nowIso();
  return {
    action,
    status: "blocked",
    startedAt: timestamp,
    completedAt: timestamp,
    summary: reason,
    followUpActions: ["Report the blocked tool action before continuing."],
    error: {
      message: reason,
    },
  };
}

function createExecutionRecord(
  result: ResearchToolExecutionResult,
  options: ResearchToolExecutionContext,
): ResearchToolExecutionRecord {
  const projectedResult = projectToolResult(result);
  return {
    action: projectedResult.action,
    result: projectedResult,
    events: [
      createToolRequestedEvent(projectedResult.action, options),
      createToolObservedEvent(projectedResult, options),
    ],
  };
}

function projectToolResult(
  result: ResearchToolExecutionResult,
): ResearchToolExecutionResult {
  if (!isShellToolName(result.action.toolName)) return result;
  const action = projectToolAction(result.action);
  return {
    ...result,
    action,
    ...(result.output === undefined
      ? {}
      : { output: projectShellToolOutput(result.output) }),
  };
}

function projectToolAction(action: ResearchToolAction): ResearchToolAction {
  return isShellToolName(action.toolName)
    ? { ...action, input: sanitizeShellActionInput(action.input) }
    : action;
}

function projectToolActionInput(action: ResearchToolAction): Record<string, unknown> {
  return isShellToolName(action.toolName)
    ? sanitizeShellActionInput(action.input)
    : action.input;
}

function projectShellToolOutput(output: unknown): unknown {
  if (!isRecord(output)) return output;
  const projected = { ...output };
  delete projected.stdin;
  if (Array.isArray(projected.args) && projected.args.every((value) => typeof value === "string")) {
    projected.args = redactShellArguments(projected.args);
  }
  return projected;
}

function modelToolResultMaxCharacters(toolName: string): number {
  return MODEL_TOOL_RESULT_MAX_CHARS_BY_TOOL.get(toolName) ?? DEFAULT_MODEL_TOOL_RESULT_MAX_CHARS;
}

function truncateModelToolResult(
  text: string,
  toolName: string,
  pagination?: ResearchToolExecutionResult["pagination"],
): string {
  const maxCharacters = modelToolResultMaxCharacters(toolName);
  if (text.length <= maxCharacters) return text;
  const half = Math.floor(maxCharacters / 2);
  return [
    text.slice(0, half),
    pagination
      ? `\n\n[Tool result truncated for model context: ${pagination.omittedEnd - pagination.omittedStart} characters omitted. Call tool_result_page with toolActionId=${JSON.stringify(pagination.toolActionId)} and offset=${pagination.omittedStart} to read the omitted text in bounded pages; continue with nextOffset.]\n\n`
      : `\n\n[Tool result truncated for model context: ${text.length - maxCharacters} characters omitted. Re-run a narrower command if the omitted section is needed.]\n\n`,
    text.slice(-half),
  ].join("");
}

function isShellToolName(toolName: string): boolean {
  return toolName === "shell.run" || toolName === "shell_run";
}

function isResearchActionClass(value: unknown): value is ResearchActionClass {
  return (
    value === "recall" ||
    value === "search" ||
    value === "inspect" ||
    value === "analyze" ||
    value === "experiment" ||
    value === "synthesize" ||
    value === "ask_user" ||
    value === "respond" ||
    value === "stop"
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
