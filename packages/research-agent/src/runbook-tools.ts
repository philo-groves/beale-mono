import { createHash } from "node:crypto";
import { nowIso } from "./ids.js";
import {
  RunbookStore,
  type RunbookCellInput,
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
      purpose: { type: "string", description: "The reusable research procedure, proof objective, or decision this runbook preserves." },
      cells: { type: "array", maxItems: 20, items: cellParameters },
    },
  };
  const appendParameters = {
    type: "object",
    required: ["id", "expectedRevision", "cells"],
    properties: {
      id: { type: "string" },
      expectedRevision: { type: "number" },
      cells: { type: "array", minItems: 1, maxItems: 20, items: cellParameters },
    },
  };
  return [
    tool(
      "runbook.list",
      "runbook_list",
      "List workspace runbooks before creating or repeating a reusable procedure.",
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
      "Read a bounded page of one workspace runbook, including recorded code cells, results, and execution.latestSuccessfulRunId for finding promotion.",
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
      "Create a revisioned Jupyter-format research runbook for a reusable procedure, proof sequence, or environment-specific workflow. A healthy runbook records prerequisites and expected evidence in markdown, then uses bounded repeatable code cells with an explicit supported language.",
      "write",
      createParameters,
      (input, context) => {
        const created = store.create({
          title: requiredText(input.title, "title"),
          purpose: requiredText(input.purpose, "purpose"),
          ...(Array.isArray(input.cells) ? { cells: input.cells.map(parseCell) } : {}),
        }, context?.modelAuthor);
        return { output: created.runbook, artifactRefs: [created.artifactRef] };
      },
    ),
    tool(
      "runbook.append",
      "runbook_append",
      "Append concise markdown or code cells to an existing runbook using its current revision. Put proof commands in explicitly typed code cells and execute them with runbook.run; Auto-Review denies proofing issued directly through shell.run.",
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
  ];
}

function createCellParameters(platform: NodeJS.Platform): Record<string, unknown> {
  const supportedRunners = platform === "win32"
    ? "sh, bash, zsh, python, python3, javascript, node, ruby, perl, and pwsh"
    : "sh, bash, zsh, python, python3, javascript, node, ruby, and perl";
  return {
    type: "object",
    required: ["kind", "source"],
    properties: {
      kind: { type: "string", enum: ["markdown", "code"] },
      source: { type: "string", description: "Markdown prose or the exact executable code/command sequence." },
      language: { type: "string", description: `Required for executable code cells. Supported runners: ${supportedRunners}.` },
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
    ...(text(input.language) ? { language: text(input.language)! } : {}),
    ...(text(input.summary) ? { summary: text(input.summary)! } : {}),
    ...(typeof input.stdout === "string" ? { stdout: input.stdout } : {}),
    ...(typeof input.stderr === "string" ? { stderr: input.stderr } : {}),
    ...(typeof input.exitCode === "number" ? { exitCode: input.exitCode } : {}),
  };
}

function isRecord(value: unknown): value is Record<string, unknown> { return typeof value === "object" && value !== null && !Array.isArray(value); }
function text(value: unknown): string | null { return typeof value === "string" && value.trim() ? value.trim() : null; }
function requiredText(value: unknown, field: string, allowEmpty = false): string { if (typeof value !== "string" || (!allowEmpty && !value.trim())) throw new Error(`${field} must be a ${allowEmpty ? "string" : "non-empty string"}.`); return allowEmpty ? value : value.trim(); }
function requiredInteger(value: unknown, field: string): number { if (typeof value !== "number" || !Number.isSafeInteger(value)) throw new Error(`${field} must be an integer.`); return value; }
function requiredArray(value: unknown, field: string): unknown[] { if (!Array.isArray(value)) throw new Error(`${field} must be an array.`); return value; }
function requiredRecord(value: unknown, field: string): Record<string, unknown> { if (!isRecord(value)) throw new Error(`${field} must be an object.`); return value; }
