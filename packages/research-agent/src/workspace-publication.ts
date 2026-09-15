import { existsSync, readFileSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import type { DatabaseSync, SQLInputValue } from "node:sqlite";
import { openResearchDatabase } from "./database.js";
import { createResearchStorageLayout, loadResearchStorageManifest } from "./storage.js";
import { assertWorkspaceChild, atomicWorkspaceWrite, checkpointWorkspace, publishWorkspaceFiles, readWorkspaceProject, recoverWorkspacePublication, retainWorkspaceArtifact, workspaceFileHash, workspaceContentHash, type WorkspaceCheckpointResult, type WorkspaceCommitContext } from "./workspace-project.js";
import { markWorkspaceResearchIndexReady } from "./workspace-research-index.js";

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
const PRIOR_ART_PAYLOAD_PATH = /^references\/prior-art\/[a-f0-9]{64}\.json$/u;
function recordIdentifier(value: unknown): string {
  if (typeof value !== "string" || value.length === 0) throw new Error("Research export requires a stable non-empty record ID.");
  return value;
}

function publishPriorArtRows(rows: readonly Row[], files: Record<string, string>): Row[] {
  return rows.map((row) => {
    if (typeof row.data_json !== 'string') throw new Error('Saved prior art is missing its canonical data payload.');
    const data = JSON.parse(row.data_json) as unknown;
    if (!data || typeof data !== 'object' || Array.isArray(data)) throw new Error('Saved prior-art data must be a JSON object.');
    const document = row.kind === 'document' ? data as Row : null;
    const documentContent = document && typeof document.text === 'string' && Array.isArray(document.links)
      ? { text: document.text, links: document.links }
      : null;
    const payload = json(documentContent ?? data);
    const contentHash = workspaceContentHash(payload);
    const path = `references/prior-art/${contentHash}.json`;
    files[path] = payload;
    const { data_json: _dataJson, ...metadata } = row;
    if (documentContent) {
      const { text: _text, links: _links, ...dataMetadata } = document!;
      return { ...metadata, data: dataMetadata, dataRef: { path, contentHash, projection: 'document-content' } };
    }
    return { ...metadata, dataRef: { path, contentHash, projection: 'full' } };
  });
}

/**
 * Canonical record IDs are opaque and may predate the workspace-file projection.
 * Keep already-safe IDs readable, but derive a stable segment for namespaced or
 * otherwise filesystem-unsafe IDs. The original ID remains in the file content
 * and is the authority used by imports and typed operations.
 */
function fileIdentifier(value: unknown): string {
  const id = recordIdentifier(value);
  return /^[a-zA-Z0-9][a-zA-Z0-9_.-]*$/u.test(id)
    ? id
    : `encoded-${workspaceContentHash(id)}`;
}

/**
 * Materializes one workspace-scoped derived-index snapshot into canonical research files.
 * Schema-v2 workspaces treat the completed files as authority; schema-v1 workspaces
 * retain this operation as a compatibility export. Database files and credentials
 * are never published.
 */
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
      // A relative "." reference identifies the workspace directory itself.
      // It is valid evidence metadata, but there are no bounded bytes to pin.
      // Keep strict child validation for every other path so traversal and
      // symlink checks retain their existing fail-closed behavior.
      if (candidate !== resolve(root)) assertWorkspaceChild(root, candidate);
      // Evidence paths may intentionally identify a workspace directory. Keep
      // that reference in the canonical record, but only pin byte-addressable
      // regular files as raw evidence. Recursively retaining a directory would
      // be unbounded, and reading it as a file raises EISDIR on POSIX hosts.
      if (!statSync(candidate).isFile()) return;
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
      ? (database.prepare(`SELECT * FROM ${table} WHERE ${where}`).all(...parameters) as Row[])
        .sort((left, right) => JSON.stringify(left).localeCompare(JSON.stringify(right))) : [];
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
      const id = recordIdentifier(claim.id);
      const fileId = fileIdentifier(id);
      const evidence = rows("app_server_claim_evidence", "claim_id = ?", [id]);
      for (const entry of evidence) retainArtifact(entry.reference_id);
      files[`claims/${fileId}.json`] = json({ schemaVersion: 1, ...claim, evidence,
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
      const id = recordIdentifier(memory.id);
      const fileId = fileIdentifier(id);
      const evidence = rows("memory_evidence_refs", "node_id = ?", [id]);
      for (const entry of evidence) retainPath(entry.path, entry.path_base);
      const { body, ...metadata } = memory;
      files[`memories/${fileId}.md`] = `<!-- Beale canonical memory; edit through memory tools or validated import. -->\n\n\`\`\`json\n${json({ schemaVersion: 1, ...metadata, workspace_id: options.workspaceId,
        workspaces: rows("memory_node_workspaces", "node_id = ? AND workspace_id = ?", [id, options.workspaceId]),
        tags: rows("memory_node_tags", "node_id = ?", [id]),
        assets: rows("memory_node_assets", "node_id = ?", [id]), evidence,
        edges: rows("memory_edges", "from_id = ?", [id]).filter((edge) => memoryIds.has(edge.to_id)),
        sessions: rows("memory_node_sessions", "node_id = ?", [id]),
        validations: rows("memory_node_catalog_validations", "node_id = ?", [id]),
        authorship: rows("app_server_model_authorship", "resource_kind = ? AND resource_id = ?", ["memory", id]),
      })}\`\`\`\n\n${String(body ?? "")}\n`;
    }
    for (const [table, category] of [["app_server_runbooks", "runbooks"], ["app_server_reports", "reports"]] as const) {
      for (const record of owned(table)) {
        const id = recordIdentifier(record.id);
        const fileId = fileIdentifier(id);
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
        files[`${category}/${fileId}/${category === "runbooks" ? "runbook" : "report"}.${extension}`] = content;
        files[`${category}/${fileId}/record.json`] = json({ schemaVersion: 2, ...record,
          revisions: rows('app_server_artifact_revisions', 'artifact_kind = ? AND artifact_id = ?', [category === 'runbooks' ? 'runbook' : 'report', id]),
          authorship: rows('app_server_model_authorship', 'resource_kind = ? AND resource_id = ?', [category === 'runbooks' ? 'runbook' : 'report', id]),
        });
        retainArtifact(record.submission_packet_artifact_id);
        retainArtifact(record.recording_artifact_id);
        artifactPaths.set(path, `${category}/${fileId}/${category === "runbooks" ? "runbook" : "report"}.${extension}`);
      }
    }
    for (const track of owned("campaign_tracks")) {
      const investigationId = recordIdentifier(track.id);
      const investigationFileId = fileIdentifier(investigationId);
      const observations = rows("campaign_track_observations", "investigation_id = ?", [investigationId]);
      files[`investigations/${investigationFileId}/record.json`] = json({
        schemaVersion: 2,
        ...track,
        sessions: rows("campaign_track_sessions", "investigation_id = ?", [investigationId]),
        resources: rows("campaign_track_resources", "investigation_id = ?", [investigationId]),
        questions: rows("campaign_track_questions", "investigation_id = ?", [investigationId]),
        experiments: rows("campaign_track_experiments", "investigation_id = ?", [investigationId]),
        observations: observations.map((observation) => ({
          ...observation,
          evidence: rows("campaign_track_observation_evidence", "observation_id = ?", [String(observation.id)]),
        })),
        nextActions: rows("campaign_track_next_actions", "investigation_id = ?", [investigationId]),
        memoryClaimReviews: rows("campaign_track_claim_reviews", "investigation_id = ?", [investigationId]),
        researchClaimReviews: rows("campaign_track_research_claim_reviews", "investigation_id = ?", [investigationId]),
      });
    }
    if (has('campaign_track_replay_runs') || has('campaign_track_consolidations')) {
      files['references/campaign-state.json'] = json({
        schemaVersion: 1,
        workspaceId: options.workspaceId,
        replayRuns: owned('campaign_track_replay_runs'),
        consolidations: owned('campaign_track_consolidations'),
      });
    }
    if (has('app_server_research_resources') || has('resource_prior_art')) {
      const resources = owned('app_server_research_resources');
      const priorArt = publishPriorArtRows(owned('resource_prior_art'), files);
      files['references/resources.json'] = json({
        schemaVersion: 2,
        workspaceId: options.workspaceId,
        resources: resources.map((resource) => ({
          ...resource,
          touches: rows('app_server_research_resource_touches', 'resource_id = ?', [String(resource.id)]),
        })),
        priorArt,
      });
    }
    const executions = owned("app_server_runbook_executions");
    for (const execution of executions) {
      const runId = recordIdentifier(execution.run_id);
      const path = `evidence/execution-${fileIdentifier(runId)}.json`;
      const cells = rows("app_server_runbook_cell_executions", "run_id = ?", [runId]);
      files[path] = json({ schemaVersion: 1, ...execution, cells });
      if (execution.completed_at) pins[path] = workspaceContentHash(files[path]!);
    }
    for (const session of owned("app_server_sessions")) {
      const id = recordIdentifier(session.id);
      const fileId = fileIdentifier(id);
      files[`traces/${fileId}/summary.md`] = `# ${String(session.title)}\n\nSession: ${id}\nStatus: ${String(session.status)}\n\n${String(session.summary)}\n`;
      if (options.sessionId === id && has("app_server_session_events")) {
        // Bounded pages avoid loading an entire long-running transcript into memory.
        exportTrace(database, root, id, fileId);
      }
    }
    database.exec("COMMIT");
    for (const id of referencedArtifacts) {
      const entry = artifactById.get(id)!;
      const hash = entry.contentHash.replace(/^sha256:/u, "");
      const path = `evidence/raw/${hash}`;
      const sizeBytes = retainWorkspaceArtifact(root, path, entry.path, hash);
      rawFiles[path] = hash;
      const evidencePath = `evidence/${fileIdentifier(id)}.json`;
      files[evidencePath] = json({ schemaVersion: 1, id, kind: entry.kind, purpose: entry.purpose, path, contentHash: hash, sizeBytes, sourceEventIds: entry.sourceEventIds });
      pins[evidencePath] = workspaceContentHash(files[evidencePath]!);
      artifactPaths.set(resolve(entry.path), path);
    }
    // Exact known storage paths become portable references; never publish database or credential material.
    for (const [path, content] of Object.entries(files)) {
      if (path.endsWith('.ipynb') || path.endsWith('.md') || PRIOR_ART_PAYLOAD_PATH.test(path)) continue;
      let portable = content;
      for (const [absolute, exported] of artifactPaths) {
        portable = portable.split(absolute).join(exported).split(absolute.replace(/\\/gu, "\\\\")).join(exported);
      }
      portable = portable.split(root).join("workspace:").split(root.replace(/\\/gu, "\\\\")).join("workspace:");
      files[path] = portable;
      if (path in pins) pins[path] = workspaceContentHash(portable);
    }
    publishWorkspaceFiles(root, files, pins, rawFiles);
    if (project.schemaVersion === 2 && project.researchAuthority === "files") markWorkspaceResearchIndexReady(options);
    return attribution;
  } catch (error) {
    try { database.exec("ROLLBACK"); } catch { /* Read snapshot already ended. */ }
    throw error;
  } finally { database.close(); }
}

function exportTrace(database: DatabaseSync, root: string, sessionId: string, sessionFileId: string): void {
  // Shards are immutable exports at a bounded size; they are excluded from Git.
  let offset = 0;
  while (true) {
    const page = database.prepare("SELECT event_offset, event_json FROM app_server_session_events WHERE session_id=? AND event_offset>=? ORDER BY event_offset LIMIT 250").all(sessionId, offset) as Array<{ event_offset: number; event_json: string }>;
    if (!page.length) break;
    atomicWorkspaceWrite(root, `traces/${sessionFileId}/events-${offset}.jsonl`, page.map((row) => row.event_json).join("\n") + "\n");
    offset = page[page.length - 1]!.event_offset + 1;
  }
}

export function checkpointWorkspaceResearch(options: WorkspacePublicationOptions, reason: string): WorkspaceCheckpointResult {
  const context: WorkspaceCommitContext = {};
  return checkpointWorkspace(options.workspaceRoot, reason, () => { Object.assign(context, publishWorkspaceResearch(options)); }, context);
}
