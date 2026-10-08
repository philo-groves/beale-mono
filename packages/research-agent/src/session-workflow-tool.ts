import { nowIso } from "./ids.js";
import { SessionWorkflowStore } from "./session-workflows.js";
import type { SessionWorkflowAssignment, SessionWorkflowNotebookCell } from "./session-workflows.js";
import type { RunbookStore } from "./runbooks.js";
import type { ResearchExecutableTool } from "./tool-registry.js";

export type SessionWorkflowProgressInput =
  | { action: "status"; afterPath?: string }
  | { action: "advance"; stepId: string; note: string; runId?: string }
  | { action: "scope"; targets: Array<{ system: string; path: string }> }
  | { action: "inventory"; paths: string[] }
  | { action: "inspect"; ranges: Array<{ path: string; startLine: number; endLine: number }> }
  | { action: "cover"; path: string; startLine: number; endLine: number }
  | { action: "cover"; ranges: Array<{ path: string; startLine: number; endLine: number }> };

const PARAMETERS = {
  type: "object",
  required: ["action"],
  properties: {
    action: { type: "string", enum: ["status", "advance", "scope", "inventory", "inspect", "cover"] },
    stepId: { type: "string", description: "Current step id to complete before moving to the next step." },
    note: { type: "string", maxLength: 1000, description: "Concise factual record of what completed this step, including evidence references or unresolved limits when relevant." },
    runId: { type: "string", maxLength: 200, description: "Successful runbook.run result for the current code cell. Required before advancing a code cell." },
    afterPath: { type: "string", description: "For status, continue the uncovered-file listing after this path cursor." },
    targets: { type: "array", maxItems: 100, items: { type: "object", required: ["system", "path"], properties: {
      system: { type: "string" }, path: { type: "string" }
    } }, description: "Complete mapping from each operator-named system to one or more exact authorized repository or subdirectory paths. Replaces the previous mapping." },
    paths: { type: "array", items: { type: "string" }, description: "Up to 20 declared repository or subdirectory paths to inventory per call." },
    path: { type: "string", description: "Inventoried code file actually inspected." },
    startLine: { type: "integer" },
    endLine: { type: "integer" },
    ranges: { type: "array", maxItems: 1000, items: { type: "object", required: ["path", "startLine", "endLine"], properties: {
      path: { type: "string" }, startLine: { type: "integer" }, endLine: { type: "integer" }
    } }, description: "For inspect, up to 20 ranges and 400 total lines; the tool returns source text and records those lines. Cover is retained only for older auditor assignments." }
  }
};

export function ensureSessionWorkflowRunbook(workflows: SessionWorkflowStore, runbooks: RunbookStore,
  sessionId: string): SessionWorkflowAssignment | null {
  let assignment = workflows.getAssignment(sessionId);
  if (!assignment || !assignment.definition.notebook.cells.some((cell) => cell.cell_type === "code")) return assignment;
  if (!assignment.runbookId) {
    const created = runbooks.create({
      title: `${assignment.definition.title} session notebook`,
      purpose: `Execution notebook for the assigned ${assignment.definition.title} workflow.`,
      cells: assignment.definition.notebook.cells.map((cell) => ({
        kind: cell.cell_type,
        source: cell.source.join(""),
        summary: cell.metadata.beale.title,
        features: ["runtime"],
        ...(cell.cell_type === "code" ? { language: cell.metadata.beale.language } : {}),
      })),
    });
    workflows.attachRunbook(sessionId, created.runbook.id);
    assignment = workflows.getAssignment(sessionId)!;
  }
  if (!runbooks.get(assignment.runbookId!, { limit: 1 })) throw new Error("The attached workflow runbook is unavailable.");
  return assignment;
}

export function verifySessionWorkflowCodeRun(runbooks: Pick<RunbookStore, "get" | "getExecution">,
  assignment: SessionWorkflowAssignment, cell: SessionWorkflowNotebookCell, runId: string): void {
  if (!assignment.runbookId) throw new Error("The workflow runbook is unavailable.");
  const index = assignment.definition.notebook.cells.findIndex((candidate) => candidate.id === cell.id);
  const recordedCell = runbooks.get(assignment.runbookId, { offset: index + 1, limit: 1 })?.cells[0];
  if (!recordedCell || recordedCell.kind !== "code" || recordedCell.source !== cell.source.join("")
    || recordedCell.language !== cell.metadata.beale.language) throw new Error("The attached runbook code cell no longer matches this workflow snapshot.");
  const execution = runbooks.getExecution(assignment.runbookId, runId, { startCellId: recordedCell.id, endCellId: recordedCell.id });
  if (execution.status !== "succeeded" || execution.sessionId !== assignment.sessionId
    || !execution.selectedCellIds.includes(recordedCell.id)
    || execution.cells[0]?.source !== cell.source.join("")
    || execution.cells[0]?.language !== cell.metadata.beale.language
    || execution.cells[0]?.result?.status !== "succeeded") {
    throw new Error("A successful execution of this exact workflow code cell in this session is required.");
  }
}

export function createSessionWorkflowTool(
  store: SessionWorkflowStore,
  sessionId: string,
  allowedRoots: string[],
  verifyRun?: (assignment: SessionWorkflowAssignment, cell: SessionWorkflowNotebookCell, runId: string) => void
): ResearchExecutableTool {
  const assignment = store.getAssignment(sessionId);
  const actions = assignment?.definition.kind === "repository_auditor"
    ? assignment.definition.revision >= 2 ? ["status", "advance", "scope", "inventory", "inspect"] : ["status", "advance", "inventory", "cover"]
    : ["status", "advance"];
  const allowedProperties = new Set(["action", "stepId", "note", "runId", "afterPath",
    ...(actions.includes("scope") ? ["targets"] : []),
    ...(actions.includes("inventory") ? ["paths"] : []),
    ...(actions.includes("inspect") || actions.includes("cover") ? ["ranges"] : []),
    ...(actions.includes("cover") ? ["path", "startLine", "endLine"] : [])]);
  const properties = Object.fromEntries(Object.entries(PARAMETERS.properties).filter(([name]) => allowedProperties.has(name)));
  const parameters = { ...PARAMETERS, properties: { ...properties, action: { type: "string", enum: actions } } };
  return {
    descriptor: {
      name: "workflow.progress",
      transportName: "workflow_progress",
      description: assignment?.definition.kind === "repository_auditor"
        ? "Read and advance the assigned workflow. The Repository Auditor declares scope, inventories code, and uses inspect to return bounded source excerpts while recording those exact lines across attempts."
        : "Read and advance the session's assigned ordered workflow; report the current step and durable progress.",
      actionClasses: ["inspect", "analyze", "synthesize"],
      sideEffects: "write",
      requiredPermissions: [],
      inputSchema: parameters,
      metadata: { provider: "appServer.session" }
    },
    parameters,
    async execute(action) {
      const startedAt = nowIso();
      try {
        const input = action.input as SessionWorkflowProgressInput;
        if (input.action === "inspect") {
          const inspected = store.inspect(sessionId, input.ranges);
          store.markEngaged(sessionId);
          const output = { ...inspected, assignment: store.getAssignment(sessionId)! };
          return { action, status: "complete" as const, startedAt, completedAt: nowIso(),
            summary: `Inspected ${inspected.excerpts.reduce((count, excerpt) => count + excerpt.lines.length, 0)} source lines.`,
            output, modelOutput: output, artifactRefs: [], followUpActions: [] };
        }
        if (input.action === "cover" && (store.getAssignment(sessionId)?.definition.revision ?? 0) >= 2) {
          throw new Error("Use workflow.progress inspect; coverage is recorded from source returned by that action.");
        }
        const result = input.action === "status"
          ? store.getAssignment(sessionId, input.afterPath)
          : input.action === "advance"
            ? store.advance(sessionId, input.stepId, input.note, input.runId, verifyRun)
          : input.action === "scope"
            ? store.declareAuditTargets(sessionId, input.targets, allowedRoots)
          : input.action === "inventory"
              ? store.inventory(sessionId, input.paths, allowedRoots)
              : input.action === "cover"
                ? "ranges" in input
                  ? store.coverMany(sessionId, input.ranges)
                  : store.cover(sessionId, input.path, input.startLine, input.endLine)
                : (() => { throw new Error("Unknown workflow action."); })();
        if (!result) throw new Error("This session has no assigned workflow.");
        store.markEngaged(sessionId);
        const current = store.getAssignment(sessionId, input.action === 'status' ? input.afterPath : undefined)!;
        return {
          action, status: "complete" as const, startedAt, completedAt: nowIso(),
          summary: `Workflow ${current.definition.title}: ${current.completedAt ? "complete" : current.definition.steps[current.stepIndex]?.title ?? "complete"}.`,
          output: current,
          modelOutput: current,
          artifactRefs: [], followUpActions: []
        };
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        return { action, status: "error" as const, startedAt, completedAt: nowIso(), summary: message,
          error: { message }, artifactRefs: [], followUpActions: ["Correct the workflow action and retry."] };
      }
    }
  };
}

export function sessionWorkflowInstructions(store: SessionWorkflowStore, sessionId: string): string | null {
  const assignment = store.getAssignment(sessionId);
  if (!assignment) return null;
  const { definition, values, stepIndex } = assignment;
  const systemsLabel = definition.fields.find((field) => field.id === "systems")?.label ?? "Systems to audit";
  return [
    "# Assigned session workflow",
    `The human operator assigned the ${definition.title} workflow. Begin with workflow.progress status, follow the current step, and call workflow.progress advance with that step's id and a factual completion note only after doing its work. Notes and state persist across resumed attempts.`,
    definition.description,
    ...definition.fields.map((field) => `${field.label}${field.description ? ` (${field.description})` : ""}: ${values[field.id] ?? ""}`),
    ...definition.notebook.cells.map((cell, index) => {
      const source = cell.source.join("");
      const heading = `${index + 1}. ${cell.metadata.beale.title}${index === stepIndex ? " (current)" : ""}`;
      return cell.cell_type === "code"
        ? `${heading} [${cell.metadata.beale.language} code cell, id ${cell.id}]:\n\n${source}`
        : `${heading} [markdown cell, id ${cell.id}]:\n\n${source}`;
    }),
    "Follow notebook cells in order. Markdown cells provide procedure and interpretation. Code cells are in the attached runbook, which is a Jupyter notebook artifact. Use runbook.get to find the matching code cell, run it with runbook.run in the authorized environment, inspect its result, then call workflow.progress advance with that runId and a factual completion note. A code cell cannot advance without a successful execution of its exact source in this session.",
    ...(assignment.runbookId ? [`Attached execution runbook ID: ${assignment.runbookId}. Its first cell is an overview; workflow cell ${stepIndex + 1} maps to runbook cell ${stepIndex + 2}.`] : []),
    ...(definition.kind === "repository_auditor" ? definition.revision >= 2 ? [
      `Repository Auditor procedure: Treat each nonempty line in ${systemsLabel} as one named system. Call workflow.progress scope with the complete system-to-path mapping, including every interacting repository. Inventory only those declared paths, up to 20 per call. For large codebases, call inspect with up to 20 file ranges and 400 lines per call; the returned source excerpt is the coverage record. Status lists each file's next uncovered line span; page files with afterPath using nextUncoveredCursor. Do not claim coverage from a filename, search result, or unseen output. Inventory includes Git-tracked and unignored untracked text files; ignored, empty, binary, missing, and uninitialized submodule content does not count toward coverage. Resolve or report each exclusion explicitly. The review step requires every inventoried line; final objective achievement refreshes the inventory.`
    ] : ["For this older auditor assignment, inventory scope paths and use cover only for lines actually inspected with other source-reading tools."] : [])
  ].join("\n\n");
}
