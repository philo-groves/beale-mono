import { randomUUID } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import type { DatabaseSync } from "node:sqlite";
import { applyDatabaseMigrations } from "./database-migrations.js";
import { openResearchDatabase } from "./database.js";
import { getDefaultMemoryDatabasePath } from "./storage.js";
import { assertWorkspaceChild, atomicWorkspaceWrite, isPublishedWorkspacePath, readPublishedWorkspaceFile, readWorkspaceProject, workspaceContentHash, workspacePathProblem, workspaceResearchAuthority } from "./workspace-project.js";

export type ResearchTopicMessageKind = "message" | "evidence" | "decision" | "system";
export type ResearchTopicMemberStatus = "pending" | "running" | "completed" | "interrupted" | "errored" | "unknown";
export type ResearchTopicSharedResourceKind = "file" | "runbook" | "memory";
export type ResearchTopicLinkKind = "claim" | "memory" | "runbook" | "file" | "session" | "topic";
export const MAX_RESEARCH_TOPIC_NAME_WORDS = 8;

export interface ResearchTopicRecord {
  id: string;
  workspaceId: string;
  name: string;
  title: string;
  topic: string;
  overviewMarkdown: string;
  createdBySessionId: string | null;
  createdByAgentPath: string;
  createdAt: string;
  updatedAt: string;
  archivedAt?: string | null;
  mergedIntoTopicId?: string | null;
}

export interface ResearchTopicMemberRecord {
  id: string;
  topicId: string;
  sessionId: string | null;
  agentId: string | null;
  agentPath: string;
  provider: string | null;
  model: string | null;
  role: string;
  status: ResearchTopicMemberStatus;
  joinedAt: string;
  lastSeenAt: string;
}

export interface ResearchTopicMessageRecord {
  id: string;
  topicId: string;
  sessionId: string | null;
  attemptId: string | null;
  memberId: string | null;
  senderAgentPath: string;
  kind: ResearchTopicMessageKind;
  contentMarkdown: string;
  evidenceRefs: string[];
  metadata: Record<string, unknown>;
  createdAt: string;
}

export interface ResearchTopicSharedResourceRecord {
  id: string;
  topicId: string;
  sessionId: string | null;
  memberId: string | null;
  messageId: string;
  senderAgentPath: string;
  kind: ResearchTopicSharedResourceKind;
  resourceId: string;
  title: string;
  createdAt: string;
  updatedAt: string;
}

export interface ResearchTopicSummary extends ResearchTopicRecord {
  memberCount: number;
  messageCount: number;
  latestMessagePreview: string | null;
}

export interface ResearchTopicPageRecord {
  id: string;
  topicId: string;
  title: string;
  contentMarkdown: string;
  createdAt: string;
  updatedAt: string;
}

export interface ResearchTopicLinkRecord {
  id: string;
  topicId: string;
  kind: ResearchTopicLinkKind;
  resourceId: string;
  title: string;
  createdAt: string;
}

export interface ResearchTopicDetail {
  topic: ResearchTopicRecord;
  members: ResearchTopicMemberRecord[];
  messages: ResearchTopicMessageRecord[];
  sharedResources: ResearchTopicSharedResourceRecord[];
  pages: ResearchTopicPageRecord[];
  links: ResearchTopicLinkRecord[];
  mergedTopics: ResearchTopicRecord[];
}

function topicSnapshotContent(workspaceId: string, detail: ResearchTopicDetail): string {
  return `${JSON.stringify({
    schemaVersion: 1,
    workspaceId,
    topic: detail.topic,
    pages: detail.pages,
    links: detail.links,
  }, null, 2)}\n`;
}

/** Return expected hashes only for topic files represented by typed canonical storage. */
export function matchingStoredResearchTopicSnapshots(
  options: { workspaceRoot: string; workspaceId: string; databasePath: string },
  paths: readonly string[]
): Map<string, string> {
  const matching = new Map<string, string>();
  const candidates = paths.filter((path) => /^references\/topics\/[a-zA-Z0-9][a-zA-Z0-9_.-]*\.json$/u.test(path));
  if (candidates.length === 0) return matching;
  const project = readWorkspaceProject(options.workspaceRoot);
  if (project?.workspaceId !== options.workspaceId) return matching;
  const store = new ResearchTopicStore({ databasePath: options.databasePath });
  try {
    for (const path of candidates) {
      const topicId = path.slice('references/topics/'.length, -'.json'.length);
      const detail = store.get(options.workspaceId, topicId);
      if (!detail || detail.topic.id !== topicId) continue;
      const absolute = join(options.workspaceRoot, path);
      assertWorkspaceChild(options.workspaceRoot, absolute);
      const expected = topicSnapshotContent(options.workspaceId, detail);
      if (existsSync(absolute) && readFileSync(absolute, 'utf8') === expected) {
        matching.set(path, workspaceContentHash(expected));
      }
    }
  } finally {
    store.close();
  }
  return matching;
}

export interface CreateResearchTopicInput {
  workspaceId: string;
  name: string;
  title?: string;
  topic: string;
  overviewMarkdown?: string;
  createdBySessionId?: string | null;
  createdByAgentPath?: string;
  createdAt?: string;
}

export interface JoinResearchTopicInput {
  workspaceId: string;
  topic: string;
  sessionId?: string | null;
  agentId?: string | null;
  agentPath: string;
  provider?: string | null;
  model?: string | null;
  role?: string;
  status?: ResearchTopicMemberStatus;
  joinedAt?: string;
}

export interface ResearchTopicStoreOptions {
  databasePath?: string;
  workspaceRoot?: string;
}

interface TopicRow {
  id?: unknown;
  workspace_id?: unknown;
  name?: unknown;
  title?: unknown;
  topic?: unknown;
  overview_markdown?: unknown;
  created_by_session_id?: unknown;
  created_by_agent_path?: unknown;
  created_at?: unknown;
  updated_at?: unknown;
  archived_at?: unknown;
  merged_into_topic_id?: unknown;
}

const TOPIC_MIGRATIONS = [
  {
    version: 1,
    name: "create_workspace_topics",
    up(database: DatabaseSync): void {
      database.exec(`
      CREATE TABLE IF NOT EXISTS app_server_topics (
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
      CREATE INDEX IF NOT EXISTS app_server_topics_workspace_updated
      ON app_server_topics(workspace_id, updated_at DESC, id DESC);

      CREATE TABLE IF NOT EXISTS app_server_topic_members (
        id TEXT PRIMARY KEY,
        topic_id TEXT NOT NULL REFERENCES app_server_topics(id) ON DELETE CASCADE,
        session_id TEXT NOT NULL,
        agent_id TEXT,
        agent_path TEXT NOT NULL,
        provider TEXT,
        model TEXT,
        role TEXT NOT NULL,
        joined_at TEXT NOT NULL,
        last_seen_at TEXT NOT NULL,
        UNIQUE(topic_id, session_id, agent_path)
      );
      CREATE INDEX IF NOT EXISTS app_server_topic_members_topic
      ON app_server_topic_members(topic_id, joined_at ASC, id ASC);

      CREATE TABLE IF NOT EXISTS app_server_topic_messages (
        id TEXT PRIMARY KEY,
        topic_id TEXT NOT NULL REFERENCES app_server_topics(id) ON DELETE CASCADE,
        session_id TEXT,
        attempt_id TEXT,
        member_id TEXT REFERENCES app_server_topic_members(id) ON DELETE SET NULL,
        sender_agent_path TEXT NOT NULL,
        kind TEXT NOT NULL CHECK(kind IN ('message', 'evidence', 'decision', 'system')),
        content_markdown TEXT NOT NULL,
        evidence_refs_json TEXT NOT NULL,
        metadata_json TEXT NOT NULL,
        created_at TEXT NOT NULL
      );
      CREATE INDEX IF NOT EXISTS app_server_topic_messages_topic_created
      ON app_server_topic_messages(topic_id, created_at ASC, id ASC);
      `);
    },
  },
  {
    version: 2,
    name: "add_topic_member_status",
    up(database: DatabaseSync): void {
      database.exec(`
        ALTER TABLE app_server_topic_members
        ADD COLUMN status TEXT NOT NULL DEFAULT 'unknown'
        CHECK(status IN ('pending', 'running', 'completed', 'interrupted', 'errored', 'unknown'));
      `);
    },
  },
  {
    version: 3,
    name: "add_topic_shared_resources",
    up(database: DatabaseSync): void {
      database.exec(`
        CREATE TABLE IF NOT EXISTS app_server_topic_shared_resources (
          id TEXT PRIMARY KEY,
          topic_id TEXT NOT NULL REFERENCES app_server_topics(id) ON DELETE CASCADE,
          session_id TEXT,
          member_id TEXT REFERENCES app_server_topic_members(id) ON DELETE SET NULL,
          message_id TEXT NOT NULL REFERENCES app_server_topic_messages(id) ON DELETE CASCADE,
          sender_agent_path TEXT NOT NULL,
          resource_kind TEXT NOT NULL CHECK(resource_kind IN ('file', 'runbook', 'memory')),
          resource_id TEXT NOT NULL,
          title TEXT NOT NULL,
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL,
          UNIQUE(topic_id, resource_kind, resource_id)
        );
        CREATE INDEX IF NOT EXISTS app_server_topic_shared_resources_topic_updated
        ON app_server_topic_shared_resources(topic_id, updated_at DESC, id DESC);
      `);
    },
  },
  {
    version: 4,
    name: "add_topic_archiving",
    up(database: DatabaseSync): void {
      const columns = database.prepare("PRAGMA table_info(app_server_topics)").all() as Array<{ name?: unknown }>;
      if (!columns.some((column) => column.name === "archived_at")) {
        database.exec("ALTER TABLE app_server_topics ADD COLUMN archived_at TEXT;");
      }
      database.exec(`
        CREATE INDEX IF NOT EXISTS app_server_topics_workspace_archived_updated
        ON app_server_topics(workspace_id, archived_at, updated_at DESC, id DESC);
      `);
    },
  },
  {
    version: 5,
    name: "add_topic_pages_and_links",
    up(database: DatabaseSync): void {
      database.exec(`
        ALTER TABLE app_server_topics ADD COLUMN overview_markdown TEXT NOT NULL DEFAULT '';
        CREATE TABLE app_server_topic_pages (
          id TEXT PRIMARY KEY,
          topic_id TEXT NOT NULL REFERENCES app_server_topics(id) ON DELETE CASCADE,
          title TEXT NOT NULL,
          content_markdown TEXT NOT NULL,
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL
        );
        CREATE INDEX app_server_topic_pages_topic_updated
        ON app_server_topic_pages(topic_id, updated_at DESC, id DESC);
        CREATE TABLE app_server_topic_links (
          id TEXT PRIMARY KEY,
          topic_id TEXT NOT NULL REFERENCES app_server_topics(id) ON DELETE CASCADE,
          resource_kind TEXT NOT NULL CHECK(resource_kind IN ('claim', 'memory', 'runbook', 'file', 'session', 'topic')),
          resource_id TEXT NOT NULL,
          title TEXT NOT NULL,
          created_at TEXT NOT NULL,
          UNIQUE(topic_id, resource_kind, resource_id)
        );
        CREATE INDEX app_server_topic_links_resource
        ON app_server_topic_links(resource_kind, resource_id);
      `);
    },
  },
  {
    version: 6,
    name: "import_legacy_channels_as_topics",
    up(database: DatabaseSync): void {
      const hasTable = (name: string): boolean => Boolean(database.prepare(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?"
      ).get(name));
      if (!hasTable("app_server_channels")) return;
      const hasColumn = (table: string, column: string): boolean => (
        database.prepare(`PRAGMA table_info(${table})`).all() as Array<{ name: string }>
      ).some((entry) => entry.name === column);
      const archived = hasColumn("app_server_channels", "archived_at") ? "archived_at" : "NULL";
      database.exec(`
        INSERT OR IGNORE INTO app_server_topics
          (id, workspace_id, name, title, topic, created_by_session_id,
           created_by_agent_path, created_at, updated_at, archived_at, overview_markdown)
        SELECT id, workspace_id, name, title, topic, created_by_session_id,
               created_by_agent_path, created_at, updated_at, ${archived}, topic
        FROM app_server_channels;
      `);
      if (hasTable("app_server_channel_members")) {
        const status = hasColumn("app_server_channel_members", "status") ? "status" : "'unknown'";
        database.exec(`
          INSERT OR IGNORE INTO app_server_topic_members
            (id, topic_id, session_id, agent_id, agent_path, provider, model, role, joined_at, last_seen_at, status)
          SELECT id, channel_id, session_id, agent_id, agent_path, provider, model, role,
                 joined_at, last_seen_at, ${status}
          FROM app_server_channel_members;
        `);
      }
      if (hasTable("app_server_channel_messages")) {
        database.exec(`
          INSERT OR IGNORE INTO app_server_topic_messages
            (id, topic_id, session_id, attempt_id, member_id, sender_agent_path,
             kind, content_markdown, evidence_refs_json, metadata_json, created_at)
          SELECT id, channel_id, session_id, attempt_id, member_id, sender_agent_path,
                 kind, content_markdown, evidence_refs_json, metadata_json, created_at
          FROM app_server_channel_messages;
        `);
      }
      if (hasTable("app_server_channel_shared_resources")) {
        database.exec(`
          INSERT OR IGNORE INTO app_server_topic_shared_resources
            (id, topic_id, session_id, member_id, message_id, sender_agent_path,
             resource_kind, resource_id, title, created_at, updated_at)
          SELECT id, channel_id, session_id, member_id, message_id, sender_agent_path,
                 resource_kind, resource_id, title, created_at, updated_at
          FROM app_server_channel_shared_resources;
          INSERT OR IGNORE INTO app_server_topic_links
            (id, topic_id, resource_kind, resource_id, title, created_at)
          SELECT 'topic_link_' || id, channel_id, resource_kind, resource_id, title, created_at
          FROM app_server_channel_shared_resources;
        `);
      }
    },
  },
  {
    version: 7,
    name: "add_reversible_topic_merges",
    up(database: DatabaseSync): void {
      database.exec(`
        ALTER TABLE app_server_topics ADD COLUMN merged_into_topic_id TEXT REFERENCES app_server_topics(id);
        CREATE INDEX app_server_topics_merge_target ON app_server_topics(merged_into_topic_id);
      `);
    },
  },
] as const;

export class ResearchTopicStore {
  public readonly databasePath: string;
  private readonly database: DatabaseSync;
  private readonly workspaceRoot: string | null;

  public constructor(options: ResearchTopicStoreOptions = {}) {
    this.workspaceRoot = options.workspaceRoot && workspaceResearchAuthority(options.workspaceRoot) === "files"
      ? resolve(options.workspaceRoot) : null;
    this.databasePath = options.databasePath
      ?? process.env.APP_SERVER_DATABASE_PATH?.trim()
      ?? getDefaultMemoryDatabasePath(options.workspaceRoot ?? process.cwd());
    if (this.databasePath !== ":memory:") mkdirSync(dirname(this.databasePath), { recursive: true });
    this.database = openResearchDatabase(this.databasePath);
    if (this.databasePath !== ":memory:" && existsSync(this.databasePath)) chmodSync(this.databasePath, 0o600);
    this.database.exec("PRAGMA busy_timeout = 5000;");
    this.database.exec("PRAGMA foreign_keys = ON;");
    this.database.exec("PRAGMA journal_mode = WAL;");
    applyDatabaseMigrations(this.database, "app_server_topics", TOPIC_MIGRATIONS);
    if (this.workspaceRoot) this.hydrateWorkspaceFiles();
  }

  public close(): void {
    this.database.close();
  }

  public create(input: CreateResearchTopicInput): ResearchTopicRecord {
    const workspaceId = requiredText(input.workspaceId, "Workspace id");
    const name = normalizeResearchTopicName(input.name);
    const now = input.createdAt ?? new Date().toISOString();
    const topic: ResearchTopicRecord = {
      id: `topic_${randomUUID().replaceAll("-", "")}`,
      workspaceId,
      name,
      title: optionalText(input.title) ?? researchTopicTitle(name),
      topic: requiredText(input.topic, "Topic topic"),
      overviewMarkdown: optionalText(input.overviewMarkdown) ?? requiredText(input.topic, "Topic purpose"),
      createdBySessionId: optionalText(input.createdBySessionId),
      createdByAgentPath: optionalText(input.createdByAgentPath) ?? "/human",
      createdAt: now,
      updatedAt: now,
      archivedAt: null,
    };
    try {
      this.database.prepare(`
        INSERT INTO app_server_topics (
          id, workspace_id, name, title, topic, created_by_session_id,
          created_by_agent_path, created_at, updated_at, overview_markdown
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
      `).run(
        topic.id, topic.workspaceId, topic.name, topic.title, topic.topic,
        topic.createdBySessionId, topic.createdByAgentPath, topic.createdAt, topic.updatedAt,
        topic.overviewMarkdown,
      );
    } catch (error) {
      if (/UNIQUE constraint failed/iu.test(error instanceof Error ? error.message : String(error))) {
        throw new Error(`Topic already exists in this workspace: ${name}`);
      }
      throw error;
    }
    this.writeSnapshot(topic.id);
    return topic;
  }

  public list(workspaceId: string, limit = 200, archived = false): ResearchTopicSummary[] {
    const rows = this.database.prepare(`
      SELECT topic.*,
        (SELECT COUNT(*) FROM app_server_topic_members AS member WHERE member.topic_id = topic.id) AS member_count,
        (SELECT COUNT(*) FROM app_server_topic_messages AS message WHERE message.topic_id = topic.id) AS message_count,
        (SELECT message.content_markdown FROM app_server_topic_messages AS message
          WHERE message.topic_id = topic.id ORDER BY message.created_at DESC, message.rowid DESC LIMIT 1) AS latest_message
      FROM app_server_topics AS topic
      WHERE topic.workspace_id = ? AND topic.archived_at IS ${archived ? "NOT " : ""}NULL
      ORDER BY topic.updated_at DESC, topic.id DESC
      LIMIT ?
    `).all(requiredText(workspaceId, "Workspace id"), boundedLimit(limit)) as Array<TopicRow & {
      member_count?: unknown;
      message_count?: unknown;
      latest_message?: unknown;
    }>;
    return rows.map((row) => ({
      ...decodeTopic(row),
      memberCount: numericCount(row.member_count),
      messageCount: numericCount(row.message_count),
      latestMessagePreview: typeof row.latest_message === "string" ? row.latest_message.slice(0, 240) : null,
    }));
  }

  public get(workspaceId: string, topic: string, messageLimit = 0): ResearchTopicDetail | null {
    const record = this.resolve(workspaceId, topic);
    if (!record) return null;
    const members = this.database.prepare(`
      SELECT * FROM app_server_topic_members
      WHERE topic_id = ? ORDER BY joined_at ASC, id ASC
    `).all(record.id).map((row) => decodeMember(row as Record<string, unknown>));
    const messages = this.database.prepare(`
      SELECT * FROM (
        SELECT *, rowid AS topic_sequence FROM app_server_topic_messages
        WHERE topic_id = ? ORDER BY created_at DESC, rowid DESC LIMIT ?
      ) ORDER BY created_at ASC, topic_sequence ASC
    `).all(record.id, boundedMessageLimit(messageLimit)).map((row) => decodeMessage(row as Record<string, unknown>));
    const sharedResources = this.database.prepare(`
      SELECT * FROM app_server_topic_shared_resources
      WHERE topic_id = ? ORDER BY updated_at DESC, id DESC
    `).all(record.id).map((row) => decodeSharedResource(row as Record<string, unknown>));
    const pages = this.database.prepare(`
      SELECT * FROM app_server_topic_pages WHERE topic_id = ? ORDER BY created_at ASC, id ASC
    `).all(record.id).map((row) => decodePage(row as Record<string, unknown>));
    const links = this.database.prepare(`
      SELECT * FROM app_server_topic_links WHERE topic_id = ? ORDER BY created_at DESC, id DESC
    `).all(record.id).map((row) => decodeLink(row as Record<string, unknown>));
    const mergedTopics = this.database.prepare(`
      SELECT * FROM app_server_topics WHERE workspace_id = ? AND merged_into_topic_id = ? ORDER BY updated_at DESC, id DESC
    `).all(workspaceId, record.id).map((row) => decodeTopic(row as TopicRow));
    return { topic: record, members, messages, sharedResources, pages, links, mergedTopics };
  }

  public join(input: JoinResearchTopicInput): ResearchTopicMemberRecord {
    const topic = this.require(input.workspaceId, input.topic);
    const joinedAt = input.joinedAt ?? new Date().toISOString();
    const sessionId = optionalText(input.sessionId) ?? "";
    const agentPath = requiredText(input.agentPath, "Agent path");
    const id = `topic_member_${randomUUID().replaceAll("-", "")}`;
    this.database.prepare(`
      INSERT INTO app_server_topic_members (
        id, topic_id, session_id, agent_id, agent_path, provider, model, role, status, joined_at, last_seen_at
      ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
      ON CONFLICT(topic_id, session_id, agent_path) DO UPDATE SET
        agent_id = excluded.agent_id,
        provider = excluded.provider,
        model = excluded.model,
        role = excluded.role,
        status = CASE
          WHEN excluded.status = 'unknown' THEN app_server_topic_members.status
          ELSE excluded.status
        END,
        last_seen_at = excluded.last_seen_at
    `).run(
      id, topic.id, sessionId, optionalText(input.agentId), agentPath,
      optionalText(input.provider), optionalText(input.model), optionalText(input.role) ?? "researcher",
      normalizeTopicMemberStatus(input.status), joinedAt, joinedAt,
    );
    const row = this.database.prepare(`
      SELECT * FROM app_server_topic_members
      WHERE topic_id = ? AND session_id IS ? AND agent_path = ?
    `).get(topic.id, sessionId, agentPath) as Record<string, unknown> | undefined;
    if (!row) throw new Error(`Topic member registration failed for ${agentPath}.`);
    return decodeMember(row);
  }

  public updateOverview(workspaceId: string, topic: string, contentMarkdown: string, expectedUpdatedAt?: string): ResearchTopicRecord {
    const record = this.require(workspaceId, topic);
    const content = boundedMarkdown(contentMarkdown);
    if (expectedUpdatedAt && expectedUpdatedAt !== record.updatedAt) throw new Error("Topic changed since it was opened. Refresh before editing.");
    const updatedAt = nextTimestamp(record.updatedAt);
    this.database.prepare("UPDATE app_server_topics SET overview_markdown = ?, updated_at = ? WHERE id = ? AND workspace_id = ?")
      .run(content, updatedAt, record.id, record.workspaceId);
    this.writeSnapshot(record.id);
    return { ...record, overviewMarkdown: content, updatedAt };
  }

  public savePage(workspaceId: string, topic: string, input: {
    id?: string;
    title: string;
    contentMarkdown: string;
    expectedUpdatedAt?: string;
  }): ResearchTopicPageRecord {
    const record = this.require(workspaceId, topic);
    const title = requiredText(input.title, "Page title").slice(0, 200);
    const contentMarkdown = boundedMarkdown(input.contentMarkdown);
    const now = new Date().toISOString();
    if (input.id) {
      const existing = this.database.prepare("SELECT * FROM app_server_topic_pages WHERE id = ? AND topic_id = ?")
        .get(input.id, record.id) as Record<string, unknown> | undefined;
      if (!existing) throw new Error("Topic page not found.");
      if (input.expectedUpdatedAt && input.expectedUpdatedAt !== existing.updated_at) throw new Error("Page changed since it was opened. Refresh before editing.");
      const updatedAt = nextTimestamp(storedText(existing.updated_at, "Page update timestamp"));
      this.database.prepare("UPDATE app_server_topic_pages SET title = ?, content_markdown = ?, updated_at = ? WHERE id = ? AND topic_id = ?")
        .run(title, contentMarkdown, updatedAt, input.id, record.id);
      this.touch(record.id, updatedAt);
      this.writeSnapshot(record.id);
      return { ...decodePage(existing), title, contentMarkdown, updatedAt };
    }
    const page: ResearchTopicPageRecord = {
      id: `topic_page_${randomUUID().replaceAll("-", "")}`,
      topicId: record.id, title, contentMarkdown, createdAt: now, updatedAt: now,
    };
    this.database.prepare(`
      INSERT INTO app_server_topic_pages (id, topic_id, title, content_markdown, created_at, updated_at)
      VALUES (?, ?, ?, ?, ?, ?)
    `).run(page.id, page.topicId, page.title, page.contentMarkdown, page.createdAt, page.updatedAt);
    this.touch(record.id, now);
    this.writeSnapshot(record.id);
    return page;
  }

  public deletePage(workspaceId: string, topic: string, pageId: string): void {
    const record = this.require(workspaceId, topic);
    const result = this.database.prepare("DELETE FROM app_server_topic_pages WHERE id = ? AND topic_id = ?")
      .run(pageId, record.id);
    if (Number(result.changes) !== 1) throw new Error("Topic page not found.");
    this.touch(record.id);
    this.writeSnapshot(record.id);
  }

  public link(workspaceId: string, topic: string, input: {
    kind: ResearchTopicLinkKind;
    resourceId: string;
    title: string;
  }): ResearchTopicLinkRecord {
    const record = this.require(workspaceId, topic);
    const kind = normalizeLinkKind(input.kind);
    let resourceId = requiredText(input.resourceId, "Linked resource id");
    const title = requiredText(input.title, "Linked resource title").slice(0, 200);
    if (kind === "topic") {
      const linkedTopic = this.resolve(workspaceId, resourceId);
      if (!linkedTopic) throw new Error("Linked topic must belong to this workspace.");
      resourceId = linkedTopic.id;
      if (resourceId === record.id) throw new Error("A topic cannot link to itself.");
    } else if (kind === "file") {
      const problem = workspacePathProblem(resourceId);
      if (problem) throw new Error(`Invalid linked workspace path: ${problem}.`);
      if (this.workspaceRoot) {
        const path = resolve(this.workspaceRoot, resourceId);
        assertWorkspaceChild(this.workspaceRoot, path);
        if (!existsSync(path)) throw new Error("Linked workspace file does not exist.");
      }
    } else {
      const table = {
        claim: "app_server_research_claims",
        memory: "memory_nodes",
        runbook: "app_server_runbooks",
        session: "app_server_sessions",
      }[kind];
      if (this.database.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?").get(table)
        && !this.database.prepare(`SELECT 1 FROM ${table} WHERE id = ? AND workspace_id = ?`).get(resourceId, workspaceId)) {
        throw new Error(`Linked ${kind} must belong to this workspace.`);
      }
    }
    const link: ResearchTopicLinkRecord = {
      id: `topic_link_${randomUUID().replaceAll("-", "")}`,
      topicId: record.id, kind, resourceId, title, createdAt: new Date().toISOString(),
    };
    this.database.prepare(`
      INSERT INTO app_server_topic_links (id, topic_id, resource_kind, resource_id, title, created_at)
      VALUES (?, ?, ?, ?, ?, ?)
      ON CONFLICT(topic_id, resource_kind, resource_id) DO UPDATE SET title = excluded.title
    `).run(link.id, link.topicId, link.kind, link.resourceId, link.title, link.createdAt);
    this.touch(record.id);
    this.writeSnapshot(record.id);
    const stored = this.database.prepare("SELECT * FROM app_server_topic_links WHERE topic_id = ? AND resource_kind = ? AND resource_id = ?")
      .get(record.id, kind, resourceId) as Record<string, unknown>;
    return decodeLink(stored);
  }

  public unlink(workspaceId: string, topic: string, linkId: string): void {
    const record = this.require(workspaceId, topic);
    const result = this.database.prepare("DELETE FROM app_server_topic_links WHERE id = ? AND topic_id = ?")
      .run(linkId, record.id);
    if (Number(result.changes) !== 1) throw new Error("Topic link not found.");
    this.touch(record.id);
    this.writeSnapshot(record.id);
  }

  public merge(workspaceId: string, sourceId: string, targetId: string): { source: ResearchTopicRecord; target: ResearchTopicRecord } {
    const source = this.require(workspaceId, sourceId);
    const target = this.require(workspaceId, targetId);
    if (source.id === target.id) throw new Error("A topic cannot be merged into itself.");
    if (source.archivedAt || source.mergedIntoTopicId) throw new Error("Only an active topic can be merged.");
    if (this.database.prepare("SELECT 1 FROM app_server_topics WHERE merged_into_topic_id = ? LIMIT 1").get(source.id)) {
      throw new Error("Undo dependent topic merges before merging their target.");
    }
    if (target.archivedAt || target.mergedIntoTopicId) throw new Error("Merge target must be active.");
    const now = nextTimestamp(source.updatedAt);
    this.database.exec("BEGIN IMMEDIATE;");
    try {
      const targetState = this.database.prepare("SELECT archived_at, merged_into_topic_id FROM app_server_topics WHERE id = ? AND workspace_id = ?")
        .get(target.id, workspaceId) as { archived_at: string | null; merged_into_topic_id: string | null } | undefined;
      if (!targetState || targetState.archived_at || targetState.merged_into_topic_id) throw new Error("Merge target changed. Refresh before merging.");
      const result = this.database.prepare(`UPDATE app_server_topics
        SET merged_into_topic_id = ?, archived_at = ?, updated_at = ?
        WHERE id = ? AND workspace_id = ? AND merged_into_topic_id IS NULL AND archived_at IS NULL`)
        .run(target.id, now, now, source.id, workspaceId);
      if (Number(result.changes) !== 1) throw new Error("Source topic changed. Refresh before merging.");
      this.touch(target.id, now);
      this.database.exec("COMMIT;");
    } catch (error) {
      this.database.exec("ROLLBACK;");
      throw error;
    }
    this.writeSnapshot(source.id);
    this.writeSnapshot(target.id);
    return { source: this.require(workspaceId, source.id), target: this.require(workspaceId, target.id) };
  }

  public unmerge(workspaceId: string, sourceId: string): ResearchTopicRecord {
    const source = this.require(workspaceId, sourceId);
    if (!source.mergedIntoTopicId) throw new Error("Topic is not merged.");
    const targetId = source.mergedIntoTopicId;
    this.database.prepare(`UPDATE app_server_topics
      SET merged_into_topic_id = NULL, archived_at = NULL, updated_at = ?
      WHERE id = ? AND workspace_id = ?`).run(nextTimestamp(source.updatedAt), source.id, workspaceId);
    this.touch(targetId);
    this.writeSnapshot(source.id);
    this.writeSnapshot(targetId);
    return this.require(workspaceId, source.id);
  }

  public search(workspaceId: string, query: string, limit = 50): ResearchTopicSummary[] {
    const words = requiredText(query, "Search query").toLocaleLowerCase().slice(0, 512).split(/\s+/u).slice(0, 12);
    const matchingWord = `(
      instr(lower(topic.name || ' ' || topic.title || ' ' || topic.topic || ' ' || topic.overview_markdown), ?) > 0
      OR EXISTS (SELECT 1 FROM app_server_topic_pages AS page WHERE page.topic_id = topic.id
        AND instr(lower(page.title || ' ' || page.content_markdown), ?) > 0)
      OR EXISTS (SELECT 1 FROM app_server_topic_links AS link WHERE link.topic_id = topic.id
        AND instr(lower(link.title), ?) > 0)
      OR EXISTS (SELECT 1 FROM app_server_topics AS alias WHERE alias.merged_into_topic_id = topic.id
        AND instr(lower(alias.name || ' ' || alias.title || ' ' || alias.topic), ?) > 0)
    )`;
    const rows = this.database.prepare(`
      SELECT topic.*,
        (SELECT COUNT(*) FROM app_server_topic_members AS member WHERE member.topic_id = topic.id) AS member_count,
        (SELECT COUNT(*) FROM app_server_topic_messages AS message WHERE message.topic_id = topic.id) AS message_count,
        (SELECT message.content_markdown FROM app_server_topic_messages AS message
          WHERE message.topic_id = topic.id ORDER BY message.created_at DESC, message.rowid DESC LIMIT 1) AS latest_message
      FROM app_server_topics AS topic
      WHERE topic.workspace_id = ? AND topic.archived_at IS NULL
        AND ${words.map(() => matchingWord).join(" AND ")}
      ORDER BY topic.updated_at DESC, topic.id DESC
      LIMIT ?
    `).all(requiredText(workspaceId, "Workspace id"), ...words.flatMap((word) => [word, word, word, word]), boundedLimit(limit)) as Array<TopicRow & {
      member_count?: unknown;
      message_count?: unknown;
      latest_message?: unknown;
    }>;
    return rows.map((row) => ({
      ...decodeTopic(row),
      memberCount: numericCount(row.member_count),
      messageCount: numericCount(row.message_count),
      latestMessagePreview: typeof row.latest_message === "string" ? row.latest_message.slice(0, 240) : null,
    }));
  }

  private touch(topicId: string, updatedAt = new Date().toISOString()): void {
    const row = this.database.prepare("SELECT updated_at FROM app_server_topics WHERE id = ?").get(topicId) as { updated_at: string } | undefined;
    if (!row) throw new Error("Topic not found.");
    this.database.prepare("UPDATE app_server_topics SET updated_at = ? WHERE id = ?")
      .run(nextTimestamp(row.updated_at, updatedAt), topicId);
  }

  private snapshotPath(topicId: string): string {
    if (!/^[a-zA-Z0-9][a-zA-Z0-9_.-]*$/u.test(topicId)) throw new Error("Topic ID is not a safe workspace filename.");
    return `references/topics/${topicId}.json`;
  }

  private writeSnapshot(topicId: string): void {
    if (!this.workspaceRoot) return;
    const project = readWorkspaceProject(this.workspaceRoot);
    if (!project) throw new Error("Research workspace is unavailable.");
    const detail = this.get(project.workspaceId, topicId);
    if (!detail) throw new Error("Topic snapshot source is unavailable.");
    atomicWorkspaceWrite(this.workspaceRoot, this.snapshotPath(topicId), topicSnapshotContent(project.workspaceId, detail));
  }

  private hydrateWorkspaceFiles(): void {
    const root = this.workspaceRoot!;
    const project = readWorkspaceProject(root);
    if (!project) throw new Error("Research workspace is unavailable.");
    const directory = join(root, "references", "topics");
    mkdirSync(directory, { recursive: true });
    const merges: Array<{ sourceId: string; targetId: string }> = [];
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      if (!entry.isFile() || !entry.name.endsWith(".json")) continue;
      const topicId = entry.name.slice(0, -5);
      const path = this.snapshotPath(topicId);
      const absolute = join(root, path);
      assertWorkspaceChild(root, absolute);
      const content = readFileSync(absolute, "utf8");
      const published = isPublishedWorkspacePath(root, path);
      const stored = this.get(project.workspaceId, topicId);
      if ((published && readPublishedWorkspaceFile(root, path) !== content)
        || !published) {
        if (!stored || stored.topic.id !== topicId || topicSnapshotContent(project.workspaceId, stored) !== content) {
          throw new Error(`${path}: use typed research operations for topic snapshots.`);
        }
      }
      const parsed = JSON.parse(content) as Record<string, unknown>;
      const topic = parsed.topic as Partial<ResearchTopicRecord> | undefined;
      if (parsed.schemaVersion !== 1 || parsed.workspaceId !== project.workspaceId
        || topic?.id !== topicId || topic.workspaceId !== project.workspaceId
        || typeof topic.name !== "string" || typeof topic.title !== "string"
        || typeof topic.topic !== "string" || typeof topic.overviewMarkdown !== "string"
        || !Array.isArray(parsed.pages) || !Array.isArray(parsed.links)) {
        throw new Error(`${path}: invalid topic snapshot.`);
      }
      if (topic.mergedIntoTopicId != null && (typeof topic.mergedIntoTopicId !== "string" || topic.mergedIntoTopicId === topicId)) {
        throw new Error(`${path}: invalid merge target.`);
      }
      if (typeof topic.mergedIntoTopicId === "string") {
        merges.push({ sourceId: topicId, targetId: topic.mergedIntoTopicId });
      }
      this.database.exec("BEGIN IMMEDIATE;");
      try {
        this.database.prepare(`
          INSERT INTO app_server_topics
            (id, workspace_id, name, title, topic, created_by_session_id,
             created_by_agent_path, created_at, updated_at, archived_at, overview_markdown, merged_into_topic_id)
          VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
          ON CONFLICT(id) DO UPDATE SET
            name = excluded.name, title = excluded.title, topic = excluded.topic,
            updated_at = excluded.updated_at, archived_at = excluded.archived_at,
            overview_markdown = excluded.overview_markdown,
            merged_into_topic_id = excluded.merged_into_topic_id
        `).run(topicId, project.workspaceId, topic.name, topic.title, topic.topic,
          topic.createdBySessionId ?? null, topic.createdByAgentPath ?? "/human",
          topic.createdAt ?? new Date().toISOString(), topic.updatedAt ?? new Date().toISOString(),
          topic.archivedAt ?? null, topic.overviewMarkdown, null);
        this.database.prepare("DELETE FROM app_server_topic_pages WHERE topic_id = ?").run(topicId);
        for (const value of parsed.pages) {
          const page = value as Partial<ResearchTopicPageRecord>;
          if (page.topicId !== topicId || typeof page.id !== "string" || typeof page.title !== "string"
            || typeof page.contentMarkdown !== "string" || typeof page.createdAt !== "string" || typeof page.updatedAt !== "string") {
            throw new Error(`${path}: invalid topic page.`);
          }
          this.database.prepare(`
            INSERT INTO app_server_topic_pages (id, topic_id, title, content_markdown, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?)
          `).run(page.id, topicId, page.title, page.contentMarkdown, page.createdAt, page.updatedAt);
        }
        this.database.prepare("DELETE FROM app_server_topic_links WHERE topic_id = ?").run(topicId);
        for (const value of parsed.links) {
          const link = value as Partial<ResearchTopicLinkRecord>;
          if (link.topicId !== topicId || typeof link.id !== "string" || typeof link.resourceId !== "string"
            || typeof link.title !== "string" || typeof link.createdAt !== "string") throw new Error(`${path}: invalid topic link.`);
          const kind = normalizeLinkKind(link.kind);
          this.database.prepare(`
            INSERT INTO app_server_topic_links (id, topic_id, resource_kind, resource_id, title, created_at)
            VALUES (?, ?, ?, ?, ?, ?)
          `).run(link.id, topicId, kind, link.resourceId, link.title, link.createdAt);
        }
        this.database.exec("COMMIT;");
      } catch (error) {
        this.database.exec("ROLLBACK;");
        throw error;
      }
    }
    for (const merge of merges) {
      const result = this.database.prepare(`UPDATE app_server_topics SET merged_into_topic_id = ?
        WHERE id = ? AND workspace_id = ?`).run(merge.targetId, merge.sourceId, project.workspaceId);
      if (Number(result.changes) !== 1) throw new Error(`Merged topic snapshot is missing: ${merge.sourceId}`);
    }
    if (this.database.prepare(`
      SELECT 1 FROM app_server_topics AS source
      JOIN app_server_topics AS target ON target.id = source.merged_into_topic_id
      WHERE source.workspace_id = ? AND
        (target.workspace_id <> source.workspace_id OR target.merged_into_topic_id IS NOT NULL OR target.archived_at IS NOT NULL)
      LIMIT 1
    `).get(project.workspaceId)) throw new Error("Topic merge snapshots contain an invalid target.");
    const rows = this.database.prepare("SELECT id FROM app_server_topics WHERE workspace_id = ?")
      .all(project.workspaceId) as Array<{ id: string }>;
    for (const row of rows) {
      const path = this.snapshotPath(row.id);
      if (existsSync(join(root, path))) continue;
      if (isPublishedWorkspacePath(root, path)) throw new Error(`${path}: canonical topic snapshot is missing.`);
      this.writeSnapshot(row.id);
    }
  }

  public delete(workspaceId: string, topic: string): { topicId: string; deleted: true } {
    const record = this.require(workspaceId, topic);
    if (this.database.prepare("SELECT 1 FROM app_server_topics WHERE merged_into_topic_id = ? LIMIT 1").get(record.id)) {
      throw new Error("Restore or remerge source topics before deleting this merge target.");
    }
    const result = this.database.prepare("DELETE FROM app_server_topics WHERE id = ? AND workspace_id = ?")
      .run(record.id, record.workspaceId);
    if (Number(result.changes) !== 1) throw new Error(`Topic could not be deleted: ${topic}`);
    if (this.workspaceRoot) {
      const path = join(this.workspaceRoot, this.snapshotPath(record.id));
      assertWorkspaceChild(this.workspaceRoot, path);
      rmSync(path, { force: true });
    }
    return { topicId: record.id, deleted: true };
  }

  public archive(workspaceId: string, topic: string, archivedAt = new Date().toISOString()): ResearchTopicRecord {
    const record = this.require(workspaceId, topic);
    if (this.database.prepare("SELECT 1 FROM app_server_topics WHERE merged_into_topic_id = ? LIMIT 1").get(record.id)) {
      throw new Error("Undo dependent topic merges before archiving their target.");
    }
    this.database.prepare("UPDATE app_server_topics SET archived_at = ? WHERE id = ? AND workspace_id = ?")
      .run(archivedAt, record.id, record.workspaceId);
    this.writeSnapshot(record.id);
    return { ...record, archivedAt };
  }

  public restore(workspaceId: string, topic: string): ResearchTopicRecord {
    const record = this.require(workspaceId, topic);
    if (record.mergedIntoTopicId) throw new Error("Undo the merge to restore this topic.");
    this.database.prepare("UPDATE app_server_topics SET archived_at = NULL WHERE id = ? AND workspace_id = ?")
      .run(record.id, record.workspaceId);
    this.writeSnapshot(record.id);
    return { ...record, archivedAt: null };
  }

  private resolve(workspaceId: string, topic: string): ResearchTopicRecord | null {
    const normalizedWorkspaceId = requiredText(workspaceId, "Workspace id");
    const identifier = requiredText(topic, "Topic");
    const normalizedName = researchTopicNameSlug(identifier);
    const row = this.database.prepare(`
      SELECT * FROM app_server_topics
      WHERE workspace_id = ? AND (id = ? OR name = ?)
      LIMIT 1
    `).get(normalizedWorkspaceId, identifier, normalizedName) as TopicRow | undefined;
    if (!row) return null;
    const record = decodeTopic(row);
    return record.mergedIntoTopicId && identifier !== record.id
      ? this.resolve(normalizedWorkspaceId, record.mergedIntoTopicId)
      : record;
  }

  private require(workspaceId: string, topic: string): ResearchTopicRecord {
    const record = this.resolve(workspaceId, topic);
    if (!record) throw new Error(`Topic not found in workspace: ${topic}`);
    return record;
  }
}

export function normalizeResearchTopicName(value: string): string {
  const words = researchTopicNameWords(requiredText(value, "Topic name"));
  if (words.length > MAX_RESEARCH_TOPIC_NAME_WORDS) {
    throw new Error(`Topic name must contain at most ${MAX_RESEARCH_TOPIC_NAME_WORDS} words.`);
  }
  const normalized = words.join("-");
  if (!normalized) throw new Error("Topic name must contain a letter or number.");
  if (normalized.length > 64) throw new Error("Topic name must contain at most 64 characters.");
  return normalized;
}

function researchTopicNameSlug(value: string): string {
  return researchTopicNameWords(value).join("-").slice(0, 64);
}

function researchTopicNameWords(value: string): string[] {
  return value.toLocaleLowerCase().match(/[a-z0-9]+/gu) ?? [];
}

export function researchTopicTitle(name: string): string {
  return normalizeResearchTopicName(name)
    .split("-")
    .map((part) => part ? `${part[0]!.toLocaleUpperCase()}${part.slice(1)}` : part)
    .join(" ");
}

function decodeTopic(row: TopicRow): ResearchTopicRecord {
  return {
    id: storedText(row.id, "Topic id"),
    workspaceId: storedText(row.workspace_id, "Topic workspace id"),
    name: storedText(row.name, "Topic name"),
    title: storedText(row.title, "Topic title"),
    topic: storedText(row.topic, "Topic topic"),
    overviewMarkdown: storedText(row.overview_markdown, "Topic overview"),
    createdBySessionId: optionalText(row.created_by_session_id),
    createdByAgentPath: storedText(row.created_by_agent_path, "Topic creator"),
    createdAt: storedText(row.created_at, "Topic creation timestamp"),
    updatedAt: storedText(row.updated_at, "Topic update timestamp"),
    archivedAt: optionalText(row.archived_at),
    mergedIntoTopicId: optionalText(row.merged_into_topic_id),
  };
}

function decodePage(row: Record<string, unknown>): ResearchTopicPageRecord {
  return {
    id: storedText(row.id, "Topic page id"),
    topicId: storedText(row.topic_id, "Topic page topic id"),
    title: storedText(row.title, "Topic page title"),
    contentMarkdown: storedText(row.content_markdown, "Topic page content"),
    createdAt: storedText(row.created_at, "Topic page creation timestamp"),
    updatedAt: storedText(row.updated_at, "Topic page update timestamp"),
  };
}

function decodeLink(row: Record<string, unknown>): ResearchTopicLinkRecord {
  return {
    id: storedText(row.id, "Topic link id"),
    topicId: storedText(row.topic_id, "Topic link topic id"),
    kind: normalizeLinkKind(row.resource_kind),
    resourceId: storedText(row.resource_id, "Topic link resource id"),
    title: storedText(row.title, "Topic link title"),
    createdAt: storedText(row.created_at, "Topic link creation timestamp"),
  };
}

function decodeMember(row: Record<string, unknown>): ResearchTopicMemberRecord {
  return {
    id: storedText(row.id, "Topic member id"),
    topicId: storedText(row.topic_id, "Topic member topic id"),
    sessionId: optionalText(row.session_id),
    agentId: optionalText(row.agent_id),
    agentPath: storedText(row.agent_path, "Topic member agent path"),
    provider: optionalText(row.provider),
    model: optionalText(row.model),
    role: storedText(row.role, "Topic member role"),
    status: normalizeTopicMemberStatus(row.status),
    joinedAt: storedText(row.joined_at, "Topic member join timestamp"),
    lastSeenAt: storedText(row.last_seen_at, "Topic member activity timestamp"),
  };
}

function decodeMessage(row: Record<string, unknown>): ResearchTopicMessageRecord {
  return {
    id: storedText(row.id, "Topic message id"),
    topicId: storedText(row.topic_id, "Topic message topic id"),
    sessionId: optionalText(row.session_id),
    attemptId: optionalText(row.attempt_id),
    memberId: optionalText(row.member_id),
    senderAgentPath: storedText(row.sender_agent_path, "Topic message sender"),
    kind: normalizeMessageKind(storedText(row.kind, "Topic message kind")),
    contentMarkdown: storedText(row.content_markdown, "Topic message content"),
    evidenceRefs: parsedStringArray(row.evidence_refs_json, "Topic evidence references"),
    metadata: parsedRecord(row.metadata_json, "Topic message metadata"),
    createdAt: storedText(row.created_at, "Topic message timestamp"),
  };
}

function decodeSharedResource(row: Record<string, unknown>): ResearchTopicSharedResourceRecord {
  return {
    id: storedText(row.id, "Shared topic resource id"),
    topicId: storedText(row.topic_id, "Shared topic resource topic id"),
    sessionId: optionalText(row.session_id),
    memberId: optionalText(row.member_id),
    messageId: storedText(row.message_id, "Shared topic resource message id"),
    senderAgentPath: storedText(row.sender_agent_path, "Shared topic resource sender"),
    kind: normalizeSharedResourceKind(row.resource_kind),
    resourceId: storedText(row.resource_id, "Shared topic resource locator"),
    title: storedText(row.title, "Shared topic resource title"),
    createdAt: storedText(row.created_at, "Shared topic resource creation timestamp"),
    updatedAt: storedText(row.updated_at, "Shared topic resource update timestamp"),
  };
}

function normalizeMessageKind(value: unknown): ResearchTopicMessageKind {
  if (value === undefined) return "message";
  if (value === "message" || value === "evidence" || value === "decision" || value === "system") return value;
  throw new Error(`Unsupported topic message kind: ${String(value)}`);
}

function normalizeSharedResourceKind(value: unknown): ResearchTopicSharedResourceKind {
  if (value === "file" || value === "runbook" || value === "memory") return value;
  throw new Error(`Unsupported shared topic resource kind: ${String(value)}`);
}

function normalizeLinkKind(value: unknown): ResearchTopicLinkKind {
  if (value === "claim" || value === "memory" || value === "runbook" || value === "file" || value === "session" || value === "topic") return value;
  throw new Error(`Unsupported topic link kind: ${String(value)}`);
}

function boundedMarkdown(value: unknown): string {
  if (typeof value !== "string" || value.length > 128_000) throw new Error("Topic page content must be text under 128,000 characters.");
  return value;
}

function normalizeTopicMemberStatus(value: unknown): ResearchTopicMemberStatus {
  if (
    value === "pending"
    || value === "running"
    || value === "completed"
    || value === "interrupted"
    || value === "errored"
    || value === "unknown"
  ) return value;
  if (value === undefined || value === null) return "unknown";
  throw new Error(`Unsupported topic member status: ${String(value)}`);
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
  return Math.max(0, Math.min(2_000, Math.trunc(value)));
}

function numericCount(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? Math.max(0, Math.trunc(value)) : 0;
}

function nextTimestamp(previous: string, candidate = new Date().toISOString()): string {
  return candidate > previous ? candidate : new Date(Date.parse(previous) + 1).toISOString();
}
