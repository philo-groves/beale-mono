import { createHash, randomUUID } from "node:crypto";
import { spawnSync } from "node:child_process";
import { lstatSync, openSync, readSync, realpathSync, closeSync, statSync } from "node:fs";
import { relative, resolve, sep } from "node:path";
import { DatabaseSync } from "node:sqlite";
import type { AppServerSessionStatus } from "./session-store.js";

export interface SessionWorkflowField {
  id: string;
  label: string;
  description?: string;
  required: boolean;
  placeholder?: string;
  multiline?: boolean;
}

export interface SessionWorkflowStep {
  id: string;
  title: string;
  instructions: string;
}

export type SessionWorkflowNotebookCell = {
  id: string;
  cell_type: "markdown" | "code";
  metadata: { beale: { title: string; language?: string }; vscode?: { languageId: string } };
  source: string[];
  execution_count?: null;
  outputs?: [];
};

export interface SessionWorkflowNotebook {
  nbformat: 4;
  nbformat_minor: 5;
  metadata: { beale: { kind: "session_workflow" } };
  cells: SessionWorkflowNotebookCell[];
}

export interface SessionWorkflowDefinition {
  id: string;
  title: string;
  description: string;
  kind: "repository_auditor" | "custom";
  fields: SessionWorkflowField[];
  notebook: SessionWorkflowNotebook;
  /** Derived compatibility projection for saved assignments and progress APIs. */
  steps: SessionWorkflowStep[];
  revision: number;
}

export interface SessionWorkflowDraft {
  title: string;
  description: string;
  fields: SessionWorkflowField[];
  notebook?: SessionWorkflowNotebook;
  /** Accepted for older clients; new editors send the notebook. */
  steps?: SessionWorkflowStep[];
}

export interface SessionWorkflowUpdateInput extends SessionWorkflowDraft {
  id: string;
  expectedRevision: number;
}

export interface SessionWorkflowAssignment {
  sessionId: string;
  workspaceId: string;
  definition: SessionWorkflowDefinition;
  values: Record<string, string>;
  stepIndex: number;
  engagedAt: string | null;
  completedAt: string | null;
  completedSteps: Array<{ stepId: string; completedAt: string; note: string }>;
  runbookId: string | null;
  scopePaths: string[];
  auditTargets: Array<{ system: string; path: string }>;
  fileCount: number;
  coveredFileCount: number;
  nextUncovered: Array<{ path: string; coveredLines: number; totalLines: number; nextRange: { startLine: number; endLine: number } }>;
  nextUncoveredCursor: string | null;
}

export interface SessionWorkflowRunSummary {
  sessionId: string;
  workflowId: string;
  title: string;
  revision: number;
  startedAt: string | null;
  engagedAt: string | null;
  completedAt: string | null;
  stepIndex: number;
  cellCount: number;
  sessionStatus: AppServerSessionStatus | null;
  sessionEndedAt: string | null;
}

export const REPOSITORY_AUDITOR_WORKFLOW: SessionWorkflowDefinition = {
  id: "beale.repository-auditor",
  title: "Repository auditor",
  description: "Map each requested system to concrete repository paths, inspect its code in bounded passes, and retain exact source coverage across sessions.",
  kind: "repository_auditor",
  revision: 2,
  fields: [
    { id: "systems", label: "Systems to audit (one per line)", description: "Name every repository, module, or interacting system to cover.", required: true, multiline: true, placeholder: "Repository, module, subdirectory, or interacting system" },
    { id: "focus", label: "Audit focus", description: "Optional risks, entry points, or behavior to prioritize.", required: false, placeholder: "Risks, entry points, or behavior to prioritize" }
  ],
  steps: [
    { id: "scope", title: "Resolve and declare scope", instructions: "For every operator-named system, identify the exact authorized Git repository or subdirectory paths that contain its code. Use workflow.progress scope with a system-to-path entry for every system. Map interacting repositories separately. Confirm the intended revisions and explain any path that cannot be resolved; do not silently narrow a requested system." },
    { id: "inventory", title: "Inventory source", instructions: "Use workflow.progress inventory on every declared path. The inventory includes regular, nonempty, nonbinary files that Git tracks or does not ignore. Check file counts and page the uncovered list. Identify ignored files, submodules, generated source, or other code omitted from this inventory; add a declared path for a separately checked-out repository when needed." },
    { id: "map", title: "Map architecture and attack surface", instructions: "Use workflow.progress inspect for build and package manifests, entry points, public interfaces, trust boundaries, data formats, privilege changes, and cross-repository calls. Record concrete call and data paths to guide review. Directory listings and search hits do not count as line coverage." },
    { id: "review", title: "Inspect all inventoried source", instructions: "Use workflow.progress inspect in bounded file and line batches, following callers, callees, parsers, error paths, and state transitions. The tool returns source and marks only those displayed lines as inspected. Recheck changes after inventory refresh. The review step cannot finish while any inventoried file has uncovered lines." },
    { id: "validate", title: "Validate candidate issues", instructions: "For each candidate, distinguish a hypothesis from a target observation. Reproduce or disprove it with authorized tools and evidence, consider alternate paths and mitigations, and retain references for confirmed conclusions. A complete audit may find no confirmed issue." },
    { id: "report", title: "Report coverage and findings", instructions: "State the mapped systems and revisions, completed coverage, omitted or uninspectable code, confirmed findings with evidence, rejected hypotheses, and residual uncertainty. Line coverage records inspection, not proof that the code is safe. Do not claim complete coverage outside the declared inventory." }
  ],
  get notebook() { return notebookFromSteps(this.steps); }
};

type SqlRow = Record<string, unknown>;
const DEFINITION_CATALOG_ID = 'global';

function boundedText(value: unknown, label: string, max: number): string {
  if (typeof value !== "string" || !value.trim() || value.length > max) throw new Error(`${label} must be non-empty and at most ${max} characters.`);
  return value.trim();
}

function auditSystemLabels(value: string): string[] {
  const labels = value.split(/\r?\n/u).map((label) => label.trim()).filter(Boolean);
  if (labels.length < 1 || labels.length > 40 || new Set(labels).size !== labels.length) {
    throw new Error("Repository auditor systems must list 1 to 40 distinct systems, one per line.");
  }
  return labels;
}

function normalizeFields(input: SessionWorkflowField[]): SessionWorkflowField[] {
  if (!Array.isArray(input) || input.length > 20) throw new Error("A workflow may have up to 20 text fields.");
  const ids = new Set<string>();
  return input.map((field) => {
    const id = boundedText(field.id, "Field id", 80);
    if (!/^[a-z][a-z0-9_-]*$/u.test(id) || ids.has(id)) throw new Error("Workflow field ids must be unique lowercase identifiers.");
    ids.add(id);
    const description = field.description;
    return { id, label: boundedText(field.label, "Field label", 160), required: field.required === true,
      ...(description === undefined || (typeof description === "string" && !description.trim())
        ? {} : { description: boundedText(description, "Field description", 240) }),
      ...(field.multiline ? { multiline: true } : {}),
      ...(field.placeholder ? { placeholder: boundedText(field.placeholder, "Field placeholder", 240) } : {}) };
  });
}

const WORKFLOW_CODE_LANGUAGES = new Set(["python", "python3", "py", "shell", "sh", "posix-shell", "bash", "zsh", "javascript", "js", "node", "ruby", "perl", "powershell", "pwsh"]);

function sourceLines(source: string): string[] {
  return source.match(/[^\n]*\n|[^\n]+$/gu) ?? [""];
}

function notebookFromSteps(steps: SessionWorkflowStep[]): SessionWorkflowNotebook {
  return { nbformat: 4, nbformat_minor: 5, metadata: { beale: { kind: "session_workflow" } },
    cells: steps.map((step) => ({ id: step.id, cell_type: "markdown", metadata: { beale: { title: step.title } }, source: sourceLines(step.instructions) })) };
}

function normalizeNotebook(input: SessionWorkflowNotebook | undefined, legacySteps: SessionWorkflowStep[] | undefined): SessionWorkflowNotebook {
  const notebook = input ?? (Array.isArray(legacySteps) ? notebookFromSteps(legacySteps) : undefined);
  if (!notebook || notebook.nbformat !== 4 || notebook.nbformat_minor !== 5 || !Array.isArray(notebook.cells)
    || notebook.cells.length < 1 || notebook.cells.length > 40) {
    throw new Error("A workflow needs a Jupyter nbformat 4.5 notebook with 1 to 40 cells.");
  }
  const ids = new Set<string>();
  let total = 0;
  const cells: SessionWorkflowNotebookCell[] = notebook.cells.map((cell) => {
    const id = boundedText(cell.id, "Cell id", 80);
    if (!/^[a-z][a-z0-9_-]*$/u.test(id) || ids.has(id)) throw new Error("Workflow cell ids must be unique lowercase identifiers.");
    ids.add(id);
    if (cell.cell_type !== "markdown" && cell.cell_type !== "code") throw new Error("Workflow cells must be markdown or code.");
    const title = boundedText(cell.metadata?.beale?.title, "Cell title", 160);
    if (!Array.isArray(cell.source) || cell.source.some((line) => typeof line !== "string")) throw new Error("Workflow cell source must be notebook text lines.");
    const source = cell.source.join("");
    if (!source.trim() || source.length > 8_000) throw new Error("Cell source must be non-empty and at most 8,000 characters.");
    const language = cell.cell_type === "code" ? boundedText(cell.metadata?.beale?.language, "Code language", 40).toLowerCase() : undefined;
    if (language && !WORKFLOW_CODE_LANGUAGES.has(language)) throw new Error(`Unsupported workflow code language: ${language}.`);
    total += title.length + source.length;
    return cell.cell_type === "code"
      ? { id, cell_type: "code", metadata: { beale: { title, language: language! }, vscode: { languageId: language! } },
        source: sourceLines(source), execution_count: null, outputs: [] }
      : { id, cell_type: "markdown" as const, metadata: { beale: { title } }, source: sourceLines(source) };
  });
  if (total > 32_000) throw new Error("Workflow notebook text exceeds the 32,000-character limit.");
  return { nbformat: 4, nbformat_minor: 5, metadata: { beale: { kind: "session_workflow" } }, cells };
}

function stepsFromNotebook(notebook: SessionWorkflowNotebook): SessionWorkflowStep[] {
  return notebook.cells.map((cell) => ({ id: cell.id, title: cell.metadata.beale.title, instructions: cell.source.join("") }));
}

function hydrateDefinition(definition: SessionWorkflowDefinition): SessionWorkflowDefinition {
  const notebook = definition.notebook ?? notebookFromSteps(definition.steps);
  return { ...definition, notebook, steps: stepsFromNotebook(notebook) };
}

function validateDefinition(input: SessionWorkflowDraft & { id: string; kind: "custom" }): SessionWorkflowDefinition {
  const title = boundedText(input.title, "Workflow title", 160);
  const description = boundedText(input.description, "Workflow description", 2_000);
  if (input.kind !== "custom") throw new Error("Only custom workflows may be created by the operator.");
  const fields = normalizeFields(input.fields);
  const notebook = normalizeNotebook(input.notebook, input.steps);
  return { id: boundedText(input.id, "Workflow id", 128), title, description, kind: "custom", fields, notebook, steps: stepsFromNotebook(notebook), revision: 1 };
}

function validateAuditorEdit(input: SessionWorkflowUpdateInput, revision: number): SessionWorkflowDefinition {
  const baseline = REPOSITORY_AUDITOR_WORKFLOW;
  if (!Array.isArray(input.fields) || input.fields.length < baseline.fields.length) {
    throw new Error("Built-in auditor core fields and cells must retain their fixed structure.");
  }
  const fields = normalizeFields(input.fields);
  for (const [index, fixed] of baseline.fields.entries()) {
    const field = fields[index]!;
    if (field.id !== fixed.id || field.required !== fixed.required || (field.multiline === true) !== (fixed.multiline === true)) {
      throw new Error("Built-in auditor field ids, order, and requirements cannot change.");
    }
  }
  const notebook = normalizeNotebook(input.notebook, input.steps);
  if (notebook.cells.length !== baseline.steps.length || notebook.cells.some((cell, index) => cell.id !== baseline.steps[index]!.id)) {
    throw new Error("Built-in auditor core fields and cells must retain their fixed structure.");
  }
  return { id: baseline.id, kind: baseline.kind, revision,
    title: boundedText(input.title, "Workflow title", 160),
    description: boundedText(input.description, "Workflow description", 2_000), fields, notebook, steps: stepsFromNotebook(notebook) };
}

export class SessionWorkflowStore {
  private readonly db: DatabaseSync;

  public constructor(databasePath: string, private readonly workspaceId: string) {
    this.db = new DatabaseSync(databasePath);
    this.db.exec("PRAGMA busy_timeout = 5000");
    this.db.exec(`CREATE TABLE IF NOT EXISTS beale_session_workflow_definitions (
      workspace_id TEXT NOT NULL, id TEXT NOT NULL, definition_json TEXT NOT NULL,
      PRIMARY KEY (workspace_id, id));
      CREATE TABLE IF NOT EXISTS beale_session_workflow_assignments (
      workspace_id TEXT NOT NULL, session_id TEXT NOT NULL, workflow_id TEXT, definition_json TEXT NOT NULL,
      values_json TEXT NOT NULL, step_index INTEGER NOT NULL DEFAULT 0, started_at TEXT, engaged_at TEXT, completed_at TEXT,
      step_history_json TEXT NOT NULL DEFAULT '[]', runbook_id TEXT,
      PRIMARY KEY (workspace_id, session_id));
      CREATE TABLE IF NOT EXISTS beale_session_workflow_files (
      workspace_id TEXT NOT NULL, session_id TEXT NOT NULL, path TEXT NOT NULL,
      content_hash TEXT NOT NULL, total_lines INTEGER NOT NULL, covered_lines INTEGER NOT NULL DEFAULT 0, covered_json TEXT NOT NULL DEFAULT '[]',
      PRIMARY KEY (workspace_id, session_id, path));
      CREATE TABLE IF NOT EXISTS beale_session_workflow_scopes (
      workspace_id TEXT NOT NULL, session_id TEXT NOT NULL, path TEXT NOT NULL,
      PRIMARY KEY (workspace_id, session_id, path));
      CREATE TABLE IF NOT EXISTS beale_session_workflow_targets (
      workspace_id TEXT NOT NULL, session_id TEXT NOT NULL, system_label TEXT NOT NULL, path TEXT NOT NULL,
      PRIMARY KEY (workspace_id, session_id, system_label, path));
      CREATE INDEX IF NOT EXISTS beale_session_workflow_files_session ON beale_session_workflow_files(workspace_id, session_id);`);
    const assignmentColumns = this.db.prepare("PRAGMA table_info(beale_session_workflow_assignments)").all() as SqlRow[];
    if (!assignmentColumns.some((column) => column.name === "engaged_at")) {
      this.db.exec("ALTER TABLE beale_session_workflow_assignments ADD COLUMN engaged_at TEXT");
    }
    if (!assignmentColumns.some((column) => column.name === "started_at")) {
      this.db.exec("ALTER TABLE beale_session_workflow_assignments ADD COLUMN started_at TEXT");
      this.db.exec("UPDATE beale_session_workflow_assignments SET started_at = COALESCE(engaged_at, completed_at)");
    }
    if (!assignmentColumns.some((column) => column.name === "step_history_json")) {
      this.db.exec("ALTER TABLE beale_session_workflow_assignments ADD COLUMN step_history_json TEXT NOT NULL DEFAULT '[]'");
    }
    if (!assignmentColumns.some((column) => column.name === "runbook_id")) {
      this.db.exec("ALTER TABLE beale_session_workflow_assignments ADD COLUMN runbook_id TEXT");
    }
    if (!assignmentColumns.some((column) => column.name === "workflow_id")) {
      this.db.exec("ALTER TABLE beale_session_workflow_assignments ADD COLUMN workflow_id TEXT");
      this.db.exec("UPDATE beale_session_workflow_assignments SET workflow_id = json_extract(definition_json, '$.id')");
    }
    this.db.exec("CREATE INDEX IF NOT EXISTS beale_session_workflow_assignments_workflow ON beale_session_workflow_assignments(workspace_id, workflow_id)");
    const columns = this.db.prepare("PRAGMA table_info(beale_session_workflow_files)").all() as SqlRow[];
    if (!columns.some((column) => column.name === "covered_lines")) {
      this.db.exec("ALTER TABLE beale_session_workflow_files ADD COLUMN covered_lines INTEGER NOT NULL DEFAULT 0");
      for (const row of this.db.prepare("SELECT workspace_id, session_id, path, covered_json FROM beale_session_workflow_files").all() as SqlRow[]) {
        this.db.prepare("UPDATE beale_session_workflow_files SET covered_lines = ? WHERE workspace_id = ? AND session_id = ? AND path = ?")
          .run(coveredCount(JSON.parse(String(row.covered_json)) as number[][]), String(row.workspace_id), String(row.session_id), String(row.path));
      }
    }
  }

  public close(): void { this.db.close(); }

  public list(): SessionWorkflowDefinition[] {
    const rows = this.db.prepare("SELECT id, definition_json FROM beale_session_workflow_definitions WHERE workspace_id = ? ORDER BY id")
      .all(DEFINITION_CATALOG_ID) as SqlRow[];
    const definitions = rows.map((row) => hydrateDefinition(JSON.parse(String(row.definition_json)) as SessionWorkflowDefinition));
    return [definitions.find((definition) => definition.id === REPOSITORY_AUDITOR_WORKFLOW.id) ?? REPOSITORY_AUDITOR_WORKFLOW,
      ...definitions.filter((definition) => definition.id !== REPOSITORY_AUDITOR_WORKFLOW.id)];
  }

  public getDefinition(id: string): SessionWorkflowDefinition | null {
    const row = this.db.prepare("SELECT definition_json FROM beale_session_workflow_definitions WHERE workspace_id = ? AND id = ?")
      .get(DEFINITION_CATALOG_ID, id) as SqlRow | undefined;
    return row ? hydrateDefinition(JSON.parse(String(row.definition_json)) as SessionWorkflowDefinition)
      : id === REPOSITORY_AUDITOR_WORKFLOW.id ? REPOSITORY_AUDITOR_WORKFLOW : null;
  }

  public listRuns(workflowId: string): SessionWorkflowRunSummary[] {
    const id = boundedText(workflowId, "Workflow id", 128);
    const rows = this.db.prepare(`SELECT session_id, step_index, started_at, engaged_at, completed_at,
      json_extract(definition_json, '$.title') AS title,
      json_extract(definition_json, '$.revision') AS revision,
      COALESCE(json_array_length(definition_json, '$.notebook.cells'), json_array_length(definition_json, '$.steps'), 0) AS cell_count
      FROM beale_session_workflow_assignments
      WHERE workspace_id = ? AND workflow_id = ?
      ORDER BY rowid DESC`).all(this.workspaceId, id) as SqlRow[];
    return rows.map((row) => {
      return { sessionId: String(row.session_id), workflowId: id, title: String(row.title),
        revision: Number(row.revision), startedAt: row.started_at ? String(row.started_at) : null,
        engagedAt: row.engaged_at ? String(row.engaged_at) : null,
        completedAt: row.completed_at ? String(row.completed_at) : null,
        stepIndex: Number(row.step_index), cellCount: Number(row.cell_count),
        sessionStatus: null, sessionEndedAt: null };
    });
  }

  public create(input: SessionWorkflowDraft): SessionWorkflowDefinition {
    const definition = validateDefinition({ ...input, id: `workflow-${randomUUID()}`, kind: "custom" });
    this.db.prepare("INSERT INTO beale_session_workflow_definitions (workspace_id, id, definition_json) VALUES (?, ?, ?)")
      .run(DEFINITION_CATALOG_ID, definition.id, JSON.stringify(definition));
    return definition;
  }

  public update(input: SessionWorkflowUpdateInput): SessionWorkflowDefinition {
    if (!Number.isSafeInteger(input.expectedRevision) || input.expectedRevision < 1) throw new Error("Expected workflow revision is required.");
    this.db.exec("BEGIN IMMEDIATE");
    try {
      const current = this.getDefinition(boundedText(input.id, "Workflow id", 128));
      if (!current) throw new Error("Unknown workflow.");
      if (current.revision !== input.expectedRevision) throw new Error("Workflow changed; reload it before saving.");
      const next = current.kind === "repository_auditor"
        ? validateAuditorEdit(input, current.revision + 1)
        : { ...validateDefinition({ ...input, id: current.id, kind: "custom" }), revision: current.revision + 1 };
      this.db.prepare(`INSERT INTO beale_session_workflow_definitions (workspace_id, id, definition_json) VALUES (?, ?, ?)
        ON CONFLICT(workspace_id, id) DO UPDATE SET definition_json = excluded.definition_json`)
        .run(DEFINITION_CATALOG_ID, next.id, JSON.stringify(next));
      this.db.exec("COMMIT");
      return next;
    } catch (error) { this.db.exec("ROLLBACK"); throw error; }
  }

  public assign(sessionId: string, workflowId: string, inputValues: Record<string, string>): SessionWorkflowAssignment {
    const existing = this.getAssignment(sessionId);
    const definition = existing?.definition ?? this.getDefinition(workflowId);
    if (!definition) throw new Error(`Unknown session workflow: ${workflowId}`);
    const values: Record<string, string> = {};
    const supplied = Object.keys(inputValues);
    if (supplied.some((id) => !definition.fields.some((field) => field.id === id))) throw new Error("Unknown workflow configuration field.");
    for (const field of definition.fields) {
      const suppliedValue = inputValues[field.id];
      const value = typeof suppliedValue === "string" ? suppliedValue.trim() : "";
      if (value.length > 4_000) throw new Error(`Workflow field ${field.label} is too long.`);
      if (field.required && !value) throw new Error(`Workflow field ${field.label} is required.`);
      values[field.id] = value;
    }
    if (Object.values(values).reduce((length, value) => length + value.length, 0) > 16_000) {
      throw new Error("Workflow configuration exceeds the 16,000-character limit.");
    }
    if (definition.kind === "repository_auditor" && definition.revision >= 2) auditSystemLabels(values.systems ?? "");
    if (existing) {
      if (existing.definition.id !== workflowId || JSON.stringify(existing.values) !== JSON.stringify(values)) {
        throw new Error("The session workflow assignment cannot change after session start.");
      }
      return existing;
    }
    this.db.prepare(`INSERT INTO beale_session_workflow_assignments
      (workspace_id, session_id, workflow_id, definition_json, values_json, started_at) VALUES (?, ?, ?, ?, ?, ?)
      ON CONFLICT(workspace_id, session_id) DO NOTHING`)
      .run(this.workspaceId, sessionId, definition.id, JSON.stringify(definition), JSON.stringify(values), new Date().toISOString());
    const assigned = this.getAssignment(sessionId)!;
    if (assigned.definition.id !== workflowId || JSON.stringify(assigned.values) !== JSON.stringify(values)) {
      throw new Error("The session workflow assignment cannot change after session start.");
    }
    return assigned;
  }

  public getAssignment(sessionId: string, afterPath?: string): SessionWorkflowAssignment | null {
    const row = this.db.prepare("SELECT * FROM beale_session_workflow_assignments WHERE workspace_id = ? AND session_id = ?")
      .get(this.workspaceId, sessionId) as SqlRow | undefined;
    if (!row) return null;
    const counts = this.db.prepare(`SELECT COUNT(*) AS file_count,
      COALESCE(SUM(CASE WHEN covered_lines >= total_lines THEN 1 ELSE 0 END), 0) AS covered_count
      FROM beale_session_workflow_files WHERE workspace_id = ? AND session_id = ?`)
      .get(this.workspaceId, sessionId) as SqlRow;
    const scopePaths = (this.db.prepare("SELECT path FROM beale_session_workflow_scopes WHERE workspace_id = ? AND session_id = ? ORDER BY path")
      .all(this.workspaceId, sessionId) as SqlRow[]).map((scope) => String(scope.path));
    const auditTargets = (this.db.prepare("SELECT system_label, path FROM beale_session_workflow_targets WHERE workspace_id = ? AND session_id = ? ORDER BY system_label, path")
      .all(this.workspaceId, sessionId) as SqlRow[]).map((target) => ({ system: String(target.system_label), path: String(target.path) }));
    const uncoveredQuery = `SELECT path, total_lines, covered_lines, covered_json FROM beale_session_workflow_files
      WHERE workspace_id = ? AND session_id = ? AND covered_lines < total_lines
      ${afterPath ? 'AND path > ?' : ''} ORDER BY path LIMIT 50`;
    const uncovered = (this.db.prepare(uncoveredQuery)
      .all(this.workspaceId, sessionId, ...(afterPath ? [boundedText(afterPath, 'Cursor', 4_096)] : [])) as SqlRow[]).map((file) => ({
        path: String(file.path), totalLines: Number(file.total_lines), coveredLines: Number(file.covered_lines),
        nextRange: nextUncoveredRange(JSON.parse(String(file.covered_json)) as number[][], Number(file.total_lines))
      }));
    return {
      sessionId, workspaceId: this.workspaceId,
      definition: hydrateDefinition(JSON.parse(String(row.definition_json)) as SessionWorkflowDefinition),
      values: JSON.parse(String(row.values_json)) as Record<string, string>,
      stepIndex: Number(row.step_index), engagedAt: row.engaged_at ? String(row.engaged_at) : null,
      completedAt: row.completed_at ? String(row.completed_at) : null,
      completedSteps: JSON.parse(String(row.step_history_json)) as SessionWorkflowAssignment["completedSteps"],
      runbookId: row.runbook_id ? String(row.runbook_id) : null,
      scopePaths,
      auditTargets,
      fileCount: Number(counts.file_count), coveredFileCount: Number(counts.covered_count),
      nextUncovered: uncovered,
      nextUncoveredCursor: uncovered.length === 50 ? uncovered.at(-1)!.path : null
    };
  }

  public attachRunbook(sessionId: string, runbookId: string): void {
    const assignment = this.getAssignment(sessionId);
    if (!assignment) throw new Error("This session has no assigned workflow.");
    if (assignment.runbookId && assignment.runbookId !== runbookId) throw new Error("The workflow runbook is already attached.");
    this.db.prepare("UPDATE beale_session_workflow_assignments SET runbook_id = ? WHERE workspace_id = ? AND session_id = ? AND runbook_id IS NULL")
      .run(boundedText(runbookId, "Runbook id", 200), this.workspaceId, sessionId);
  }

  public advance(sessionId: string, stepId: string, note: string, runId?: string,
    verifyRun?: (assignment: SessionWorkflowAssignment, cell: SessionWorkflowNotebookCell, runId: string) => void): SessionWorkflowAssignment {
    const assignment = this.getAssignment(sessionId);
    if (!assignment) throw new Error("This session has no assigned workflow.");
    const completedNote = boundedText(note, "Step completion note", 1_000);
    const step = assignment.definition.steps[assignment.stepIndex];
    if (!step || step.id !== stepId) throw new Error("Complete the current workflow step before advancing.");
    const cell = assignment.definition.notebook.cells[assignment.stepIndex];
    if (cell?.cell_type !== "code" && runId) throw new Error("Only code cells accept a runId.");
    const executionRunId = runId ? boundedText(runId, "Run ID", 200) : undefined;
    if (cell?.cell_type === "code") {
      if (!assignment.runbookId || !executionRunId || !verifyRun) throw new Error("Run the current code cell through its attached runbook and supply its successful runId before advancing.");
      verifyRun(assignment, cell, executionRunId);
    }
    if (assignment.definition.kind === "repository_auditor") {
      if (assignment.definition.revision >= 2 && stepId === "scope" && assignment.auditTargets.length === 0) {
        throw new Error("Map every named system to a concrete path with workflow.progress scope before advancing.");
      }
      if (stepId === "inventory" && assignment.fileCount === 0) throw new Error("Inventory the prompted code before advancing.");
      if (assignment.definition.revision >= 2 && stepId === "inventory") {
        for (const target of assignment.auditTargets) {
          if (!assignment.scopePaths.includes(target.path)) throw new Error(`Inventory the declared path for ${target.system} before advancing.`);
          const row = this.db.prepare(`SELECT COUNT(*) AS count FROM beale_session_workflow_files
            WHERE workspace_id = ? AND session_id = ? AND (path = ? OR substr(path, 1, length(?) + 1) = ?)`)
            .get(this.workspaceId, sessionId, target.path, target.path, `${target.path}${sep}`) as SqlRow;
          if (Number(row.count) === 0) throw new Error(`No inventoried text files were found for ${target.system}; resolve the scope before advancing.`);
        }
      }
      if (stepId === "review" && (assignment.fileCount === 0 || assignment.coveredFileCount !== assignment.fileCount)) {
        throw new Error("Review cannot complete until every inventoried text file is fully covered.");
      }
    }
    const next = assignment.stepIndex + 1;
    const completedAt = new Date().toISOString();
    const completedSteps = [...assignment.completedSteps, { stepId, completedAt,
      note: executionRunId ? `${completedNote} (runId: ${executionRunId})` : completedNote }].slice(-100);
    this.db.prepare("UPDATE beale_session_workflow_assignments SET step_index = ?, completed_at = ?, step_history_json = ? WHERE workspace_id = ? AND session_id = ?")
      .run(next, next === assignment.definition.steps.length ? completedAt : null, JSON.stringify(completedSteps), this.workspaceId, sessionId);
    return this.getAssignment(sessionId)!;
  }

  public markEngaged(sessionId: string): void {
    this.db.prepare("UPDATE beale_session_workflow_assignments SET engaged_at = COALESCE(engaged_at, ?) WHERE workspace_id = ? AND session_id = ?")
      .run(new Date().toISOString(), this.workspaceId, sessionId);
  }

  public declareAuditTargets(sessionId: string, inputTargets: Array<{ system: string; path: string }>, allowedRoots: string[]): SessionWorkflowAssignment {
    const assignment = this.getAssignment(sessionId);
    if (!assignment || assignment.definition.kind !== "repository_auditor" || assignment.definition.revision < 2) {
      throw new Error("Scope declaration is unavailable for this workflow.");
    }
    if (!Array.isArray(inputTargets) || inputTargets.length < 1 || inputTargets.length > 100) {
      throw new Error("Declare 1 to 100 system-to-path entries.");
    }
    const systems = auditSystemLabels(assignment.values.systems ?? "");
    const roots = allowedRoots.flatMap((path) => { try { return [realpathSync(path)]; } catch { return []; } });
    const targets = inputTargets.map((target) => {
      const system = boundedText(target.system, "System", 4_000);
      if (!systems.includes(system)) throw new Error(`Unknown requested system: ${system}`);
      const path = realpathSync(boundedText(target.path, "Scope path", 4_096));
      if (!roots.some((allowed) => path === allowed || path.startsWith(`${allowed}${sep}`))) {
        throw new Error("Scope path is outside known repository roots.");
      }
      if (!statSync(path).isDirectory()) throw new Error("Scope path must be a directory.");
      return { system, path };
    }).sort((a, b) => a.system < b.system ? -1 : a.system > b.system ? 1 : a.path < b.path ? -1 : a.path > b.path ? 1 : 0);
    if (systems.some((system) => !targets.some((target) => target.system === system))) {
      throw new Error("Map every operator-named system to at least one path.");
    }
    if (new Set(targets.map((target) => `${target.system}\0${target.path}`)).size !== targets.length) {
      throw new Error("Duplicate system-to-path entry.");
    }
    if (JSON.stringify(targets) === JSON.stringify(assignment.auditTargets)) return assignment;
    this.db.exec("BEGIN IMMEDIATE");
    try {
      this.db.prepare("DELETE FROM beale_session_workflow_targets WHERE workspace_id = ? AND session_id = ?")
        .run(this.workspaceId, sessionId);
      const insert = this.db.prepare("INSERT INTO beale_session_workflow_targets (workspace_id, session_id, system_label, path) VALUES (?, ?, ?, ?)");
      for (const target of targets) insert.run(this.workspaceId, sessionId, target.system, target.path);
      this.db.prepare(`DELETE FROM beale_session_workflow_scopes WHERE workspace_id = ? AND session_id = ?
        AND NOT EXISTS (SELECT 1 FROM beale_session_workflow_targets AS target WHERE target.workspace_id = ? AND target.session_id = ? AND target.path = beale_session_workflow_scopes.path)`)
        .run(this.workspaceId, sessionId, this.workspaceId, sessionId);
      this.db.prepare(`DELETE FROM beale_session_workflow_files WHERE workspace_id = ? AND session_id = ?
        AND NOT EXISTS (SELECT 1 FROM beale_session_workflow_targets AS target WHERE target.workspace_id = ? AND target.session_id = ?
          AND (beale_session_workflow_files.path = target.path OR substr(beale_session_workflow_files.path, 1, length(target.path) + 1) = target.path || ?))`)
        .run(this.workspaceId, sessionId, this.workspaceId, sessionId, sep);
      if (assignment.stepIndex > 0) this.db.prepare("UPDATE beale_session_workflow_assignments SET step_index = 1, completed_at = NULL WHERE workspace_id = ? AND session_id = ?")
        .run(this.workspaceId, sessionId);
      this.db.exec("COMMIT");
    } catch (error) { this.db.exec("ROLLBACK"); throw error; }
    return this.getAssignment(sessionId)!;
  }

  public inventory(sessionId: string, scopePaths: string[], allowedRoots: string[]): SessionWorkflowAssignment {
    const assignment = this.getAssignment(sessionId);
    if (!assignment || assignment.definition.kind !== "repository_auditor") throw new Error("Repository auditor is not assigned.");
    if (assignment.stepIndex < assignment.definition.steps.findIndex((step) => step.id === "inventory")) {
      throw new Error("Complete the scope step before inventorying source.");
    }
    if (!Array.isArray(scopePaths) || scopePaths.length < 1 || scopePaths.length > 20) throw new Error("Provide 1 to 20 scope paths.");
    const roots = allowedRoots.flatMap((path) => {
      try { return [realpathSync(path)]; } catch { return []; }
    });
    const insert = this.db.prepare(`INSERT INTO beale_session_workflow_files
      (workspace_id, session_id, path, content_hash, total_lines, covered_lines, covered_json) VALUES (?, ?, ?, ?, ?, 0, '[]')
      ON CONFLICT(workspace_id, session_id, path) DO UPDATE SET
      content_hash = excluded.content_hash, total_lines = excluded.total_lines,
      covered_lines = CASE WHEN content_hash = excluded.content_hash THEN covered_lines ELSE 0 END,
      covered_json = CASE WHEN content_hash = excluded.content_hash THEN covered_json ELSE '[]' END`);
    for (const path of scopePaths) {
      const root = realpathSync(boundedText(path, "Scope path", 4_096));
        if (assignment.definition.revision >= 2 && !assignment.auditTargets.some((target) => target.path === root)) {
          throw new Error("Inventory paths must match the declared system-to-path scope.");
        }
        if (!roots.some((allowed) => root === allowed || root.startsWith(`${allowed}${sep}`))) throw new Error("Scope path is outside known repository roots.");
        if (!statSync(root).isDirectory()) throw new Error("Scope path must be a directory.");
        const listing = spawnSync("git", ["-C", root, "ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", "."], { encoding: "buffer", maxBuffer: 128 * 1024 * 1024 });
        if (listing.status !== 0 || !listing.stdout) throw new Error("Repository auditor requires a Git-tracked scope.");
        const files: Array<{ path: string; hash: string; lines: number }> = [];
        const seen = new Set<string>();
        for (const relativePath of listing.stdout.toString("utf8").split("\0")) {
          if (!relativePath) continue;
          const filePath = resolve(root, relativePath);
          if (relative(root, filePath).startsWith("..")) continue;
          let fingerprint: ReturnType<typeof fingerprintFile> | null = null;
          try { if (lstatSync(filePath).isFile()) fingerprint = fingerprintFile(filePath); } catch { continue; }
          if (!fingerprint || fingerprint.binary || fingerprint.lines === 0) continue;
          seen.add(filePath);
          files.push({ path: filePath, hash: fingerprint.hash, lines: fingerprint.lines });
        }
        const oldPaths = (this.db.prepare("SELECT path FROM beale_session_workflow_files WHERE workspace_id = ? AND session_id = ?")
          .all(this.workspaceId, sessionId) as SqlRow[]).map((row) => String(row.path))
          .filter((oldPath) => (oldPath === root || oldPath.startsWith(`${root}${sep}`)) && !seen.has(oldPath));
        this.db.exec("BEGIN IMMEDIATE");
        try {
          for (const file of files) insert.run(this.workspaceId, sessionId, file.path, file.hash, file.lines);
          this.db.prepare("INSERT OR IGNORE INTO beale_session_workflow_scopes (workspace_id, session_id, path) VALUES (?, ?, ?)")
            .run(this.workspaceId, sessionId, root);
          const remove = this.db.prepare("DELETE FROM beale_session_workflow_files WHERE workspace_id = ? AND session_id = ? AND path = ?");
          for (const oldPath of oldPaths) remove.run(this.workspaceId, sessionId, oldPath);
          this.db.exec("COMMIT");
        } catch (error) { this.db.exec("ROLLBACK"); throw error; }
    }
    const updated = this.getAssignment(sessionId)!;
    const reviewIndex = updated.definition.steps.findIndex((step) => step.id === "review");
    if (reviewIndex >= 0 && updated.stepIndex > reviewIndex && (updated.fileCount === 0 || updated.coveredFileCount !== updated.fileCount)) {
      this.db.prepare("UPDATE beale_session_workflow_assignments SET step_index = ?, completed_at = NULL WHERE workspace_id = ? AND session_id = ?")
        .run(reviewIndex, this.workspaceId, sessionId);
      return this.getAssignment(sessionId)!;
    }
    return updated;
  }

  public cover(sessionId: string, path: string, startLine: number, endLine: number): SessionWorkflowAssignment {
    return this.coverMany(sessionId, [{ path, startLine, endLine }]);
  }

  public inspect(sessionId: string, inputRanges: Array<{ path: string; startLine: number; endLine: number }>): {
    assignment: SessionWorkflowAssignment;
    excerpts: Array<{ path: string; lines: Array<{ number: number; text: string }> }>;
  } {
    const assignment = this.getAssignment(sessionId);
    if (!assignment || assignment.definition.kind !== "repository_auditor") {
      throw new Error("Repository auditor is not assigned.");
    }
    const earliestReviewStep = assignment.definition.steps.findIndex((step) => step.id === "map");
    if (assignment.stepIndex < (earliestReviewStep >= 0 ? earliestReviewStep : assignment.definition.steps.findIndex((step) => step.id === "review"))) {
      throw new Error("Complete the inventory step before inspecting source.");
    }
    if (!Array.isArray(inputRanges) || inputRanges.length < 1 || inputRanges.length > 20) {
      throw new Error("Inspect 1 to 20 file ranges per call.");
    }
    const grouped = new Map<string, number[][]>();
    let requestedLines = 0;
    for (const range of inputRanges) {
      const path = realpathSync(boundedText(range.path, "File path", 4_096));
      const row = this.db.prepare("SELECT total_lines FROM beale_session_workflow_files WHERE workspace_id = ? AND session_id = ? AND path = ?")
        .get(this.workspaceId, sessionId, path) as SqlRow | undefined;
      if (!row) throw new Error("File is not in the auditor inventory.");
      if (!Number.isSafeInteger(range.startLine) || !Number.isSafeInteger(range.endLine)
        || range.startLine < 1 || range.endLine < range.startLine || range.endLine > Number(row.total_lines)) {
        throw new Error("Inspection range is outside the file.");
      }
      requestedLines += range.endLine - range.startLine + 1;
      if (requestedLines > 400) throw new Error("Inspect at most 400 lines per call.");
      grouped.set(path, [...(grouped.get(path) ?? []), [range.startLine, range.endLine]]);
    }
    const excerpts: Array<{ path: string; lines: Array<{ number: number; text: string }> }> = [];
    let remainingBytes = 48 * 1024;
    for (const [path, ranges] of grouped) {
      const excerpt = readSelectedLines(path, mergeRanges(ranges), remainingBytes);
      remainingBytes -= excerpt.bytes;
      excerpts.push({ path, lines: excerpt.lines });
    }
    return { assignment: this.coverMany(sessionId, inputRanges), excerpts };
  }

  public refreshInventory(sessionId: string): SessionWorkflowAssignment {
    const assignment = this.getAssignment(sessionId);
    if (!assignment || assignment.definition.kind !== "repository_auditor") throw new Error("Repository auditor is not assigned.");
    if (assignment.scopePaths.length === 0) return assignment;
    let refreshed = assignment;
    for (let index = 0; index < assignment.scopePaths.length; index += 20) {
      const paths = assignment.scopePaths.slice(index, index + 20);
      refreshed = this.inventory(sessionId, paths, paths);
    }
    return refreshed;
  }

  public coverMany(sessionId: string, inputRanges: Array<{ path: string; startLine: number; endLine: number }>): SessionWorkflowAssignment {
    const assignment = this.getAssignment(sessionId);
    if (!assignment || assignment.definition.kind !== "repository_auditor") throw new Error("Repository auditor is not assigned.");
    const earliestReviewStep = assignment.definition.steps.findIndex((step) => step.id === "map");
    if (assignment.stepIndex < (earliestReviewStep >= 0 ? earliestReviewStep : assignment.definition.steps.findIndex((step) => step.id === "review"))) {
      throw new Error("Complete the inventory step before recording inspected lines.");
    }
    if (!Array.isArray(inputRanges) || inputRanges.length < 1 || inputRanges.length > 1_000) throw new Error("Provide 1 to 1,000 inspected ranges.");
    const grouped = new Map<string, number[][]>();
    for (const range of inputRanges) {
      const filePath = realpathSync(boundedText(range.path, "File path", 4_096));
      grouped.set(filePath, [...(grouped.get(filePath) ?? []), [range.startLine, range.endLine]]);
    }
    const updates: Array<{ path: string; ranges: number[][] }> = [];
    for (const [filePath, added] of grouped) {
      const row = this.db.prepare("SELECT total_lines, covered_json, content_hash FROM beale_session_workflow_files WHERE workspace_id = ? AND session_id = ? AND path = ?")
        .get(this.workspaceId, sessionId, filePath) as SqlRow | undefined;
      if (!row) throw new Error("File is not in the auditor inventory.");
      const total = Number(row.total_lines);
      if (added.some(([startLine, endLine]) => !Number.isSafeInteger(startLine) || !Number.isSafeInteger(endLine)
        || startLine! < 1 || endLine! < startLine! || endLine! > total)) throw new Error("Coverage range is outside the file.");
      if (fingerprintFile(filePath).hash !== row.content_hash) throw new Error("File changed; refresh the inventory before recording coverage.");
      updates.push({ path: filePath, ranges: mergeRanges([...(JSON.parse(String(row.covered_json)) as number[][]), ...added]) });
    }
    this.db.exec("BEGIN IMMEDIATE");
    try {
      const update = this.db.prepare("UPDATE beale_session_workflow_files SET covered_json = ?, covered_lines = ? WHERE workspace_id = ? AND session_id = ? AND path = ?");
      for (const item of updates) update.run(JSON.stringify(item.ranges), coveredCount(item.ranges), this.workspaceId, sessionId, item.path);
      this.db.exec("COMMIT");
    } catch (error) { this.db.exec("ROLLBACK"); throw error; }
    return this.getAssignment(sessionId)!;
  }
}

export function requireAssignedSessionWorkflowDisposition(store: SessionWorkflowStore, sessionId: string, input: unknown): void {
  let assignment = store.getAssignment(sessionId);
  if (!assignment) return;
  if (!assignment.engagedAt) throw new Error("Use workflow.progress for the assigned session workflow before recording final disposition.");
  const achieved = typeof input === "object" && input !== null && "outcome" in input && input.outcome === "objective_achieved";
  if (achieved && assignment.definition.kind === "repository_auditor") assignment = store.refreshInventory(sessionId);
  if (assignment.definition.kind === "repository_auditor" && (assignment.fileCount === 0 || assignment.coveredFileCount !== assignment.fileCount)
    && achieved) {
    throw new Error("The repository auditor has uncovered inventoried text files; complete coverage before recording objective_achieved.");
  }
  if (!assignment.completedAt && achieved) {
    throw new Error("Complete the assigned session workflow before recording objective_achieved. Use a partial or blocked disposition when work remains.");
  }
}

function mergeRanges(ranges: number[][]): number[][] {
  const sorted = ranges.sort((a, b) => a[0]! - b[0]!);
  const result: number[][] = [];
  for (const range of sorted) {
    const last = result.at(-1);
    if (last && range[0]! <= last[1]! + 1) last[1] = Math.max(last[1]!, range[1]!);
    else result.push([...range]);
  }
  return result;
}

function coveredCount(ranges: number[][]): number { return ranges.reduce((sum, range) => sum + range[1]! - range[0]! + 1, 0); }

function nextUncoveredRange(ranges: number[][], totalLines: number): { startLine: number; endLine: number } {
  let startLine = 1;
  for (const [coveredStart, coveredEnd] of ranges) {
    if (startLine < coveredStart!) return { startLine, endLine: Math.min(startLine + 199, coveredStart! - 1) };
    startLine = Math.max(startLine, coveredEnd! + 1);
  }
  return { startLine, endLine: Math.min(startLine + 199, totalLines) };
}

function readSelectedLines(path: string, ranges: number[][], maxBytes: number): {
  lines: Array<{ number: number; text: string }>;
  bytes: number;
} {
  const fd = openSync(path, "r");
  const chunk = Buffer.allocUnsafe(64 * 1024);
  const lines: Array<{ number: number; text: string }> = [];
  let number = 1;
  let parts: Buffer[] = [];
  let partBytes = 0;
  let outputBytes = 0;
  let totalBytes = 0;
  let lastByte = -1;
  const selected = (): boolean => ranges.some(([start, end]) => number >= start! && number <= end!);
  const append = (part: Buffer): void => {
    if (!selected() || part.length === 0) return;
    partBytes += part.length;
    if (outputBytes + partBytes > maxBytes) throw new Error("Inspection output exceeds 48 KiB; request fewer lines.");
    parts.push(Buffer.from(part));
  };
  const finish = (): void => {
    if (selected()) {
      lines.push({ number, text: Buffer.concat(parts, partBytes).toString("utf8") });
      outputBytes += partBytes;
    }
    parts = [];
    partBytes = 0;
    number += 1;
  };
  try {
    for (;;) {
      const length = readSync(fd, chunk, 0, chunk.length, null);
      if (!length) break;
      totalBytes += length;
      lastByte = chunk[length - 1]!;
      let offset = 0;
      for (;;) {
        const newline = chunk.subarray(0, length).indexOf(10, offset);
        if (newline < 0) break;
        append(chunk.subarray(offset, newline));
        finish();
        offset = newline + 1;
      }
      append(chunk.subarray(offset, length));
    }
    if (totalBytes > 0 && lastByte !== 10) finish();
  } finally { closeSync(fd); }
  return { lines, bytes: outputBytes };
}

function fingerprintFile(path: string): { hash: string; lines: number; binary: boolean } {
  const hash = createHash("sha256");
  const fd = openSync(path, "r");
  const chunk = Buffer.allocUnsafe(64 * 1024);
  let bytes = 0;
  let newlines = 0;
  let lastByte = -1;
  let binary = false;
  try {
    for (;;) {
      const length = readSync(fd, chunk, 0, chunk.length, null);
      if (!length) break;
      hash.update(chunk.subarray(0, length));
      bytes += length;
      lastByte = chunk[length - 1]!;
      for (let index = 0; index < length; index += 1) {
        if (chunk[index] === 0) binary = true;
        if (chunk[index] === 10) newlines += 1;
      }
    }
  } finally { closeSync(fd); }
  return { hash: hash.digest("hex"), lines: bytes === 0 ? 0 : newlines + (lastByte === 10 ? 0 : 1), binary };
}
