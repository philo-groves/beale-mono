import { createHash, randomUUID } from "node:crypto";
import { chmodSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import type { DatabaseSync } from "node:sqlite";
import { pathToFileURL } from "node:url";
import { MemoryGraphStore, type MemoryContext } from "./memory-graph.js";
import { openResearchDatabase } from "./database.js";
import { modelAuthorsForResource, recordModelAuthorship, type ModelAuthor } from "./model-authorship.js";
import {
  registerResearchStorageArtifact,
  type ResearchStorageArtifactManifestEntry,
} from "./storage.js";
import type { ResearchArtifactRef, ResearchStorageLayout } from "./types.js";
import { readPreBealeRecord } from "./legacy-compatibility.js";

export type RunbookCellKind = "markdown" | "code";
export type RunbookExecutionStatus = "queued" | "running" | "succeeded" | "failed" | "blocked" | "skipped";
export type TartTransportPreference = "auto" | "guest-agent" | "ssh";
export type RunbookCellExecutor =
  | { kind: "host"; timeoutSeconds: number }
  | {
      kind: "tart-vm";
      vmName: string;
      artifactId?: string;
      workspacePath?: string;
      runAs: "guest" | "root";
      transport: TartTransportPreference;
      argv: string[];
      timeoutSeconds: number;
      retainOnFailure: boolean;
    };
export const RUNBOOK_DEFAULT_FEATURES = ["setup", "runtime", "cleanup"] as const;
export const RUNBOOK_DEFAULT_TIMEOUT_SECONDS = 300;
export const RUNBOOK_MAX_TIMEOUT_SECONDS = 1_800;
export const MAX_RUNBOOK_MUTATION_CELLS = 100;
export const RUNBOOK_PROOF_TARGETS = ["localhost", "device", "vm", "web", "other"] as const;
export type RunbookProofTarget = (typeof RUNBOOK_PROOF_TARGETS)[number];

export class RunbookTitleConflictError extends Error {
  public constructor(title: string) { super(`A runbook with title ${title} was created while preparing this workflow.`); }
}

export interface RunbookExecutionState {
  runId: string;
  status: RunbookExecutionStatus;
  startedAt: string;
  completedAt?: string;
  durationMs?: number;
  exitCode?: number | null;
  error?: string;
  proofTarget: RunbookProofTarget;
  deviceOs?: string;
  evidence?: Record<string, string | number | boolean | null>;
}

export interface RunbookExecutionPlanCell {
  id: string;
  source: string;
  language: string | null;
  executor: RunbookCellExecutor;
  features: string[];
  cwd?: string;
}

export interface RunbookCellInput {
  kind: RunbookCellKind;
  source: string;
  features: string[];
  executor?: RunbookCellExecutor;
  language?: string;
  cwd?: string;
  summary?: string;
  stdout?: string;
  stderr?: string;
  exitCode?: number;
}

export interface RunbookCellRecord extends RunbookCellInput {
  id: string;
  index: number;
  active: boolean;
}

export interface RunbookExecutionMetrics {
  runCount: number;
  completedRunCount: number;
  executedCellCount: number;
  latest: {
    runId: string;
    status: "running" | "succeeded" | "failed" | "blocked";
    startedAt: string;
  } | null;
  latestSuccessfulRunId: string | null;
}

export interface RunbookExecutionSelection {
  cellId?: string;
  startCellId?: string;
  endCellId?: string;
}

export interface RunbookExecutionProvenance {
  expectedContentRevision?: number;
  sourceRevision?: string;
  environmentFingerprint?: string;
  /** Supplied by the host tool context, never by model arguments. */
  actorId?: string;
}

export interface RunbookRecord {
  id: string;
  workspaceId: string;
  workspaceName: string;
  subjectId: string | null;
  subjectName: string | null;
  sessionId: string | null;
  title: string;
  purpose: string;
  artifactId: string;
  enabledFeatures: string[];
  cellCount: number;
  revision: number;
  contentRevision: number;
  duplicateOfRunbookId: string | null;
  duplicateMarkedAt: string | null;
  duplicateRunbooks: RunbookDuplicateSummary[];
  execution: RunbookExecutionMetrics;
  createdAt: string;
  updatedAt: string;
  authors: ModelAuthor[];
}

export interface RunbookDuplicateSummary {
  id: string;
  title: string;
  purpose: string;
  revision: number;
  markedAt: string;
}

export interface RunbookPage extends RunbookRecord {
  offset: number;
  limit: number;
  nextOffset: number | null;
  nextCellId: string | null;
  previousOffset: number | null;
  totalCells: number;
  cells: RunbookCellRecord[];
}

interface RunbookRow {
  id: string;
  workspace_id: string;
  workspace_name: string;
  subject_id: string | null;
  subject_name: string | null;
  session_id: string | null;
  title: string;
  purpose: string;
  artifact_id: string;
  relative_path: string;
  content_hash: string;
  size_bytes: number;
  revision: number;
  content_revision: number;
  duplicate_of_runbook_id: string | null;
  duplicate_marked_at: string | null;
  created_at: string;
  updated_at: string;
}

interface RunbookExecutionRow {
  status: "running" | "succeeded" | "failed" | "blocked";
  snapshot_json: string | null;
  content_revision: number | null;
  content_hash: string | null;
  source_revision: string | null;
  environment_fingerprint: string | null;
  session_id: string | null;
  actor_id: string | null;
  full_run: number;
  started_at: string;
  completed_at: string | null;
  selected_cell_ids_json: string | null;
  required_cell_ids_json: string | null;
}

interface NotebookCell {
  id?: string;
  cell_type: "markdown" | "code";
  metadata: Record<string, unknown>;
  source: string[];
  execution_count?: number | null;
  outputs?: Array<Record<string, unknown>>;
}

interface RunbookNotebook {
  cells: NotebookCell[];
  metadata: {
    beale: Record<string, unknown>;
  };
  nbformat: 4;
  nbformat_minor: 5;
}

export class RunbookStore {
  private readonly database: DatabaseSync;

  public constructor(
    private readonly databasePath: string,
    private readonly storageLayout: ResearchStorageLayout,
    private readonly context: MemoryContext,
  ) {
    mkdirSync(dirname(databasePath), { recursive: true });
    this.database = openResearchDatabase(databasePath);
    if (databasePath !== ":memory:") chmodSync(databasePath, 0o600);
    this.database.exec("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000;");
    MemoryGraphStore.initializeSchema(this.database);
  }

  public close(): void {
    this.database.close();
  }

  public getContext(): MemoryContext {
    return { ...this.context };
  }

  public list(options: { query?: string; limit?: number } = {}): RunbookRecord[] {
    const query = options.query?.trim().toLowerCase() ?? "";
    const limit = clampInteger(options.limit ?? 50, 1, 200);
    return (this.database
      .prepare("SELECT * FROM app_server_runbooks WHERE workspace_id = ? AND duplicate_of_runbook_id IS NULL ORDER BY updated_at DESC, id")
      .all(this.context.workspaceId) as unknown as RunbookRow[])
      .filter((row) => !query || `${row.title}\n${row.purpose}`.toLowerCase().includes(query))
      .slice(0, limit)
      .map((row) => this.toRecord(row));
  }

  public findByTitle(title: string): RunbookRecord[] {
    return (this.database.prepare(`SELECT * FROM app_server_runbooks
      WHERE workspace_id = ? AND title = ? COLLATE NOCASE AND duplicate_of_runbook_id IS NULL
      ORDER BY updated_at DESC, id LIMIT 2`).all(this.context.workspaceId, requiredText(title, "title", 240)) as unknown as RunbookRow[])
      .map((row) => this.toRecord(row));
  }

  /** Read-only catalog references from selected workspaces attached to the active Subject. */
  public listSubjectReferences(options: { query?: string; limit?: number; workspaceIds?: readonly string[] } = {}): RunbookRecord[] {
    const query = options.query?.trim().toLowerCase() ?? "";
    const limit = clampInteger(options.limit ?? 50, 1, 200);
    const workspaceIds = uniqueStrings(options.workspaceIds ?? []);
    return (this.database.prepare(`SELECT * FROM app_server_runbooks
      WHERE subject_id = ? AND duplicate_of_runbook_id IS NULL${workspaceIds.length
        ? ` AND workspace_id IN (${workspaceIds.map(() => "?").join(",")})`
        : ""}
      ORDER BY updated_at DESC, id`).all(
        this.context.subjectId ?? `subject_workspace:${this.context.workspaceId}`,
        ...workspaceIds,
      ) as unknown as RunbookRow[])
      .filter((row) => !query || `${row.title}\n${row.purpose}`.toLowerCase().includes(query))
      .slice(0, limit)
      .map((row) => this.toRecord(row));
  }

  public get(id: string, options: {
    offset?: number;
    limit?: number;
    startCellId?: string;
    endCellId?: string;
  } = {}): RunbookPage | null {
    const row = this.readRow(id);
    if (!row) return null;
    return this.pageFromRow(id, row, options);
  }

  /** Read a host-authorized same-Subject runbook without enabling foreign mutation. */
  public getSubjectReference(id: string, workspaceId: string, options: {
    offset?: number;
    limit?: number;
    startCellId?: string;
    endCellId?: string;
  } = {}): RunbookPage | null {
    if (workspaceId === this.context.workspaceId) return this.get(id, options);
    const row = this.readRowForWorkspace(id, workspaceId);
    if (!row || row.subject_id !== this.context.subjectId) return null;
    return this.pageFromRow(id, row, options);
  }

  private pageFromRow(id: string, row: RunbookRow, options: {
    offset?: number;
    limit?: number;
    startCellId?: string;
    endCellId?: string;
  }): RunbookPage {
    const notebook = this.readNotebook(row);
    const limit = clampInteger(options.limit ?? 40, 1, 100);
    if (options.offset !== undefined && (options.startCellId || options.endCellId)) {
      throw new Error("offset cannot be combined with startCellId or endCellId.");
    }
    const records = notebook.cells.map((cell, index) =>
      notebookCellToRecord(cell, index, notebookEnabledFeatures(notebook)));
    const range = cellRange(records, options.startCellId, options.endCellId, id);
    const ranged = Boolean(options.startCellId || options.endCellId);
    const offset = ranged
      ? range.start
      : clampInteger(options.offset ?? 0, 0, notebook.cells.length);
    const end = ranged
      ? Math.min(range.end + 1, offset + limit)
      : Math.min(notebook.cells.length, offset + limit);
    return {
      ...this.toRecord(row, notebook.cells.length),
      offset,
      limit,
      nextOffset: !ranged && end < notebook.cells.length ? end : null,
      nextCellId: ranged && end < range.end + 1 ? records[end]?.id ?? null : null,
      previousOffset: !ranged && offset > 0 ? Math.max(0, offset - limit) : null,
      totalCells: notebook.cells.length,
      cells: records.slice(offset, end),
    };
  }

  public markDuplicate(
    id: string,
    input: { expectedRevision: number; parentRunbookId: string; reason: string },
    author?: ModelAuthor,
  ): RunbookRecord {
    const current = this.readRow(requiredText(id, "id", 200));
    if (!current) throw new Error(`Runbook not found in this workspace: ${id}`);
    if (current.revision !== input.expectedRevision) {
      throw new Error(`Runbook revision conflict for ${id}: expected ${input.expectedRevision}, found ${current.revision}.`);
    }
    if (current.duplicate_of_runbook_id) {
      throw new Error(`Runbook ${id} is already marked as a duplicate of ${current.duplicate_of_runbook_id}.`);
    }
    const parentRunbookId = requiredText(input.parentRunbookId, "Canonical parent runbook id", 200);
    requiredText(input.reason, "Duplicate reason", 4_000);
    if (parentRunbookId === id) throw new Error("A runbook cannot be marked as a duplicate of itself.");
    const parent = this.readRow(parentRunbookId);
    if (!parent) throw new Error(`Canonical parent runbook not found: ${parentRunbookId}.`);
    if (parent.duplicate_of_runbook_id) {
      throw new Error(`Canonical parent ${parentRunbookId} is itself a duplicate; choose its canonical parent instead.`);
    }
    const childCount = this.database.prepare(
      "SELECT COUNT(*) AS count FROM app_server_runbooks WHERE duplicate_of_runbook_id = ?",
    ).get(id) as { count?: unknown } | undefined;
    if (Number(childCount?.count ?? 0) > 0) {
      throw new Error(`Runbook ${id} owns duplicate runbooks; undo or reassign them before coalescing it.`);
    }
    const now = new Date().toISOString();
    const nextRevision = current.revision + 1;
    const result = this.database.prepare(`UPDATE app_server_runbooks SET
      duplicate_of_runbook_id = ?, duplicate_marked_at = ?, updated_at = ?, revision = ?
      WHERE id = ? AND workspace_id = ? AND revision = ? AND duplicate_of_runbook_id IS NULL`).run(
      parentRunbookId, now, now, nextRevision, id, this.context.workspaceId, current.revision,
    );
    if (Number(result.changes) !== 1) throw new Error(`Runbook revision conflict for ${id}.`);
    recordModelAuthorship(this.database, "runbook", id, nextRevision, author, now);
    return this.toRecord(parent);
  }

  public undoDuplicate(
    id: string,
    input: { expectedRevision: number; reason: string },
    author?: ModelAuthor,
  ): RunbookRecord {
    const current = this.readRow(requiredText(id, "id", 200));
    if (!current) throw new Error(`Runbook not found in this workspace: ${id}`);
    if (current.revision !== input.expectedRevision) {
      throw new Error(`Runbook revision conflict for ${id}: expected ${input.expectedRevision}, found ${current.revision}.`);
    }
    if (!current.duplicate_of_runbook_id) throw new Error(`Runbook ${id} is not marked as a duplicate.`);
    requiredText(input.reason, "Duplicate undo reason", 4_000);
    const now = new Date().toISOString();
    const nextRevision = current.revision + 1;
    const result = this.database.prepare(`UPDATE app_server_runbooks SET
      duplicate_of_runbook_id = NULL, duplicate_marked_at = NULL, updated_at = ?, revision = ?
      WHERE id = ? AND workspace_id = ? AND revision = ? AND duplicate_of_runbook_id IS NOT NULL`).run(
      now, nextRevision, id, this.context.workspaceId, current.revision,
    );
    if (Number(result.changes) !== 1) throw new Error(`Runbook revision conflict for ${id}.`);
    recordModelAuthorship(this.database, "runbook", id, nextRevision, author, now);
    return this.toRecord({ ...current, duplicate_of_runbook_id: null, duplicate_marked_at: null, updated_at: now, revision: nextRevision });
  }

  public create(input: {
    title: string;
    purpose: string;
    cells?: RunbookCellInput[];
    enabledFeatures?: string[];
  }, author?: ModelAuthor, uniqueTitle = false): { runbook: RunbookRecord; artifactRef: ResearchArtifactRef } {
    const title = requiredText(input.title, "title", 240);
    const purpose = requiredText(input.purpose, "purpose", 4_000);
    const cells = (input.cells ?? []).map(validateCell);
    if (cells.length > MAX_RUNBOOK_MUTATION_CELLS) throw new Error(`A runbook can be created with at most ${MAX_RUNBOOK_MUTATION_CELLS} cells.`);
    const enabledFeatures = input.enabledFeatures === undefined
      ? [...RUNBOOK_DEFAULT_FEATURES]
      : validateFeatures(input.enabledFeatures, "enabledFeatures", false);

    const id = `runbook_${randomUUID()}`;
    const artifactId = id;
    const now = new Date().toISOString();
    const relativePath = join("runbooks", safeSegment(this.context.workspaceId), `${id}.ipynb`);
    const notebook = createNotebook({
      id,
      title,
      purpose,
      context: this.context,
      revision: 1,
      contentRevision: 1,
      createdAt: now,
      updatedAt: now,
      cells,
      enabledFeatures,
    });

    this.database.exec("BEGIN IMMEDIATE");
    try {
      if (uniqueTitle && this.database.prepare(`SELECT 1 FROM app_server_runbooks
        WHERE workspace_id = ? AND title = ? COLLATE NOCASE AND duplicate_of_runbook_id IS NULL LIMIT 1`).get(this.context.workspaceId, title)) {
        throw new RunbookTitleConflictError(title);
      }
      const entry = this.writeAndRegister(id, artifactId, relativePath, title, notebook);
      this.database
        .prepare(
          `INSERT INTO app_server_runbooks (
             id, workspace_id, workspace_name, subject_id, subject_name, session_id,
             title, purpose, artifact_id, relative_path, content_hash,
             size_bytes, revision, content_revision, created_at, updated_at
           ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
        )
        .run(
          id,
          this.context.workspaceId,
          this.context.workspaceName,
          this.context.subjectId ?? null,
          this.context.subjectName ?? null,
          this.context.sessionId ?? null,
          title,
          purpose,
          artifactId,
          relativePath,
          entry.contentHash,
          entry.sizeBytes,
          1,
          1,
          now,
          now,
        );
      this.recordRevision(id, 1, now);
      recordModelAuthorship(this.database, "runbook", id, 1, author, now);
      this.database.exec("COMMIT");
      const row = this.readRow(id);
      if (!row) throw new Error(`Runbook was not persisted: ${id}`);
      return { runbook: this.toRecord(row, notebook.cells.length), artifactRef: artifactRef(entry, title) };
    } catch (error) {
      this.database.exec("ROLLBACK");
      throw error;
    }
  }

  public append(input: {
    id: string;
    expectedRevision: number;
    cells: RunbookCellInput[];
  }, author?: ModelAuthor, replaceCells = false): { runbook: RunbookRecord; artifactRef: ResearchArtifactRef } {
    const id = requiredText(input.id, "id", 200);
    if (!Number.isSafeInteger(input.expectedRevision) || input.expectedRevision < 1) throw new Error("expectedRevision must be a positive integer.");
    if (!Array.isArray(input.cells) || input.cells.length === 0) throw new Error("cells must contain at least one cell.");
    if (input.cells.length > MAX_RUNBOOK_MUTATION_CELLS) throw new Error(`At most ${MAX_RUNBOOK_MUTATION_CELLS} cells can be appended at once.`);
    const cells = input.cells.map(validateCell);
    this.database.exec("BEGIN IMMEDIATE");
    try {
      const row = this.readRow(id);
      if (!row) throw new Error(`Runbook not found in this workspace: ${id}`);
      this.requireCanonical(row);
      if (row.revision !== input.expectedRevision) {
        throw new Error(`Runbook revision conflict for ${id}: expected ${input.expectedRevision}, found ${row.revision}.`);
      }
      const current = this.readNotebook(row);
      delete current.metadata.beale.status;
      const revision = row.revision + 1;
      const contentRevision = row.content_revision + 1;
      const updatedAt = new Date().toISOString();
      const notebook: RunbookNotebook = {
        ...current,
        cells: [...(replaceCells ? [] : current.cells), ...cells.map(inputToNotebookCell)],
        metadata: {
          beale: {
            ...current.metadata.beale,
            schemaVersion: 3,
            revision,
            contentRevision,
            updatedAt,
          },
        },
      };
      const entry = this.writeAndRegister(id, row.artifact_id, row.relative_path, row.title, notebook);
      this.database
        .prepare(
          `UPDATE app_server_runbooks
           SET content_hash = ?, size_bytes = ?, revision = ?, content_revision = ?, updated_at = ?
           WHERE id = ? AND workspace_id = ? AND revision = ?`,
        )
        .run(
          entry.contentHash,
          entry.sizeBytes,
          revision,
          contentRevision,
          updatedAt,
          id,
          this.context.workspaceId,
          input.expectedRevision,
        );
      this.recordRevision(id, revision, updatedAt);
      recordModelAuthorship(this.database, "runbook", id, revision, author, updatedAt);
      this.database.exec("COMMIT");
      const updated = this.readRow(id);
      if (!updated) throw new Error(`Runbook disappeared after append: ${id}`);
      return { runbook: this.toRecord(updated, notebook.cells.length), artifactRef: artifactRef(entry, row.title) };
    } catch (error) {
      this.database.exec("ROLLBACK");
      throw error;
    }
  }

  public configure(input: {
    id: string;
    expectedRevision: number;
    enabledFeatures: string[];
    cellFeatures?: Array<{ cellId: string; features: string[] }>;
    cellExecutors?: Array<{ cellId: string; executor: RunbookCellExecutor }>;
  }, author?: ModelAuthor): { runbook: RunbookRecord; artifactRef: ResearchArtifactRef } {
    const id = requiredText(input.id, "id", 200);
    if (!Number.isSafeInteger(input.expectedRevision) || input.expectedRevision < 1) throw new Error("expectedRevision must be a positive integer.");
    const enabledFeatures = validateFeatures(input.enabledFeatures, "enabledFeatures", false);
    const changes = (input.cellFeatures ?? []).map((change, index) => {
      if (!isRecord(change)) throw new Error(`cellFeatures[${index}] must be an object.`);
      return {
        cellId: requiredText(change.cellId, `cellFeatures[${index}].cellId`, 200),
        features: validateCellFeatures(change.features, `cellFeatures[${index}].features`),
      };
    });
    if (new Set(changes.map((change) => change.cellId)).size !== changes.length) throw new Error("cellFeatures cannot update the same cell more than once.");
    const executorChanges = (input.cellExecutors ?? []).map((change, index) => {
      if (!isRecord(change)) throw new Error(`cellExecutors[${index}] must be an object.`);
      return {
        cellId: requiredText(change.cellId, `cellExecutors[${index}].cellId`, 200),
        executor: validateCellExecutor(change.executor, `cellExecutors[${index}].executor`),
      };
    });
    if (new Set(executorChanges.map((change) => change.cellId)).size !== executorChanges.length) throw new Error("cellExecutors cannot update the same cell more than once.");
    this.database.exec("BEGIN IMMEDIATE");
    try {
      const row = this.readRow(id);
      if (!row) throw new Error(`Runbook not found in this workspace: ${id}`);
      this.requireCanonical(row);
      if (row.revision !== input.expectedRevision) {
        throw new Error(`Runbook revision conflict for ${id}: expected ${input.expectedRevision}, found ${row.revision}.`);
      }
      const notebook = this.readNotebook(row);
      for (const change of changes) {
        const cellIndex = notebook.cells.findIndex((cell, index) => notebookCellId(cell, index) === change.cellId);
        if (cellIndex < 0) throw new Error(`Runbook cell not found: ${change.cellId}`);
        const cell = notebook.cells[cellIndex]!;
        const beale = isRecord(cell.metadata.beale) ? cell.metadata.beale : {};
        cell.metadata.beale = { ...beale, features: change.features };
      }
      for (const change of executorChanges) {
        const cellIndex = notebook.cells.findIndex((cell, index) => notebookCellId(cell, index) === change.cellId);
        if (cellIndex < 0) throw new Error(`Runbook cell not found: ${change.cellId}`);
        const cell = notebook.cells[cellIndex]!;
        if (cell.cell_type !== "code") throw new Error(`Only code cells can select an executor: ${change.cellId}`);
        const beale = isRecord(cell.metadata.beale) ? cell.metadata.beale : {};
        cell.metadata.beale = { ...beale, executor: change.executor };
      }
      const revision = row.revision + 1;
      const contentRevision = row.content_revision + 1;
      const updatedAt = new Date().toISOString();
      notebook.metadata.beale = {
        ...notebook.metadata.beale,
        schemaVersion: 3,
        enabledFeatures,
        revision,
        contentRevision,
        updatedAt,
      };
      const entry = this.writeAndRegister(id, row.artifact_id, row.relative_path, row.title, notebook);
      const result = this.database.prepare(`UPDATE app_server_runbooks
        SET content_hash = ?, size_bytes = ?, revision = ?, content_revision = ?, updated_at = ?
        WHERE id = ? AND workspace_id = ? AND revision = ?`).run(
        entry.contentHash, entry.sizeBytes, revision, contentRevision, updatedAt,
        id, this.context.workspaceId, input.expectedRevision,
      );
      if (Number(result.changes) !== 1) throw new Error(`Runbook revision conflict for ${id}.`);
      this.recordRevision(id, revision, updatedAt);
      recordModelAuthorship(this.database, "runbook", id, revision, author, updatedAt);
      this.database.exec("COMMIT");
      const updated = this.readRow(id);
      if (!updated) throw new Error(`Runbook disappeared after feature configuration: ${id}`);
      return { runbook: this.toRecord(updated, notebook.cells.length), artifactRef: artifactRef(entry, row.title) };
    } catch (error) {
      this.database.exec("ROLLBACK");
      throw error;
    }
  }

  public editCell(input: {
    id: string;
    expectedRevision: number;
    cellId: string;
    source: string;
  }, author?: ModelAuthor): { runbook: RunbookRecord; artifactRef: ResearchArtifactRef } {
    const id = requiredText(input.id, "id", 200);
    const cellId = requiredText(input.cellId, "cellId", 200);
    if (!Number.isSafeInteger(input.expectedRevision) || input.expectedRevision < 1) throw new Error("expectedRevision must be a positive integer.");
    const source = requiredText(input.source, "cell source", 64_000);
    this.database.exec("BEGIN IMMEDIATE");
    try {
      const row = this.readRow(id);
      if (!row) throw new Error(`Runbook not found in this workspace: ${id}`);
      this.requireCanonical(row);
      if (row.revision !== input.expectedRevision) {
        throw new Error(`Runbook revision conflict for ${id}: expected ${input.expectedRevision}, found ${row.revision}.`);
      }
      const notebook = this.readNotebook(row);
      const cell = requireNotebookCell(notebook, cellId);
      cell.source = sourceLines(source);
      cell.execution_count = null;
      cell.outputs = [];
      const cellMetadata = isRecord(cell.metadata.beale) ? cell.metadata.beale : {};
      delete cellMetadata.latestRun;
      delete cellMetadata.exitCode;
      cell.metadata.beale = cellMetadata;
      const revision = row.revision + 1;
      const contentRevision = row.content_revision + 1;
      const updatedAt = new Date().toISOString();
      notebook.metadata.beale = { ...notebook.metadata.beale, schemaVersion: 3, revision, contentRevision, updatedAt };
      const entry = this.writeAndRegister(id, row.artifact_id, row.relative_path, row.title, notebook);
      const result = this.database.prepare(`UPDATE app_server_runbooks
        SET content_hash = ?, size_bytes = ?, revision = ?, content_revision = ?, updated_at = ?
        WHERE id = ? AND workspace_id = ? AND revision = ?`).run(
        entry.contentHash, entry.sizeBytes, revision, contentRevision, updatedAt,
        id, this.context.workspaceId, input.expectedRevision,
      );
      if (Number(result.changes) !== 1) throw new Error(`Runbook revision conflict for ${id}.`);
      this.recordRevision(id, revision, updatedAt);
      recordModelAuthorship(this.database, "runbook", id, revision, author, updatedAt);
      this.database.exec("COMMIT");
      const updated = this.readRow(id);
      if (!updated) throw new Error(`Runbook disappeared after cell edit: ${id}`);
      return { runbook: this.toRecord(updated, notebook.cells.length), artifactRef: artifactRef(entry, row.title) };
    } catch (error) {
      this.database.exec("ROLLBACK");
      throw error;
    }
  }

  public executionPlan(id: string, selection: RunbookExecutionSelection = {}): RunbookExecutionPlanCell[] {
    const row = this.readRow(requiredText(id, "id", 200));
    if (!row) throw new Error(`Runbook not found in this workspace: ${id}`);
    this.requireCanonical(row);
    const cellId = optionalText(selection.cellId, "cellId", 200);
    const startCellId = optionalText(selection.startCellId, "startCellId", 200);
    const endCellId = optionalText(selection.endCellId, "endCellId", 200);
    if (cellId && (startCellId || endCellId)) {
      throw new Error("cellId cannot be combined with startCellId or endCellId.");
    }
    const notebook = this.readNotebook(row);
    const enabledFeatures = notebookEnabledFeatures(notebook);
    const executableCells = notebook.cells
      .map((cell, index) => ({ cell, id: notebookCellId(cell, index) }))
      .filter(({ cell }) => cell.cell_type === "code")
      .map(({ cell, id: candidateId }) => {
        const appServer = isRecord(cell.metadata.beale) ? cell.metadata.beale : {};
        const vscode = isRecord(cell.metadata.vscode) ? cell.metadata.vscode : {};
        return {
          id: candidateId,
          source: cell.source.join(""),
          language: typeof appServer.language === "string"
            ? appServer.language
            : typeof vscode.languageId === "string"
              ? vscode.languageId
              : null,
          executor: notebookCellExecutor(cell),
          features: cellFeatures(cell),
          ...(typeof appServer.cwd === "string" ? { cwd: appServer.cwd } : {}),
          active: cellIsActive(cell, enabledFeatures),
        };
      });
    if (executableCells.length === 0) throw new Error("Runbook has no executable code cells.");
    if (cellId) {
      const cell = executableCells.find((candidate) => candidate.id === cellId);
      if (!cell) throw new Error(`Code cell not found in runbook ${id}: ${cellId}`);
      if (!cell.active) throw new Error(`Code cell is deactivated by runbook feature toggles: ${cellId}`);
      return [{ id: cell.id, source: cell.source, language: cell.language, executor: cell.executor, features: cell.features, ...(cell.cwd ? { cwd: cell.cwd } : {}) }];
    }
    const startIndex = startCellId
      ? executableCells.findIndex((candidate) => candidate.id === startCellId)
      : 0;
    const endIndex = endCellId
      ? executableCells.findIndex((candidate) => candidate.id === endCellId)
      : executableCells.length - 1;
    if (startCellId && startIndex < 0) throw new Error(`Start code cell not found in runbook ${id}: ${startCellId}`);
    if (endCellId && endIndex < 0) throw new Error(`End code cell not found in runbook ${id}: ${endCellId}`);
    if (startIndex > endIndex) throw new Error("startCellId must precede or equal endCellId in runbook order.");
    const selected = executableCells.slice(startIndex, endIndex + 1).filter((cell) => cell.active);
    if (selected.length === 0) throw new Error("No active code cells remain in the selected runbook range.");
    return selected.map(({ id: selectedId, source, language, executor, features, cwd }) => ({ id: selectedId, source, language, executor, features, ...(cwd ? { cwd } : {}) }));
  }

  public getExecution(id: string, runId: string, options: {
    offset?: number;
    limit?: number;
    startCellId?: string;
    endCellId?: string;
  } = {}, requestedWorkspaceId?: string) {
    const workspaceId = requestedWorkspaceId ?? this.context.workspaceId;
    if (workspaceId !== this.context.workspaceId) {
      const runbook = this.readRowForWorkspace(id, workspaceId);
      if (!runbook || runbook.subject_id !== this.context.subjectId) {
        throw new Error(`Runbook not found in a same-Subject workspace: ${id}`);
      }
    }
    const limit = Math.max(1, Math.min(50, Math.floor(options.limit ?? 12)));
    if (options.offset !== undefined && (options.startCellId || options.endCellId)) {
      throw new Error("offset cannot be combined with startCellId or endCellId.");
    }
    const row = this.database.prepare(`SELECT * FROM app_server_runbook_executions
      WHERE runbook_id = ? AND run_id = ? AND workspace_id = ?`).get(id, runId, workspaceId) as RunbookExecutionRow | undefined;
    if (!row) throw new Error(`Runbook execution not found in the selected workspace: ${runId}`);
    const snapshot: unknown = typeof row.snapshot_json === "string" ? JSON.parse(row.snapshot_json) : null;
    const plan = isRecord(snapshot) && Array.isArray(snapshot.cells) ? snapshot.cells.filter(isRecord) : [];
    const identifiedPlan: Array<Record<string, unknown> & { id: string }> = plan
      .map((cell) => ({ ...cell, id: requiredText(cell.id, "cell.id", 200) }));
    const range = cellRange(identifiedPlan, options.startCellId, options.endCellId, id);
    const ranged = Boolean(options.startCellId || options.endCellId);
    const offset = ranged
      ? range.start
      : Math.max(0, Math.min(plan.length, Math.floor(options.offset ?? 0)));
    const rangeEnd = ranged ? range.end + 1 : plan.length;
    const end = Math.min(rangeEnd, offset + limit);
    const cells = identifiedPlan.slice(offset, end).map((cell) => {
      const recorded = this.database.prepare(`SELECT result_json FROM app_server_runbook_cell_executions
        WHERE run_id = ? AND cell_id = ?`).get(runId, requiredText(cell.id, "cell.id", 200)) as { result_json: string } | undefined;
      const result: unknown = recorded ? JSON.parse(recorded.result_json) : null;
      return {
        id: requiredText(cell.id, "cell.id", 200), source: typeof cell.source === "string" ? cell.source : "",
        language: typeof cell.language === "string" ? cell.language : null,
        executor: validateCellExecutor(cell.executor, "cell.executor"),
        result: isRecord(result) ? result : null,
      };
    });
    return {
      runbookId: id, runId, status: row.status,
      snapshotAvailable: isRecord(snapshot),
      contentRevision: row.content_revision ?? null, contentHash: row.content_hash ?? null,
      sourceRevision: row.source_revision ?? null, environmentFingerprint: row.environment_fingerprint ?? null,
      sessionId: row.session_id ?? null, actorId: row.actor_id ?? null,
      fullRun: row.full_run === 1, startedAt: row.started_at, completedAt: row.completed_at,
      selectedCellIds: typeof row.selected_cell_ids_json === "string" ? JSON.parse(row.selected_cell_ids_json) as string[] : [],
      requiredCellIds: typeof row.required_cell_ids_json === "string" ? JSON.parse(row.required_cell_ids_json) as string[] : [],
      cells, totalCells: plan.length, offset, limit,
      nextOffset: !ranged && end < rangeEnd ? end : null,
      nextCellId: ranged && end < rangeEnd ? identifiedPlan[end]?.id ?? null : null,
      previousOffset: !ranged && offset > 0 ? Math.max(0, offset - limit) : null,
    };
  }

  public beginExecution(
    id: string,
    runId: string,
    cellIds: readonly string[],
    proofTarget: RunbookProofTarget,
    deviceOs?: string,
    provenance: RunbookExecutionProvenance = {},
  ): void {
    const startedAt = new Date().toISOString();
    let snapshotJson = "";
    let requiredCellIds: string[] = [];
    let contentRevision = 0;
    this.updateNotebook(id, (notebook, row) => {
      contentRevision = row.content_revision;
      if (provenance.expectedContentRevision !== undefined && provenance.expectedContentRevision !== contentRevision) {
        throw new Error("Runbook content changed before execution; read the current runbook and retry.");
      }
      const plan = this.executionPlan(id);
      requiredCellIds = plan.map((cell) => cell.id);
      if (cellIds.length === 0 || new Set(cellIds).size !== cellIds.length
        || cellIds.some((cellId) => !requiredCellIds.includes(cellId))) {
        throw new Error("Runbook execution selection must contain unique active code cells.");
      }
      snapshotJson = JSON.stringify({ contentRevision, enabledFeatures: notebookEnabledFeatures(notebook), cells: plan });
      notebook.metadata.beale.latestRun = {
        runId,
        status: "running",
        startedAt,
        cellCount: cellIds.length,
        proofTarget,
        ...(deviceOs ? { deviceOs } : {}),
      };
      notebook.cells.forEach((cell, index) => {
        const cellId = notebookCellId(cell, index);
        if (!cellIds.includes(cellId)) return;
        setCellExecution(cell, { runId, status: "queued", startedAt, proofTarget, ...(deviceOs ? { deviceOs } : {}) });
      });
    }, () => {
      this.database.prepare(`INSERT INTO app_server_runbook_executions (
        run_id, runbook_id, workspace_id, status, proof_target, device_os,
        started_at, completed_at, duration_ms, error, selected_cell_count, completed_cell_count,
        content_revision, content_hash, snapshot_json, selected_cell_ids_json, required_cell_ids_json,
        source_revision, environment_fingerprint, session_id, actor_id, full_run
      ) VALUES (?, ?, ?, 'running', ?, ?, ?, NULL, NULL, NULL, ?, 0, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`).run(
        runId,
        id,
        this.context.workspaceId,
        proofTarget,
        deviceOs ?? null,
        startedAt,
        cellIds.length,
        contentRevision,
        `sha256:${createHash("sha256").update(snapshotJson).digest("hex")}`,
        snapshotJson,
        JSON.stringify(cellIds),
        JSON.stringify(requiredCellIds),
        provenance.sourceRevision?.trim() || null,
        provenance.environmentFingerprint?.trim() || null,
        this.context.sessionId ?? null,
        provenance.actorId?.trim() || null,
        JSON.stringify(cellIds) === JSON.stringify(requiredCellIds) ? 1 : 0,
      );
    });
  }

  public beginCellExecution(
    id: string,
    runId: string,
    cellId: string,
    proofTarget: RunbookProofTarget,
    deviceOs?: string,
  ): void {
    const startedAt = new Date().toISOString();
    this.updateNotebook(id, (notebook) => {
      const cell = requireNotebookCell(notebook, cellId);
      setCellExecution(cell, { runId, status: "running", startedAt, proofTarget, ...(deviceOs ? { deviceOs } : {}) });
    });
  }

  public completeCellExecution(input: {
    id: string;
    runId: string;
    cellId: string;
    status: "succeeded" | "failed" | "blocked";
    startedAt: string;
    completedAt: string;
    durationMs: number;
    stdout?: string;
    stderr?: string;
    exitCode?: number | null;
    error?: string;
    proofTarget: RunbookProofTarget;
    deviceOs?: string;
    evidence?: Record<string, string | number | boolean | null>;
  }): void {
    this.updateNotebook(input.id, (notebook) => {
      const cell = requireNotebookCell(notebook, input.cellId);
      setCellExecution(cell, {
        runId: input.runId,
        status: input.status,
        startedAt: input.startedAt,
        completedAt: input.completedAt,
        durationMs: Math.max(0, Math.round(input.durationMs)),
        ...(input.exitCode !== undefined ? { exitCode: input.exitCode } : {}),
        ...(input.error ? { error: input.error.slice(0, 2_000) } : {}),
        proofTarget: input.proofTarget,
        ...(input.deviceOs ? { deviceOs: input.deviceOs } : {}),
        ...(input.evidence ? { evidence: input.evidence } : {}),
      });
      const appServer = isRecord(cell.metadata.beale) ? cell.metadata.beale : {};
      cell.metadata.beale = {
        ...appServer,
        ...(input.exitCode !== undefined ? { exitCode: input.exitCode } : {}),
      };
      cell.execution_count = (cell.execution_count ?? 0) + 1;
      cell.outputs = [];
      if (input.stdout) cell.outputs.push({ output_type: "stream", name: "stdout", text: sourceLines(input.stdout) });
      if (input.stderr) cell.outputs.push({ output_type: "stream", name: "stderr", text: sourceLines(input.stderr) });
      if (input.error && !input.stderr) {
        cell.outputs.push({ output_type: "error", ename: input.status, evalue: input.error, traceback: [input.error] });
      }
    }, () => {
      const selected = this.database.prepare(`SELECT 1 FROM app_server_runbook_executions run,
          json_each(run.selected_cell_ids_json) cell
        WHERE run.run_id = ? AND run.runbook_id = ? AND run.workspace_id = ?
          AND run.status = 'running' AND cell.value = ?`).get(input.runId, input.id, this.context.workspaceId, input.cellId);
      if (!selected) throw new Error("Cell is not part of the active runbook execution.");
      this.database.prepare(`INSERT INTO app_server_runbook_cell_executions
        (run_id, cell_id, status, exit_code, result_json) VALUES (?, ?, ?, ?, ?)`).run(
        input.runId, input.cellId, input.status, input.exitCode ?? null,
        JSON.stringify({ ...input, stdout: input.stdout?.slice(0, 64_000), stderr: input.stderr?.slice(0, 64_000) }),
      );
      const result = this.database.prepare(`UPDATE app_server_runbook_executions
        SET completed_cell_count = completed_cell_count + 1
        WHERE run_id = ? AND runbook_id = ? AND workspace_id = ? AND status = 'running'`).run(
        input.runId,
        input.id,
        this.context.workspaceId,
      );
      if (Number(result.changes) !== 1) {
        throw new Error(`Runbook execution ledger entry is missing or already complete: ${input.runId}`);
      }
    });
  }

  public skipCellExecutions(
    id: string,
    runId: string,
    cellIds: readonly string[],
    reason: string,
    proofTarget: RunbookProofTarget,
    deviceOs?: string,
  ): void {
    if (cellIds.length === 0) return;
    const completedAt = new Date().toISOString();
    this.updateNotebook(id, (notebook) => {
      notebook.cells.forEach((cell, index) => {
        const cellId = notebookCellId(cell, index);
        if (!cellIds.includes(cellId)) return;
        setCellExecution(cell, {
          runId,
          status: "skipped",
          startedAt: completedAt,
          completedAt,
          durationMs: 0,
          error: reason.slice(0, 2_000),
          proofTarget,
          ...(deviceOs ? { deviceOs } : {}),
        });
      });
    });
  }

  public completeExecution(input: {
    id: string;
    runId: string;
    status: "succeeded" | "failed" | "blocked";
    startedAt: string;
    completedAt: string;
    durationMs: number;
    error?: string;
    proofTarget: RunbookProofTarget;
    deviceOs?: string;
  }): void {
    this.updateNotebook(input.id, (notebook) => {
      notebook.metadata.beale.latestRun = {
        runId: input.runId,
        status: input.status,
        startedAt: input.startedAt,
        completedAt: input.completedAt,
        durationMs: Math.max(0, Math.round(input.durationMs)),
        ...(input.error ? { error: input.error.slice(0, 2_000) } : {}),
        proofTarget: input.proofTarget,
        ...(input.deviceOs ? { deviceOs: input.deviceOs } : {}),
      };
    }, () => {
      if (input.status === "succeeded") {
        const incomplete = this.database.prepare(`SELECT 1 FROM app_server_runbook_executions run,
            json_each(run.selected_cell_ids_json) selected
          LEFT JOIN app_server_runbook_cell_executions cell
            ON cell.run_id = run.run_id AND cell.cell_id = selected.value
          WHERE run.run_id = ? AND (cell.status IS NULL OR cell.status <> 'succeeded') LIMIT 1`).get(input.runId);
        if (incomplete) throw new Error("Successful runbook execution requires a successful recorded result for every selected cell.");
      }
      const result = this.database.prepare(`UPDATE app_server_runbook_executions SET
        status = ?, completed_at = ?, duration_ms = ?, error = ?
        WHERE run_id = ? AND runbook_id = ? AND workspace_id = ? AND status = 'running'`).run(
        input.status,
        input.completedAt,
        Math.max(0, Math.round(input.durationMs)),
        input.error?.slice(0, 2_000) ?? null,
        input.runId,
        input.id,
        this.context.workspaceId,
      );
      if (Number(result.changes) !== 1) {
        throw new Error(`Runbook execution ledger entry is missing or already complete: ${input.runId}`);
      }
    });
  }

  private updateNotebook(
    id: string,
    mutate: (notebook: RunbookNotebook, row: RunbookRow) => void,
    persistExecution?: () => void,
  ): void {
    this.database.exec("BEGIN IMMEDIATE");
    try {
      const row = this.readRow(requiredText(id, "id", 200));
      if (!row) throw new Error(`Runbook not found in this workspace: ${id}`);
      this.requireCanonical(row);
      const notebook = this.readNotebook(row);
      mutate(notebook, row);
      persistExecution?.();
      delete notebook.metadata.beale.status;
      const revision = row.revision + 1;
      const updatedAt = new Date().toISOString();
      notebook.metadata.beale = {
        ...notebook.metadata.beale,
        schemaVersion: 3,
        revision,
        updatedAt,
      };
      const entry = this.writeAndRegister(id, row.artifact_id, row.relative_path, row.title, notebook);
      const result = this.database.prepare(
        `UPDATE app_server_runbooks
         SET content_hash = ?, size_bytes = ?, revision = ?, updated_at = ?
         WHERE id = ? AND workspace_id = ? AND revision = ?`,
      ).run(entry.contentHash, entry.sizeBytes, revision, updatedAt, id, this.context.workspaceId, row.revision);
      if (Number(result.changes) !== 1) throw new Error(`Runbook revision conflict while recording execution state: ${id}`);
      this.database.exec("COMMIT");
    } catch (error) {
      this.database.exec("ROLLBACK");
      throw error;
    }
  }

  private readRow(id: string): RunbookRow | null {
    return this.readRowForWorkspace(id, this.context.workspaceId);
  }

  private readRowForWorkspace(id: string, workspaceId: string): RunbookRow | null {
    return (this.database
      .prepare("SELECT * FROM app_server_runbooks WHERE id = ? AND workspace_id = ?")
      .get(id, workspaceId) as unknown as RunbookRow | undefined) ?? null;
  }

  private recordRevision(artifactId: string, revision: number, createdAt: string): void {
    this.database.prepare(`INSERT INTO app_server_artifact_revisions (
      artifact_kind, artifact_id, workspace_id, session_id, revision, created_at, revision_kind
    ) VALUES ('runbook', ?, ?, ?, ?, ?, 'content')`).run(
      artifactId,
      this.context.workspaceId,
      this.context.sessionId ?? null,
      revision,
      createdAt,
    );
  }

  private toRecord(row: RunbookRow, knownCellCount?: number): RunbookRecord {
    const duplicates = this.database.prepare(`SELECT id, title, purpose, revision, duplicate_marked_at
      FROM app_server_runbooks
      WHERE workspace_id = ? AND duplicate_of_runbook_id = ?
      ORDER BY duplicate_marked_at, id`).all(row.workspace_id, row.id) as Array<{
        id: string; title: string; purpose: string; revision: number; duplicate_marked_at: string;
      }>;
    return {
      id: row.id,
      workspaceId: row.workspace_id,
      workspaceName: row.workspace_name,
      subjectId: row.subject_id,
      subjectName: row.subject_name,
      sessionId: row.session_id,
      title: row.title,
      purpose: row.purpose,
      artifactId: row.artifact_id,
      enabledFeatures: notebookEnabledFeatures(this.readNotebook(row)),
      cellCount: knownCellCount ?? this.readNotebook(row).cells.length,
      revision: row.revision,
      contentRevision: row.content_revision,
      duplicateOfRunbookId: row.duplicate_of_runbook_id,
      duplicateMarkedAt: row.duplicate_marked_at,
      duplicateRunbooks: duplicates.map((duplicate) => ({
        id: duplicate.id,
        title: duplicate.title,
        purpose: duplicate.purpose,
        revision: duplicate.revision,
        markedAt: duplicate.duplicate_marked_at,
      })),
      execution: this.readExecutionMetrics(row.id, row.workspace_id),
      createdAt: row.created_at,
      updatedAt: row.updated_at,
      authors: modelAuthorsForResource(this.database, "runbook", row.id),
    };
  }

  private requireCanonical(row: RunbookRow): void {
    if (row.duplicate_of_runbook_id) {
      throw new Error(`Runbook ${row.id} is marked as a duplicate of ${row.duplicate_of_runbook_id}; update or run the canonical runbook instead.`);
    }
  }

  private readExecutionMetrics(runbookId: string, workspaceId = this.context.workspaceId): RunbookExecutionMetrics {
    const totals = this.database.prepare(`SELECT
      COUNT(*) AS run_count,
      SUM(CASE WHEN status <> 'running' THEN 1 ELSE 0 END) AS completed_run_count,
      COALESCE(SUM(completed_cell_count), 0) AS executed_cell_count
      FROM app_server_runbook_executions
      WHERE runbook_id = ? AND workspace_id = ?`).get(runbookId, workspaceId) as {
        run_count?: unknown;
        completed_run_count?: unknown;
        executed_cell_count?: unknown;
      } | undefined;
    const latest = this.database.prepare(`SELECT run_id, status, started_at
      FROM app_server_runbook_executions
      WHERE runbook_id = ? AND workspace_id = ?
      ORDER BY started_at DESC, run_id DESC LIMIT 1`).get(runbookId, workspaceId) as {
        run_id?: unknown;
        status?: unknown;
        started_at?: unknown;
      } | undefined;
    const latestSuccessful = this.database.prepare(`SELECT run_id
      FROM app_server_runbook_executions run
      WHERE runbook_id = ? AND workspace_id = ? AND status = 'succeeded' AND full_run = 1
        AND content_revision = (SELECT content_revision FROM app_server_runbooks WHERE id = run.runbook_id)
        AND selected_cell_count = completed_cell_count
      ORDER BY completed_at DESC, started_at DESC, run_id DESC LIMIT 1`).get(runbookId, workspaceId) as {
        run_id?: unknown;
      } | undefined;
    return {
      runCount: numberOrZero(totals?.run_count),
      completedRunCount: numberOrZero(totals?.completed_run_count),
      executedCellCount: numberOrZero(totals?.executed_cell_count),
      latest: typeof latest?.run_id === "string" && isRunLedgerStatus(latest.status) && typeof latest.started_at === "string"
        ? { runId: latest.run_id, status: latest.status, startedAt: latest.started_at }
        : null,
      latestSuccessfulRunId: typeof latestSuccessful?.run_id === "string" ? latestSuccessful.run_id : null,
    };
  }

  private readNotebook(row: RunbookRow): RunbookNotebook {
    const path = this.absolutePath(row.relative_path);
    const parsed = JSON.parse(readFileSync(path, "utf8")) as unknown;
    if (!isRecord(parsed) || parsed.nbformat !== 4 || !Array.isArray(parsed.cells) || !isRecord(parsed.metadata)) {
      throw new Error(`Runbook artifact is not a supported app-server nbformat 4 notebook: ${row.id}`);
    }
    const appServerMetadata = isRecord(parsed.metadata.beale)
      ? parsed.metadata.beale
      : readPreBealeRecord(parsed.metadata);
    if (!appServerMetadata) {
      throw new Error(`Runbook artifact is not a supported app-server nbformat 4 notebook: ${row.id}`);
    }
    parsed.metadata.beale = appServerMetadata;
    for (const cell of parsed.cells) {
      if (!isRecord(cell) || !isRecord(cell.metadata) || isRecord(cell.metadata.beale)) continue;
      const previousMetadata = readPreBealeRecord(cell.metadata);
      if (previousMetadata) cell.metadata.beale = previousMetadata;
    }
    return parsed as unknown as RunbookNotebook;
  }

  private writeAndRegister(
    id: string,
    artifactId: string,
    relativePath: string,
    title: string,
    notebook: RunbookNotebook,
  ): ResearchStorageArtifactManifestEntry {
    const path = this.absolutePath(relativePath);
    mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
    const temporaryPath = `${path}.${randomUUID()}.tmp`;
    writeFileSync(temporaryPath, `${JSON.stringify(notebook, null, 2)}\n`, { encoding: "utf8", mode: 0o600 });
    renameSync(temporaryPath, path);
    chmodSync(path, 0o600);
    return registerResearchStorageArtifact(this.storageLayout, {
      id: artifactId,
      path,
      kind: "runbook",
      purpose: `Research runbook: ${title}`,
    });
  }

  private absolutePath(relativePath: string): string {
    const root = resolve(this.storageLayout.artifactDirectoryPath);
    const path = resolve(root, relativePath);
    const child = relative(root, path);
    if (!child || child === ".." || child.startsWith("../") || child.startsWith("..\\")) throw new Error("Runbook path escaped the artifact directory.");
    return path;
  }
}

function createNotebook(input: {
  id: string;
  title: string;
  purpose: string;
  context: MemoryContext;
  revision: number;
  contentRevision: number;
  createdAt: string;
  updatedAt: string;
  cells: RunbookCellInput[];
  enabledFeatures: string[];
}): RunbookNotebook {
  return {
    cells: [
      inputToNotebookCell({ kind: "markdown", source: `# ${input.title}\n\n${input.purpose}`, summary: "Runbook purpose", features: [...RUNBOOK_DEFAULT_FEATURES] }),
      ...input.cells.map(inputToNotebookCell),
    ],
    metadata: {
      beale: {
        schemaVersion: 3,
        artifactFamily: "runbook",
        runbookId: input.id,
        workspaceId: input.context.workspaceId,
        workspaceName: input.context.workspaceName,
        subjectId: input.context.subjectId ?? null,
        subjectName: input.context.subjectName ?? null,
        sessionId: input.context.sessionId ?? null,
        revision: input.revision,
        contentRevision: input.contentRevision,
        createdAt: input.createdAt,
        updatedAt: input.updatedAt,
        enabledFeatures: input.enabledFeatures,
      },
    },
    nbformat: 4,
    nbformat_minor: 5,
  };
}

function inputToNotebookCell(cell: RunbookCellInput): NotebookCell {
  const metadata: Record<string, unknown> = {
    beale: {
      ...(cell.language ? { language: cell.language } : {}),
      ...(cell.cwd ? { cwd: cell.cwd } : {}),
      features: cell.features,
      ...(cell.executor ? { executor: cell.executor } : {}),
      ...(cell.summary ? { summary: cell.summary } : {}),
      recordedAt: new Date().toISOString(),
      ...(cell.exitCode !== undefined ? { exitCode: cell.exitCode } : {}),
    },
    ...(cell.language ? { vscode: { languageId: cell.language } } : {}),
  };
  const id = `cell-${randomUUID()}`;
  if (cell.kind === "markdown") return { id, cell_type: "markdown", metadata, source: sourceLines(cell.source) };
  const outputs: Array<Record<string, unknown>> = [];
  if (cell.stdout) outputs.push({ output_type: "stream", name: "stdout", text: sourceLines(cell.stdout) });
  if (cell.stderr) outputs.push({ output_type: "stream", name: "stderr", text: sourceLines(cell.stderr) });
  return { id, cell_type: "code", metadata, source: sourceLines(cell.source), execution_count: null, outputs };
}

function notebookCellId(cell: NotebookCell, index: number): string {
  return typeof cell.id === "string" && cell.id.trim() ? cell.id.trim() : `cell-${index + 1}`;
}

function requireNotebookCell(notebook: RunbookNotebook, cellId: string): NotebookCell {
  const cell = notebook.cells.find((candidate, index) => notebookCellId(candidate, index) === cellId);
  if (!cell || cell.cell_type !== "code") throw new Error(`Runbook code cell not found: ${cellId}`);
  return cell;
}

function setCellExecution(cell: NotebookCell, execution: RunbookExecutionState): void {
  const appServer = isRecord(cell.metadata.beale) ? cell.metadata.beale : {};
  cell.metadata.beale = { ...appServer, latestRun: execution };
}

function notebookCellToRecord(cell: NotebookCell, index: number, enabledFeatures: readonly string[]): RunbookCellRecord {
  const appServer = isRecord(cell.metadata?.beale) ? cell.metadata.beale : {};
  const outputs = Array.isArray(cell.outputs) ? cell.outputs : [];
  return {
    id: notebookCellId(cell, index),
    index,
    kind: cell.cell_type,
    source: cell.source.join(""),
    features: cellFeatures(cell),
    active: cellIsActive(cell, enabledFeatures),
    executor: notebookCellExecutor(cell),
    ...(typeof appServer.language === "string" ? { language: appServer.language } : {}),
    ...(typeof appServer.cwd === "string" ? { cwd: appServer.cwd } : {}),
    ...(typeof appServer.summary === "string" ? { summary: appServer.summary } : {}),
    ...(typeof appServer.exitCode === "number" ? { exitCode: appServer.exitCode } : {}),
    ...streamOutput(outputs, "stdout"),
    ...streamOutput(outputs, "stderr"),
  };
}

function streamOutput(outputs: Array<Record<string, unknown>>, name: "stdout" | "stderr"): Partial<RunbookCellInput> {
  const text = outputs
    .filter((output) => output.output_type === "stream" && output.name === name)
    .flatMap((output) => Array.isArray(output.text) ? output.text.filter((item): item is string => typeof item === "string") : typeof output.text === "string" ? [output.text] : [])
    .join("");
  return text ? { [name]: text } : {};
}

function validateCell(value: RunbookCellInput): RunbookCellInput {
  if (!isRecord(value)) throw new Error("Each runbook cell must be an object.");
  if (value.kind !== "markdown" && value.kind !== "code") throw new Error("Runbook cell kind must be markdown or code.");
  const source = requiredText(value.source, "cell source", 64_000);
  const features = validateCellFeatures(value.features, "cell features");
  const executor = value.executor === undefined ? undefined : validateCellExecutor(value.executor, "cell executor");
  if (value.kind === "markdown" && executor) throw new Error("Markdown cells cannot select an executor.");
  const language = optionalText(value.language, "cell language", 40);
  const cwd = optionalText(value.cwd, "cell cwd", 4_096);
  const summary = optionalText(value.summary, "cell summary", 500);
  const stdout = optionalText(value.stdout, "cell stdout", 64_000, true);
  const stderr = optionalText(value.stderr, "cell stderr", 64_000, true);
  if (value.exitCode !== undefined && (!Number.isSafeInteger(value.exitCode) || value.exitCode < 0)) throw new Error("cell exitCode must be a non-negative integer.");
  return {
    kind: value.kind,
    source,
    features,
    ...(executor ? { executor } : {}),
    ...(language ? { language } : {}),
    ...(cwd ? { cwd } : {}),
    ...(summary ? { summary } : {}),
    ...(stdout !== undefined ? { stdout } : {}),
    ...(stderr !== undefined ? { stderr } : {}),
    ...(value.exitCode !== undefined ? { exitCode: value.exitCode } : {}),
  };
}

function notebookCellExecutor(cell: NotebookCell): RunbookCellExecutor {
  const beale = isRecord(cell.metadata.beale) ? cell.metadata.beale : {};
  return beale.executor === undefined
    ? { kind: "host", timeoutSeconds: RUNBOOK_DEFAULT_TIMEOUT_SECONDS }
    : validateCellExecutor(beale.executor, "cell executor");
}

function validateCellExecutor(value: unknown, field: string): RunbookCellExecutor {
  if (!isRecord(value)) throw new Error(`${field} must be an object.`);
  if (value.kind === "host") {
    const timeoutSeconds = value.timeoutSeconds === undefined
      ? RUNBOOK_DEFAULT_TIMEOUT_SECONDS
      : value.timeoutSeconds;
    if (typeof timeoutSeconds !== "number" || !Number.isSafeInteger(timeoutSeconds)
      || timeoutSeconds < 1 || timeoutSeconds > RUNBOOK_MAX_TIMEOUT_SECONDS) {
      throw new Error(`${field}.timeoutSeconds must be an integer from 1 to ${RUNBOOK_MAX_TIMEOUT_SECONDS}.`);
    }
    return { kind: "host", timeoutSeconds };
  }
  if (value.kind !== "tart-vm") throw new Error(`${field}.kind must be host or tart-vm.`);
  const artifactId = optionalText(value.artifactId, `${field}.artifactId`, 240);
  const workspacePath = optionalText(value.workspacePath, `${field}.workspacePath`, 4_096);
  if (Boolean(artifactId) === Boolean(workspacePath)) throw new Error(`${field} requires exactly one of artifactId or workspacePath.`);
  if (!Array.isArray(value.argv) || value.argv.length > 128 || value.argv.some((argument) => typeof argument !== "string" || argument.length > 4_096 || argument.includes("\0"))) {
    throw new Error(`${field}.argv must be an array of at most 128 bounded strings.`);
  }
  const timeoutSeconds = value.timeoutSeconds === undefined ? RUNBOOK_DEFAULT_TIMEOUT_SECONDS : value.timeoutSeconds;
  if (typeof timeoutSeconds !== "number" || !Number.isSafeInteger(timeoutSeconds) || timeoutSeconds < 1 || timeoutSeconds > RUNBOOK_MAX_TIMEOUT_SECONDS) throw new Error(`${field}.timeoutSeconds must be an integer from 1 to ${RUNBOOK_MAX_TIMEOUT_SECONDS}.`);
  const runAs = value.runAs === undefined ? "guest" : value.runAs;
  if (runAs !== "guest" && runAs !== "root") throw new Error(`${field}.runAs must be guest or root.`);
  const transport = value.transport === undefined ? "auto" : value.transport;
  if (transport !== "auto" && transport !== "guest-agent" && transport !== "ssh") {
    throw new Error(`${field}.transport must be auto, guest-agent, or ssh.`);
  }
  return {
    kind: "tart-vm",
    vmName: requiredText(value.vmName, `${field}.vmName`, 128),
    ...(artifactId ? { artifactId } : {}),
    ...(workspacePath ? { workspacePath } : {}),
    runAs,
    transport,
    argv: [...value.argv],
    timeoutSeconds,
    retainOnFailure: value.retainOnFailure === true,
  };
}

function notebookEnabledFeatures(notebook: RunbookNotebook): string[] {
  return Array.isArray(notebook.metadata.beale.enabledFeatures)
    ? validateFeatures(notebook.metadata.beale.enabledFeatures, "runbook enabledFeatures", false)
    : [...RUNBOOK_DEFAULT_FEATURES];
}

function cellFeatures(cell: NotebookCell): string[] {
  const beale = isRecord(cell.metadata.beale) ? cell.metadata.beale : {};
  return Array.isArray(beale.features)
    ? validateCellFeatures(beale.features, "cell features")
    : [...RUNBOOK_DEFAULT_FEATURES];
}

function cellIsActive(cell: NotebookCell, enabledFeatures: readonly string[]): boolean {
  const enabled = new Set(enabledFeatures);
  return cellFeatures(cell).some((feature) => enabled.has(feature));
}

function validateCellFeatures(value: unknown, field: string): string[] {
  const features = validateFeatures(value, field, true);
  if (!features.some((feature) => (RUNBOOK_DEFAULT_FEATURES as readonly string[]).includes(feature))) {
    throw new Error(`${field} must include setup, runtime, or cleanup.`);
  }
  return features;
}

function validateFeatures(value: unknown, field: string, requireNonEmpty: boolean): string[] {
  if (!Array.isArray(value)) throw new Error(`${field} must be an array of text tags.`);
  if (value.length > 32) throw new Error(`${field} can contain at most 32 tags.`);
  const features = [...new Set(value.map((feature, index) => requiredText(feature, `${field}[${index}]`, 64).toLowerCase()))];
  if (requireNonEmpty && features.length === 0) throw new Error(`${field} must contain at least one tag.`);
  return features;
}

function artifactRef(entry: ResearchStorageArtifactManifestEntry, title: string): ResearchArtifactRef {
  return {
    id: entry.id,
    kind: "runbook",
    uri: pathToFileURL(entry.path).href,
    summary: `Research runbook: ${title}`,
    contentHash: entry.contentHash,
  };
}

function sourceLines(source: string): string[] {
  const lines = source.match(/.*(?:\n|$)/g)?.filter(Boolean) ?? [];
  return lines.length > 0 ? lines : [""];
}

function requiredText(value: unknown, field: string, maxLength: number): string {
  if (typeof value !== "string" || !value.trim()) throw new Error(`${field} must be a non-empty string.`);
  if (value.length > maxLength) throw new Error(`${field} exceeds ${maxLength} characters.`);
  return value.trim();
}

function optionalText(value: unknown, field: string, maxLength: number, allowEmpty = false): string | undefined {
  if (value === undefined) return undefined;
  if (typeof value !== "string" || (!allowEmpty && !value.trim())) throw new Error(`${field} must be a string.`);
  if (value.length > maxLength) throw new Error(`${field} exceeds ${maxLength} characters.`);
  return allowEmpty ? value : value.trim();
}

function safeSegment(value: string): string {
  const segment = value.replace(/[^a-zA-Z0-9_-]+/g, "_");
  return segment || createHash("sha256").update(value).digest("hex").slice(0, 20);
}

function clampInteger(value: number, minimum: number, maximum: number): number {
  if (!Number.isFinite(value)) return minimum;
  return Math.max(minimum, Math.min(maximum, Math.floor(value)));
}

function uniqueStrings(values: readonly string[]): string[] {
  return [...new Set(values.map((value) => value.trim()).filter(Boolean))];
}

function cellRange<T extends { id: string }>(
  cells: readonly T[],
  startCellId: string | undefined,
  endCellId: string | undefined,
  runbookId: string,
): { start: number; end: number } {
  const start = startCellId ? cells.findIndex((cell) => cell.id === startCellId) : 0;
  const end = endCellId ? cells.findIndex((cell) => cell.id === endCellId) : cells.length - 1;
  if (startCellId && start < 0) throw new Error(`Start cell not found in runbook ${runbookId}: ${startCellId}`);
  if (endCellId && end < 0) throw new Error(`End cell not found in runbook ${runbookId}: ${endCellId}`);
  if (start > end && cells.length > 0) throw new Error("startCellId must precede or equal endCellId in runbook order.");
  return { start: Math.max(0, start), end: Math.max(-1, end) };
}

function numberOrZero(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : 0;
}

function isRunLedgerStatus(value: unknown): value is "running" | "succeeded" | "failed" | "blocked" {
  return value === "running" || value === "succeeded" || value === "failed" || value === "blocked";
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
