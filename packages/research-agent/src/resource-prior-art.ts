import { randomUUID } from "node:crypto";
import type { DatabaseSync } from "node:sqlite";
import { openResearchDatabase } from "./database.js";
import type { PriorArtSearchResult } from "./prior-art-tools.js";
import type { PublicDocument } from "./public-document-tools.js";

export interface ResourcePriorArtSummary {
  id: string;
  resourceId: string;
  kind: "search" | "document";
  title: string;
  recordedAt: string;
  sessionId: string | null;
  revision: string | null;
  disposition: string;
  resultCount: number;
}
export interface ResourcePriorArtPage {
  entries: ResourcePriorArtSummary[];
  nextBefore: number | null;
}
export interface ResourcePriorArtRecordPage {
  historyId: string;
  resourceId: string;
  recordIndex: number;
  textOffset: number;
  nextTextOffset: number | null;
  totalCharacters: number;
  text: string;
}
export type ResourcePriorArtDetail = ResourcePriorArtSummary & (
  | { kind: "search"; data: PriorArtSearchResult; offset: number; nextOffset: number | null }
  | { kind: "document"; data: PublicDocument; offset: number; nextOffset: number | null }
);
export interface ResourcePriorArtContext {
  store: ResourcePriorArtStore;
  sessionId?: string | undefined;
}

/** Immutable source observations, scoped to a workspace and a resource identity. */
export class ResourcePriorArtStore {
  readonly #db: DatabaseSync;
  readonly #ownsConnection: boolean;
  constructor(database: string | DatabaseSync, readonly workspaceId: string) {
    this.#ownsConnection = typeof database === "string";
    this.#db = typeof database === "string" ? openResearchDatabase(database) : database;
    this.#db.exec(`CREATE TABLE IF NOT EXISTS resource_prior_art (
      sequence INTEGER PRIMARY KEY AUTOINCREMENT,
      id TEXT NOT NULL UNIQUE, workspace_id TEXT NOT NULL, resource_id TEXT NOT NULL,
      action_id TEXT NOT NULL, kind TEXT NOT NULL, title TEXT NOT NULL,
      recorded_at TEXT NOT NULL, session_id TEXT, revision TEXT,
      disposition TEXT NOT NULL, result_count INTEGER NOT NULL, data_json TEXT NOT NULL,
      UNIQUE(workspace_id, resource_id, action_id)
    );
    CREATE INDEX IF NOT EXISTS resource_prior_art_resource
      ON resource_prior_art(workspace_id, resource_id, sequence DESC);`);
  }
  close(): void { if (this.#ownsConnection) this.#db.close(); }

  requireResource(value: unknown): string {
    if (typeof value !== "string" || !value.trim()) throw new Error("resourceId is required to save prior art. Obtain it from resource.catalog.");
    const row = this.#db.prepare("SELECT id FROM app_server_research_resources WHERE workspace_id = ? AND (id = ? OR scope_asset_id = ?)").get(this.workspaceId, value, value) as { id: string } | undefined;
    if (!row) throw new Error("Resource not found in this workspace.");
    return row.id;
  }

  save(resourceId: string, actionId: string, data: PriorArtSearchResult | PublicDocument, context: { sessionId?: string | undefined; revision?: string } = {}): string {
    resourceId = this.requireResource(resourceId);
    const search = "records" in data;
    const id = `prior_art_${randomUUID()}`;
    this.#db.prepare(`INSERT OR IGNORE INTO resource_prior_art
      (id, workspace_id, resource_id, action_id, kind, title, recorded_at, session_id, revision, disposition, result_count, data_json)
      VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`).run(
      id, this.workspaceId, resourceId, actionId, search ? "search" : "document",
      (search ? data.query : data.title ?? data.url).slice(0, 300), data.fetchedAt, context.sessionId ?? null,
      context.revision?.slice(0, 200) ?? null, search ? data.disposition : "source_saved", search ? data.records.length : 1, JSON.stringify(data),
    );
    return (this.#db.prepare("SELECT id FROM resource_prior_art WHERE workspace_id = ? AND resource_id = ? AND action_id = ?")
      .get(this.workspaceId, resourceId, actionId) as { id: string }).id;
  }

  list(resourceIds: readonly string[], before?: number, limit = 20): ResourcePriorArtPage {
    const ids = checkedIds(resourceIds);
    if (ids.length === 0) return { entries: [], nextBefore: null };
    const size = integer(limit, 1, 20);
    const rows = this.#db.prepare(`SELECT sequence, id, resource_id, kind, title, recorded_at, session_id, revision, disposition, result_count
      FROM resource_prior_art WHERE workspace_id = ? AND resource_id IN (${ids.map(() => "?").join(",")})
      AND sequence < ? ORDER BY sequence DESC LIMIT ?`)
      .all(this.workspaceId, ...ids, before === undefined ? Number.MAX_SAFE_INTEGER : integer(before, 1), size + 1) as Record<string, unknown>[];
    return { entries: rows.slice(0, size).map(summary), nextBefore: rows.length > size ? Number(rows[size - 1]!.sequence) : null };
  }

  get(resourceIds: readonly string[], id: string, offset = 0): ResourcePriorArtDetail {
    const ids = checkedIds(resourceIds);
    const start = integer(offset, 0);
    const row = ids.length ? this.#db.prepare(`SELECT * FROM resource_prior_art WHERE workspace_id = ? AND resource_id IN (${ids.map(() => "?").join(",")}) AND id = ?`)
      .get(this.workspaceId, ...ids, id) as Record<string, unknown> | undefined : undefined;
    if (!row) throw new Error("Saved prior art not found for this resource.");
    const base = summary(row);
    if (base.kind === "search") {
      const data = JSON.parse(String(row.data_json)) as PriorArtSearchResult;
      return { ...base, kind: "search", data: { ...data, records: data.records.slice(start, start + 5) }, offset: start, nextOffset: start + 5 < data.records.length ? start + 5 : null };
    }
    const data = JSON.parse(String(row.data_json)) as PublicDocument;
    // Page text and links together; retain the original hash and retrieval metadata.
    const pages = Math.max(Math.ceil(data.text.length / 8000), Math.ceil(data.links.length / 20));
    return { ...base, kind: "document", data: { ...data, text: data.text.slice(start * 8000, (start + 1) * 8000), links: data.links.slice(start * 20, (start + 1) * 20) }, offset: start, nextOffset: start + 1 < pages ? start + 1 : null };
  }

  readRecord(resourceIds: readonly string[], id: string, recordIndex: number, textOffset = 0): ResourcePriorArtRecordPage {
    const detail = this.get(resourceIds, id, integer(recordIndex, 0));
    if (detail.kind !== "search" || !detail.data.records[0]) throw new Error("Saved advisory record not found.");
    const text = JSON.stringify(detail.data.records[0], null, 2);
    const start = integer(textOffset, 0);
    return { historyId: id, resourceId: detail.resourceId, recordIndex, textOffset: start, nextTextOffset: start + 8000 < text.length ? start + 8000 : null, totalCharacters: text.length, text: text.slice(start, start + 8000) };
  }
}

function summary(row: Record<string, unknown>): ResourcePriorArtSummary {
  return { id: String(row.id), resourceId: String(row.resource_id), kind: row.kind as "search" | "document", title: String(row.title), recordedAt: String(row.recorded_at), sessionId: row.session_id === null ? null : String(row.session_id), revision: row.revision === null ? null : String(row.revision), disposition: String(row.disposition), resultCount: Number(row.result_count) };
}
function integer(value: number, min: number, max = Number.MAX_SAFE_INTEGER): number {
  if (!Number.isSafeInteger(value) || value < min || value > max) throw new Error("Invalid prior-art page bounds.");
  return value;
}
function checkedIds(ids: readonly string[]): readonly string[] {
  if (!Array.isArray(ids) || ids.length > 100 || ids.some((id) => typeof id !== "string" || !id)) throw new Error("Invalid resource identities.");
  return [...new Set(ids)];
}
