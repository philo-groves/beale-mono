import { randomUUID } from "node:crypto";
import { chmodSync, existsSync, mkdirSync } from "node:fs";
import { dirname } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { applyDatabaseMigrations } from "./database-migrations.js";
import { getDefaultMemoryDatabasePath } from "./storage.js";

export type ResearchChannelMessageKind = "message" | "evidence" | "decision" | "system";
export type ResearchChannelMemberStatus = "pending" | "running" | "completed" | "interrupted" | "errored" | "unknown";
export type ResearchChannelSharedResourceKind = "file" | "runbook" | "memory";
export const MAX_RESEARCH_CHANNEL_NAME_WORDS = 3;
export const MAX_RESEARCH_CHANNEL_AGENT_MESSAGE_CHARACTERS = 600;

export interface ResearchChannelRecord {
  id: string;
  workspaceId: string;
  name: string;
  title: string;
  topic: string;
  createdBySessionId: string | null;
  createdByAgentPath: string;
  createdAt: string;
  updatedAt: string;
  archivedAt?: string | null;
}

export interface ResearchChannelMemberRecord {
  id: string;
  channelId: string;
  sessionId: string | null;
  agentId: string | null;
  agentPath: string;
  provider: string | null;
  model: string | null;
  role: string;
  status: ResearchChannelMemberStatus;
  joinedAt: string;
  lastSeenAt: string;
}

export interface ResearchChannelMessageRecord {
  id: string;
  channelId: string;
  sessionId: string | null;
  attemptId: string | null;
  memberId: string | null;
  senderAgentPath: string;
  kind: ResearchChannelMessageKind;
  contentMarkdown: string;
  evidenceRefs: string[];
  metadata: Record<string, unknown>;
  createdAt: string;
}

export interface ResearchChannelSharedResourceRecord {
  id: string;
  channelId: string;
  sessionId: string | null;
  memberId: string | null;
  messageId: string;
  senderAgentPath: string;
  kind: ResearchChannelSharedResourceKind;
  resourceId: string;
  title: string;
  createdAt: string;
  updatedAt: string;
}

export interface ResearchChannelSummary extends ResearchChannelRecord {
  memberCount: number;
  messageCount: number;
  latestMessagePreview: string | null;
}

export interface ResearchChannelDetail {
  channel: ResearchChannelRecord;
  members: ResearchChannelMemberRecord[];
  messages: ResearchChannelMessageRecord[];
  sharedResources: ResearchChannelSharedResourceRecord[];
}

export interface CreateResearchChannelInput {
  workspaceId: string;
  name: string;
  title?: string;
  topic: string;
  createdBySessionId?: string | null;
  createdByAgentPath?: string;
  createdAt?: string;
}

export interface JoinResearchChannelInput {
  workspaceId: string;
  channel: string;
  sessionId?: string | null;
  agentId?: string | null;
  agentPath: string;
  provider?: string | null;
  model?: string | null;
  role?: string;
  status?: ResearchChannelMemberStatus;
  joinedAt?: string;
}

export interface AppendResearchChannelMessageInput extends JoinResearchChannelInput {
  attemptId?: string | null;
  kind?: ResearchChannelMessageKind;
  contentMarkdown: string;
  evidenceRefs?: readonly string[];
  metadata?: Record<string, unknown>;
  createdAt?: string;
}

export interface ShareResearchChannelResourceInput extends JoinResearchChannelInput {
  attemptId?: string | null;
  kind: ResearchChannelSharedResourceKind;
  resourceId: string;
  title: string;
  note?: string;
  createdAt?: string;
}

export interface ShareResearchChannelResourceResult {
  resource: ResearchChannelSharedResourceRecord;
  message: ResearchChannelMessageRecord;
}

export interface ResearchChannelStoreOptions {
  databasePath?: string;
  workspaceRoot?: string;
}

interface ChannelRow {
  id?: unknown;
  workspace_id?: unknown;
  name?: unknown;
  title?: unknown;
  topic?: unknown;
  created_by_session_id?: unknown;
  created_by_agent_path?: unknown;
  created_at?: unknown;
  updated_at?: unknown;
  archived_at?: unknown;
}

const CHANNEL_MIGRATIONS = [
  {
    version: 1,
    name: "create_workspace_channels",
    up(database: DatabaseSync): void {
      database.exec(`
      CREATE TABLE IF NOT EXISTS app_server_channels (
        id TEXT PRIMARY KEY,
        workspace_id TEXT NOT NULL,
        name TEXT NOT NULL,
        title TEXT NOT NULL,
        topic TEXT NOT NULL,
        created_by_session_id TEXT,
        created_by_agent_path TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        UNIQUE(workspace_id, name)
      );
      CREATE INDEX IF NOT EXISTS app_server_channels_workspace_updated
      ON app_server_channels(workspace_id, updated_at DESC, id DESC);

      CREATE TABLE IF NOT EXISTS app_server_channel_members (
        id TEXT PRIMARY KEY,
        channel_id TEXT NOT NULL REFERENCES app_server_channels(id) ON DELETE CASCADE,
        session_id TEXT NOT NULL,
        agent_id TEXT,
        agent_path TEXT NOT NULL,
        provider TEXT,
        model TEXT,
        role TEXT NOT NULL,
        joined_at TEXT NOT NULL,
        last_seen_at TEXT NOT NULL,
        UNIQUE(channel_id, session_id, agent_path)
      );
      CREATE INDEX IF NOT EXISTS app_server_channel_members_channel
      ON app_server_channel_members(channel_id, joined_at ASC, id ASC);

      CREATE TABLE IF NOT EXISTS app_server_channel_messages (
        id TEXT PRIMARY KEY,
        channel_id TEXT NOT NULL REFERENCES app_server_channels(id) ON DELETE CASCADE,
        session_id TEXT,
        attempt_id TEXT,
        member_id TEXT REFERENCES app_server_channel_members(id) ON DELETE SET NULL,
        sender_agent_path TEXT NOT NULL,
        kind TEXT NOT NULL CHECK(kind IN ('message', 'evidence', 'decision', 'system')),
        content_markdown TEXT NOT NULL,
        evidence_refs_json TEXT NOT NULL,
        metadata_json TEXT NOT NULL,
        created_at TEXT NOT NULL
      );
      CREATE INDEX IF NOT EXISTS app_server_channel_messages_channel_created
      ON app_server_channel_messages(channel_id, created_at ASC, id ASC);
      `);
    },
  },
  {
    version: 2,
    name: "add_channel_member_status",
    up(database: DatabaseSync): void {
      database.exec(`
        ALTER TABLE app_server_channel_members
        ADD COLUMN status TEXT NOT NULL DEFAULT 'unknown'
        CHECK(status IN ('pending', 'running', 'completed', 'interrupted', 'errored', 'unknown'));
      `);
    },
  },
  {
    version: 3,
    name: "add_channel_shared_resources",
    up(database: DatabaseSync): void {
      database.exec(`
        CREATE TABLE IF NOT EXISTS app_server_channel_shared_resources (
          id TEXT PRIMARY KEY,
          channel_id TEXT NOT NULL REFERENCES app_server_channels(id) ON DELETE CASCADE,
          session_id TEXT,
          member_id TEXT REFERENCES app_server_channel_members(id) ON DELETE SET NULL,
          message_id TEXT NOT NULL REFERENCES app_server_channel_messages(id) ON DELETE CASCADE,
          sender_agent_path TEXT NOT NULL,
          resource_kind TEXT NOT NULL CHECK(resource_kind IN ('file', 'runbook', 'memory')),
          resource_id TEXT NOT NULL,
          title TEXT NOT NULL,
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL,
          UNIQUE(channel_id, resource_kind, resource_id)
        );
        CREATE INDEX IF NOT EXISTS app_server_channel_shared_resources_channel_updated
        ON app_server_channel_shared_resources(channel_id, updated_at DESC, id DESC);
      `);
    },
  },
  {
    version: 4,
    name: "add_channel_archiving",
    up(database: DatabaseSync): void {
      const columns = database.prepare("PRAGMA table_info(app_server_channels)").all() as Array<{ name?: unknown }>;
      if (!columns.some((column) => column.name === "archived_at")) {
        database.exec("ALTER TABLE app_server_channels ADD COLUMN archived_at TEXT;");
      }
      database.exec(`
        CREATE INDEX IF NOT EXISTS app_server_channels_workspace_archived_updated
        ON app_server_channels(workspace_id, archived_at, updated_at DESC, id DESC);
      `);
    },
  },
] as const;

export class ResearchChannelStore {
  public readonly databasePath: string;
  private readonly database: DatabaseSync;

  public constructor(options: ResearchChannelStoreOptions = {}) {
    this.databasePath = options.databasePath
      ?? process.env.APP_SERVER_DATABASE_PATH?.trim()
      ?? getDefaultMemoryDatabasePath(options.workspaceRoot ?? process.cwd());
    if (this.databasePath !== ":memory:") mkdirSync(dirname(this.databasePath), { recursive: true });
    this.database = new DatabaseSync(this.databasePath);
    if (this.databasePath !== ":memory:" && existsSync(this.databasePath)) chmodSync(this.databasePath, 0o600);
    this.database.exec("PRAGMA busy_timeout = 5000;");
    this.database.exec("PRAGMA foreign_keys = ON;");
    this.database.exec("PRAGMA journal_mode = WAL;");
    applyDatabaseMigrations(this.database, "app_server_channels", CHANNEL_MIGRATIONS);
  }

  public close(): void {
    this.database.close();
  }

  public create(input: CreateResearchChannelInput): ResearchChannelRecord {
    const workspaceId = requiredText(input.workspaceId, "Workspace id");
    const name = normalizeResearchChannelName(input.name);
    const now = input.createdAt ?? new Date().toISOString();
    const channel: ResearchChannelRecord = {
      id: `channel_${randomUUID().replaceAll("-", "")}`,
      workspaceId,
      name,
      title: optionalText(input.title) ?? researchChannelTitle(name),
      topic: requiredText(input.topic, "Channel topic"),
      createdBySessionId: optionalText(input.createdBySessionId),
      createdByAgentPath: optionalText(input.createdByAgentPath) ?? "/human",
      createdAt: now,
      updatedAt: now,
      archivedAt: null,
    };
    try {
      this.database.prepare(`
        INSERT INTO app_server_channels (
          id, workspace_id, name, title, topic, created_by_session_id,
          created_by_agent_path, created_at, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
      `).run(
        channel.id, channel.workspaceId, channel.name, channel.title, channel.topic,
        channel.createdBySessionId, channel.createdByAgentPath, channel.createdAt, channel.updatedAt,
      );
    } catch (error) {
      if (/UNIQUE constraint failed/iu.test(error instanceof Error ? error.message : String(error))) {
        throw new Error(`Channel already exists in this workspace: ${name}`);
      }
      throw error;
    }
    return channel;
  }

  public list(workspaceId: string, limit = 200, archived = false): ResearchChannelSummary[] {
    const rows = this.database.prepare(`
      SELECT channel.*,
        (SELECT COUNT(*) FROM app_server_channel_members AS member WHERE member.channel_id = channel.id) AS member_count,
        (SELECT COUNT(*) FROM app_server_channel_messages AS message WHERE message.channel_id = channel.id) AS message_count,
        (SELECT message.content_markdown FROM app_server_channel_messages AS message
          WHERE message.channel_id = channel.id ORDER BY message.created_at DESC, message.rowid DESC LIMIT 1) AS latest_message
      FROM app_server_channels AS channel
      WHERE channel.workspace_id = ? AND channel.archived_at IS ${archived ? "NOT " : ""}NULL
      ORDER BY channel.updated_at DESC, channel.id DESC
      LIMIT ?
    `).all(requiredText(workspaceId, "Workspace id"), boundedLimit(limit)) as Array<ChannelRow & {
      member_count?: unknown;
      message_count?: unknown;
      latest_message?: unknown;
    }>;
    return rows.map((row) => ({
      ...decodeChannel(row),
      memberCount: numericCount(row.member_count),
      messageCount: numericCount(row.message_count),
      latestMessagePreview: typeof row.latest_message === "string" ? row.latest_message.slice(0, 240) : null,
    }));
  }

  public get(workspaceId: string, channel: string, messageLimit = 500): ResearchChannelDetail | null {
    const record = this.resolve(workspaceId, channel);
    if (!record) return null;
    const members = this.database.prepare(`
      SELECT * FROM app_server_channel_members
      WHERE channel_id = ? ORDER BY joined_at ASC, id ASC
    `).all(record.id).map((row) => decodeMember(row as Record<string, unknown>));
    const messages = this.database.prepare(`
      SELECT * FROM (
        SELECT *, rowid AS channel_sequence FROM app_server_channel_messages
        WHERE channel_id = ? ORDER BY created_at DESC, rowid DESC LIMIT ?
      ) ORDER BY created_at ASC, channel_sequence ASC
    `).all(record.id, boundedMessageLimit(messageLimit)).map((row) => decodeMessage(row as Record<string, unknown>));
    const sharedResources = this.database.prepare(`
      SELECT * FROM app_server_channel_shared_resources
      WHERE channel_id = ? ORDER BY updated_at DESC, id DESC
    `).all(record.id).map((row) => decodeSharedResource(row as Record<string, unknown>));
    return { channel: record, members, messages, sharedResources };
  }

  public join(input: JoinResearchChannelInput): ResearchChannelMemberRecord {
    const channel = this.require(input.workspaceId, input.channel);
    const joinedAt = input.joinedAt ?? new Date().toISOString();
    const sessionId = optionalText(input.sessionId) ?? "";
    const agentPath = requiredText(input.agentPath, "Agent path");
    const id = `channel_member_${randomUUID().replaceAll("-", "")}`;
    this.database.prepare(`
      INSERT INTO app_server_channel_members (
        id, channel_id, session_id, agent_id, agent_path, provider, model, role, status, joined_at, last_seen_at
      ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
      ON CONFLICT(channel_id, session_id, agent_path) DO UPDATE SET
        agent_id = excluded.agent_id,
        provider = excluded.provider,
        model = excluded.model,
        role = excluded.role,
        status = CASE
          WHEN excluded.status = 'unknown' THEN app_server_channel_members.status
          ELSE excluded.status
        END,
        last_seen_at = excluded.last_seen_at
    `).run(
      id, channel.id, sessionId, optionalText(input.agentId), agentPath,
      optionalText(input.provider), optionalText(input.model), optionalText(input.role) ?? "researcher",
      normalizeChannelMemberStatus(input.status), joinedAt, joinedAt,
    );
    const row = this.database.prepare(`
      SELECT * FROM app_server_channel_members
      WHERE channel_id = ? AND session_id IS ? AND agent_path = ?
    `).get(channel.id, sessionId, agentPath) as Record<string, unknown> | undefined;
    if (!row) throw new Error(`Channel member registration failed for ${agentPath}.`);
    return decodeMember(row);
  }

  public append(input: AppendResearchChannelMessageInput): ResearchChannelMessageRecord {
    const channel = this.require(input.workspaceId, input.channel);
    const member = this.join(input);
    const createdAt = input.createdAt ?? new Date().toISOString();
    const message: ResearchChannelMessageRecord = {
      id: `channel_message_${randomUUID().replaceAll("-", "")}`,
      channelId: channel.id,
      sessionId: optionalText(input.sessionId),
      attemptId: optionalText(input.attemptId),
      memberId: member.id,
      senderAgentPath: member.agentPath,
      kind: normalizeMessageKind(input.kind),
      contentMarkdown: requiredText(input.contentMarkdown, "Channel message"),
      evidenceRefs: normalizedStringList(input.evidenceRefs, 48, "evidenceRefs"),
      metadata: input.metadata ?? {},
      createdAt,
    };
    this.database.exec("BEGIN IMMEDIATE;");
    try {
      this.database.prepare(`
        INSERT INTO app_server_channel_messages (
          id, channel_id, session_id, attempt_id, member_id, sender_agent_path,
          kind, content_markdown, evidence_refs_json, metadata_json, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
      `).run(
        message.id, message.channelId, message.sessionId, message.attemptId, message.memberId,
        message.senderAgentPath, message.kind, message.contentMarkdown,
        JSON.stringify(message.evidenceRefs), JSON.stringify(message.metadata), message.createdAt,
      );
      this.database.prepare("UPDATE app_server_channels SET updated_at = ? WHERE id = ?")
        .run(createdAt, channel.id);
      this.database.exec("COMMIT;");
    } catch (error) {
      this.database.exec("ROLLBACK;");
      throw error;
    }
    return message;
  }

  public share(input: ShareResearchChannelResourceInput): ShareResearchChannelResourceResult {
    const channel = this.require(input.workspaceId, input.channel);
    const member = this.join(input);
    const createdAt = input.createdAt ?? new Date().toISOString();
    const kind = normalizeSharedResourceKind(input.kind);
    const message: ResearchChannelMessageRecord = {
      id: `channel_message_${randomUUID().replaceAll("-", "")}`,
      channelId: channel.id,
      sessionId: optionalText(input.sessionId),
      attemptId: optionalText(input.attemptId),
      memberId: member.id,
      senderAgentPath: member.agentPath,
      kind: "message",
      contentMarkdown: optionalText(input.note) ?? `Shared a ${kind}.`,
      evidenceRefs: [],
      metadata: { source: "channel_share", sharedResourceKind: kind },
      createdAt,
    };
    const resourceId = requiredText(input.resourceId, "Shared resource id");
    const title = requiredText(input.title, "Shared resource title");
    const resource: ResearchChannelSharedResourceRecord = {
      id: `channel_shared_${randomUUID().replaceAll("-", "")}`,
      channelId: channel.id,
      sessionId: message.sessionId,
      memberId: member.id,
      messageId: message.id,
      senderAgentPath: member.agentPath,
      kind,
      resourceId,
      title,
      createdAt,
      updatedAt: createdAt,
    };
    this.database.exec("BEGIN IMMEDIATE;");
    try {
      this.database.prepare(`
        INSERT INTO app_server_channel_messages (
          id, channel_id, session_id, attempt_id, member_id, sender_agent_path,
          kind, content_markdown, evidence_refs_json, metadata_json, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
      `).run(
        message.id, message.channelId, message.sessionId, message.attemptId, message.memberId,
        message.senderAgentPath, message.kind, message.contentMarkdown,
        JSON.stringify(message.evidenceRefs), JSON.stringify(message.metadata), message.createdAt,
      );
      this.database.prepare(`
        INSERT INTO app_server_channel_shared_resources (
          id, channel_id, session_id, member_id, message_id, sender_agent_path,
          resource_kind, resource_id, title, created_at, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(channel_id, resource_kind, resource_id) DO UPDATE SET
          session_id = excluded.session_id,
          member_id = excluded.member_id,
          message_id = excluded.message_id,
          sender_agent_path = excluded.sender_agent_path,
          title = excluded.title,
          updated_at = excluded.updated_at
      `).run(
        resource.id, resource.channelId, resource.sessionId, resource.memberId, resource.messageId,
        resource.senderAgentPath, resource.kind, resource.resourceId, resource.title,
        resource.createdAt, resource.updatedAt,
      );
      this.database.prepare("UPDATE app_server_channels SET updated_at = ? WHERE id = ?")
        .run(createdAt, channel.id);
      this.database.exec("COMMIT;");
    } catch (error) {
      this.database.exec("ROLLBACK;");
      throw error;
    }
    const stored = this.database.prepare(`
      SELECT * FROM app_server_channel_shared_resources
      WHERE channel_id = ? AND resource_kind = ? AND resource_id = ?
    `).get(channel.id, kind, resourceId) as Record<string, unknown> | undefined;
    if (!stored) throw new Error(`Channel resource sharing failed for ${resourceId}.`);
    return { resource: decodeSharedResource(stored), message };
  }

  public delete(workspaceId: string, channel: string): { channelId: string; deleted: true } {
    const record = this.require(workspaceId, channel);
    const result = this.database.prepare("DELETE FROM app_server_channels WHERE id = ? AND workspace_id = ?")
      .run(record.id, record.workspaceId);
    if (Number(result.changes) !== 1) throw new Error(`Channel could not be deleted: ${channel}`);
    return { channelId: record.id, deleted: true };
  }

  public archive(workspaceId: string, channel: string, archivedAt = new Date().toISOString()): ResearchChannelRecord {
    const record = this.require(workspaceId, channel);
    this.database.prepare("UPDATE app_server_channels SET archived_at = ? WHERE id = ? AND workspace_id = ?")
      .run(archivedAt, record.id, record.workspaceId);
    return { ...record, archivedAt };
  }

  public restore(workspaceId: string, channel: string): ResearchChannelRecord {
    const record = this.require(workspaceId, channel);
    this.database.prepare("UPDATE app_server_channels SET archived_at = NULL WHERE id = ? AND workspace_id = ?")
      .run(record.id, record.workspaceId);
    return { ...record, archivedAt: null };
  }

  private resolve(workspaceId: string, channel: string): ResearchChannelRecord | null {
    const normalizedWorkspaceId = requiredText(workspaceId, "Workspace id");
    const identifier = requiredText(channel, "Channel");
    const normalizedName = researchChannelNameSlug(identifier);
    const row = this.database.prepare(`
      SELECT * FROM app_server_channels
      WHERE workspace_id = ? AND (id = ? OR name = ?)
      LIMIT 1
    `).get(normalizedWorkspaceId, identifier, normalizedName) as ChannelRow | undefined;
    return row ? decodeChannel(row) : null;
  }

  private require(workspaceId: string, channel: string): ResearchChannelRecord {
    const record = this.resolve(workspaceId, channel);
    if (!record) throw new Error(`Channel not found in workspace: ${channel}`);
    return record;
  }
}

export function normalizeResearchChannelName(value: string): string {
  const words = researchChannelNameWords(requiredText(value, "Channel name"));
  if (words.length > MAX_RESEARCH_CHANNEL_NAME_WORDS) {
    throw new Error(`Channel name must contain at most ${MAX_RESEARCH_CHANNEL_NAME_WORDS} words.`);
  }
  const normalized = words.join("-");
  if (!normalized) throw new Error("Channel name must contain a letter or number.");
  if (normalized.length > 64) throw new Error("Channel name must contain at most 64 characters.");
  return normalized;
}

function researchChannelNameSlug(value: string): string {
  return researchChannelNameWords(value).join("-").slice(0, 64);
}

function researchChannelNameWords(value: string): string[] {
  return value.toLocaleLowerCase().match(/[a-z0-9]+/gu) ?? [];
}

export function researchChannelTitle(name: string): string {
  return normalizeResearchChannelName(name)
    .split("-")
    .map((part) => part ? `${part[0]!.toLocaleUpperCase()}${part.slice(1)}` : part)
    .join(" ");
}

function decodeChannel(row: ChannelRow): ResearchChannelRecord {
  return {
    id: storedText(row.id, "Channel id"),
    workspaceId: storedText(row.workspace_id, "Channel workspace id"),
    name: storedText(row.name, "Channel name"),
    title: storedText(row.title, "Channel title"),
    topic: storedText(row.topic, "Channel topic"),
    createdBySessionId: optionalText(row.created_by_session_id),
    createdByAgentPath: storedText(row.created_by_agent_path, "Channel creator"),
    createdAt: storedText(row.created_at, "Channel creation timestamp"),
    updatedAt: storedText(row.updated_at, "Channel update timestamp"),
    archivedAt: optionalText(row.archived_at),
  };
}

function decodeMember(row: Record<string, unknown>): ResearchChannelMemberRecord {
  return {
    id: storedText(row.id, "Channel member id"),
    channelId: storedText(row.channel_id, "Channel member channel id"),
    sessionId: optionalText(row.session_id),
    agentId: optionalText(row.agent_id),
    agentPath: storedText(row.agent_path, "Channel member agent path"),
    provider: optionalText(row.provider),
    model: optionalText(row.model),
    role: storedText(row.role, "Channel member role"),
    status: normalizeChannelMemberStatus(row.status),
    joinedAt: storedText(row.joined_at, "Channel member join timestamp"),
    lastSeenAt: storedText(row.last_seen_at, "Channel member activity timestamp"),
  };
}

function decodeMessage(row: Record<string, unknown>): ResearchChannelMessageRecord {
  return {
    id: storedText(row.id, "Channel message id"),
    channelId: storedText(row.channel_id, "Channel message channel id"),
    sessionId: optionalText(row.session_id),
    attemptId: optionalText(row.attempt_id),
    memberId: optionalText(row.member_id),
    senderAgentPath: storedText(row.sender_agent_path, "Channel message sender"),
    kind: normalizeMessageKind(storedText(row.kind, "Channel message kind")),
    contentMarkdown: storedText(row.content_markdown, "Channel message content"),
    evidenceRefs: parsedStringArray(row.evidence_refs_json, "Channel evidence references"),
    metadata: parsedRecord(row.metadata_json, "Channel message metadata"),
    createdAt: storedText(row.created_at, "Channel message timestamp"),
  };
}

function decodeSharedResource(row: Record<string, unknown>): ResearchChannelSharedResourceRecord {
  return {
    id: storedText(row.id, "Shared channel resource id"),
    channelId: storedText(row.channel_id, "Shared channel resource channel id"),
    sessionId: optionalText(row.session_id),
    memberId: optionalText(row.member_id),
    messageId: storedText(row.message_id, "Shared channel resource message id"),
    senderAgentPath: storedText(row.sender_agent_path, "Shared channel resource sender"),
    kind: normalizeSharedResourceKind(row.resource_kind),
    resourceId: storedText(row.resource_id, "Shared channel resource locator"),
    title: storedText(row.title, "Shared channel resource title"),
    createdAt: storedText(row.created_at, "Shared channel resource creation timestamp"),
    updatedAt: storedText(row.updated_at, "Shared channel resource update timestamp"),
  };
}

function normalizeMessageKind(value: unknown): ResearchChannelMessageKind {
  if (value === undefined) return "message";
  if (value === "message" || value === "evidence" || value === "decision" || value === "system") return value;
  throw new Error(`Unsupported channel message kind: ${String(value)}`);
}

function normalizeSharedResourceKind(value: unknown): ResearchChannelSharedResourceKind {
  if (value === "file" || value === "runbook" || value === "memory") return value;
  throw new Error(`Unsupported shared channel resource kind: ${String(value)}`);
}

function normalizeChannelMemberStatus(value: unknown): ResearchChannelMemberStatus {
  if (
    value === "pending"
    || value === "running"
    || value === "completed"
    || value === "interrupted"
    || value === "errored"
    || value === "unknown"
  ) return value;
  if (value === undefined || value === null) return "unknown";
  throw new Error(`Unsupported channel member status: ${String(value)}`);
}

function parsedStringArray(value: unknown, label: string): string[] {
  const parsed = parseJson(value, label);
  if (!Array.isArray(parsed) || parsed.some((entry) => typeof entry !== "string")) throw new Error(`${label} is invalid.`);
  return parsed;
}

function parsedRecord(value: unknown, label: string): Record<string, unknown> {
  const parsed = parseJson(value, label);
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) throw new Error(`${label} is invalid.`);
  return parsed as Record<string, unknown>;
}

function parseJson(value: unknown, label: string): unknown {
  if (typeof value !== "string") throw new Error(`${label} is missing.`);
  try {
    return JSON.parse(value) as unknown;
  } catch {
    throw new Error(`${label} is invalid JSON.`);
  }
}

function normalizedStringList(value: readonly string[] | undefined, maximum: number, label: string): string[] {
  if (value === undefined) return [];
  if (value.length > maximum || value.some((entry) => !entry.trim())) throw new Error(`${label} has invalid entries.`);
  return [...new Set(value.map((entry) => entry.trim()))];
}

function requiredText(value: unknown, label: string): string {
  if (typeof value !== "string" || !value.trim()) throw new Error(`${label} must be a non-empty string.`);
  return value.trim();
}

function storedText(value: unknown, label: string): string {
  if (typeof value !== "string") throw new Error(`${label} is missing or invalid.`);
  return value;
}

function optionalText(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function boundedLimit(value: number): number {
  return Math.max(1, Math.min(500, Math.trunc(value)));
}

function boundedMessageLimit(value: number): number {
  return Math.max(1, Math.min(2_000, Math.trunc(value)));
}

function numericCount(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? Math.max(0, Math.trunc(value)) : 0;
}
