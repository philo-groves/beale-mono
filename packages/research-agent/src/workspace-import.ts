import { readFileSync } from "node:fs";
import { join } from "node:path";
import { MemoryGraphStore, type MemoryGraphStoreOptions } from "./memory-graph.js";
import { FindingStore } from "./findings.js";
import { RunbookStore, type RunbookCellInput } from "./runbooks.js";
import { ReportStore } from "./reports.js";
import { createResearchStorageLayout } from "./storage.js";
import { assertWorkspaceChild, atomicWorkspaceWrite, preserveWorkspaceFile, readPublishedWorkspaceFile } from "./workspace-project.js";
import type { WorkspacePublicationOptions } from "./workspace-publication.js";

type RecordValue = Record<string, unknown>;
function record(value: unknown): RecordValue {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("Expected a structured research record.");
  return value as RecordValue;
}
function unchangedExcept(before: RecordValue, after: RecordValue, allowed: string[]): void {
  for (const key of new Set([...Object.keys(before), ...Object.keys(after)])) {
    if (!allowed.includes(key) && JSON.stringify(before[key]) !== JSON.stringify(after[key])) throw new Error(`${key} is host-managed. Use the canonical transition/evidence operations instead of importing changes to it.`);
  }
}
function memoryDocument(content: string): { metadata: RecordValue; body: string } {
  const match = /```json\r?\n([\s\S]*?)\r?\n```\r?\n\r?\n([\s\S]*)$/u.exec(content);
  if (!match) throw new Error("Memory file must retain its JSON metadata block.");
  return { metadata: record(JSON.parse(match[1]!)), body: match[2]!.replace(/\n$/u, "") };
}

/** Imports one explicit edit through existing revision and evidence validation, never by replaying Git. */
export function importWorkspaceResearchFile(options: WorkspacePublicationOptions, path: string, expectedRevision: number, resolvedProfile?: MemoryGraphStoreOptions['resolvedProfile']): void {
  assertWorkspaceChild(options.workspaceRoot, join(options.workspaceRoot, path));
  if (!Number.isSafeInteger(expectedRevision) || expectedRevision < 1) throw new Error("expectedRevision must be a positive integer.");
  const baseline = readPublishedWorkspaceFile(options.workspaceRoot, path);
  const edited = readFileSync(join(options.workspaceRoot, path), "utf8");
  if (Buffer.byteLength(edited) > 5 * 1024 * 1024) throw new Error("Research import exceeds the 5 MiB limit.");
  const context = { workspaceId: options.workspaceId, workspaceName: options.workspaceId, subjectId: `subject_workspace:${options.workspaceId}`, subjectName: options.workspaceId };
  const layout = createResearchStorageLayout({ ...options, artifactDirectoryPath: options.artifactDirectoryPath });
  preserveWorkspaceFile(options.workspaceRoot, join(options.workspaceRoot, path));
  if (/^claims\/[^/]+\.json$/u.test(path)) {
    const before = record(JSON.parse(baseline));
    const after = record(JSON.parse(edited));
    context.subjectId = String(before.subject_id ?? context.subjectId);
    unchangedExcept(before, after, ["title", "summary", "impact", "confidence", "classification"]);
    if (before.revision !== expectedRevision || before.workspace_id !== options.workspaceId) throw new Error("Claim import revision or workspace mismatch.");
    const graph = new MemoryGraphStore({ workspaceRoot: options.workspaceRoot, databasePath: options.databasePath, context, ...(resolvedProfile ? { resolvedProfile } : {}) });
    const claims = new FindingStore(graph);
    try {
      claims.revise(String(before.id), { expectedRevision, reason: "Explicit workspace file import", title: after.title as string, summary: after.summary as string, impact: after.impact as string, confidence: after.confidence as number, classification: after.classification as string });
    } finally { claims.close(); graph.close(); }
  } else if (/^memories\/[^/]+\.md$/u.test(path)) {
    const before = memoryDocument(baseline);
    const after = memoryDocument(edited);
    context.subjectId = String(before.metadata.subject_id ?? context.subjectId);
    context.subjectName = String(before.metadata.subject_name ?? context.subjectName);
    unchangedExcept(before.metadata, after.metadata, ["title", "summary", "confidence"]);
    if (before.metadata.revision !== expectedRevision || before.metadata.workspace_id !== options.workspaceId) throw new Error("Memory import revision or workspace mismatch.");
    const graph = new MemoryGraphStore({ workspaceRoot: options.workspaceRoot, databasePath: options.databasePath, context, ...(resolvedProfile ? { resolvedProfile } : {}) });
    try {
      graph.correct(String(before.metadata.id), expectedRevision, { title: after.metadata.title as string, summary: after.metadata.summary as string, confidence: after.metadata.confidence as number, body: after.body });
    } finally { graph.close(); }
  } else if (/^reports\/[^/]+\/report\.md$/u.test(path)) {
    const metadata = record(JSON.parse(readPublishedWorkspaceFile(options.workspaceRoot, path.replace(/report\.md$/u, "record.json"))));
    if (metadata.revision !== expectedRevision || metadata.workspace_id !== options.workspaceId) throw new Error("Report import revision or workspace mismatch.");
    const store = new ReportStore(options.databasePath, layout, context);
    try { store.revise({ id: String(metadata.id), expectedRevision, content: edited }); }
    finally { store.close(); }
  } else if (/^runbooks\/[^/]+\/runbook\.ipynb$/u.test(path)) {
    const before = record(JSON.parse(baseline));
    const after = record(JSON.parse(edited));
    unchangedExcept(before, after, ["cells"]);
    const metadata = record(record(before.metadata).beale);
    if (metadata.revision !== expectedRevision || metadata.workspaceId !== options.workspaceId) throw new Error("Runbook import revision or workspace mismatch.");
    if (!Array.isArray(after.cells) || !Array.isArray(before.cells) || after.cells.length !== before.cells.length) throw new Error("Runbook file import edits existing cell sources; use runbook.append for new cells.");
    const cells = after.cells.map((value, index): RunbookCellInput => {
      const cell = record(value);
      const original = record((before.cells as unknown[])[index]);
      unchangedExcept(original, cell, ["source"]);
      const metadata = record(record(cell.metadata).beale);
      if (!Array.isArray(cell.source) || cell.source.some((line) => typeof line !== "string")) throw new Error("Runbook cell source must be an array of strings.");
      return { kind: cell.cell_type as RunbookCellInput["kind"], source: cell.source.join(""), features: metadata.features as string[], ...(metadata.executor ? { executor: metadata.executor as NonNullable<RunbookCellInput["executor"]> } : {}), ...(typeof metadata.language === "string" ? { language: metadata.language } : {}) };
    });
    const store = new RunbookStore(options.databasePath, layout, context);
    try { store.append({ id: String(metadata.runbookId), expectedRevision, cells }, undefined, true); }
    finally { store.close(); }
  } else throw new Error("Import supports claim prose, memory prose, report content, and existing runbook cell sources. Evidence and status changes require canonical research operations.");
  // Restore the known published bytes only after validated persistence; the next publication replaces them.
  // The operator's imported bytes remain available in the recovery store even if publication fails.
  atomicWorkspaceWrite(options.workspaceRoot, path, baseline);
}
