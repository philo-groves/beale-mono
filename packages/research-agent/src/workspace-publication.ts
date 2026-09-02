import { existsSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import type { DatabaseSync, SQLInputValue } from "node:sqlite";
import { openResearchDatabase } from "./database.js";
import { createResearchStorageLayout, loadResearchStorageManifest } from "./storage.js";
import { assertWorkspaceChild, atomicWorkspaceWrite, checkpointWorkspace, publishWorkspaceFiles, readWorkspaceProject, recoverWorkspacePublication, retainWorkspaceArtifact, workspaceFileHash, workspaceContentHash, type WorkspaceCheckpointResult, type WorkspaceCommitContext } from "./workspace-project.js";

type Row = Record<string, unknown>;
export interface WorkspacePublicationOptions {
  workspaceRoot: string;
  workspaceId: string;
  databasePath: string;
  artifactDirectoryPath: string;
  sessionId?: string;
  investigationId?: string;
}

const json = (value: unknown): string => JSON.stringify(value, null, 2) + "\n";
function identifier(value: unknown): string {
  if (typeof value !== "string" || !/^[a-zA-Z0-9][a-zA-Z0-9_.-]*$/u.test(value)) throw new Error("Research export requires a stable filesystem-safe record ID.");
  return value;
}

/** Reads one SQLite snapshot, scoped to one workspace. Database files and session launch credentials are never exported. */
export function publishWorkspaceResearch(options: WorkspacePublicationOptions): WorkspaceCommitContext | undefined {
  const root = options.workspaceRoot;
  const project = readWorkspaceProject(root);
  if (!project) return;
  if (project.workspaceId !== options.workspaceId) throw new Error("Canonical publication workspace identity mismatch.");
  recoverWorkspacePublication(root);
  const database = openResearchDatabase(options.databasePath, { readOnly: true });
  const files: Record<string, string> = {};
  const pins: Record<string, string> = {};
  const rawFiles: Record<string, string> = {};
  const attribution: WorkspaceCommitContext = {
    ...(options.sessionId ? { sessionId: options.sessionId } : {}),
    ...(options.investigationId ? { investigationId: options.investigationId } : {}),
  };
  const referencedArtifacts = new Set<string>();
  const artifactPaths = new Map<string, string>();
  const layout = createResearchStorageLayout({ workspaceRoot: root, databasePath: options.databasePath, artifactDirectoryPath: options.artifactDirectoryPath });
  const manifest = loadResearchStorageManifest(layout);
  const artifactById = new Map(manifest.artifacts.map((entry) => [entry.id, entry]));
  const artifactByPath = new Map(manifest.artifacts.map((entry) => [resolve(entry.path), entry]));
  function retainArtifact(id: unknown): void {
    if (typeof id === "string" && artifactById.has(id)) referencedArtifacts.add(id);
  }
  function retainPath(path: unknown, base: unknown): void {
    if (typeof path !== "string") return;
    const candidate = resolve(base === "workspace" ? root : options.artifactDirectoryPath, path);
    const artifact = artifactByPath.get(candidate);
    if (artifact) retainArtifact(artifact.id);
    else if (base === "workspace" && existsSync(candidate)) {
      assertWorkspaceChild(root, candidate);
      const hash = workspaceFileHash(candidate);
      const exported = `evidence/raw/${hash}`;
      const sizeBytes = retainWorkspaceArtifact(root, exported, candidate, hash);
      rawFiles[exported] = hash;
      files[`evidence/${hash}.json`] = json({ schemaVersion: 1, contentHash: hash, sizeBytes, path: exported });
      pins[`evidence/${hash}.json`] = workspaceContentHash(files[`evidence/${hash}.json`]!);
      artifactPaths.set(candidate, exported);
    }
  }
  try {
    database.exec("BEGIN");
    const has = (table: string) => Boolean(database.prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?").get(table));
    const rows = (table: string, where: string, parameters: SQLInputValue[] = [options.workspaceId]): Row[] => has(table)
      ? database.prepare(`SELECT * FROM ${table} WHERE ${where}`).all(...parameters) as Row[] : [];
    const owned = (table: string) => rows(table, "workspace_id = ?");
    if (!attribution.investigationId && options.sessionId && has('campaign_tracks') && has('campaign_track_sessions')) {
      const track = database.prepare(`SELECT t.id FROM campaign_tracks t JOIN campaign_track_sessions s ON s.investigation_id = t.id
        WHERE t.workspace_id = ? AND s.session_id = ? ORDER BY s.linked_at DESC, t.id LIMIT 1`).get(options.workspaceId, options.sessionId) as { id: string } | undefined;
      if (track) attribution.investigationId = track.id;
    }
    if (has('scope_versions')) files['references/scope.json'] = json({ schemaVersion: 1, workspaceId: options.workspaceId,
      scopes: owned('scope_versions').map((scope) => ({ ...scope, assets: rows('scope_assets', 'scope_version_id = ?', [String(scope.id)]).filter((asset) => asset.kind !== 'credential_ref') })),
      rules: owned('workspace_rules'), subject: owned('workspace_research_subjects'),
    });
    for (const claim of owned("app_server_research_claims")) {
      const id = identifier(claim.id);
      const evidence = rows("app_server_claim_evidence", "claim_id = ?", [id]);
      for (const entry of evidence) retainArtifact(entry.reference_id);
      files[`claims/${id}.json`] = json({ schemaVersion: 1, ...claim, evidence,
        transitions: rows("app_server_claim_transitions", "claim_id = ?", [id]),
        authorship: rows("app_server_claim_authorship", "claim_id = ?", [id]),
        components: rows("app_server_claim_components", "claim_id = ?", [id]),
      });
    }
    const memories = has("memory_node_workspaces")
      ? rows("memory_nodes", "id IN (SELECT node_id FROM memory_node_workspaces WHERE workspace_id = ?)")
      : owned("memory_nodes");
    const memoryIds = new Set(memories.map((row) => row.id));
    for (const memory of memories) {
      const id = identifier(memory.id);
      const evidence = rows("memory_evidence_refs", "node_id = ?", [id]);
      for (const entry of evidence) retainPath(entry.path, entry.path_base);
      const { body, ...metadata } = memory;
      files[`memories/${id}.md`] = `<!-- Beale canonical memory; edit through memory tools or validated import. -->\n\n\`\`\`json\n${json({ schemaVersion: 1, ...metadata, workspace_id: options.workspaceId,
        tags: rows("memory_node_tags", "node_id = ?", [id]),
        assets: rows("memory_node_assets", "node_id = ?", [id]), evidence,
        edges: rows("memory_edges", "from_id = ?", [id]).filter((edge) => memoryIds.has(edge.to_id)),
      })}\`\`\`\n\n${String(body ?? "")}\n`;
    }
    for (const [table, category] of [["app_server_runbooks", "runbooks"], ["app_server_reports", "reports"]] as const) {
      for (const record of owned(table)) {
        const id = identifier(record.id);
        const path = resolve(options.artifactDirectoryPath, String(record.relative_path));
        assertWorkspaceChild(options.artifactDirectoryPath, path);
        const content = readFileSync(path, "utf8");
        if (String(record.content_hash).replace(/^sha256:/u, "") !== workspaceContentHash(content)) throw new Error(`${category}/${id}: artifact changed during publication; retry after the canonical write completes.`);
        const extension = category === "runbooks" ? "ipynb" : "md";
        if (category === 'runbooks') {
          const notebook = JSON.parse(content) as { cells?: Array<{ metadata?: { beale?: { executor?: { artifactId?: string; workspacePath?: string } } } }> };
          for (const cell of notebook.cells ?? []) {
            const executor = cell.metadata?.beale?.executor;
            retainArtifact(executor?.artifactId);
            retainPath(executor?.workspacePath, 'workspace');
          }
        }
        files[`${category}/${id}/${category === "runbooks" ? "runbook" : "report"}.${extension}`] = content;
        files[`${category}/${id}/record.json`] = json({ schemaVersion: 1, ...record });
        retainArtifact(record.submission_packet_artifact_id);
        retainArtifact(record.recording_artifact_id);
        artifactPaths.set(path, `${category}/${id}/${category === "runbooks" ? "runbook" : "report"}.${extension}`);
      }
    }
    for (const track of owned("campaign_tracks")) files[`investigations/${identifier(track.id)}/record.json`] = json({ schemaVersion: 1, ...track });
    const executions = owned("app_server_runbook_executions");
    for (const execution of executions) {
      const cells = rows("app_server_runbook_cell_executions", "run_id = ?", [String(execution.run_id)]);
      files[`evidence/execution-${identifier(execution.run_id)}.json`] = json({ schemaVersion: 1, ...execution, cells });
      if (execution.completed_at) pins[`evidence/execution-${identifier(execution.run_id)}.json`] = workspaceContentHash(files[`evidence/execution-${identifier(execution.run_id)}.json`]!);
    }
    for (const session of owned("app_server_sessions")) {
      const id = identifier(session.id);
      files[`traces/${id}/summary.md`] = `# ${String(session.title)}\n\nSession: ${id}\nStatus: ${String(session.status)}\n\n${String(session.summary)}\n`;
      if (options.sessionId === id && has("app_server_session_events")) {
        // Bounded pages avoid loading an entire long-running transcript into memory.
        exportTrace(database, root, id);
      }
    }
    database.exec("COMMIT");
    for (const id of referencedArtifacts) {
      const entry = artifactById.get(id)!;
      const hash = entry.contentHash.replace(/^sha256:/u, "");
      const path = `evidence/raw/${hash}`;
      const sizeBytes = retainWorkspaceArtifact(root, path, entry.path, hash);
      rawFiles[path] = hash;
      files[`evidence/${identifier(id)}.json`] = json({ schemaVersion: 1, id, kind: entry.kind, purpose: entry.purpose, path, contentHash: hash, sizeBytes, sourceEventIds: entry.sourceEventIds });
      pins[`evidence/${identifier(id)}.json`] = workspaceContentHash(files[`evidence/${identifier(id)}.json`]!);
      artifactPaths.set(resolve(entry.path), path);
    }
    // Exact known storage paths become portable references; never publish database or credential material.
    for (const [path, content] of Object.entries(files)) {
      if (path.endsWith('.ipynb') || path.endsWith('.md')) continue;
      let portable = content;
      for (const [absolute, exported] of artifactPaths) {
        portable = portable.split(absolute).join(exported).split(absolute.replace(/\\/gu, "\\\\")).join(exported);
      }
      portable = portable.split(root).join("workspace:").split(root.replace(/\\/gu, "\\\\")).join("workspace:");
      files[path] = portable;
      if (path in pins) pins[path] = workspaceContentHash(portable);
    }
    publishWorkspaceFiles(root, files, pins, rawFiles);
    return attribution;
  } catch (error) {
    try { database.exec("ROLLBACK"); } catch { /* Read snapshot already ended. */ }
    throw error;
  } finally { database.close(); }
}

function exportTrace(database: DatabaseSync, root: string, sessionId: string): void {
  // Shards are immutable exports at a bounded size; they are excluded from Git.
  let offset = 0;
  while (true) {
    const page = database.prepare("SELECT event_offset, event_json FROM app_server_session_events WHERE session_id=? AND event_offset>=? ORDER BY event_offset LIMIT 250").all(sessionId, offset) as Array<{ event_offset: number; event_json: string }>;
    if (!page.length) break;
    atomicWorkspaceWrite(root, `traces/${sessionId}/events-${offset}.jsonl`, page.map((row) => row.event_json).join("\n") + "\n");
    offset = page[page.length - 1]!.event_offset + 1;
  }
}

export function checkpointWorkspaceResearch(options: WorkspacePublicationOptions, reason: string): WorkspaceCheckpointResult {
  const context: WorkspaceCommitContext = {};
  return checkpointWorkspace(options.workspaceRoot, reason, () => { Object.assign(context, publishWorkspaceResearch(options)); }, context);
}
