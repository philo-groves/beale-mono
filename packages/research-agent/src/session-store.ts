import { createHash, randomUUID } from "node:crypto";
import { chmodSync, existsSync, mkdirSync } from "node:fs";
import { dirname } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { applyDatabaseMigrations } from "./database-migrations.js";
import { getDefaultMemoryDatabasePath } from "./storage.js";
import { normalizeResearchProfile, researchProfileHash } from "./research-profile.js";
import { readCompatibleRecordValue } from "./legacy-compatibility.js";

export const APP_SERVER_SESSION_SCHEMA_VERSION = 1 as const;

export type AppServerSessionStatus =
  | "active"
  | "paused"
  | "blocked"
  | "completed"
  | "failed"
  | "stopped";

export interface AppServerSessionDisposition {
  outcome: string;
  summary: string;
  externalStateRequired: boolean;
  blockerDependencies: readonly unknown[];
  recordedAt?: string;
  [key: string]: unknown;
}

export interface AppServerSessionEvent {
  id: string;
  kind: string;
  timestamp: string;
  summary: string;
  payload: unknown;
  agentId?: string;
  agentPath?: string;
  parentAgentId?: string;
}

export interface AppServerSessionCapture {
  attemptId: string;
  capturedAt: string;
  schemaVersion: number;
  eventStreams: {
    timeline: AppServerSessionCaptureEventReference;
    agentDiagnostics: AppServerSessionCaptureEventReference;
  };
  raw: Record<string, unknown>;
}

export interface AppServerSessionCaptureEventReference {
  source: "app_server_session_events";
  sessionId: string;
  attemptId: string;
  count: number;
}

export interface AppServerSessionCaptureSummary {
  attemptId: string;
  capturedAt: string;
  schemaVersion: number;
  sizeBytes: number;
  contentHash: string;
  eventStreams: AppServerSessionCapture["eventStreams"];
}

export interface AppServerSessionAttempt {
  id: string;
  parentAttemptId: string | null;
  status: AppServerSessionStatus;
  summary: string;
  startedAt: string;
  endedAt: string | null;
  capture: AppServerSessionCapture | null;
  metadata: Record<string, unknown>;
}

export interface AppServerSessionRecord {
  schemaVersion: typeof APP_SERVER_SESSION_SCHEMA_VERSION;
  id: string;
  workspaceId: string;
  status: AppServerSessionStatus;
  title: string;
  prompt: string;
  summary: string;
  provider: string | null;
  model: string;
  reasoningEffort: string;
  workflowId: string | null;
  profile: Record<string, unknown> | null;
  metadata: Record<string, unknown>;
  finalDisposition: AppServerSessionDisposition | null;
  finalResponse: string | null;
  attempts: AppServerSessionAttempt[];
  events: AppServerSessionEvent[];
  createdAt: string;
  startedAt: string;
  endedAt: string | null;
  updatedAt: string;
  revision: number;
}

export type AppServerSessionAttemptSummary = Omit<AppServerSessionAttempt, "capture">;

export interface AppServerSessionTokenUsage {
  totalTokens: number;
  totalCostUsd?: number;
  inputTokens?: number;
  outputTokens?: number;
  cacheReadTokens?: number;
  cachePromptTokens?: number;
}

export interface AppServerSessionActivityCounts {
  memorySearches: number;
  memoryUpdates: number;
}

export type AppServerSessionSummary = Omit<
  AppServerSessionRecord,
  "attempts" | "events" | "finalResponse"
> & {
  attempts: AppServerSessionAttemptSummary[];
  lastMessageAt: string | null;
  tokenUsage: AppServerSessionTokenUsage;
  activityCounts: AppServerSessionActivityCounts;
};

export interface AppServerSessionUpdate {
  session: AppServerSessionSummary;
  finalResponse: string | null;
  events: AppServerSessionEvent[];
  eventOffset: number;
  nextAfterEventId: string | null;
  hasEarlier: boolean;
  hasMore: boolean;
}

export type AppServerSessionEventStream = "all" | "transcript" | "trace" | "commentary";

export interface AppServerSessionEventPageInput {
  afterEventId?: string | null;
  limit?: number;
  maxBytes?: number;
  tail?: boolean;
  stream?: AppServerSessionEventStream;
}

export interface AppServerSessionEventPage {
  sessionId: string;
  stream: AppServerSessionEventStream;
  events: AppServerSessionEvent[];
  eventOffset: number;
  nextAfterEventId: string | null;
  hasEarlier: boolean;
  hasMore: boolean;
}

export interface AppServerSessionCollaborationState {
  sessionId: string;
  revision: number;
  rooms: AppServerSessionEvent[];
  members: AppServerSessionEvent[];
  messages: AppServerSessionEvent[];
  subagents: AppServerSessionEvent[];
}

export interface AppServerSessionMutationReceipt {
  sessionId: string;
  status: AppServerSessionStatus;
  revision: number;
  updatedAt: string;
}

export interface CreateAppServerSessionInput {
  id: string;
  workspaceId: string;
  attemptId: string;
  title: string;
  prompt: string;
  provider?: string | null;
  model: string;
  reasoningEffort: string;
  workflowId?: string | null;
  profile?: Record<string, unknown> | null;
  metadata?: Record<string, unknown>;
  attemptMetadata?: Record<string, unknown>;
  createdAt?: string;
}

export interface AppServerSessionTransitionInput {
  status: AppServerSessionStatus;
  summary: string;
  attemptId?: string;
  expectedRevision?: number;
  disposition?: AppServerSessionDisposition | null;
  metadata?: Record<string, unknown>;
  configuration?: {
    prompt?: string;
    provider?: string | null;
    model?: string;
    reasoningEffort?: string;
    workflowId?: string | null;
  };
  at?: string;
}

export interface RecoverInterruptedAppServerSessionsInput {
  reason?: string;
  at?: string;
}

export interface AppServerSessionRecoveryReport {
  workspaceId: string;
  recoveredAt: string;
  reason: string;
  interruptedSessions: number;
  interruptedAttempts: number;
  sessionIds: string[];
}

export interface ImportAppServerSessionCaptureInput {
  attemptId: string;
  capture: unknown;
  expectedRevision?: number;
}

export interface BeginAppServerSessionAttemptInput {
  attemptId: string;
  parentAttemptId?: string | null;
  summary: string;
  expectedRevision?: number;
  startedAt?: string;
  metadata?: Record<string, unknown>;
}

export interface AppServerSessionStoreOptions {
  databasePath?: string;
  workspaceRoot?: string;
  readOnly?: boolean;
}

interface StoredSessionRow {
  document_json?: unknown;
  document_hash?: unknown;
}

interface StoredSessionEventRow {
  event_json?: unknown;
  content_hash?: unknown;
}

interface StoredSessionCaptureRow {
  attempt_id?: unknown;
  capture_json?: unknown;
  content_hash?: unknown;
}

const APP_SERVER_SESSION_MIGRATIONS = [
  {
    version: 1,
    name: "create_session_aggregates",
    up(database: DatabaseSync): void {
      database.exec(`
        CREATE TABLE IF NOT EXISTS app_server_sessions (
          id TEXT PRIMARY KEY,
          workspace_id TEXT NOT NULL,
          status TEXT NOT NULL,
          title TEXT NOT NULL,
          summary TEXT NOT NULL,
          document_json TEXT NOT NULL,
          revision INTEGER NOT NULL,
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS app_server_sessions_workspace_updated
        ON app_server_sessions(workspace_id, updated_at DESC, id DESC);
      `);
    },
  },
  {
    version: 2,
    name: "normalize_session_event_streams",
    up(database: DatabaseSync): void {
      if (!columnExists(database, "app_server_sessions", "document_hash")) {
        database.exec("ALTER TABLE app_server_sessions ADD COLUMN document_hash TEXT;");
      }
      database.exec(`
        CREATE TABLE IF NOT EXISTS app_server_session_events (
          session_id TEXT NOT NULL REFERENCES app_server_sessions(id) ON DELETE CASCADE,
          event_offset INTEGER NOT NULL CHECK (event_offset >= 0),
          event_id TEXT NOT NULL,
          event_json TEXT NOT NULL,
          content_hash TEXT NOT NULL,
          PRIMARY KEY(session_id, event_offset),
          UNIQUE(session_id, event_id)
        );
      `);
      const rows = database.prepare(
        "SELECT id, document_json FROM app_server_sessions ORDER BY created_at ASC, id ASC",
      ).all() as Array<{ id?: unknown; document_json?: unknown }>;
      const deleteEvents = database.prepare("DELETE FROM app_server_session_events WHERE session_id = ?");
      const insertEvent = database.prepare(`
        INSERT INTO app_server_session_events (
          session_id, event_offset, event_id, event_json, content_hash
        ) VALUES (?, ?, ?, ?, ?)
      `);
      const updateDocument = database.prepare(`
        UPDATE app_server_sessions SET document_json = ?, document_hash = ? WHERE id = ?
      `);
      for (const row of rows) {
        const sessionId = requiredStoredString(row.id, "app-server session id");
        const session = decodeStoredSession(row.document_json);
        if (session.events.length > 0) {
          deleteEvents.run(sessionId);
          for (const [offset, candidate] of session.events.entries()) {
            const event = normalizeEvent(candidate);
            const eventDocument = JSON.stringify(event);
            insertEvent.run(sessionId, offset, event.id, eventDocument, hashJson(eventDocument));
          }
        }
        const document = storedSessionDocument(session, false);
        updateDocument.run(document, hashJson(document), sessionId);
      }
    },
  },
  {
    version: 3,
    name: "normalize_session_captures",
    up(database: DatabaseSync): void {
      database.exec(`
        CREATE TABLE IF NOT EXISTS app_server_session_captures (
          session_id TEXT NOT NULL REFERENCES app_server_sessions(id) ON DELETE CASCADE,
          attempt_id TEXT NOT NULL,
          capture_json TEXT NOT NULL,
          content_hash TEXT NOT NULL,
          PRIMARY KEY(session_id, attempt_id)
        );
      `);
      const rows = database.prepare(
        "SELECT id, document_json FROM app_server_sessions ORDER BY created_at ASC, id ASC",
      ).all() as Array<{ id?: unknown; document_json?: unknown }>;
      const upsertCapture = database.prepare(`
        INSERT INTO app_server_session_captures (
          session_id, attempt_id, capture_json, content_hash
        ) VALUES (?, ?, ?, ?)
        ON CONFLICT(session_id, attempt_id) DO UPDATE SET
          capture_json = excluded.capture_json,
          content_hash = excluded.content_hash
      `);
      const updateDocument = database.prepare(`
        UPDATE app_server_sessions SET document_json = ?, document_hash = ? WHERE id = ?
      `);
      for (const row of rows) {
        const sessionId = requiredStoredString(row.id, "app-server session id");
        const session = decodeStoredSession(row.document_json);
        for (const attempt of session.attempts) {
          if (!attempt.capture) continue;
          const document = JSON.stringify(attempt.capture);
          upsertCapture.run(sessionId, attempt.id, document, hashJson(document));
        }
        const document = storedSessionDocument(session);
        updateDocument.run(document, hashJson(document), sessionId);
      }
    },
  },
  {
    version: 4,
    name: "compact_session_capture_event_histories",
    up(database: DatabaseSync): void {
      const rows = database.prepare(`
        SELECT session_id, attempt_id, capture_json, content_hash
        FROM app_server_session_captures
        ORDER BY session_id ASC, attempt_id ASC
      `).all() as Array<{
        session_id?: unknown;
        attempt_id?: unknown;
        capture_json?: unknown;
        content_hash?: unknown;
      }>;
      const update = database.prepare(`
        UPDATE app_server_session_captures
        SET capture_json = ?, content_hash = ?
        WHERE session_id = ? AND attempt_id = ?
      `);
      for (const row of rows) {
        const sessionId = requiredStoredString(row.session_id, "app-server session capture session id");
        const attemptId = requiredStoredString(row.attempt_id, "app-server session capture attempt id");
        const document = requiredStoredString(row.capture_json, "app-server session capture");
        verifyJsonHash(document, row.content_hash, "app-server session capture");
        const compacted = compactStoredCapture(JSON.parse(document) as unknown, sessionId, attemptId);
        const compactedDocument = JSON.stringify(compacted);
        update.run(compactedDocument, hashJson(compactedDocument), sessionId, attemptId);
      }
    },
  },
  {
    version: 5,
    name: "materialize_session_summary_metrics",
    up(database: DatabaseSync): void {
      database.exec(`
        CREATE TABLE IF NOT EXISTS app_server_session_summary_metrics (
          session_id TEXT PRIMARY KEY REFERENCES app_server_sessions(id) ON DELETE CASCADE,
          completed_turn_tokens INTEGER NOT NULL DEFAULT 0,
          completed_turn_cost_usd REAL NOT NULL DEFAULT 0,
          latest_reported_total_tokens INTEGER,
          last_message_at TEXT
        );

        INSERT OR IGNORE INTO app_server_session_summary_metrics (session_id)
        SELECT id FROM app_server_sessions;

        WITH completed_turns AS (
          SELECT
            session_id,
            COALESCE(SUM(COALESCE(
              json_extract(event_json, '$.payload.usage.totalTokens'),
              json_extract(event_json, '$.payload.usage.total_tokens'),
              0
            )), 0) AS total_tokens,
            COALESCE(SUM(COALESCE(
              json_extract(event_json, '$.payload.usage.cost.total'),
              json_extract(event_json, '$.payload.usage.totalCostUsd'),
              0
            )), 0) AS total_cost
          FROM app_server_session_events
          WHERE json_extract(event_json, '$.kind') = 'agent.event'
            AND json_extract(event_json, '$.payload.type') = 'turn_completed'
          GROUP BY session_id
        )
        UPDATE app_server_session_summary_metrics
        SET
          completed_turn_tokens = COALESCE((
            SELECT total_tokens FROM completed_turns
            WHERE completed_turns.session_id = app_server_session_summary_metrics.session_id
          ), 0),
          completed_turn_cost_usd = COALESCE((
            SELECT total_cost FROM completed_turns
            WHERE completed_turns.session_id = app_server_session_summary_metrics.session_id
          ), 0);

        WITH latest_usage_offsets AS (
          SELECT session_id, MAX(event_offset) AS event_offset
          FROM app_server_session_events
          WHERE json_extract(event_json, '$.kind') = 'beale.model_session_update'
          GROUP BY session_id
        ), latest_usage AS (
          SELECT event.session_id, COALESCE(
            json_extract(event.event_json, '$.payload.record.patch.metadata.latestReportedTotalTokens'),
            json_extract(event.event_json, '$.payload.record.patch.metadata.latest_reported_total_tokens')
          ) AS total_tokens
          FROM app_server_session_events AS event
          JOIN latest_usage_offsets AS latest
            ON latest.session_id = event.session_id AND latest.event_offset = event.event_offset
        )
        UPDATE app_server_session_summary_metrics
        SET latest_reported_total_tokens = (
          SELECT total_tokens FROM latest_usage
          WHERE latest_usage.session_id = app_server_session_summary_metrics.session_id
        );

        WITH latest_message_offsets AS (
          SELECT session_id, MAX(event_offset) AS event_offset
          FROM app_server_session_events
          WHERE json_extract(event_json, '$.kind') = 'beale.transcript'
          GROUP BY session_id
        ), latest_messages AS (
          SELECT event.session_id, COALESCE(
            json_extract(event.event_json, '$.payload.record.createdAt'),
            json_extract(event.event_json, '$.timestamp')
          ) AS created_at
          FROM app_server_session_events AS event
          JOIN latest_message_offsets AS latest
            ON latest.session_id = event.session_id AND latest.event_offset = event.event_offset
        )
        UPDATE app_server_session_summary_metrics
        SET last_message_at = (
          SELECT created_at FROM latest_messages
          WHERE latest_messages.session_id = app_server_session_summary_metrics.session_id
        );
      `);
    },
  },
  {
    version: 6,
    name: "materialize_session_usage_breakdown",
    up(database: DatabaseSync): void {
      database.exec(`
        ALTER TABLE app_server_session_summary_metrics
          ADD COLUMN completed_turn_input_tokens INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE app_server_session_summary_metrics
          ADD COLUMN completed_turn_output_tokens INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE app_server_session_summary_metrics
          ADD COLUMN completed_turn_cache_read_tokens INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE app_server_session_summary_metrics
          ADD COLUMN completed_turn_cache_prompt_tokens INTEGER NOT NULL DEFAULT 0;

        WITH completed_turns AS (
          SELECT
            session_id,
            COALESCE(SUM(COALESCE(
              json_extract(event_json, '$.payload.usage.promptTokens'),
              json_extract(event_json, '$.payload.usage.prompt_tokens'),
              COALESCE(
                json_extract(event_json, '$.payload.usage.inputTokens'),
                json_extract(event_json, '$.payload.usage.input'),
                json_extract(event_json, '$.payload.usage.input_tokens'),
                0
              ) + COALESCE(
                json_extract(event_json, '$.payload.usage.cacheReadTokens'),
                json_extract(event_json, '$.payload.usage.cacheRead'),
                json_extract(event_json, '$.payload.usage.cache_read_tokens'),
                0
              ) + COALESCE(
                json_extract(event_json, '$.payload.usage.cacheWriteTokens'),
                json_extract(event_json, '$.payload.usage.cacheWrite'),
                json_extract(event_json, '$.payload.usage.cache_write_tokens'),
                0
              )
            )), 0) AS input_tokens,
            COALESCE(SUM(COALESCE(
              json_extract(event_json, '$.payload.usage.outputTokens'),
              json_extract(event_json, '$.payload.usage.output'),
              json_extract(event_json, '$.payload.usage.output_tokens'),
              0
            )), 0) AS output_tokens,
            COALESCE(SUM(COALESCE(
              json_extract(event_json, '$.payload.usage.cacheReadTokens'),
              json_extract(event_json, '$.payload.usage.cacheRead'),
              json_extract(event_json, '$.payload.usage.cache_read_tokens'),
              0
            )), 0) AS cache_read_tokens,
            COALESCE(SUM(COALESCE(
              json_extract(event_json, '$.payload.usage.promptTokens'),
              json_extract(event_json, '$.payload.usage.prompt_tokens'),
              COALESCE(
                json_extract(event_json, '$.payload.usage.inputTokens'),
                json_extract(event_json, '$.payload.usage.input'),
                json_extract(event_json, '$.payload.usage.input_tokens'),
                0
              ) + COALESCE(
                json_extract(event_json, '$.payload.usage.cacheReadTokens'),
                json_extract(event_json, '$.payload.usage.cacheRead'),
                json_extract(event_json, '$.payload.usage.cache_read_tokens'),
                0
              ) + COALESCE(
                json_extract(event_json, '$.payload.usage.cacheWriteTokens'),
                json_extract(event_json, '$.payload.usage.cacheWrite'),
                json_extract(event_json, '$.payload.usage.cache_write_tokens'),
                0
              )
            )), 0) AS cache_prompt_tokens
          FROM app_server_session_events
          WHERE json_extract(event_json, '$.kind') = 'agent.event'
            AND json_extract(event_json, '$.payload.type') = 'turn_completed'
          GROUP BY session_id
        )
        UPDATE app_server_session_summary_metrics
        SET
          completed_turn_input_tokens = COALESCE((
            SELECT input_tokens FROM completed_turns
            WHERE completed_turns.session_id = app_server_session_summary_metrics.session_id
          ), 0),
          completed_turn_output_tokens = COALESCE((
            SELECT output_tokens FROM completed_turns
            WHERE completed_turns.session_id = app_server_session_summary_metrics.session_id
          ), 0),
          completed_turn_cache_read_tokens = COALESCE((
            SELECT cache_read_tokens FROM completed_turns
            WHERE completed_turns.session_id = app_server_session_summary_metrics.session_id
          ), 0),
          completed_turn_cache_prompt_tokens = COALESCE((
            SELECT cache_prompt_tokens FROM completed_turns
            WHERE completed_turns.session_id = app_server_session_summary_metrics.session_id
          ), 0);
      `);
    },
  },
  {
    version: 7,
    name: "materialize_session_tool_activity",
    up(database: DatabaseSync): void {
      database.exec(`
        CREATE TABLE app_server_session_tool_activity (
          session_id TEXT NOT NULL REFERENCES app_server_sessions(id) ON DELETE CASCADE,
          activity_key TEXT NOT NULL,
          tool_name TEXT NOT NULL,
          created_at TEXT NOT NULL,
          PRIMARY KEY(session_id, activity_key, tool_name)
        );
        CREATE INDEX app_server_session_tool_activity_tool_idx
          ON app_server_session_tool_activity(session_id, tool_name);
      `);
      const eventPage = database.prepare(`
        SELECT rowid AS row_id, session_id, event_json
        FROM app_server_session_events
        WHERE rowid > ?
        ORDER BY rowid ASC
        LIMIT 1000
      `);
      const insert = database.prepare(`
        INSERT OR IGNORE INTO app_server_session_tool_activity (
          session_id, activity_key, tool_name, created_at
        ) VALUES (?, ?, ?, ?)
      `);
      let afterRowId = 0;
      while (true) {
        const events = eventPage.all(afterRowId) as Array<{
          row_id?: unknown;
          session_id?: unknown;
          event_json?: unknown;
        }>;
        if (events.length === 0) break;
        for (const row of events) {
          const sessionId = requiredStoredString(row.session_id, "app-server session tool activity session id");
          const document = requiredStoredString(row.event_json, "app-server session tool activity event");
          const event = normalizeEvent(JSON.parse(document) as AppServerSessionEvent);
          for (const activity of sessionToolActivityEntries(event)) {
            insert.run(sessionId, activity.key, activity.toolName, event.timestamp);
          }
        }
        const lastRowId = events.at(-1)?.row_id;
        if (typeof lastRowId !== "number" || !Number.isFinite(lastRowId) || lastRowId <= afterRowId) {
          throw new Error("app-server session tool activity migration cursor did not advance.");
        }
        afterRowId = lastRowId;
      }
    },
  },
] as const;

export class AppServerSessionStore {
  public readonly databasePath: string;
  private readonly database: DatabaseSync;
  private readonly normalizedEventStorage: boolean;
  private readonly normalizedCaptureStorage: boolean;
  private readonly sessionDocumentHashes: boolean;
  private readonly summaryMetricsStorage: boolean;
  private readonly summaryUsageBreakdownStorage: boolean;
  private readonly sessionToolActivityStorage: boolean;

  public constructor(options: AppServerSessionStoreOptions = {}) {
    this.databasePath = options.databasePath
      ?? process.env.APP_SERVER_DATABASE_PATH?.trim()
      ?? getDefaultMemoryDatabasePath(options.workspaceRoot ?? process.cwd());
    const readOnly = options.readOnly === true
      && this.databasePath !== ":memory:"
      && existsSync(this.databasePath);
    if (readOnly) {
      const readDatabase = new DatabaseSync(this.databasePath, { readOnly: true });
      readDatabase.exec("PRAGMA busy_timeout = 5000;");
      readDatabase.exec("PRAGMA foreign_keys = ON;");
      const schema = readDatabase.prepare(
        "SELECT 1 AS present FROM sqlite_master WHERE type = 'table' AND name = 'app_server_sessions'",
      ).get() as { present?: unknown } | undefined;
      if (schema?.present === 1) {
        this.database = readDatabase;
        this.normalizedEventStorage = tableExists(readDatabase, "app_server_session_events");
        this.normalizedCaptureStorage = tableExists(readDatabase, "app_server_session_captures");
        this.sessionDocumentHashes = columnExists(readDatabase, "app_server_sessions", "document_hash");
        this.summaryMetricsStorage = tableExists(readDatabase, "app_server_session_summary_metrics");
        this.summaryUsageBreakdownStorage = this.summaryMetricsStorage
          && columnExists(readDatabase, "app_server_session_summary_metrics", "completed_turn_input_tokens");
        this.sessionToolActivityStorage = tableExists(readDatabase, "app_server_session_tool_activity");
        return;
      }
      readDatabase.close();
    }
    mkdirSync(dirname(this.databasePath), { recursive: true });
    this.database = new DatabaseSync(this.databasePath);
    if (this.databasePath !== ":memory:") chmodSync(this.databasePath, 0o600);
    this.database.exec("PRAGMA busy_timeout = 5000;");
    this.database.exec("PRAGMA foreign_keys = ON;");
    this.database.exec("PRAGMA journal_mode = WAL;");
    applyDatabaseMigrations(this.database, "app_server_sessions", APP_SERVER_SESSION_MIGRATIONS);
    this.normalizedEventStorage = true;
    this.normalizedCaptureStorage = true;
    this.sessionDocumentHashes = true;
    this.summaryMetricsStorage = true;
    this.summaryUsageBreakdownStorage = true;
    this.sessionToolActivityStorage = true;
  }

  public close(): void {
    this.database.close();
  }

  public create(input: CreateAppServerSessionInput): AppServerSessionRecord {
    const now = input.createdAt ?? new Date().toISOString();
    const record: AppServerSessionRecord = {
      schemaVersion: APP_SERVER_SESSION_SCHEMA_VERSION,
      id: requiredString(input.id, "Session id"),
      workspaceId: requiredString(input.workspaceId, "Workspace id"),
      status: "active",
      title: requiredString(input.title, "Session title"),
      prompt: requiredString(input.prompt, "Session prompt"),
      summary: "app-server research session started.",
      provider: optionalString(input.provider),
      model: requiredString(input.model, "Session model"),
      reasoningEffort: requiredString(input.reasoningEffort, "Session reasoning effort"),
      workflowId: optionalString(input.workflowId),
      profile: input.profile ?? null,
      metadata: input.metadata ?? {},
      finalDisposition: null,
      finalResponse: null,
      attempts: [{
        id: requiredString(input.attemptId, "Attempt id"),
        parentAttemptId: null,
        status: "active",
        summary: "app-server research attempt started.",
        startedAt: now,
        endedAt: null,
        capture: null,
        metadata: input.attemptMetadata ?? {},
      }],
      events: [],
      createdAt: now,
      startedAt: now,
      endedAt: null,
      updatedAt: now,
      revision: 1,
    };
    this.database.prepare(`
      INSERT INTO app_server_sessions (
        id, workspace_id, status, title, summary, document_json, document_hash,
        revision, created_at, updated_at
      ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
    `).run(
      record.id,
      record.workspaceId,
      record.status,
      record.title,
      record.summary,
      storedSessionDocument(record),
      sessionDocumentHash(record),
      record.revision,
      record.createdAt,
      record.updatedAt,
    );
    return record;
  }

  public get(sessionId: string): AppServerSessionRecord | null {
    const normalizedSessionId = requiredString(sessionId, "Session id");
    const session = this.getSessionCore(normalizedSessionId);
    if (!session) return null;
    if (this.normalizedEventStorage) session.events = this.readEvents(normalizedSessionId);
    if (this.normalizedCaptureStorage) this.hydrateCaptures(session);
    return session;
  }

  public getSummary(sessionId: string): AppServerSessionSummary | null {
    const session = this.getSessionCore(requiredString(sessionId, "Session id"));
    if (!session) return null;
    return sessionSummary(
      session,
      this.readSummaryTokenUsage([session.id]).get(session.id),
      this.readSummaryLastMessageAt([session.id]).get(session.id),
      this.readSummaryActivityCounts([session.id]).get(session.id),
    );
  }

  public getCapture(sessionId: string, attemptId: string): AppServerSessionCapture | null {
    const normalizedSessionId = requiredString(sessionId, "Session id");
    const normalizedAttemptId = requiredString(attemptId, "Attempt id");
    const row = this.database.prepare(`
      SELECT attempt_id, capture_json, content_hash FROM app_server_session_captures
      WHERE session_id = ? AND attempt_id = ?
    `).get(normalizedSessionId, normalizedAttemptId) as StoredSessionCaptureRow | undefined;
    return row ? decodeCaptureRow(row, normalizedSessionId) : null;
  }

  public listCaptureSummaries(sessionId: string): AppServerSessionCaptureSummary[] {
    const normalizedSessionId = requiredString(sessionId, "Session id");
    const rows = this.database.prepare(`
      SELECT attempt_id, capture_json, content_hash FROM app_server_session_captures
      WHERE session_id = ? ORDER BY attempt_id ASC
    `).all(normalizedSessionId) as StoredSessionCaptureRow[];
    return rows.map((row) => {
      const document = requiredStoredString(row.capture_json, "app-server session capture");
      const capture = decodeCaptureRow(row, normalizedSessionId);
      return {
        attemptId: capture.attemptId,
        capturedAt: capture.capturedAt,
        schemaVersion: capture.schemaVersion,
        sizeBytes: Buffer.byteLength(document),
        contentHash: requiredStoredString(row.content_hash, "app-server session capture hash"),
        eventStreams: capture.eventStreams,
      };
    });
  }

  public list(workspaceId: string, limit = 100): AppServerSessionRecord[] {
    const boundedLimit = Math.max(1, Math.min(500, Math.trunc(limit)));
    const rows = this.database.prepare(`
      SELECT document_json${this.sessionDocumentHashes ? ", document_hash" : ""} FROM app_server_sessions
      WHERE workspace_id = ?
      ORDER BY updated_at DESC, id DESC
      LIMIT ?
    `).all(requiredString(workspaceId, "Workspace id"), boundedLimit) as StoredSessionRow[];
    return rows.map((row) => this.decodeSessionRow(row)).map((session) => {
      if (this.normalizedEventStorage) session.events = this.readEvents(session.id);
      if (this.normalizedCaptureStorage) this.hydrateCaptures(session);
      return session;
    });
  }

  public listSummaries(workspaceId: string, limit = 100): AppServerSessionSummary[] {
    const boundedLimit = Math.max(1, Math.min(500, Math.trunc(limit)));
    const rows = this.database.prepare(`
      SELECT document_json${this.sessionDocumentHashes ? ", document_hash" : ""} FROM app_server_sessions
      WHERE workspace_id = ?
      ORDER BY updated_at DESC, id DESC
      LIMIT ?
    `).all(requiredString(workspaceId, "Workspace id"), boundedLimit) as StoredSessionRow[];
    const sessions = rows.map((row) => this.decodeSessionRow(row));
    const tokenUsage = this.readSummaryTokenUsage(sessions.map((session) => session.id));
    const lastMessageAt = this.readSummaryLastMessageAt(sessions.map((session) => session.id));
    const activityCounts = this.readSummaryActivityCounts(sessions.map((session) => session.id));
    return sessions.map((session) => sessionSummary(
      session,
      tokenUsage.get(session.id),
      lastMessageAt.get(session.id),
      activityCounts.get(session.id),
    ));
  }

  public listForWorkspaces(workspaceIds: readonly string[], limitPerWorkspace = 100): AppServerSessionRecord[] {
    const normalizedWorkspaceIds = [...new Set(workspaceIds.map((workspaceId) => requiredString(workspaceId, "Workspace id")))];
    if (normalizedWorkspaceIds.length === 0) return [];
    const boundedLimit = Math.max(1, Math.min(500, Math.trunc(limitPerWorkspace)));
    const placeholders = normalizedWorkspaceIds.map(() => "?").join(", ");
    const rows = this.database.prepare(`
      SELECT document_json${this.sessionDocumentHashes ? ", document_hash" : ""} FROM (
        SELECT document_json${this.sessionDocumentHashes ? ", document_hash" : ""}, updated_at, id,
          ROW_NUMBER() OVER (
            PARTITION BY workspace_id
            ORDER BY updated_at DESC, id DESC
          ) AS workspace_rank
        FROM app_server_sessions
        WHERE workspace_id IN (${placeholders})
      )
      WHERE workspace_rank <= ?
      ORDER BY updated_at DESC, id DESC
    `).all(...normalizedWorkspaceIds, boundedLimit) as StoredSessionRow[];
    return rows.map((row) => this.decodeSessionRow(row)).map((session) => {
      if (this.normalizedEventStorage) session.events = this.readEvents(session.id);
      if (this.normalizedCaptureStorage) this.hydrateCaptures(session);
      return session;
    });
  }

  public listSummariesForWorkspaces(
    workspaceIds: readonly string[],
    limitPerWorkspace = 100,
  ): AppServerSessionSummary[] {
    const normalizedWorkspaceIds = [...new Set(workspaceIds.map((workspaceId) => requiredString(workspaceId, "Workspace id")))];
    if (normalizedWorkspaceIds.length === 0) return [];
    const boundedLimit = Math.max(1, Math.min(500, Math.trunc(limitPerWorkspace)));
    const placeholders = normalizedWorkspaceIds.map(() => "?").join(", ");
    const rows = this.database.prepare(`
      SELECT document_json${this.sessionDocumentHashes ? ", document_hash" : ""} FROM (
        SELECT document_json${this.sessionDocumentHashes ? ", document_hash" : ""}, updated_at, id,
          ROW_NUMBER() OVER (
            PARTITION BY workspace_id
            ORDER BY updated_at DESC, id DESC
          ) AS workspace_rank
        FROM app_server_sessions
        WHERE workspace_id IN (${placeholders})
      )
      WHERE workspace_rank <= ?
      ORDER BY updated_at DESC, id DESC
    `).all(...normalizedWorkspaceIds, boundedLimit) as StoredSessionRow[];
    const sessions = rows.map((row) => this.decodeSessionRow(row));
    const tokenUsage = this.readSummaryTokenUsage(sessions.map((session) => session.id));
    const lastMessageAt = this.readSummaryLastMessageAt(sessions.map((session) => session.id));
    const activityCounts = this.readSummaryActivityCounts(sessions.map((session) => session.id));
    return sessions.map((session) => sessionSummary(
      session,
      tokenUsage.get(session.id),
      lastMessageAt.get(session.id),
      activityCounts.get(session.id),
    ));
  }

  public getUpdate(
    sessionId: string,
    afterEventId?: string | null,
    input: Omit<AppServerSessionEventPageInput, "afterEventId" | "stream"> = {},
  ): AppServerSessionUpdate | null {
    const normalizedSessionId = requiredString(sessionId, "Session id");
    if (this.normalizedEventStorage) {
      const session = this.getSessionCore(normalizedSessionId);
      if (!session) return null;
      const page = this.getEventPage(normalizedSessionId, {
        ...input,
        ...(afterEventId !== undefined ? { afterEventId } : {}),
        stream: "all",
        tail: optionalString(afterEventId) ? false : input.tail ?? true,
      });
      return {
        session: sessionSummary(
          session,
          this.readSummaryTokenUsage([session.id]).get(session.id),
          this.readSummaryLastMessageAt([session.id]).get(session.id),
          this.readSummaryActivityCounts([session.id]).get(session.id),
        ),
        finalResponse: session.finalResponse,
        events: page.events,
        eventOffset: page.eventOffset,
        nextAfterEventId: page.nextAfterEventId,
        hasEarlier: page.hasEarlier,
        hasMore: page.hasMore,
      };
    }
    const session = this.get(normalizedSessionId);
    if (!session) return null;
    const normalizedAfterEventId = optionalString(afterEventId);
    const matchedIndex = normalizedAfterEventId
      ? session.events.findIndex((event) => event.id === normalizedAfterEventId)
      : -1;
    const eventOffset = matchedIndex >= 0 ? matchedIndex + 1 : 0;
    return {
      session: sessionSummary(
        session,
        this.readSummaryTokenUsage([session.id]).get(session.id),
        this.readSummaryLastMessageAt([session.id]).get(session.id),
        this.readSummaryActivityCounts([session.id]).get(session.id),
      ),
      finalResponse: session.finalResponse,
      events: session.events.slice(eventOffset),
      eventOffset,
      nextAfterEventId: session.events.at(-1)?.id ?? null,
      hasEarlier: eventOffset > 0,
      hasMore: false,
    };
  }

  public getEventPage(sessionId: string, input: AppServerSessionEventPageInput = {}): AppServerSessionEventPage {
    const normalizedSessionId = requiredString(sessionId, "Session id");
    if (!this.getSessionCore(normalizedSessionId)) throw new Error(`Session not found: ${normalizedSessionId}`);
    const stream = input.stream ?? "all";
    if (stream !== "all" && stream !== "transcript" && stream !== "trace" && stream !== "commentary") {
      throw new Error(`Unsupported session event stream: ${stream}.`);
    }
    const limit = boundedInteger(input.limit, 500, 1, 2_000);
    const maxBytes = boundedInteger(input.maxBytes, 2 * 1024 * 1024, 1_024, 8 * 1024 * 1024);
    const afterEventId = optionalString(input.afterEventId);
    const cursorOffset = afterEventId ? this.eventOffsetForCursor(normalizedSessionId, afterEventId) : null;
    const tail = input.tail === true && !afterEventId;
    const filter = eventStreamSql(stream);
    const direction = tail ? "DESC" : "ASC";
    const comparison = cursorOffset === null ? "" : "AND event_offset > ?";
    const query = this.database.prepare(`
      SELECT event_offset, event_json, content_hash FROM app_server_session_events
      WHERE session_id = ? ${comparison} ${filter}
      ORDER BY event_offset ${direction}
      LIMIT ?
    `);
    const rows = (cursorOffset === null
      ? query.all(normalizedSessionId, limit + 1)
      : query.all(normalizedSessionId, cursorOffset, limit + 1)
    ) as Array<StoredSessionEventRow & { event_offset?: unknown }>;
    const orderedRows = tail ? [...rows].reverse() : rows;
    const selected: typeof orderedRows = [];
    let bytes = 0;
    const candidates = tail ? [...orderedRows].reverse() : orderedRows;
    for (const row of candidates) {
      if (selected.length >= limit) break;
      const document = requiredStoredString(row.event_json, "app-server session event");
      const nextBytes = Buffer.byteLength(document);
      if (selected.length > 0 && bytes + nextBytes > maxBytes) break;
      selected.push(row);
      bytes += Math.min(nextBytes, maxBytes);
    }
    if (tail) selected.reverse();
    const decodedEvents = selected.map((row) => {
      const document = requiredStoredString(row.event_json, "app-server session event");
      return Buffer.byteLength(document) > maxBytes
        ? projectOversizedSessionEvent(decodeEventRow(row), Buffer.byteLength(document))
        : decodeEventRow(row);
    });
    const projectedEvents = stream === "commentary"
      ? decodedEvents.map((event) => projectCommentarySessionEvent(normalizedSessionId, event))
      : decodedEvents;
    const candidateTranscriptKeys = stream === "commentary"
      ? new Set(projectedEvents.flatMap((event, index) => {
          const sourceEvent = decodedEvents[index];
          if (sourceEvent?.kind !== "model.output" && sourceEvent?.kind !== "model.thought") return [];
          const key = commentaryTranscriptCorrelationKey(recordValue(event.payload)?.record);
          return key ? [key] : [];
        }))
      : new Set<string>();
    const persistedTranscriptKeys = this.commentaryTranscriptCorrelationKeys(
      normalizedSessionId,
      candidateTranscriptKeys,
    );
    const events = projectedEvents.flatMap((event, index) => {
      if (stream !== "commentary") return [event];
      const sourceEvent = decodedEvents[index];
      const correlationKey = sourceEvent?.kind === "model.output" || sourceEvent?.kind === "model.thought"
        ? commentaryTranscriptCorrelationKey(recordValue(event.payload)?.record)
        : null;
      return correlationKey && persistedTranscriptKeys.has(correlationKey) ? [] : [event];
    });
    const firstOffset = numericOffset(selected[0]?.event_offset);
    const lastOffset = numericOffset(selected.at(-1)?.event_offset);
    const bounds = this.eventStreamBounds(normalizedSessionId, stream);
    return {
      sessionId: normalizedSessionId,
      stream,
      events,
      eventOffset: firstOffset ?? (cursorOffset === null ? 0 : cursorOffset + 1),
      nextAfterEventId: selected.length > 0
        ? decodeEventRow(selected.at(-1)!).id
        : afterEventId ?? null,
      hasEarlier: firstOffset !== null && bounds.minimum !== null && firstOffset > bounds.minimum,
      hasMore: lastOffset !== null && bounds.maximum !== null && lastOffset < bounds.maximum,
    };
  }

  private commentaryTranscriptCorrelationKeys(sessionId: string, candidates: ReadonlySet<string>): Set<string> {
    if (candidates.size === 0) return new Set();
    const placeholders = [...candidates].map(() => '?').join(', ');
    const rows = this.database.prepare(`
      SELECT event_json, content_hash FROM app_server_session_events
      WHERE session_id = ? AND json_extract(event_json, '$.kind') = 'beale.transcript'
        AND (
          COALESCE(json_extract(event_json, '$.payload.record.source'), '') || char(0) ||
          COALESCE(json_extract(event_json, '$.payload.record.metadata.agentPath'), '/root') || char(0) ||
          COALESCE(json_extract(event_json, '$.payload.record.metadata.responseId'), '') || char(0) ||
          COALESCE(json_extract(event_json, '$.payload.record.metadata.itemId'), '') || char(0) ||
          COALESCE(CAST(json_extract(event_json, '$.payload.record.metadata.turn') AS TEXT), '')
        ) IN (${placeholders})
      ORDER BY event_offset ASC
    `).all(sessionId, ...candidates) as StoredSessionEventRow[];
    return new Set(rows.flatMap((row) => {
      const event = decodeEventRow(row);
      const key = commentaryTranscriptCorrelationKey(recordValue(event.payload)?.record);
      return key ? [key] : [];
    }));
  }

  public getEventDetails(sessionId: string, eventIds: readonly string[]): AppServerSessionEvent[] {
    const normalizedSessionId = requiredString(sessionId, "Session id");
    const normalizedIds = [...new Set(eventIds.map((eventId) => requiredString(eventId, "Session event id")))];
    if (normalizedIds.length === 0) return [];
    if (normalizedIds.length > 100) throw new Error("At most 100 session event details may be requested at once.");
    const placeholders = normalizedIds.map(() => "?").join(", ");
    const rows = this.database.prepare(`
      SELECT event_json, content_hash FROM app_server_session_events
      WHERE session_id = ? AND (
        event_id IN (${placeholders})
        OR EXISTS (
          SELECT 1 FROM json_each(json_extract(event_json, '$.payload.records')) AS nested
          WHERE json_extract(nested.value, '$.id') IN (${placeholders})
        )
      )
      ORDER BY event_offset ASC
    `).all(normalizedSessionId, ...normalizedIds, ...normalizedIds) as StoredSessionEventRow[];
    return rows.map(decodeEventRow);
  }

  public getCollaborationState(sessionId: string, messageLimit = 200): AppServerSessionCollaborationState {
    const normalizedSessionId = requiredString(sessionId, "Session id");
    const session = this.getSessionCore(normalizedSessionId);
    if (!session) throw new Error(`Session not found: ${normalizedSessionId}`);
    const readKind = (kind: string, limit: number): AppServerSessionEvent[] => {
      const rows = this.database.prepare(`
        SELECT event_json, content_hash FROM app_server_session_events
        WHERE session_id = ? AND json_extract(event_json, '$.kind') = ?
        ORDER BY event_offset DESC
        LIMIT ?
      `).all(normalizedSessionId, kind, limit) as StoredSessionEventRow[];
      return rows.map(decodeEventRow).reverse();
    };
    const roomEvents = readKind("beale.breakout_room", 2_000);
    const memberEvents = readKind("beale.breakout_member", 4_000);
    const messageEvents = readKind(
      "beale.breakout_message",
      boundedInteger(messageLimit, 200, 1, 1_000),
    );
    const subagentRows = this.database.prepare(`
      SELECT event_json, content_hash FROM app_server_session_events
      WHERE session_id = ?
        AND json_extract(event_json, '$.kind') = 'agent.event'
        AND json_extract(event_json, '$.payload.type') = 'subagent.activity'
      ORDER BY event_offset DESC
      LIMIT 4000
    `).all(normalizedSessionId) as StoredSessionEventRow[];
    return {
      sessionId: normalizedSessionId,
      revision: session.revision,
      rooms: latestRecordEvents(roomEvents),
      members: latestRecordEvents(memberEvents),
      messages: messageEvents,
      subagents: subagentRows.map(decodeEventRow).reverse(),
    };
  }

  public recoverInterrupted(
    workspaceId: string,
    input: RecoverInterruptedAppServerSessionsInput = {},
  ): AppServerSessionRecoveryReport {
    const normalizedWorkspaceId = requiredString(workspaceId, "Workspace id");
    const recoveredAt = input.at ?? new Date().toISOString();
    const reason = optionalString(input.reason) ?? "workspace_open";
    this.database.exec("BEGIN IMMEDIATE;");
    try {
      const rows = this.database.prepare(`
        SELECT document_json, document_hash FROM app_server_sessions
        WHERE workspace_id = ? AND status = 'active'
        ORDER BY updated_at ASC, id ASC
      `).all(normalizedWorkspaceId) as StoredSessionRow[];
      const sessions = rows.map((row) => this.decodeSessionRow(row));
      let interruptedAttempts = 0;
      for (const session of sessions) {
        const recoveredAttemptIds: string[] = [];
        for (const attempt of session.attempts) {
          if (attempt.status !== "active") continue;
          attempt.status = "paused";
          attempt.summary = "Paused after the app-server process was interrupted.";
          attempt.endedAt = null;
          attempt.metadata = {
            ...attempt.metadata,
            interruptedByRecovery: true,
            recoveryReason: reason,
            recoveredAt,
          };
          recoveredAttemptIds.push(attempt.id);
          interruptedAttempts += 1;
        }
        session.status = "paused";
        session.summary = "Paused after the app-server process was interrupted.";
        session.endedAt = null;
        session.metadata = {
          ...session.metadata,
          interruptedByRecovery: true,
          recoveryReason: reason,
          recoveredAt,
          previousStatus: "active",
          recoveredAttemptIds,
        };
        const recoveryEvent = normalizeEvent({
          id: `session_recovery_${randomUUID()}`,
          kind: "session.recovery",
          timestamp: recoveredAt,
          summary: "Workspace recovery paused an interrupted app-server session.",
          payload: {
            interruptedByRecovery: true,
            previousStatus: "active",
            recoveredAt,
            reason,
            attemptId: recoveredAttemptIds.at(-1) ?? null,
            recoveredAttemptIds,
          },
          agentPath: "/root",
        });
        this.insertEvents(session.id, [recoveryEvent]);
        session.revision += 1;
        session.updatedAt = recoveredAt;
        const document = storedSessionDocument(session);
        const result = this.database.prepare(`
          UPDATE app_server_sessions
          SET status = ?, summary = ?, document_json = ?, document_hash = ?, revision = ?, updated_at = ?
          WHERE id = ? AND revision = ? AND status = 'active'
        `).run(
          session.status,
          session.summary,
          document,
          hashJson(document),
          session.revision,
          session.updatedAt,
          session.id,
          session.revision - 1,
        );
        if (Number(result.changes) !== 1) {
          throw new Error(`Session revision conflict while recovering ${session.id}.`);
        }
      }
      this.database.exec("COMMIT;");
      return {
        workspaceId: normalizedWorkspaceId,
        recoveredAt,
        reason,
        interruptedSessions: sessions.length,
        interruptedAttempts,
        sessionIds: sessions.map((session) => session.id),
      };
    } catch (error) {
      this.database.exec("ROLLBACK;");
      throw error;
    }
  }

  public beginAttempt(sessionId: string, input: BeginAppServerSessionAttemptInput): AppServerSessionRecord {
    return this.mutate(sessionId, input.expectedRevision, (session) => {
      if (session.status === "active") {
        throw new Error(`Session ${session.id} already has an active attempt.`);
      }
      const now = input.startedAt ?? new Date().toISOString();
      session.status = "active";
      session.summary = requiredString(input.summary, "Attempt summary");
      session.endedAt = null;
      session.attempts.push({
        id: requiredString(input.attemptId, "Attempt id"),
        parentAttemptId: optionalString(input.parentAttemptId),
        status: "active",
        summary: session.summary,
        startedAt: now,
        endedAt: null,
        capture: null,
        metadata: input.metadata ?? {},
      });
    });
  }

  public appendEvent(sessionId: string, event: AppServerSessionEvent): AppServerSessionRecord {
    const session = this.appendNormalizedEvent(sessionId, event);
    session.events = this.readEvents(session.id);
    this.hydrateCaptures(session);
    return session;
  }

  public appendEventReceipt(sessionId: string, event: AppServerSessionEvent): AppServerSessionMutationReceipt {
    return sessionMutationReceipt(this.appendNormalizedEvent(sessionId, event));
  }

  public transition(sessionId: string, input: AppServerSessionTransitionInput): AppServerSessionRecord {
    return this.mutate(sessionId, input.expectedRevision, (session) => {
      const now = input.at ?? new Date().toISOString();
      session.status = input.status;
      session.summary = requiredString(input.summary, "Lifecycle summary");
      session.metadata = { ...session.metadata, ...(input.metadata ?? {}) };
      if (input.configuration) {
        if (input.configuration.prompt !== undefined) {
          session.prompt = requiredString(input.configuration.prompt, "Session prompt");
        }
        if (input.configuration.provider !== undefined) {
          session.provider = optionalString(input.configuration.provider);
        }
        if (input.configuration.model !== undefined) {
          session.model = requiredString(input.configuration.model, "Session model");
        }
        if (input.configuration.reasoningEffort !== undefined) {
          session.reasoningEffort = input.configuration.reasoningEffort.trim();
        }
        if (input.configuration.workflowId !== undefined) {
          session.workflowId = optionalString(input.configuration.workflowId);
        }
      }
      if (input.disposition !== undefined) session.finalDisposition = input.disposition;
      const attempt = input.attemptId
        ? session.attempts.find((candidate) => candidate.id === input.attemptId)
        : session.attempts.at(-1);
      if (attempt) {
        attempt.status = input.status;
        attempt.summary = session.summary;
        attempt.endedAt = terminalStatus(input.status) ? now : null;
      }
      session.endedAt = terminalStatus(input.status) ? now : null;
    });
  }

  public importCapture(sessionId: string, input: ImportAppServerSessionCaptureInput): AppServerSessionRecord {
    const capture = decodeCapture(input.capture);
    return this.mutate(sessionId, input.expectedRevision, (session) => {
      const attempt = session.attempts.find((candidate) => candidate.id === input.attemptId);
      if (!attempt) throw new Error(`Attempt not found for capture import: ${input.attemptId}`);
      if (attempt.capture) throw new Error(`Attempt ${attempt.id} already has an imported capture.`);
      validateCaptureResearchProfile(capture, session);

      const capturedAt = optionalString(capture.capturedAt) ?? new Date().toISOString();
      const agent = recordValue(capture.agent) ?? {};
      const goal = recordValue(agent.goal);
      const goalStatus = optionalString(goal?.status);
      const agentStatus = optionalString(agent.status);
      const completed = agentStatus === "complete" && goalStatus !== "active";
      const status: AppServerSessionStatus = completed && goalStatus === "blocked"
        ? "blocked"
        : completed
          ? "completed"
          : "failed";
      const summary = completionSummary(agentStatus, goalStatus, agent);
      const disposition = decodeDisposition(agent.finalDisposition);
      const response = optionalString(agent.outputText);

      attempt.capture = compactImportedCapture(capture, session.id, attempt.id, capturedAt);
      attempt.status = status;
      attempt.summary = summary;
      attempt.endedAt = capturedAt;
      session.status = status;
      session.summary = summary;
      session.finalDisposition = disposition;
      session.finalResponse = response;
      session.endedAt = capturedAt;

      for (const candidate of Array.isArray(capture.eventTimeline) ? capture.eventTimeline : []) {
        const event = captureEvent(candidate);
        if (event && !session.events.some((existing) => existing.id === event.id)) session.events.push(event);
      }
    });
  }

  private mutate(
    sessionId: string,
    expectedRevision: number | undefined,
    update: (session: AppServerSessionRecord) => void,
  ): AppServerSessionRecord {
    this.database.exec("BEGIN IMMEDIATE;");
    try {
      const session = this.getSessionCore(requiredString(sessionId, "Session id"));
      if (!session) throw new Error(`Session not found: ${sessionId}`);
      if (expectedRevision !== undefined && session.revision !== expectedRevision) {
        throw new Error(
          `Session revision conflict for ${sessionId}: expected ${expectedRevision}, received ${session.revision}.`,
        );
      }
      const storedEventCount = session.events.length;
      update(session);
      session.revision += 1;
      session.updatedAt = new Date().toISOString();
      this.insertEvents(session.id, session.events.slice(storedEventCount));
      this.insertCaptures(session);
      const document = storedSessionDocument(session);
      const result = this.database.prepare(`
        UPDATE app_server_sessions
        SET status = ?, title = ?, summary = ?, document_json = ?, document_hash = ?, revision = ?, updated_at = ?
        WHERE id = ? AND revision = ?
      `).run(
        session.status,
        session.title,
        session.summary,
        document,
        hashJson(document),
        session.revision,
        session.updatedAt,
        session.id,
        session.revision - 1,
      );
      if (Number(result.changes) !== 1) throw new Error(`Session revision conflict for ${sessionId}.`);
      this.database.exec("COMMIT;");
      return session;
    } catch (error) {
      this.database.exec("ROLLBACK;");
      throw error;
    }
  }

  private appendNormalizedEvent(sessionId: string, event: AppServerSessionEvent): AppServerSessionRecord {
    const normalizedSessionId = requiredString(sessionId, "Session id");
    const normalized = normalizeEvent(event);
    this.database.exec("BEGIN IMMEDIATE;");
    try {
      const session = this.getSessionCore(normalizedSessionId);
      if (!session) throw new Error(`Session not found: ${normalizedSessionId}`);
      const duplicate = this.database.prepare(`
        SELECT 1 AS present FROM app_server_session_events
        WHERE session_id = ? AND event_id = ?
      `).get(normalizedSessionId, normalized.id) as { present?: unknown } | undefined;
      if (duplicate?.present !== 1) {
        this.insertEvents(normalizedSessionId, [normalized]);
        if (normalized.kind === "session.title") {
          const payload = recordValue(normalized.payload);
          const title = optionalString(payload?.title);
          if (payload?.status === "generated" && title) session.title = title;
        }
      }
      session.revision += 1;
      session.updatedAt = new Date().toISOString();
      const document = storedSessionDocument(session);
      const result = this.database.prepare(`
        UPDATE app_server_sessions
        SET title = ?, document_json = ?, document_hash = ?, revision = ?, updated_at = ?
        WHERE id = ? AND revision = ?
      `).run(
        session.title,
        document,
        hashJson(document),
        session.revision,
        session.updatedAt,
        session.id,
        session.revision - 1,
      );
      if (Number(result.changes) !== 1) throw new Error(`Session revision conflict for ${normalizedSessionId}.`);
      this.database.exec("COMMIT;");
      return session;
    } catch (error) {
      this.database.exec("ROLLBACK;");
      throw error;
    }
  }

  private getSessionCore(sessionId: string): AppServerSessionRecord | null {
    const row = this.database.prepare(`
      SELECT document_json${this.sessionDocumentHashes ? ", document_hash" : ""}
      FROM app_server_sessions WHERE id = ?
    `).get(sessionId) as StoredSessionRow | undefined;
    return row ? this.decodeSessionRow(row) : null;
  }

  private decodeSessionRow(row: StoredSessionRow): AppServerSessionRecord {
    const document = requiredStoredString(row.document_json, "app-server session document");
    if (this.sessionDocumentHashes) verifyJsonHash(document, row.document_hash, "app-server session document");
    return decodeStoredSession(document);
  }

  private readEvents(sessionId: string, fromOffset = 0): AppServerSessionEvent[] {
    if (!this.normalizedEventStorage) return [];
    const rows = this.database.prepare(`
      SELECT event_json, content_hash FROM app_server_session_events
      WHERE session_id = ? AND event_offset >= ?
      ORDER BY event_offset ASC
    `).all(sessionId, fromOffset) as StoredSessionEventRow[];
    return rows.map(decodeEventRow);
  }

  private eventStreamBounds(
    sessionId: string,
    stream: AppServerSessionEventStream,
  ): { minimum: number | null; maximum: number | null } {
    const row = this.database.prepare(`
      SELECT MIN(event_offset) AS minimum, MAX(event_offset) AS maximum
      FROM app_server_session_events
      WHERE session_id = ? ${eventStreamSql(stream)}
    `).get(sessionId) as { minimum?: unknown; maximum?: unknown } | undefined;
    return {
      minimum: numericOffset(row?.minimum),
      maximum: numericOffset(row?.maximum),
    };
  }

  private eventOffsetForCursor(sessionId: string, eventId: string): number | null {
    const row = this.database.prepare(`
      SELECT event_offset FROM app_server_session_events
      WHERE session_id = ? AND (
        event_id = ?
        OR EXISTS (
          SELECT 1 FROM json_each(json_extract(event_json, '$.payload.records')) AS nested
          WHERE json_extract(nested.value, '$.id') = ?
        )
      )
      ORDER BY event_offset DESC
      LIMIT 1
    `).get(sessionId, eventId, eventId) as { event_offset?: unknown } | undefined;
    return numericOffset(row?.event_offset);
  }

  private readSummaryTokenUsage(sessionIds: readonly string[]): Map<string, AppServerSessionTokenUsage> {
    const totals = new Map<string, AppServerSessionTokenUsage>();
    if (!this.normalizedEventStorage || sessionIds.length === 0) return totals;
    if (this.summaryMetricsStorage) {
      for (const sessionId of sessionIds) totals.set(sessionId, { totalTokens: 0 });
      const placeholders = sessionIds.map(() => "?").join(", ");
      const rows = this.database.prepare(`
        SELECT session_id, completed_turn_tokens, completed_turn_cost_usd, latest_reported_total_tokens
          ${this.summaryUsageBreakdownStorage ? `,
            completed_turn_input_tokens, completed_turn_output_tokens,
            completed_turn_cache_read_tokens, completed_turn_cache_prompt_tokens
          ` : ""}
        FROM app_server_session_summary_metrics
        WHERE session_id IN (${placeholders})
      `).all(...sessionIds) as Array<{
        session_id?: unknown;
        completed_turn_tokens?: unknown;
        completed_turn_cost_usd?: unknown;
        latest_reported_total_tokens?: unknown;
        completed_turn_input_tokens?: unknown;
        completed_turn_output_tokens?: unknown;
        completed_turn_cache_read_tokens?: unknown;
        completed_turn_cache_prompt_tokens?: unknown;
      }>;
      for (const row of rows) {
        const sessionId = optionalString(row.session_id);
        if (!sessionId) continue;
        const turnTokens = finiteNonNegativeNumber(row.completed_turn_tokens) ?? 0;
        const reportedTokens = finiteNonNegativeNumber(row.latest_reported_total_tokens);
        const totalCostUsd = finiteNonNegativeNumber(row.completed_turn_cost_usd) ?? 0;
        const inputTokens = finiteNonNegativeNumber(row.completed_turn_input_tokens) ?? 0;
        const outputTokens = finiteNonNegativeNumber(row.completed_turn_output_tokens) ?? 0;
        const cacheReadTokens = finiteNonNegativeNumber(row.completed_turn_cache_read_tokens) ?? 0;
        const cachePromptTokens = finiteNonNegativeNumber(row.completed_turn_cache_prompt_tokens) ?? 0;
        const hasCompleteBreakdown = turnTokens > 0 && inputTokens + outputTokens === turnTokens;
        totals.set(sessionId, {
          // Completed-turn usage is the durable session aggregate. The latest
          // model-session value describes one root turn and is only a legacy
          // fallback for sessions without canonical turn events.
          totalTokens: turnTokens > 0 ? turnTokens : reportedTokens ?? 0,
          ...(totalCostUsd > 0 ? { totalCostUsd } : {}),
          ...(hasCompleteBreakdown ? { inputTokens, outputTokens } : {}),
          ...(hasCompleteBreakdown && cachePromptTokens > 0
            ? { cacheReadTokens, cachePromptTokens }
            : {}),
        });
      }
      return totals;
    }
    const latestUsageEvent = this.database.prepare(`
      SELECT event_json FROM app_server_session_events
      WHERE session_id = ?
        AND json_extract(event_json, '$.kind') = 'beale.model_session_update'
      ORDER BY event_offset DESC
      LIMIT 1
    `);
    const completedTurnUsage = this.database.prepare(`
      SELECT
        COALESCE(SUM(CASE
          WHEN json_extract(event_json, '$.kind') = 'agent.event'
            AND json_extract(event_json, '$.payload.type') = 'turn_completed'
          THEN COALESCE(
            json_extract(event_json, '$.payload.usage.totalTokens'),
            json_extract(event_json, '$.payload.usage.total_tokens'),
            0
          ) ELSE 0 END), 0) AS total_tokens,
        COALESCE(SUM(CASE
          WHEN json_extract(event_json, '$.kind') = 'agent.event'
            AND json_extract(event_json, '$.payload.type') = 'turn_completed'
          THEN COALESCE(
            json_extract(event_json, '$.payload.usage.cost.total'),
            json_extract(event_json, '$.payload.usage.totalCostUsd'),
            0
          ) ELSE 0 END), 0) AS total_cost,
        COALESCE(SUM(CASE
          WHEN json_extract(event_json, '$.kind') = 'agent.event'
            AND json_extract(event_json, '$.payload.type') = 'turn_completed'
          THEN COALESCE(
            json_extract(event_json, '$.payload.usage.promptTokens'),
            json_extract(event_json, '$.payload.usage.prompt_tokens'),
            COALESCE(json_extract(event_json, '$.payload.usage.inputTokens'), json_extract(event_json, '$.payload.usage.input'), json_extract(event_json, '$.payload.usage.input_tokens'), 0)
              + COALESCE(json_extract(event_json, '$.payload.usage.cacheReadTokens'), json_extract(event_json, '$.payload.usage.cacheRead'), json_extract(event_json, '$.payload.usage.cache_read_tokens'), 0)
              + COALESCE(json_extract(event_json, '$.payload.usage.cacheWriteTokens'), json_extract(event_json, '$.payload.usage.cacheWrite'), json_extract(event_json, '$.payload.usage.cache_write_tokens'), 0)
          ) ELSE 0 END), 0) AS input_tokens,
        COALESCE(SUM(CASE
          WHEN json_extract(event_json, '$.kind') = 'agent.event'
            AND json_extract(event_json, '$.payload.type') = 'turn_completed'
          THEN COALESCE(json_extract(event_json, '$.payload.usage.outputTokens'), json_extract(event_json, '$.payload.usage.output'), json_extract(event_json, '$.payload.usage.output_tokens'), 0)
          ELSE 0 END), 0) AS output_tokens,
        COALESCE(SUM(CASE
          WHEN json_extract(event_json, '$.kind') = 'agent.event'
            AND json_extract(event_json, '$.payload.type') = 'turn_completed'
          THEN COALESCE(json_extract(event_json, '$.payload.usage.cacheReadTokens'), json_extract(event_json, '$.payload.usage.cacheRead'), json_extract(event_json, '$.payload.usage.cache_read_tokens'), 0)
          ELSE 0 END), 0) AS cache_read_tokens,
        COALESCE(SUM(CASE
          WHEN json_extract(event_json, '$.kind') = 'agent.event'
            AND json_extract(event_json, '$.payload.type') = 'turn_completed'
          THEN COALESCE(
            json_extract(event_json, '$.payload.usage.promptTokens'),
            json_extract(event_json, '$.payload.usage.prompt_tokens'),
            COALESCE(json_extract(event_json, '$.payload.usage.inputTokens'), json_extract(event_json, '$.payload.usage.input'), json_extract(event_json, '$.payload.usage.input_tokens'), 0)
              + COALESCE(json_extract(event_json, '$.payload.usage.cacheReadTokens'), json_extract(event_json, '$.payload.usage.cacheRead'), json_extract(event_json, '$.payload.usage.cache_read_tokens'), 0)
              + COALESCE(json_extract(event_json, '$.payload.usage.cacheWriteTokens'), json_extract(event_json, '$.payload.usage.cacheWrite'), json_extract(event_json, '$.payload.usage.cache_write_tokens'), 0)
          ) ELSE 0 END), 0) AS cache_prompt_tokens
      FROM app_server_session_events WHERE session_id = ?
    `);
    for (const sessionId of sessionIds) {
      const row = latestUsageEvent.get(sessionId) as { event_json?: unknown } | undefined;
      let reportedTokens: number | null = null;
      if (typeof row?.event_json === "string") {
        const event = recordValue(JSON.parse(row.event_json));
        const payload = recordValue(event?.payload);
        const record = recordValue(payload?.record);
        const patch = recordValue(record?.patch);
        const metadata = recordValue(patch?.metadata);
        reportedTokens = finiteNonNegativeNumber(metadata?.latestReportedTotalTokens);
      }
      const aggregate = completedTurnUsage.get(sessionId) as {
        total_tokens?: unknown;
        total_cost?: unknown;
        input_tokens?: unknown;
        output_tokens?: unknown;
        cache_read_tokens?: unknown;
        cache_prompt_tokens?: unknown;
      } | undefined;
      const turnTokens = finiteNonNegativeNumber(aggregate?.total_tokens) ?? 0;
      const totalCostUsd = finiteNonNegativeNumber(aggregate?.total_cost) ?? 0;
      const inputTokens = finiteNonNegativeNumber(aggregate?.input_tokens) ?? 0;
      const outputTokens = finiteNonNegativeNumber(aggregate?.output_tokens) ?? 0;
      const cacheReadTokens = finiteNonNegativeNumber(aggregate?.cache_read_tokens) ?? 0;
      const cachePromptTokens = finiteNonNegativeNumber(aggregate?.cache_prompt_tokens) ?? 0;
      const hasCompleteBreakdown = turnTokens > 0 && inputTokens + outputTokens === turnTokens;
      const totalTokens = turnTokens > 0 ? turnTokens : reportedTokens ?? 0;
      totals.set(sessionId, {
        totalTokens,
        ...(totalCostUsd > 0 ? { totalCostUsd } : {}),
        ...(hasCompleteBreakdown ? { inputTokens, outputTokens } : {}),
        ...(hasCompleteBreakdown && cachePromptTokens > 0
          ? { cacheReadTokens, cachePromptTokens }
          : {}),
      });
    }
    return totals;
  }

  private readSummaryLastMessageAt(sessionIds: readonly string[]): Map<string, string> {
    const timestamps = new Map<string, string>();
    if (!this.normalizedEventStorage || sessionIds.length === 0) return timestamps;
    if (this.summaryMetricsStorage) {
      const placeholders = sessionIds.map(() => "?").join(", ");
      const rows = this.database.prepare(`
        SELECT session_id, last_message_at
        FROM app_server_session_summary_metrics
        WHERE session_id IN (${placeholders}) AND last_message_at IS NOT NULL
      `).all(...sessionIds) as Array<{ session_id?: unknown; last_message_at?: unknown }>;
      for (const row of rows) {
        const sessionId = optionalString(row.session_id);
        const timestamp = optionalString(row.last_message_at);
        if (sessionId && timestamp) timestamps.set(sessionId, timestamp);
      }
      return timestamps;
    }
    const latestTranscript = this.database.prepare(`
      SELECT COALESCE(
        json_extract(event_json, '$.payload.record.createdAt'),
        json_extract(event_json, '$.timestamp')
      ) AS last_message_at
      FROM app_server_session_events
      WHERE session_id = ?
        AND json_extract(event_json, '$.kind') = 'beale.transcript'
      ORDER BY event_offset DESC
      LIMIT 1
    `);
    for (const sessionId of sessionIds) {
      const row = latestTranscript.get(sessionId) as { last_message_at?: unknown } | undefined;
      const timestamp = optionalString(row?.last_message_at);
      if (timestamp) timestamps.set(sessionId, timestamp);
    }
    return timestamps;
  }

  private readSummaryActivityCounts(sessionIds: readonly string[]): Map<string, AppServerSessionActivityCounts> {
    const counts = new Map<string, AppServerSessionActivityCounts>();
    for (const sessionId of sessionIds) counts.set(sessionId, { memorySearches: 0, memoryUpdates: 0 });
    if (!this.sessionToolActivityStorage || sessionIds.length === 0) return counts;
    const placeholders = sessionIds.map(() => "?").join(", ");
    const rows = this.database.prepare(`
      SELECT
        session_id,
        SUM(CASE WHEN tool_name IN ('history.search', 'memory.search') THEN 1 ELSE 0 END) AS memory_searches,
        SUM(CASE WHEN tool_name IN ('memory.save', 'memory.correct', 'memory.link') THEN 1 ELSE 0 END) AS memory_updates
      FROM app_server_session_tool_activity
      WHERE session_id IN (${placeholders})
      GROUP BY session_id
    `).all(...sessionIds) as Array<{
      session_id?: unknown;
      memory_searches?: unknown;
      memory_updates?: unknown;
    }>;
    for (const row of rows) {
      const sessionId = optionalString(row.session_id);
      if (!sessionId) continue;
      counts.set(sessionId, {
        memorySearches: finiteNonNegativeNumber(row.memory_searches) ?? 0,
        memoryUpdates: finiteNonNegativeNumber(row.memory_updates) ?? 0,
      });
    }
    return counts;
  }

  private insertEvents(sessionId: string, events: readonly AppServerSessionEvent[]): void {
    if (events.length === 0) return;
    const offsetRow = this.database.prepare(`
      SELECT COALESCE(MAX(event_offset), -1) + 1 AS next_offset
      FROM app_server_session_events WHERE session_id = ?
    `).get(sessionId) as { next_offset?: unknown } | undefined;
    let offset = typeof offsetRow?.next_offset === "number" ? offsetRow.next_offset : 0;
    const insert = this.database.prepare(`
      INSERT OR IGNORE INTO app_server_session_events (
        session_id, event_offset, event_id, event_json, content_hash
      ) VALUES (?, ?, ?, ?, ?)
    `);
    for (const event of events) {
      const normalized = normalizeEvent(event);
      const document = JSON.stringify(normalized);
      const result = insert.run(sessionId, offset, normalized.id, document, hashJson(document));
      if (Number(result.changes) === 1) {
        this.updateSessionSummaryMetrics(sessionId, normalized);
        this.updateSessionToolActivity(sessionId, normalized);
        offset += 1;
      }
    }
  }

  private updateSessionSummaryMetrics(sessionId: string, event: AppServerSessionEvent): void {
    if (!this.summaryMetricsStorage) return;
    const payload = recordValue(event.payload);
    const completedTurn = event.kind === "agent.event" && payload?.type === "turn_completed";
    if (event.kind !== "beale.model_session_update" && event.kind !== "beale.transcript" && !completedTurn) return;
    this.database.prepare(`
      INSERT OR IGNORE INTO app_server_session_summary_metrics (session_id) VALUES (?)
    `).run(sessionId);
    if (event.kind === "beale.model_session_update") {
      const record = recordValue(payload?.record);
      const patch = recordValue(record?.patch);
      const metadata = recordValue(patch?.metadata);
      const reportedTokens = finiteNonNegativeNumber(
        metadata?.latestReportedTotalTokens ?? metadata?.latest_reported_total_tokens,
      );
      if (reportedTokens !== null) {
        this.database.prepare(`
          UPDATE app_server_session_summary_metrics
          SET latest_reported_total_tokens = ? WHERE session_id = ?
        `).run(reportedTokens, sessionId);
      }
    }
    if (completedTurn) {
      const usage = recordValue(payload.usage);
      const cost = recordValue(usage?.cost);
      const totalTokens = finiteNonNegativeNumber(usage?.totalTokens ?? usage?.total_tokens) ?? 0;
      const totalCostUsd = finiteNonNegativeNumber(cost?.total ?? usage?.totalCostUsd) ?? 0;
      const uncachedInputTokens = finiteNonNegativeNumber(
        usage?.inputTokens ?? usage?.input ?? usage?.input_tokens,
      ) ?? 0;
      const outputTokens = finiteNonNegativeNumber(
        usage?.outputTokens ?? usage?.output ?? usage?.output_tokens,
      ) ?? 0;
      const cacheReadTokens = finiteNonNegativeNumber(
        usage?.cacheReadTokens ?? usage?.cacheRead ?? usage?.cache_read_tokens,
      ) ?? 0;
      const cacheWriteTokens = finiteNonNegativeNumber(
        usage?.cacheWriteTokens ?? usage?.cacheWrite ?? usage?.cache_write_tokens,
      ) ?? 0;
      const inputTokens = finiteNonNegativeNumber(usage?.promptTokens ?? usage?.prompt_tokens)
        ?? uncachedInputTokens + cacheReadTokens + cacheWriteTokens;
      this.database.prepare(`
        UPDATE app_server_session_summary_metrics
        SET
          completed_turn_tokens = completed_turn_tokens + ?,
          completed_turn_cost_usd = completed_turn_cost_usd + ?,
          completed_turn_input_tokens = completed_turn_input_tokens + ?,
          completed_turn_output_tokens = completed_turn_output_tokens + ?,
          completed_turn_cache_read_tokens = completed_turn_cache_read_tokens + ?,
          completed_turn_cache_prompt_tokens = completed_turn_cache_prompt_tokens + ?
        WHERE session_id = ?
      `).run(
        totalTokens,
        totalCostUsd,
        inputTokens,
        outputTokens,
        cacheReadTokens,
        inputTokens,
        sessionId,
      );
    }
    if (event.kind === "beale.transcript") {
      const record = recordValue(payload?.record);
      const createdAt = optionalString(record?.createdAt) ?? event.timestamp;
      this.database.prepare(`
        UPDATE app_server_session_summary_metrics
        SET last_message_at = ? WHERE session_id = ?
      `).run(createdAt, sessionId);
    }
  }

  private updateSessionToolActivity(sessionId: string, event: AppServerSessionEvent): void {
    if (!this.sessionToolActivityStorage) return;
    const insert = this.database.prepare(`
      INSERT OR IGNORE INTO app_server_session_tool_activity (
        session_id, activity_key, tool_name, created_at
      ) VALUES (?, ?, ?, ?)
    `);
    for (const activity of sessionToolActivityEntries(event)) {
      insert.run(sessionId, activity.key, activity.toolName, event.timestamp);
    }
  }

  private hydrateCaptures(session: AppServerSessionRecord): void {
    if (!this.normalizedCaptureStorage) return;
    const rows = this.database.prepare(`
      SELECT attempt_id, capture_json, content_hash FROM app_server_session_captures
      WHERE session_id = ?
    `).all(session.id) as StoredSessionCaptureRow[];
    const captures = new Map(rows.map((row) => {
      const attemptId = requiredStoredString(row.attempt_id, "app-server session capture attempt id");
      return [attemptId, decodeCaptureRow(row, session.id)] as const;
    }));
    for (const attempt of session.attempts) attempt.capture = captures.get(attempt.id) ?? attempt.capture;
  }

  private insertCaptures(session: AppServerSessionRecord): void {
    const existingRows = this.database.prepare(`
      SELECT attempt_id FROM app_server_session_captures WHERE session_id = ?
    `).all(session.id) as Array<{ attempt_id?: unknown }>;
    const existingAttemptIds = new Set(existingRows.flatMap((row) =>
      typeof row.attempt_id === "string" ? [row.attempt_id] : []
    ));
    const insert = this.database.prepare(`
      INSERT INTO app_server_session_captures (
        session_id, attempt_id, capture_json, content_hash
      ) VALUES (?, ?, ?, ?)
    `);
    for (const attempt of session.attempts) {
      if (!attempt.capture || existingAttemptIds.has(attempt.id)) continue;
      const document = JSON.stringify(attempt.capture);
      insert.run(session.id, attempt.id, document, hashJson(document));
    }
  }
}

function storedSessionDocument(session: AppServerSessionRecord, stripCaptures = true): string {
  return JSON.stringify({
    ...session,
    attempts: stripCaptures
      ? session.attempts.map((attempt) => ({ ...attempt, capture: null }))
      : session.attempts,
    events: [],
  });
}

function sessionDocumentHash(session: AppServerSessionRecord): string {
  return hashJson(storedSessionDocument(session));
}

function hashJson(document: string): string {
  return createHash("sha256").update(document).digest("hex");
}

function verifyJsonHash(document: string, storedHash: unknown, label: string): void {
  if (typeof storedHash !== "string" || storedHash.length !== 64 || hashJson(document) !== storedHash) {
    throw new Error(`${label} failed its integrity check.`);
  }
}

function requiredStoredString(value: unknown, label: string): string {
  if (typeof value !== "string") throw new Error(`${label} is missing or invalid.`);
  return value;
}

function tableExists(database: DatabaseSync, table: string): boolean {
  return Boolean(database.prepare(
    "SELECT 1 AS present FROM sqlite_master WHERE type = 'table' AND name = ?",
  ).get(table));
}

function columnExists(database: DatabaseSync, table: string, column: string): boolean {
  if (!/^[A-Za-z_][A-Za-z0-9_]*$/u.test(table)) throw new Error(`Invalid table name: ${table}.`);
  return (database.prepare(`PRAGMA table_info(${table})`).all() as Array<{ name?: unknown }>)
    .some((row) => row.name === column);
}

function sessionSummary(
  session: AppServerSessionRecord,
  tokenUsage: AppServerSessionTokenUsage = { totalTokens: 0 },
  lastMessageAt = latestTranscriptTimestamp(session.events),
  activityCounts: AppServerSessionActivityCounts = sessionActivityCounts(session.events),
): AppServerSessionSummary {
  return {
    schemaVersion: session.schemaVersion,
    id: session.id,
    workspaceId: session.workspaceId,
    status: session.status,
    title: session.title,
    prompt: session.prompt,
    summary: session.summary,
    provider: session.provider,
    model: session.model,
    reasoningEffort: session.reasoningEffort,
    workflowId: session.workflowId,
    profile: session.profile,
    metadata: session.metadata,
    finalDisposition: session.finalDisposition,
    attempts: session.attempts.map(({ capture: _capture, ...attempt }) => attempt),
    lastMessageAt,
    tokenUsage,
    activityCounts,
    createdAt: session.createdAt,
    startedAt: session.startedAt,
    endedAt: session.endedAt,
    updatedAt: session.updatedAt,
    revision: session.revision,
  };
}

function latestTranscriptTimestamp(events: readonly AppServerSessionEvent[]): string | null {
  for (let index = events.length - 1; index >= 0; index -= 1) {
    const event = events[index];
    if (event?.kind !== "beale.transcript") continue;
    const record = recordValue(recordValue(event.payload)?.record);
    return optionalString(record?.createdAt) ?? event.timestamp;
  }
  return null;
}

function sessionActivityCounts(events: readonly AppServerSessionEvent[]): AppServerSessionActivityCounts {
  const activities = new Set<string>();
  for (const event of events) {
    for (const activity of sessionToolActivityEntries(event)) {
      activities.add(`${activity.toolName}\u0000${activity.key}`);
    }
  }
  let memorySearches = 0;
  let memoryUpdates = 0;
  for (const activity of activities) {
    const toolName = activity.slice(0, activity.indexOf("\u0000"));
    if (toolName === "history.search" || toolName === "memory.search") memorySearches += 1;
    if (toolName === "memory.save" || toolName === "memory.correct" || toolName === "memory.link") {
      memoryUpdates += 1;
    }
  }
  return { memorySearches, memoryUpdates };
}

function sessionToolActivityEntries(
  event: AppServerSessionEvent,
): Array<{ key: string; toolName: string }> {
  const entries = new Map<string, { key: string; toolName: string }>();
  const add = (
    kind: string | null,
    payload: Record<string, unknown> | null,
    fallbackKey: string,
    outer: Record<string, unknown> | null = null,
  ): void => {
    if (kind !== "tool.requested" && kind !== "tool.observed") return;
    const toolName = optionalString(payload?.toolName) ?? optionalString(outer?.toolName);
    if (!toolName || ![
      "memory.search",
      "history.search",
      "history.mark_duplicate",
      "history.undo_duplicate",
      "memory.save",
      "memory.correct",
      "memory.link",
    ].includes(toolName)) return;
    const key = optionalString(payload?.toolActionId)
      ?? optionalString(outer?.toolActionId)
      ?? optionalString(outer?.toolCallId)
      ?? fallbackKey;
    entries.set(`${toolName}\u0000${key}`, { key, toolName });
  };

  const payload = recordValue(event.payload);
  if (event.kind === "research.event") {
    const researchEvent = recordValue(payload?.event);
    add(
      optionalString(researchEvent?.kind),
      recordValue(researchEvent?.payload),
      event.id,
      researchEvent,
    );
  }

  const traceRecords = event.kind === "beale.trace_batch" && Array.isArray(payload?.records)
    ? payload.records
    : event.kind === "beale.trace" && payload?.record
      ? [payload.record]
      : [];
  for (const candidate of traceRecords) {
    const trace = recordValue(candidate);
    const tracePayload = recordValue(trace?.payload);
    const toolPayload = recordValue(tracePayload?.payload) ?? tracePayload;
    const traceType = optionalString(trace?.type);
    add(
      optionalString(readCompatibleRecordValue(tracePayload, "appServerKind"))
        ?? (traceType === "tool_call" ? "tool.requested" : traceType === "tool_result" ? "tool.observed" : null),
      toolPayload,
      optionalString(trace?.id) ?? event.id,
      {
        ...(tracePayload ?? {}),
        toolCallId: trace?.toolCallId,
      },
    );
  }
  return [...entries.values()];
}

function finiteNonNegativeNumber(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? Math.max(0, value) : null;
}

function sessionMutationReceipt(session: AppServerSessionRecord): AppServerSessionMutationReceipt {
  return {
    sessionId: session.id,
    status: session.status,
    revision: session.revision,
    updatedAt: session.updatedAt,
  };
}

function compactImportedCapture(
  capture: Record<string, unknown>,
  sessionId: string,
  attemptId: string,
  capturedAt: string,
): AppServerSessionCapture {
  const timelineCount = Array.isArray(capture.eventTimeline) ? capture.eventTimeline.length : 0;
  const agent = recordValue(capture.agent);
  const agentRaw = recordValue(agent?.raw);
  const agentDiagnosticCount = Array.isArray(agentRaw?.agentEvents) ? agentRaw.agentEvents.length : 0;
  const eventStreams: AppServerSessionCapture["eventStreams"] = {
    timeline: sessionCaptureEventReference(sessionId, attemptId, timelineCount),
    agentDiagnostics: sessionCaptureEventReference(sessionId, attemptId, agentDiagnosticCount),
  };
  const compactedRaw: Record<string, unknown> = { ...capture };
  delete compactedRaw.eventTimeline;
  compactedRaw.eventTimelineRef = eventStreams.timeline;
  if (agent) {
    const compactedAgent: Record<string, unknown> = { ...agent };
    if (agentRaw) {
      const compactedAgentRaw: Record<string, unknown> = { ...agentRaw };
      delete compactedAgentRaw.agentEvents;
      compactedAgentRaw.agentEventsRef = eventStreams.agentDiagnostics;
      compactedAgent.raw = compactedAgentRaw;
    }
    compactedRaw.agent = compactedAgent;
  }
  return {
    attemptId,
    capturedAt,
    schemaVersion: numberValue(capture.schemaVersion),
    eventStreams,
    raw: compactedRaw,
  };
}

function compactStoredCapture(value: unknown, sessionId: string, attemptId: string): AppServerSessionCapture {
  if (!isRecord(value)) throw new Error("app-server session capture is invalid.");
  const raw = recordValue(value.raw) ?? value;
  const capturedAt = optionalString(value.capturedAt) ?? optionalString(raw.capturedAt) ?? new Date(0).toISOString();
  const schemaVersion = typeof raw.schemaVersion === "number" ? raw.schemaVersion : value.schemaVersion;
  const compacted = compactImportedCapture({ ...raw, schemaVersion }, sessionId, attemptId, capturedAt);
  const storedStreams = recordValue(value.eventStreams);
  const storedTimeline = recordValue(storedStreams?.timeline);
  const storedDiagnostics = recordValue(storedStreams?.agentDiagnostics);
  const eventStreams: AppServerSessionCapture["eventStreams"] = {
    timeline: sessionCaptureEventReference(
      sessionId,
      attemptId,
      finiteNonNegativeNumber(storedTimeline?.count) ?? compacted.eventStreams.timeline.count,
    ),
    agentDiagnostics: sessionCaptureEventReference(
      sessionId,
      attemptId,
      finiteNonNegativeNumber(storedDiagnostics?.count) ?? compacted.eventStreams.agentDiagnostics.count,
    ),
  };
  const compactedRaw: Record<string, unknown> = { ...compacted.raw, eventTimelineRef: eventStreams.timeline };
  const compactedAgent = recordValue(compactedRaw.agent);
  const compactedAgentRaw = recordValue(compactedAgent?.raw);
  if (compactedAgent && compactedAgentRaw) {
    compactedRaw.agent = {
      ...compactedAgent,
      raw: { ...compactedAgentRaw, agentEventsRef: eventStreams.agentDiagnostics },
    };
  }
  return {
    ...compacted,
    schemaVersion: typeof value.schemaVersion === "number" ? numberValue(value.schemaVersion) : compacted.schemaVersion,
    eventStreams,
    raw: compactedRaw,
  };
}

function sessionCaptureEventReference(
  sessionId: string,
  attemptId: string,
  count: number,
): AppServerSessionCaptureEventReference {
  return {
    source: "app_server_session_events",
    sessionId,
    attemptId,
    count: Math.max(0, Math.trunc(count)),
  };
}

function decodeCaptureRow(row: StoredSessionCaptureRow, sessionId: string): AppServerSessionCapture {
  const attemptId = requiredStoredString(row.attempt_id, "app-server session capture attempt id");
  const document = requiredStoredString(row.capture_json, "app-server session capture");
  verifyJsonHash(document, row.content_hash, "app-server session capture");
  const parsed = JSON.parse(document) as unknown;
  return compactStoredCapture(parsed, sessionId, attemptId);
}

function decodeEventRow(row: StoredSessionEventRow): AppServerSessionEvent {
  const document = requiredStoredString(row.event_json, "app-server session event");
  verifyJsonHash(document, row.content_hash, "app-server session event");
  return normalizeEvent(JSON.parse(document) as AppServerSessionEvent);
}

function projectOversizedSessionEvent(event: AppServerSessionEvent, sizeBytes: number): AppServerSessionEvent {
  const payload = recordValue(event.payload);
  if (event.kind === "beale.trace_batch" && Array.isArray(payload?.records)) {
    return {
      ...event,
      payload: {
        detailAvailableOnRequest: true,
        sizeBytes,
        records: payload.records.slice(0, 256).flatMap((candidate) => {
          const record = recordValue(candidate);
          if (!record) return [];
          return [{
            id: optionalString(record.id) ?? `projected_${randomUUID()}`,
            runId: optionalString(record.runId) ?? "",
            attemptId: optionalString(record.attemptId),
            sequence: finiteNonNegativeNumber(record.sequence) ?? 0,
            type: optionalString(record.type) ?? "research_event",
            source: optionalString(record.source) ?? "executor",
            summary: truncateText(optionalString(record.summary) ?? "Large trace event", 400),
            payload: { detailAvailableOnRequest: true, sizeBytes },
            sensitivity: optionalString(record.sensitivity) ?? "internal",
            modelVisible: record.modelVisible !== false,
            createdAt: optionalString(record.createdAt) ?? event.timestamp,
            artifactId: optionalString(record.artifactId),
            toolCallId: optionalString(record.toolCallId),
            approvalId: optionalString(record.approvalId),
          }];
        }),
      },
    };
  }
  if (event.kind === "beale.transcript") {
    const record = recordValue(payload?.record);
    if (record) {
      return {
        ...event,
        payload: {
          record: {
            ...record,
            contentMarkdown: truncateText(optionalString(record.contentMarkdown) ?? "", 8_000),
            metadata: {
              ...(recordValue(record.metadata) ?? {}),
              detailAvailableOnRequest: true,
              sizeBytes,
            },
          },
        },
      };
    }
  }
  return {
    ...event,
    payload: { detailAvailableOnRequest: true, sizeBytes },
  };
}

function eventStreamSql(stream: AppServerSessionEventStream): string {
  if (stream === "transcript") return "AND json_extract(event_json, '$.kind') = 'beale.transcript'";
  if (stream === "commentary") {
    return `AND (
      json_extract(event_json, '$.kind') = 'beale.transcript'
      OR (
        json_extract(event_json, '$.kind') IN ('model.output', 'model.thought')
        AND json_extract(event_json, '$.payload.phase') = 'completed'
        AND length(trim(COALESCE(json_extract(event_json, '$.payload.text'), ''))) > 0
      )
      OR (
        json_extract(event_json, '$.kind') = 'research.event'
        AND json_extract(event_json, '$.payload.event.kind') = 'tool.requested'
      )
    )`;
  }
  if (stream === "trace") {
    return `AND json_extract(event_json, '$.kind') NOT IN (
      'beale.transcript', 'beale.breakout_room', 'beale.breakout_member', 'beale.breakout_message'
    )`;
  }
  return "";
}

function projectCommentarySessionEvent(sessionId: string, event: AppServerSessionEvent): AppServerSessionEvent {
  if (event.kind === "beale.transcript") return event;
  if (event.kind === "model.output" || event.kind === "model.thought") {
    const payload = recordValue(event.payload) ?? {};
    const thought = event.kind === "model.thought";
    const messagePhase = thought ? undefined : optionalString(payload.messagePhase);
    const phase = thought ? undefined : messagePhase === "commentary" ? "commentary" : "final_answer";
    const source = thought
      ? "openai_reasoning_summary"
      : phase === "commentary" ? "app_server_commentary" : "app-server";
    const agentPath = optionalString(payload.agentPath) ?? event.agentPath ?? "/root";
    return {
      id: event.id,
      kind: "beale.transcript",
      timestamp: event.timestamp,
      summary: "beale.transcript",
      payload: {
        record: {
          id: `transcript_${event.id}`,
          runId: sessionId,
          attemptId: optionalString(payload.attemptId),
          traceEventId: event.id,
          role: "assistant",
          phase: phase ?? null,
          contentMarkdown: optionalString(payload.text) ?? "",
          source,
          metadata: {
            agentPath,
            ...(optionalString(payload.agentId) ? { agentId: optionalString(payload.agentId) } : {}),
            ...(optionalString(payload.parentAgentId) ? { parentAgentId: optionalString(payload.parentAgentId) } : {}),
            ...(optionalString(payload.responseId) ? { responseId: optionalString(payload.responseId) } : {}),
            ...(optionalString(payload.itemId) ? { itemId: optionalString(payload.itemId) } : {}),
            ...(typeof payload.turn === "number" ? { turn: payload.turn } : {}),
            ...(optionalString(payload.provider) ? { provider: optionalString(payload.provider) } : {}),
            ...(optionalString(payload.model) ? { model: optionalString(payload.model) } : {}),
            ...(thought ? { phase: "progress" } : { messagePhase: phase }),
          },
          createdAt: event.timestamp,
        },
      },
      ...(optionalString(payload.agentId) ? { agentId: optionalString(payload.agentId)! } : {}),
      agentPath,
      ...(optionalString(payload.parentAgentId) ? { parentAgentId: optionalString(payload.parentAgentId)! } : {}),
    };
  }
  const payload = recordValue(event.payload);
  const researchEvent = recordValue(payload?.event);
  const toolPayload = recordValue(researchEvent?.payload);
  const toolName = optionalString(toolPayload?.toolName) ?? "tool";
  const normalizedInputs = recordValue(toolPayload?.normalizedInputs);
  const copy = commentaryToolCopy(toolName, normalizedInputs);
  const agentPath = optionalString(researchEvent?.agentPath)
    ?? optionalString(payload?.agentPath)
    ?? optionalString(event.agentPath)
    ?? "/root";
  const agentId = optionalString(researchEvent?.agentId) ?? optionalString(event.agentId);
  const parentAgentId = optionalString(researchEvent?.parentAgentId) ?? optionalString(event.parentAgentId);
  const actionId = optionalString(toolPayload?.toolActionId) ?? optionalString(researchEvent?.id) ?? event.id;
  return {
    id: event.id,
    kind: "beale.tool_summary",
    timestamp: event.timestamp,
    summary: "beale.tool_summary",
    payload: {
      record: {
        id: `tool_summary_${actionId}`,
        runId: sessionId,
        attemptId: null,
        traceEventId: null,
        role: "assistant",
        phase: "tool",
        contentMarkdown: copy.label,
        source: "app_server_tool_summary",
        metadata: {
          agentPath,
          toolName,
          toolCount: 1,
          toolPluralTemplate: copy.pluralTemplate,
        },
        createdAt: event.timestamp,
      },
    },
    ...(agentId ? { agentId } : {}),
    agentPath,
    ...(parentAgentId ? { parentAgentId } : {}),
  };
}

function commentaryTranscriptCorrelationKey(value: unknown): string | null {
  const record = recordValue(value);
  if (!record) return null;
  const source = optionalString(record.source);
  if (source !== "app_server_commentary" && source !== "app-server" && source !== "openai_reasoning_summary") {
    return null;
  }
  const metadata = recordValue(record.metadata);
  const responseId = optionalString(metadata?.responseId);
  const itemId = optionalString(metadata?.itemId);
  const turn = typeof metadata?.turn === "number" ? String(metadata.turn) : "";
  if (!responseId && !itemId) return null;
  return [
    source,
    optionalString(metadata?.agentPath) ?? "/root",
    responseId ?? "",
    itemId ?? "",
    turn,
  ].join("\u0000");
}

function commentaryToolCopy(
  toolName: string,
  inputs: Record<string, unknown> | null,
): { label: string; pluralTemplate: string } {
  if (toolName === "file.read") {
    const path = optionalString(inputs?.path);
    const fileName = path?.split(/[\\/]/u).filter(Boolean).at(-1);
    return {
      label: fileName ? `Read ${truncateText(fileName, 100)}` : "Read a file",
      pluralTemplate: "Read {count} files",
    };
  }
  if (toolName === "shell.run") {
    const utility = optionalString(inputs?.utility) ?? firstCommandWord(optionalString(inputs?.command));
    const commandName = utility?.split(/[\\/]/u).filter(Boolean).at(-1);
    return {
      label: commandName ? `Ran ${truncateText(commandName, 80)}` : "Ran a command",
      pluralTemplate: "Ran {count} commands",
    };
  }
  const copies: Readonly<Record<string, readonly [string, string]>> = {
    "memory.get": ["Read a memory", "Read {count} memories"],
    "memory.search": ["Searched memory", "Ran {count} memory searches"],
    "history.search": ["Searched workspace history", "Ran {count} workspace history searches"],
    "history.mark_duplicate": ["Marked a workspace-history duplicate", "Marked {count} workspace-history duplicates"],
    "history.undo_duplicate": ["Restored a workspace-history record", "Restored {count} workspace-history records"],
    "investigation.status": ["Read the campaign track", "Read the campaign track {count} times"],
    "investigation.recall": ["Recalled campaign evidence", "Ran {count} campaign recalls"],
    "investigation.question": ["Updated a research question", "Updated {count} research questions"],
    "investigation.experiment": ["Updated an experiment", "Updated {count} experiments"],
    "investigation.observe": ["Recorded an observation", "Recorded {count} observations"],
    "investigation.next_action": ["Updated a next action", "Updated {count} next actions"],
    "investigation.review_claim": ["Reviewed a claim", "Reviewed {count} claims"],
    "investigation.consolidate": ["Prepared memory consolidation", "Prepared memory consolidation {count} times"],
    "investigation.review_consolidation": ["Reviewed memory consolidation", "Reviewed {count} memory consolidations"],
    "repository.search": ["Searched the repository", "Ran {count} repository searches"],
    "runbook.get": ["Read a runbook", "Read {count} runbooks"],
    "report.get": ["Read a report", "Read {count} reports"],
  };
  const copy = copies[toolName];
  if (copy) return { label: copy[0], pluralTemplate: copy[1] };
  const name = toolName.split(/[._-]+/u).filter(Boolean).at(-1) ?? "tool";
  return {
    label: `Used ${name}`,
    pluralTemplate: `Used ${name} {count} times`,
  };
}

function firstCommandWord(command: string | null | undefined): string | undefined {
  return command?.trim().split(/\s+/u).find(Boolean);
}

function latestRecordEvents(events: readonly AppServerSessionEvent[]): AppServerSessionEvent[] {
  const latest = new Map<string, AppServerSessionEvent>();
  for (const event of events) {
    const payload = recordValue(event.payload);
    const record = recordValue(payload?.record);
    const id = optionalString(record?.id) ?? event.id;
    latest.set(id, event);
  }
  return [...latest.values()];
}

function boundedInteger(value: unknown, fallback: number, minimum: number, maximum: number): number {
  if (typeof value !== "number" || !Number.isFinite(value)) return fallback;
  return Math.max(minimum, Math.min(maximum, Math.trunc(value)));
}

function truncateText(value: string, maximum: number): string {
  return value.length <= maximum ? value : `${value.slice(0, Math.max(0, maximum - 1))}…`;
}

function numericOffset(value: unknown): number | null {
  return typeof value === "number" && Number.isInteger(value) && value >= 0 ? value : null;
}

function decodeStoredSession(value: unknown): AppServerSessionRecord {
  const parsed = typeof value === "string" ? JSON.parse(value) as unknown : value;
  if (!isRecord(parsed)
    || parsed.schemaVersion !== APP_SERVER_SESSION_SCHEMA_VERSION
    || typeof parsed.id !== "string"
    || typeof parsed.workspaceId !== "string"
    || typeof parsed.revision !== "number") {
    throw new Error("Stored app-server session is invalid or unsupported.");
  }
  return parsed as unknown as AppServerSessionRecord;
}

function decodeCapture(value: unknown): Record<string, unknown> {
  if (!isRecord(value) || (value.schemaVersion !== 4 && value.schemaVersion !== 5)) {
    throw new Error("app-server capture must use schema version 4 or 5.");
  }
  if (!isRecord(value.agent) || !isRecord(value.request)) {
    throw new Error("app-server capture is missing its request or agent record.");
  }
  return value;
}

function validateCaptureResearchProfile(
  capture: Record<string, unknown>,
  session: AppServerSessionRecord
): void {
  const expected = recordValue(session.profile);
  const expectedId = optionalString(expected?.id);
  const expectedHash = optionalString(expected?.hash);
  if (!expectedId && !expectedHash && !session.workflowId) return;

  const captured = recordValue(capture.researchProfile);
  if (!captured) {
    throw new Error(`app-server capture is missing the research profile pinned to session ${session.id}.`);
  }
  const capturedId = optionalString(captured.id);
  const capturedHash = optionalString(captured.hash);
  const capturedWorkflowId = optionalString(captured.workflowId);
  const snapshot = normalizeResearchProfile(captured.snapshot);
  const computedHash = researchProfileHash(snapshot);
  if (!capturedHash || capturedHash !== computedHash) {
    throw new Error(
      `app-server capture research profile hash mismatch for ${capturedId ?? snapshot.id}@${snapshot.version}.`
    );
  }
  if (
    (expectedId && capturedId !== expectedId)
    || (expectedHash && capturedHash !== expectedHash)
    || (session.workflowId && capturedWorkflowId !== session.workflowId)
  ) {
    throw new Error(
      `app-server capture research profile does not match the profile and workflow pinned to session ${session.id}.`
    );
  }
}

function decodeDisposition(value: unknown): AppServerSessionDisposition | null {
  const disposition = recordValue(value);
  if (!disposition) return null;
  const outcome = optionalString(disposition.outcome);
  const summary = optionalString(disposition.summary);
  if (!outcome || !summary || typeof disposition.externalStateRequired !== "boolean") return null;
  return {
    ...disposition,
    outcome,
    summary,
    externalStateRequired: disposition.externalStateRequired,
    blockerDependencies: Array.isArray(disposition.blockerDependencies)
      ? disposition.blockerDependencies
      : [],
  };
}

function captureEvent(value: unknown): AppServerSessionEvent | null {
  const event = recordValue(value);
  if (!event) return null;
  const id = optionalString(event.id) ?? `capture_event_${randomUUID()}`;
  const kind = optionalString(event.kind);
  if (!kind) return null;
  return normalizeEvent({
    id,
    kind,
    timestamp: optionalString(event.timestamp) ?? new Date().toISOString(),
    summary: optionalString(event.summary) ?? kind,
    payload: event.payload ?? null,
    ...(optionalString(event.agentId) ? { agentId: optionalString(event.agentId)! } : {}),
    ...(optionalString(event.agentPath) ? { agentPath: optionalString(event.agentPath)! } : {}),
    ...(optionalString(event.parentAgentId) ? { parentAgentId: optionalString(event.parentAgentId)! } : {}),
  });
}

function normalizeEvent(event: AppServerSessionEvent): AppServerSessionEvent {
  return {
    id: requiredString(event.id, "Session event id"),
    kind: requiredString(event.kind, "Session event kind"),
    timestamp: requiredString(event.timestamp, "Session event timestamp"),
    summary: requiredString(event.summary, "Session event summary"),
    payload: event.payload,
    ...(optionalString(event.agentId) ? { agentId: optionalString(event.agentId)! } : {}),
    ...(optionalString(event.agentPath) ? { agentPath: optionalString(event.agentPath)! } : {}),
    ...(optionalString(event.parentAgentId) ? { parentAgentId: optionalString(event.parentAgentId)! } : {}),
  };
}

function completionSummary(agentStatus: string | null, goalStatus: string | null, agent: Record<string, unknown>): string {
  if (agentStatus === "complete" && goalStatus === "blocked") {
    return "app-server stopped because the research goal is genuinely blocked on external state.";
  }
  if (agentStatus === "complete" && goalStatus === "active") {
    return "app-server exited while the research goal was still active.";
  }
  if (agentStatus === "complete") return "app-server completed the research session.";
  return `app-server process failed: ${optionalString(agent.outputText) ?? "Unknown app-server error."}`;
}

function terminalStatus(status: AppServerSessionStatus): boolean {
  return status === "blocked" || status === "completed" || status === "failed" || status === "stopped";
}

function requiredString(value: unknown, label: string): string {
  const normalized = optionalString(value);
  if (!normalized) throw new Error(`${label} must be a non-empty string.`);
  return normalized;
}

function optionalString(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function numberValue(value: unknown): number {
  if (typeof value !== "number" || !Number.isInteger(value) || value <= 0) {
    throw new Error("app-server capture schema version is invalid.");
  }
  return value;
}

function recordValue(value: unknown): Record<string, unknown> | null {
  return isRecord(value) ? value : null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}
