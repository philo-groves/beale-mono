import { createHash } from "node:crypto";
import { nowIso } from "./ids.js";
import {
  MAX_RUNBOOK_MUTATION_CELLS,
  RUNBOOK_DEFAULT_TIMEOUT_SECONDS,
  RUNBOOK_MAX_TIMEOUT_SECONDS,
  RUNBOOK_DEFAULT_FEATURES,
  RunbookStore,
  type RunbookCellInput,
  type RunbookCellExecutor,
} from "./runbooks.js";
import type { ResearchExecutableTool, ResearchToolExecutionContext, ResearchToolExecutionResult } from "./tool-registry.js";
import type { ResearchArtifactRef, ResearchToolAction } from "./types.js";

const LIST_PARAMETERS = {
  type: "object",
  properties: {
    query: { type: "string" },
    limit: { type: "number" },
    afterRevision: { type: "string" },
  },
};

const GET_PARAMETERS = {
  type: "object",
  required: ["id"],
  properties: {
    id: { type: "string" },
    offset: { type: "number" },
    limit: { type: "number" },
  },
};

const FEATURE_PARAMETERS = {
  type: "array",
  maxItems: 32,
  items: { type: "string", maxLength: 64 },
};

export interface RunbookToolOptions {
  platform?: NodeJS.Platform;
}

export function createRunbookTools(
  store: RunbookStore,
  options: RunbookToolOptions = {},
): ResearchExecutableTool[] {
  const cellParameters = createCellParameters(options.platform ?? process.platform);
  const createParameters = {
    type: "object",
    required: ["title", "purpose"],
    properties: {
      title: { type: "string" },
      purpose: { type: "string", description: "The cohesive reusable workflow, proof objective, or decision this runbook preserves." },
      enabledFeatures: { ...FEATURE_PARAMETERS, description: "Enabled feature text tags. Defaults to setup, runtime, and cleanup." },
      cells: { type: "array", maxItems: MAX_RUNBOOK_MUTATION_CELLS, items: cellParameters },
    },
  };
  const appendParameters = {
    type: "object",
    required: ["id", "expectedRevision", "cells"],
    properties: {
      id: { type: "string" },
      expectedRevision: { type: "number" },
      cells: { type: "array", minItems: 1, maxItems: MAX_RUNBOOK_MUTATION_CELLS, items: cellParameters },
    },
  };
  const configureParameters = {
    type: "object",
    required: ["id", "expectedRevision", "enabledFeatures"],
    properties: {
      id: { type: "string" },
      expectedRevision: { type: "number" },
      enabledFeatures: { ...FEATURE_PARAMETERS, description: "Complete list of enabled runbook feature tags. An empty list deactivates every cell." },
      cellFeatures: {
        type: "array",
        maxItems: MAX_RUNBOOK_MUTATION_CELLS,
        items: {
          type: "object",
          required: ["cellId", "features"],
          properties: {
            cellId: { type: "string" },
            features: { ...FEATURE_PARAMETERS, minItems: 1, description: "Cell tags; must include setup, runtime, or cleanup." },
          },
        },
      },
      cellExecutors: {
        type: "array",
        maxItems: MAX_RUNBOOK_MUTATION_CELLS,
        items: {
          type: "object",
          required: ["cellId", "executor"],
          properties: {
            cellId: { type: "string" },
            executor: createCellExecutorParameters(),
          },
        },
      },
    },
  };
  return [
    tool(
      "runbook.list",
      "runbook_list",
      "List the bounded workspace runbook catalog when complete runbook records are needed. Use history.search for normal workspace-history search before creating or repeating a reusable procedure.",
      "read",
      LIST_PARAMETERS,
      (input) => {
        const runbooks = store.list({
          ...(text(input.query) ? { query: text(input.query)! } : {}),
          ...(typeof input.limit === "number" ? { limit: input.limit } : {}),
        });
        const revision = createHash("sha256")
          .update(JSON.stringify({ query: text(input.query), limit: input.limit ?? null }))
          .update("\n")
          .update(runbooks.map((runbook) => `${runbook.id}:${runbook.revision}:${runbook.updatedAt}`).join("\n"))
          .digest("hex")
          .slice(0, 16);
        return { output: text(input.afterRevision) === revision
          ? { revision, unchanged: true, total: runbooks.length, runbooks: [] }
          : { revision, unchanged: false, total: runbooks.length, runbooks } };
      },
    ),
    tool(
      "runbook.get",
      "runbook_get",
      "Read a bounded page of one workspace runbook, including enabled feature toggles, cell feature tags, active state, host or Tart VM cell executors, recorded results, and execution.latestSuccessfulRunId for finding promotion.",
      "read",
      GET_PARAMETERS,
      (input) => ({
        output: store.get(requiredText(input.id, "id"), {
          ...(typeof input.offset === "number" ? { offset: input.offset } : {}),
          limit: typeof input.limit === "number" ? input.limit : 12,
        }),
      }),
    ),
    tool(
      "runbook.create",
      "runbook_create",
      "Create a revisioned Jupyter-format research runbook only when the workflow is not already represented. Keep setup, runtime, and cleanup in one cohesive runbook and label every cell with at least one of those default phase features; add narrower text tags when useful. Split only for a genuinely unrelated objective, target, or authorization boundary, not for phase changes, prerequisites, target state, review, or cleanup. Record prerequisites and expected evidence in markdown, then use repeatable code cells. Host cells require an explicit supported language and accept executor.timeoutSeconds for bounded long-running collectors instead of inheriting the shell default. A tart-vm cell references a host-built executable by workspacePath or artifactId; runbook.run materializes, stages, executes, records, and cleans it without a guest-side rewrite. Keep iterative implementation in a stable candidate artifact so the same entry cell can be rerun without per-tweak append churn.",
      "write",
      createParameters,
      (input, context) => {
        const created = store.create({
          title: requiredText(input.title, "title"),
          purpose: requiredText(input.purpose, "purpose"),
          ...(Array.isArray(input.enabledFeatures) ? { enabledFeatures: parseFeatures(input.enabledFeatures, "enabledFeatures", false) } : {}),
          ...(Array.isArray(input.cells) ? { cells: input.cells.map(parseCell) } : {}),
        }, context?.modelAuthor);
        return { output: created.runbook, artifactRefs: [created.artifactRef] };
      },
    ),
    tool(
      "runbook.append",
      "runbook_append",
      "Append concise markdown or code cells to the existing cohesive workflow using its current revision. Use setup, runtime, and cleanup feature tags to keep phases, prerequisites, target states, review steps, and cleanup in that runbook. Start another runbook only for a genuinely unrelated objective, target, or authorization boundary. Failed outputs already preserve attempt history: rerun an unchanged entry cell after editing its candidate artifact instead of appending one cell per tweak.",
      "write",
      appendParameters,
      (input, context) => {
        const appended = store.append({
          id: requiredText(input.id, "id"),
          expectedRevision: requiredInteger(input.expectedRevision, "expectedRevision"),
          cells: requiredArray(input.cells, "cells").map(parseCell),
        }, context?.modelAuthor);
        return { output: appended.runbook, artifactRefs: [appended.artifactRef] };
      },
    ),
    tool(
      "runbook.configure",
      "runbook_configure",
      "Manage a runbook's enabled feature text tags, optionally relabel cells, and retarget code cells between host and Tart VM execution. A cell runs when at least one of its tags is enabled. Every cell must retain a setup, runtime, or cleanup phase tag; custom tags can be added for narrower activation. Tart VM execution references an existing host artifact or workspace-relative executable instead of duplicating the runbook as guest code, and runAs selects the Guest Agent service identity or passwordless root execution.",
      "write",
      configureParameters,
      (input, context) => {
        const configured = store.configure({
          id: requiredText(input.id, "id"),
          expectedRevision: requiredInteger(input.expectedRevision, "expectedRevision"),
          enabledFeatures: parseFeatures(input.enabledFeatures, "enabledFeatures", false),
          ...(Array.isArray(input.cellFeatures) ? {
            cellFeatures: input.cellFeatures.map((value, index) => {
              const change = requiredRecord(value, `cellFeatures[${index}]`);
              return {
                cellId: requiredText(change.cellId, `cellFeatures[${index}].cellId`),
                features: parseFeatures(change.features, `cellFeatures[${index}].features`, true),
              };
            }),
          } : {}),
          ...(Array.isArray(input.cellExecutors) ? {
            cellExecutors: input.cellExecutors.map((value, index) => {
              const change = requiredRecord(value, `cellExecutors[${index}]`);
              return {
                cellId: requiredText(change.cellId, `cellExecutors[${index}].cellId`),
                executor: parseCellExecutor(change.executor, `cellExecutors[${index}].executor`),
              };
            }),
          } : {}),
        }, context?.modelAuthor);
        return { output: configured.runbook, artifactRefs: [configured.artifactRef] };
      },
    ),
  ];
}

function createCellParameters(platform: NodeJS.Platform): Record<string, unknown> {
  const supportedRunners = platform === "win32"
    ? "sh, bash, zsh, python, python3, javascript, node, ruby, perl, and pwsh"
    : "sh, bash, zsh, python, python3, javascript, node, ruby, and perl";
  return {
    type: "object",
    required: ["kind", "source", "features"],
    properties: {
      kind: { type: "string", enum: ["markdown", "code"] },
      source: { type: "string", description: "Markdown prose, the exact host code/command sequence, or a concise Tart VM entry description when executor.kind is tart-vm." },
      features: { ...FEATURE_PARAMETERS, minItems: 1, description: `Feature tags controlling activation. Include at least one phase tag: ${RUNBOOK_DEFAULT_FEATURES.join(", ")}.` },
      executor: createCellExecutorParameters(),
      language: { type: "string", description: `Required for host code cells and ignored by tart-vm cells. Supported host runners: ${supportedRunners}.` },
      summary: { type: "string", description: "Concise purpose, expected evidence, or interpretation of this cell." },
      stdout: { type: "string", description: "Bounded observed stdout when preserving a meaningful execution result." },
      stderr: { type: "string", description: "Bounded observed stderr when preserving a meaningful execution result." },
      exitCode: { type: "number", description: "Observed process exit code, if this cell records an execution." },
    },
  };
}

function tool(
  name: string,
  transportName: string,
  description: string,
  sideEffects: "read" | "write",
  parameters: Record<string, unknown>,
  run: (input: Record<string, unknown>, context?: ResearchToolExecutionContext) => { output: unknown; artifactRefs?: ResearchArtifactRef[] },
): ResearchExecutableTool {
  return {
    descriptor: {
      name,
      transportName,
      description,
      actionClasses: [sideEffects === "read" ? "recall" : "synthesize"],
      sideEffects,
      requiredPermissions: [sideEffects === "read" ? "artifact:read" : "artifact:write"],
      inputSchema: parameters,
      metadata: { family: "runbook", format: "jupyter-nbformat-4" },
    },
    parameters: parameters as NonNullable<ResearchExecutableTool["parameters"]>,
    async execute(action: ResearchToolAction, context?: ResearchToolExecutionContext): Promise<ResearchToolExecutionResult> {
      const startedAt = nowIso();
      try {
        const result = run(isRecord(action.input) ? action.input : {}, context);
        return {
          action,
          status: "complete",
          startedAt,
          completedAt: nowIso(),
          summary: `${name} completed.`,
          output: result.output,
          ...(result.artifactRefs?.length ? { artifactRefs: result.artifactRefs } : {}),
          followUpActions: [],
        };
      } catch (error) {
        return {
          action,
          status: "error",
          startedAt,
          completedAt: nowIso(),
          summary: `${name} failed.`,
          error: { message: error instanceof Error ? error.message : String(error) },
          followUpActions: [],
        };
      }
    },
  };
}

function parseCell(value: unknown): RunbookCellInput {
  const input = requiredRecord(value, "cell");
  const kind = requiredText(input.kind, "cell kind");
  if (kind !== "markdown" && kind !== "code") throw new Error("cell kind must be markdown or code.");
  return {
    kind,
    source: requiredText(input.source, "cell source", true),
    features: parseFeatures(input.features, "cell features", true),
    ...(input.executor === undefined ? {} : { executor: parseCellExecutor(input.executor, "cell executor") }),
    ...(text(input.language) ? { language: text(input.language)! } : {}),
    ...(text(input.summary) ? { summary: text(input.summary)! } : {}),
    ...(typeof input.stdout === "string" ? { stdout: input.stdout } : {}),
    ...(typeof input.stderr === "string" ? { stderr: input.stderr } : {}),
    ...(typeof input.exitCode === "number" ? { exitCode: input.exitCode } : {}),
  };
}

function createCellExecutorParameters(): Record<string, unknown> {
  return {
    type: "object",
    required: ["kind"],
    properties: {
      kind: { type: "string", enum: ["host", "tart-vm"] },
      vmName: { type: "string", description: "Named running Tart VM. Required for tart-vm." },
      artifactId: { type: "string", description: "Durable host artifact containing the guest executable. Mutually exclusive with workspacePath." },
      workspacePath: { type: "string", description: "Workspace-relative host build output to materialize and stage. Mutually exclusive with artifactId." },
      runAs: { type: "string", enum: ["guest", "root"], default: "guest", description: "Execute as the Guest Agent service identity or through passwordless sudo as root." },
      argv: { type: "array", maxItems: 128, items: { type: "string" }, description: "Arguments passed after the staged guest executable." },
      timeoutSeconds: {
        type: "integer",
        minimum: 1,
        maximum: RUNBOOK_MAX_TIMEOUT_SECONDS,
        default: RUNBOOK_DEFAULT_TIMEOUT_SECONDS,
        description: "Cell execution timeout. Defaults to 300 seconds and may be raised to 1800 seconds for a bounded long-running proof or collector.",
      },
      retainOnFailure: { type: "boolean", default: false, description: "Keep the staged executable only after a failed run for bounded debugging." },
    },
  };
}

function parseCellExecutor(value: unknown, field: string): RunbookCellExecutor {
  const executor = requiredRecord(value, field);
  const kind = requiredText(executor.kind, `${field}.kind`);
  if (kind === "host") {
    const timeoutSeconds = executor.timeoutSeconds === undefined
      ? RUNBOOK_DEFAULT_TIMEOUT_SECONDS
      : requiredInteger(executor.timeoutSeconds, `${field}.timeoutSeconds`);
    if (timeoutSeconds < 1 || timeoutSeconds > RUNBOOK_MAX_TIMEOUT_SECONDS) {
      throw new Error(`${field}.timeoutSeconds must be an integer from 1 to ${RUNBOOK_MAX_TIMEOUT_SECONDS}.`);
    }
    return { kind: "host", timeoutSeconds };
  }
  if (kind !== "tart-vm") throw new Error(`${field}.kind must be host or tart-vm.`);
  const artifactId = text(executor.artifactId);
  const workspacePath = text(executor.workspacePath);
  if (Boolean(artifactId) === Boolean(workspacePath)) throw new Error(`${field} requires exactly one of artifactId or workspacePath.`);
  const argv = executor.argv === undefined ? [] : requiredArray(executor.argv, `${field}.argv`);
  if (!argv.every((argument) => typeof argument === "string")) throw new Error(`${field}.argv must contain strings.`);
  const runAs = executor.runAs === undefined ? "guest" : requiredText(executor.runAs, `${field}.runAs`);
  if (runAs !== "guest" && runAs !== "root") throw new Error(`${field}.runAs must be guest or root.`);
  return {
    kind: "tart-vm",
    vmName: requiredText(executor.vmName, `${field}.vmName`),
    ...(artifactId ? { artifactId } : {}),
    ...(workspacePath ? { workspacePath } : {}),
    runAs,
    argv: argv as string[],
    timeoutSeconds: executor.timeoutSeconds === undefined ? RUNBOOK_DEFAULT_TIMEOUT_SECONDS : requiredInteger(executor.timeoutSeconds, `${field}.timeoutSeconds`),
    retainOnFailure: executor.retainOnFailure === true,
  };
}

function parseFeatures(value: unknown, field: string, requirePhase: boolean): string[] {
  if (!Array.isArray(value)) throw new Error(`${field} must be an array of text tags.`);
  const features = [...new Set(value.map((feature, index) => requiredText(feature, `${field}[${index}]`).toLowerCase()))];
  if (requirePhase && !features.some((feature) => (RUNBOOK_DEFAULT_FEATURES as readonly string[]).includes(feature))) {
    throw new Error(`${field} must include setup, runtime, or cleanup.`);
  }
  return features;
}

function isRecord(value: unknown): value is Record<string, unknown> { return typeof value === "object" && value !== null && !Array.isArray(value); }
function text(value: unknown): string | null { return typeof value === "string" && value.trim() ? value.trim() : null; }
function requiredText(value: unknown, field: string, allowEmpty = false): string { if (typeof value !== "string" || (!allowEmpty && !value.trim())) throw new Error(`${field} must be a ${allowEmpty ? "string" : "non-empty string"}.`); return allowEmpty ? value : value.trim(); }
function requiredInteger(value: unknown, field: string): number { if (typeof value !== "number" || !Number.isSafeInteger(value)) throw new Error(`${field} must be an integer.`); return value; }
function requiredArray(value: unknown, field: string): unknown[] { if (!Array.isArray(value)) throw new Error(`${field} must be an array.`); return value; }
function requiredRecord(value: unknown, field: string): Record<string, unknown> { if (!isRecord(value)) throw new Error(`${field} must be an object.`); return value; }
