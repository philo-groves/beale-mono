import { createHash } from "node:crypto";
import { nowIso } from "./ids.js";
import { workspacePathProblem } from "./workspace-project.js";
import {
  MAX_RUNBOOK_MUTATION_CELLS,
  RUNBOOK_DEFAULT_TIMEOUT_SECONDS,
  RUNBOOK_MAX_TIMEOUT_SECONDS,
  RUNBOOK_DEFAULT_FEATURES,
  RunbookStore,
  RunbookTitleConflictError,
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
    runId: { type: "string", description: "Read this immutable execution snapshot and recorded cell results instead of the current notebook. Use the runId referenced by a claim when reviewing evidence." },
    offset: { type: "number", description: "Zero-based cell offset for pagination. Do not combine with cell range fields." },
    limit: { type: "number", description: "Maximum cells to return. Use nextOffset from the response to continue." },
    startCellId: { type: "string", description: "Read beginning at this cell, inclusive." },
    endCellId: { type: "string", description: "Read through this cell, inclusive." },
  },
};

export interface RunbookReadWorkspaceReference {
  workspaceId: string;
  workspaceName: string;
}

const FEATURE_PARAMETERS = {
  type: "array",
  maxItems: 32,
  items: { type: "string", maxLength: 64 },
};

export interface RunbookToolOptions {
  platform?: NodeJS.Platform;
  referenceWorkspaces?: readonly RunbookReadWorkspaceReference[];
}

export function createRunbookTools(
  store: RunbookStore,
  options: RunbookToolOptions = {},
): ResearchExecutableTool[] {
  const context = store.getContext();
  const readableWorkspaces = readableWorkspaceCatalog(context.workspaceId, context.workspaceName, options.referenceWorkspaces);
  const getParameters = {
    ...GET_PARAMETERS,
    properties: {
      ...GET_PARAMETERS.properties,
      workspaceId: { type: "string", enum: readableWorkspaces.map((workspace) => workspace.workspaceId), description: "Workspace owning the runbook. Omit for the current workspace; another advertised same-Subject workspace is read-only." },
    },
  };
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
  const prepareParameters = {
    type: "object",
    required: ["title", "purpose", "language"],
    properties: {
      id: { type: "string", description: "Optional existing runbook ID when more than one runbook shares the title." },
      title: { type: "string", description: "Exact workflow title used to find an existing runbook in this workspace." },
      purpose: { type: "string", description: "Reusable proof objective and expected decision." },
      candidatePath: { type: "string", description: "Optional workspace-relative file beneath investigations/. Omit for a stable path generated from title and purpose." },
      entryCommand: { type: "string", description: "Optional bounded host command containing candidatePath. Omit for a language-specific entry command." },
      language: { type: "string", description: "Candidate implementation language, such as python3, sh, or node. The entry command runs as a host shell cell from the workspace root." },
    },
  };
  const editParameters = {
    type: "object",
    required: ["id", "expectedRevision", "cellId", "source"],
    properties: {
      id: { type: "string" },
      expectedRevision: { type: "integer", minimum: 1 },
      cellId: { type: "string", description: "Existing code cell ID from runbook.prepare or runbook.get." },
      source: { type: "string", description: "Replacement command or code. The cell ID, feature tags, and executor are preserved." },
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
      "Read a bounded page or inclusive cell range from one workspace runbook, including feature toggles, cell executors, and recorded results. workspaceId may select a host-verified same-Subject workspace and remains read-only. Follow nextOffset for ordinary pagination. For a range that exceeds limit, reuse endCellId and continue from nextCellId. Supply runId to review an immutable execution snapshot and its results. execution.latestSuccessfulRunId identifies a full successful run of the current content; claim promotion additionally requires matching source/environment provenance.",
      "read",
      getParameters,
      (input) => {
        const selectedWorkspace = selectReadableWorkspace(input.workspaceId, readableWorkspaces);
        const readOnlyReference = selectedWorkspace.workspaceId !== context.workspaceId;
        const pageOptions = {
          ...(typeof input.offset === "number" ? { offset: input.offset } : {}),
          ...(text(input.startCellId) ? { startCellId: text(input.startCellId)! } : {}),
          ...(text(input.endCellId) ? { endCellId: text(input.endCellId)! } : {}),
          limit: typeof input.limit === "number" ? input.limit : 12,
        };
        const runbook = text(input.runId)
          ? store.getExecution(requiredText(input.id, "id"), requiredText(input.runId, "runId"), pageOptions, selectedWorkspace.workspaceId)
          : readOnlyReference
            ? store.getSubjectReference(requiredText(input.id, "id"), selectedWorkspace.workspaceId, pageOptions)
            : store.get(requiredText(input.id, "id"), pageOptions);
        return { output: readOnlyReference && runbook && typeof runbook === "object"
          ? {
              ...runbook,
              workspace: { id: selectedWorkspace.workspaceId, name: selectedWorkspace.workspaceName, readOnlyReference: true },
            }
          : runbook };
      },
    ),
    tool(
      "runbook.prepare",
      "runbook_prepare",
      "Before writing a candidate proof, find or create its cohesive runbook and runtime entry cell in one call. Title, purpose, and language generate a stable investigations/ candidate path and bounded entry command; supply either explicitly when needed. Exact-title matches are reused when the command already exists; an empty workflow gains the entry cell. A different existing code command requires inspection and runbook.edit, preventing duplicate proof cells. Returns the candidate path, entry command, runbook revision, and entry cell ID for implementation and later runbook.run.",
      "write",
      prepareParameters,
      (input, context) => {
        const title = requiredText(input.title, "title");
        const purpose = requiredText(input.purpose, "purpose");
        const language = requiredText(input.language, "language");
        const candidatePath = input.candidatePath === undefined
          ? defaultCandidatePath(title, purpose, language)
          : requiredText(input.candidatePath, "candidatePath");
        const entryCommand = input.entryCommand === undefined
          ? defaultEntryCommand(language, candidatePath)
          : requiredText(input.entryCommand, "entryCommand");
        if (!candidatePath.startsWith("investigations/") || workspacePathProblem(candidatePath)) {
          throw new Error("candidatePath must be a valid workspace-relative file beneath investigations/.");
        }
        if (candidatePath.length > 4_096) throw new Error("candidatePath exceeds 4096 characters.");
        if (input.entryCommand === undefined && !/^[A-Za-z0-9_./-]+$/u.test(candidatePath)) {
          throw new Error("A candidatePath containing shell-special characters requires an explicit entryCommand.");
        }
        if (!entryCommand.includes(candidatePath)) throw new Error("entryCommand must reference candidatePath.");
        const selectedId = text(input.id);
        const matches = selectedId ? [store.get(selectedId, { limit: 1 })].filter((value) => value !== null) : store.findByTitle(title);
        if (selectedId && matches.length === 0) throw new Error(`Runbook not found in this workspace: ${selectedId}`);
        if (matches.length > 1) throw new Error(`Multiple runbooks have the title ${title}; inspect them with runbook.list and pass id to select one.`);
        let runbook = matches[0];
        let artifactRef: ResearchArtifactRef | undefined;
        let disposition: "created" | "reused" | "added-entry" = "reused";
        if (!runbook) {
          try {
            const created = store.create({ title, purpose, cells: [{ kind: "code", source: entryCommand, language: "sh", cwd: ".", features: ["runtime"], summary: `Entry command for ${candidatePath}` }] }, context?.modelAuthor, true);
            runbook = created.runbook;
            artifactRef = created.artifactRef;
            disposition = "created";
          } catch (error) {
            if (!(error instanceof RunbookTitleConflictError)) throw error;
            const raced = store.findByTitle(title);
            if (raced.length !== 1) throw error;
            runbook = raced[0];
          }
        }
        if (!runbook) throw new Error("Prepared runbook was not persisted.");
        if (runbook.title.toLowerCase() !== title.toLowerCase()) throw new Error(`Runbook ${runbook.id} has a different title; inspect it with runbook.get.`);
        if (runbook.purpose !== purpose) throw new Error(`Runbook ${runbook.id} has a different purpose; inspect it with runbook.get before choosing a workflow.`);
        if (disposition !== "created") {
          const cells = allRunbookCells(store, runbook.id);
          const matching = cells.filter((cell) => cell.kind === "code" && cell.language === "sh" && cell.cwd === "." && cell.source.includes(candidatePath));
          if (matching.length > 1) throw new Error(`Runbook ${runbook.id} has multiple matching entry cells; inspect it with runbook.get.`);
          if (matching.length === 0) {
            if (cells.some((cell) => cell.kind === "code")) {
              throw new Error(`Runbook ${runbook.id} already has code cells with a different entry command. Inspect it with runbook.get and revise one with runbook.edit.`);
            }
            const appended = store.append({ id: runbook.id, expectedRevision: runbook.revision, cells: [
              { kind: "code", source: entryCommand, language: "sh", cwd: ".", features: ["runtime"], summary: `Entry command for ${candidatePath}` },
            ] }, context?.modelAuthor);
            runbook = appended.runbook;
            artifactRef = appended.artifactRef;
            disposition = "added-entry";
          } else {
            if (input.entryCommand !== undefined && matching[0]!.source !== entryCommand) {
              throw new Error(`Runbook ${runbook.id} has a different entry command. Inspect it with runbook.get and revise it with runbook.edit.`);
            }
            disposition = "reused";
          }
        }
        const entryCell = allRunbookCells(store, runbook.id).find((cell) => cell.kind === "code" && cell.language === "sh" && cell.cwd === "." && cell.source.includes(candidatePath));
        if (!entryCell) throw new Error("Prepared runbook entry cell was not persisted.");
        return { output: { disposition, runbookId: runbook.id, revision: runbook.revision, candidatePath, entryCommand: entryCell.source, entryCellId: entryCell.id },
          ...(artifactRef ? { artifactRefs: [artifactRef] } : {}) };
      },
    ),
    tool(
      "runbook.create",
      "runbook_create",
      "Create a revisioned Jupyter-format research runbook only when the workflow is not already represented. Use runbook.prepare first for a new candidate proof so its path and entry cell exist before implementation. Keep setup, runtime, and cleanup in one cohesive runbook and label every cell with at least one of those default phase features; add narrower text tags when useful. Split only for a genuinely unrelated objective, target, or authorization boundary, not for phase changes, prerequisites, target state, review, or cleanup. Record prerequisites and expected evidence in markdown, then use repeatable code cells. Host cells require an explicit supported language and accept executor.timeoutSeconds for bounded long-running collectors instead of inheriting the shell default. A tart-vm cell references a host-built executable by workspacePath or artifactId; runbook.run materializes, stages, executes, records, and cleans it without a guest-side rewrite. Keep iterative implementation in a stable candidate artifact so the same entry cell can be rerun without per-tweak append churn.",
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
      "Append concise markdown or code cells to the existing cohesive workflow using its current revision. Use runbook.edit when an existing cell's command changes. Use setup, runtime, and cleanup feature tags to keep phases, prerequisites, target states, review steps, and cleanup in that runbook. Start another runbook only for a genuinely unrelated objective, target, or authorization boundary. Failed outputs already preserve attempt history: rerun an unchanged entry cell after editing its candidate artifact instead of appending one cell per tweak.",
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
      "runbook.edit",
      "runbook_edit",
      "Replace an existing code cell's command or source using its current runbook revision. This preserves the cell ID, phase tags, and executor, clears the cell's prior displayed output, and invalidates successful-run eligibility for the old content. Use this when the entry procedure changes; rerun an unchanged entry cell after editing only its candidate artifact.",
      "write",
      editParameters,
      (input, context) => {
        const edited = store.editCell({
          id: requiredText(input.id, "id"),
          expectedRevision: requiredInteger(input.expectedRevision, "expectedRevision"),
          cellId: requiredText(input.cellId, "cellId"),
          source: requiredText(input.source, "source"),
        }, context?.modelAuthor);
        return { output: { ...edited.runbook, editedCellId: requiredText(input.cellId, "cellId") }, artifactRefs: [edited.artifactRef] };
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

function allRunbookCells(store: RunbookStore, id: string): NonNullable<ReturnType<RunbookStore["get"]>>["cells"] {
  const cells: NonNullable<ReturnType<RunbookStore["get"]>>["cells"] = [];
  let offset = 0;
  while (true) {
    const page = store.get(id, { offset, limit: 100 });
    if (!page) throw new Error(`Runbook not found in this workspace: ${id}`);
    cells.push(...page.cells);
    if (page.nextOffset === null) return cells;
    offset = page.nextOffset;
  }
}

const PREPARE_LANGUAGES: Readonly<Record<string, { extension: string; command: string }>> = {
  python: { extension: "py", command: "python" },
  python3: { extension: "py", command: "python3" },
  javascript: { extension: "js", command: "node" },
  node: { extension: "js", command: "node" },
  sh: { extension: "sh", command: "sh" },
  bash: { extension: "sh", command: "bash" },
  zsh: { extension: "sh", command: "zsh" },
  pwsh: { extension: "ps1", command: "pwsh -File" },
  ruby: { extension: "rb", command: "ruby" },
  perl: { extension: "pl", command: "perl" },
};

function defaultCandidatePath(title: string, purpose: string, language: string): string {
  const runner = PREPARE_LANGUAGES[language];
  if (!runner) throw new Error(`runbook.prepare cannot generate a candidate path for unsupported language: ${language}.`);
  const slug = title.toLowerCase().replace(/[^a-z0-9]+/gu, "-").replace(/^-|-$/gu, "").slice(0, 48) || "candidate";
  const identity = createHash("sha256").update(`${title.toLowerCase()}\0${purpose}`).digest("hex").slice(0, 8);
  return `investigations/${slug}-${identity}/candidate.${runner.extension}`;
}

function defaultEntryCommand(language: string, candidatePath: string): string {
  const runner = PREPARE_LANGUAGES[language];
  if (!runner) throw new Error(`runbook.prepare cannot generate an entry command for unsupported language: ${language}.`);
  return `${runner.command} ${candidatePath}`;
}

function readableWorkspaceCatalog(
  workspaceId: string,
  workspaceName: string,
  references: readonly RunbookReadWorkspaceReference[] = [],
): RunbookReadWorkspaceReference[] {
  const seen = new Set<string>();
  return [{ workspaceId, workspaceName }, ...references].filter((workspace) => {
    if (!workspace.workspaceId.trim() || !workspace.workspaceName.trim() || seen.has(workspace.workspaceId)) return false;
    seen.add(workspace.workspaceId);
    return true;
  });
}

function selectReadableWorkspace(
  value: unknown,
  readableWorkspaces: readonly RunbookReadWorkspaceReference[],
): RunbookReadWorkspaceReference {
  const workspaceId = value === undefined ? readableWorkspaces[0]?.workspaceId : requiredText(value, "workspaceId");
  const selected = readableWorkspaces.find((workspace) => workspace.workspaceId === workspaceId);
  if (!selected) throw new Error("runbook.get can read only the current workspace or a host-verified workspace sharing its research Subject.");
  return selected;
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
      cwd: { type: "string", description: "Optional working directory. Use . for the research workspace root; omitted cells inherit the session shell default." },
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
    ...(text(input.cwd) ? { cwd: text(input.cwd)! } : {}),
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
      transport: { type: "string", enum: ["auto", "guest-agent", "ssh"], default: "auto", description: "Tart host-to-guest transport. Auto prefers Guest Agent and falls back to the configured bounded SSH identity; select SSH when the proof requires network communication." },
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
  const transport = executor.transport === undefined ? "auto" : requiredText(executor.transport, `${field}.transport`);
  if (transport !== "auto" && transport !== "guest-agent" && transport !== "ssh") {
    throw new Error(`${field}.transport must be auto, guest-agent, or ssh.`);
  }
  return {
    kind: "tart-vm",
    vmName: requiredText(executor.vmName, `${field}.vmName`),
    ...(artifactId ? { artifactId } : {}),
    ...(workspacePath ? { workspacePath } : {}),
    runAs,
    transport: transport as "auto" | "guest-agent" | "ssh",
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
