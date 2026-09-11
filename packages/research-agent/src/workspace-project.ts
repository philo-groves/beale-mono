import { createHash, randomUUID } from "node:crypto";
import { spawnSync } from "node:child_process";
import { chmodSync, closeSync, copyFileSync, existsSync, lstatSync, mkdirSync, openSync, readFileSync, readSync, readdirSync, renameSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const WORKSPACE_PROJECT_VERSION = 2;
export const WORKSPACE_CHECKPOINT_INTERVAL_MS = 10 * 60_000;
export const WORKSPACE_DIRECTORIES = ["investigations", "runbooks", "reports", "evidence", "references", "memories", "claims", "traces", "scratch", "cache"] as const;
const ROOT_FILES = new Set(["AGENTS.md", "AGENTS.override.md", "README.md", ".gitignore", "workspace.json"]);
const WORKSPACE_ROOT_INTERNAL_ENTRIES = new Set([".git", ".beale"]);
const MAX_TRACKED_BYTES = 5 * 1024 * 1024;
const INDEX_PATH = "references/research-index.json";
const IGNORES = ["/.beale/", "/scratch/", "/cache/", "/traces/**/events*.jsonl", "/traces/**/outputs/", "/evidence/raw/", "**/node_modules/", "**/.git/", "*.sqlite*", "*.db", ".env", ".env.*", "*.pem", "*.key", "*.p12", "*.pfx", "*.tmp"];
const WORKSPACE_GITIGNORE_START = "# >>> Beale managed workspace layout >>>";
const WORKSPACE_GITIGNORE_END = "# <<< Beale managed workspace layout <<<";
export const WORKSPACE_LAYOUT_GUARD_PREFIX = "[[APP_SERVER_HOST_WORKSPACE_LAYOUT_GUARD_V1]]\n";
export const WORKSPACE_LAYOUT_GUARD_SUFFIX = "\n[[/APP_SERVER_HOST_WORKSPACE_LAYOUT_GUARD_V1]]";

export interface UnexpectedWorkspaceTopLevelEntry {
  name: string;
  kind: "file" | "directory" | "symlink" | "other";
}

export interface WorkspaceProject {
  schemaVersion: 1 | 2;
  workspaceId: string;
  directories: readonly string[];
  checkpointIntervalMs: number;
  /** Absent on schema-v1 database-first compatibility workspaces. */
  researchAuthority?: "files";
}
export interface WorkspaceResearchIndex {
  schemaVersion: 1;
  workspaceId: string;
  files: Record<string, string>;
  /** Hashes of immutable evidence that has been cited by canonical research. */
  pins: Record<string, string>;
  rawFiles?: Record<string, string>;
}
export interface WorkspaceCheckpointResult {
  status: "committed" | "unchanged" | "failed" | "unmanaged";
  commit?: string;
  reason: string;
  error?: string;
  imported?: boolean;
  researchIndex?: {
    state: "ready" | "released";
    publicationHash: string;
    affectedRows: number;
  };
}
export interface WorkspaceCommitContext {
  investigationId?: string;
  sessionId?: string;
}

export interface WorkspaceResearchEdit {
  path: string;
  state: "created" | "modified" | "deleted";
}

/** Stable final trailers support git log --grep and Git's trailer filtering. */
export function formatWorkspaceCommitMessage(message: string, context: WorkspaceCommitContext = {}): string {
  const body = message.trimEnd();
  const id = (value: string | undefined): string => {
    if (value === undefined) return 'none';
    if (!/^[a-zA-Z0-9][a-zA-Z0-9_.:-]*$/u.test(value)) throw new Error('Commit attribution IDs must be nonempty single-line identifiers.');
    return value;
  };
  const existing = /\n\nInvestigation-ID: ([a-zA-Z0-9][a-zA-Z0-9_.:-]*)\nSession-ID: ([a-zA-Z0-9][a-zA-Z0-9_.:-]*)$/u.exec(body);
  const subject = existing ? body.slice(0, existing.index) : body;
  if (!subject.trim()) throw new Error('A research commit requires a nonempty message before its attribution trailers.');
  if (/^(?:Investigation-ID|Session-ID):/mu.test(subject)) throw new Error('Commit attribution must appear exactly once as the final Investigation-ID and Session-ID trailers.');
  return `${subject}\n\nInvestigation-ID: ${id(context.investigationId ?? existing?.[1])}\nSession-ID: ${id(context.sessionId ?? existing?.[2])}\n`;
}
export interface WorkspaceProjectHealth {
  fileCount: number;
  totalBytes: number;
  temporaryBytes: number;
  unclassifiedFileCount: number;
  partial: boolean;
  checkpoint: WorkspaceCheckpointResult | null;
}

export function getWorkspaceProjectHealth(root: string): WorkspaceProjectHealth | null {
  if (!readWorkspaceProject(root)) return null;
  const health: WorkspaceProjectHealth = { fileCount: 0, totalBytes: 0, temporaryBytes: 0, unclassifiedFileCount: 0, partial: false, checkpoint: null };
  const deadline = Date.now() + 250;
  const stack = [root];
  while (stack.length && health.fileCount < 50_000 && Date.now() < deadline) {
    const directory = stack.pop()!;
    let entries;
    try { entries = readdirSync(directory, { withFileTypes: true }); } catch { health.partial = true; continue; }
    for (const entry of entries) {
      if (health.fileCount >= 50_000 || Date.now() >= deadline) { health.partial = true; break; }
      if (entry.name === '.git' || entry.name === '.beale') continue;
      const path = join(directory, entry.name);
      let stats;
      try { stats = lstatSync(path); } catch { health.partial = true; continue; }
      if (stats.isSymbolicLink()) continue;
      if (stats.isDirectory()) { stack.push(path); continue; }
      if (!stats.isFile()) continue;
      const child = relative(root, path).replace(/\\/gu, '/');
      health.fileCount++; health.totalBytes += stats.size;
      if (/^(?:scratch|cache)\//u.test(child)) health.temporaryBytes += stats.size;
      else if (!/^traces\/.*(?:\.jsonl$|\/outputs\/)|^evidence\/raw\//u.test(child) && workspacePathProblem(child)) health.unclassifiedFileCount++;
    }
  }
  health.partial ||= stack.length > 0;
  const path = join(root, '.git', 'beale', 'checkpoint.json');
  if (existsSync(path)) health.checkpoint = JSON.parse(readFileSync(path, 'utf8')) as WorkspaceCheckpointResult;
  return health;
}

export function workspaceContentHash(content: Uint8Array | string): string {
  return createHash("sha256").update(content).digest("hex");
}

export function readWorkspaceProject(root: string): WorkspaceProject | null {
  const path = join(root, "workspace.json");
  if (!existsSync(path)) return null;
  const value = JSON.parse(readFileSync(path, "utf8")) as Partial<WorkspaceProject>;
  if ((value.schemaVersion !== 1 && value.schemaVersion !== WORKSPACE_PROJECT_VERSION)
    || typeof value.workspaceId !== "string" || !value.workspaceId.trim()
    || (value.schemaVersion === WORKSPACE_PROJECT_VERSION && value.researchAuthority !== "files")) {
    throw new Error("Unsupported workspace.json. A Beale research workspace requires schemaVersion 1 compatibility metadata or schemaVersion 2 with file authority.");
  }
  return value as WorkspaceProject;
}

export function workspaceResearchAuthority(root: string): "files" | "database" | null {
  const project = readWorkspaceProject(root);
  if (!project) return null;
  return project.schemaVersion === WORKSPACE_PROJECT_VERSION && project.researchAuthority === "files"
    ? "files"
    : "database";
}

/** Scans the filesystem directly so Git-ignored root pollution remains visible to the agent. */
export function listUnexpectedWorkspaceTopLevelEntries(root: string): UnexpectedWorkspaceTopLevelEntry[] {
  if (!readWorkspaceProject(root)) return [];
  return readdirSync(root, { withFileTypes: true })
    .filter((entry) => !ROOT_FILES.has(entry.name)
      && !WORKSPACE_ROOT_INTERNAL_ENTRIES.has(entry.name)
      && !(WORKSPACE_DIRECTORIES as readonly string[]).includes(entry.name))
    .map((entry) => ({
      name: entry.name,
      kind: entry.isSymbolicLink()
        ? "symlink" as const
        : entry.isDirectory()
          ? "directory" as const
          : entry.isFile()
            ? "file" as const
            : "other" as const,
    }))
    .sort((left, right) => left.name.localeCompare(right.name));
}

export function workspaceLayoutGuardMessage(root: string): string | null {
  let entries: UnexpectedWorkspaceTopLevelEntry[];
  try {
    entries = listUnexpectedWorkspaceTopLevelEntries(root);
  } catch {
    return [
      WORKSPACE_LAYOUT_GUARD_PREFIX.trimEnd(),
      "Workspace layout verification failed on the canonical workspace root.",
      "Do not treat this as a clean workspace root. Preserve existing material and repair the workspace metadata or filesystem access problem; ask the operator for help if it cannot be resolved safely.",
      "This host check repeats every turn until the workspace layout can be verified.",
      WORKSPACE_LAYOUT_GUARD_SUFFIX.trimStart(),
    ].join("\n");
  }
  if (entries.length === 0) return null;
  const shown = entries.slice(0, 20).map((entry) => `${JSON.stringify(entry.name)}${entry.kind === "directory" ? "/" : ""}`);
  if (entries.length > shown.length) shown.push(`and ${entries.length - shown.length} more`);
  return [
    WORKSPACE_LAYOUT_GUARD_PREFIX.trimEnd(),
    `Workspace layout repair is required before continuing or ending this session. Unexpected top-level entries: ${shown.join(", ")}.`,
    "Move every research item into an approved directory: investigations/, runbooks/, reports/, evidence/, references/, memories/, claims/, traces/, scratch/, or cache/. Use typed research tools for canonical records when applicable. Do not delete research merely to clear this guard; remove only material known to be disposable.",
    "The workspace root may contain only AGENTS.md, AGENTS.override.md, README.md, .gitignore, workspace.json, and the approved directories. This host check is filesystem-based and repeats every turn until all unexpected entries are resolved.",
    WORKSPACE_LAYOUT_GUARD_SUFFIX.trimStart(),
  ].join("\n");
}

export function isWorkspaceLayoutGuardMessage(message: string): boolean {
  return message.startsWith(WORKSPACE_LAYOUT_GUARD_PREFIX)
    && message.endsWith(WORKSPACE_LAYOUT_GUARD_SUFFIX);
}

function managedWorkspaceGitIgnore(): string {
  return [
    WORKSPACE_GITIGNORE_START,
    "# Ignore every unexpected top-level entry; the runtime guard still reports it until moved.",
    "/*",
    ...[...ROOT_FILES].sort().map((name) => `!/${name}`),
    ...(WORKSPACE_DIRECTORIES as readonly string[]).map((name) => `!/${name}/`),
    ...IGNORES,
    WORKSPACE_GITIGNORE_END,
  ].join("\n");
}

function ensureWorkspaceGitIgnore(root: string): void {
  const path = join(root, ".gitignore");
  let existing = existsSync(path) ? readFileSync(path, "utf8") : "";
  const managedPattern = new RegExp(`${WORKSPACE_GITIGNORE_START.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&")}[\\s\\S]*?${WORKSPACE_GITIGNORE_END.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&")}\\n?`, "gu");
  existing = existing.replace(managedPattern, "").trimEnd();
  if (existing.trim() === IGNORES.join("\n")) existing = "";
  const content = `${existing ? `${existing}\n\n` : ""}${managedWorkspaceGitIgnore()}\n`;
  if (!existsSync(path) || readFileSync(path, "utf8") !== content) atomicWorkspaceWrite(root, ".gitignore", content);
}

/** A dedicated research directory is never adopted from an existing source repository. */
export function initializeWorkspaceProject(root: string, workspaceId: string): WorkspaceProject {
  root = resolve(root);
  mkdirSync(root, { recursive: true });
  const existing = readWorkspaceProject(root);
  if (existing) {
    if (existing.workspaceId !== workspaceId) throw new Error("Workspace identity does not match workspace.json.");
    if (!existsSync(join(root, '.git'))) git(root, ['init', '--initial-branch=research']);
    ensureWorkspaceGitIgnore(root);
    installWorkspaceGitHook(root);
    if (workspaceResearchAuthority(root) === 'files') restoreWorkspacePublicationMetadata(root, existing);
    let initialized = false;
    try { git(root, ['rev-parse', '--verify', 'HEAD']); initialized = true; } catch { /* Resume interrupted creation. */ }
    if (!initialized) {
      const result = checkpointWorkspace(root, 'Initialize research workspace');
      if (result.status === 'failed') throw new Error(result.error);
    }
    return existing;
  }
  if (existsSync(join(root, ".git"))) throw new Error("Choose a dedicated research directory; keep source repositories outside the workspace.");
  const unexpected = readdirSync(root).filter((name) => name !== ".beale" && !ROOT_FILES.has(name));
  if (unexpected.length) throw new Error("New research workspaces must be empty apart from AGENTS.md, README.md, and workspace configuration. Import reference material after creation.");
  const project: WorkspaceProject = {
    schemaVersion: WORKSPACE_PROJECT_VERSION,
    workspaceId,
    directories: WORKSPACE_DIRECTORIES,
    checkpointIntervalMs: WORKSPACE_CHECKPOINT_INTERVAL_MS,
    researchAuthority: "files",
  };
  for (const directory of WORKSPACE_DIRECTORIES) {
    mkdirSync(join(root, directory), { recursive: true });
    if (directory !== "scratch" && directory !== "cache") writeFileSync(join(root, directory, ".gitkeep"), "");
  }
  if (!existsSync(join(root, "AGENTS.md"))) writeFileSync(join(root, "AGENTS.md"), WORKSPACE_INSTRUCTIONS);
  if (!existsSync(join(root, "README.md"))) writeFileSync(join(root, "README.md"), "# Beale research workspace\n\nResearch files and local Git checkpoints are managed by app-server. Repositories remain outside this directory. Remote setup and synchronization are operator-controlled.\n");
  ensureWorkspaceGitIgnore(root);
  atomicWorkspaceWrite(root, "workspace.json", JSON.stringify(project, null, 2) + "\n");
  git(root, ["init", "--initial-branch=research"]);
  installWorkspaceGitHook(root);
  publishWorkspaceFiles(root, {});
  const result = checkpointWorkspace(root, "Initialize research workspace");
  if (result.status === "failed") throw new Error(result.error);
  return project;
}

/** Rebuilds only disposable publication metadata from the canonical working files. */
function restoreWorkspacePublicationMetadata(root: string, project: WorkspaceProject): void {
  const statePath = join(root, '.git', 'beale', 'publication.json');
  if (existsSync(statePath)) return;
  const indexPath = join(root, INDEX_PATH);
  if (!existsSync(indexPath)) {
    const canonicalRoots = ['claims', 'memories', 'runbooks', 'reports', 'investigations'];
    const hasResearch = canonicalRoots.some((directory) => readdirSync(join(root, directory), { withFileTypes: true })
      .some((entry) => entry.name !== '.gitkeep'));
    if (hasResearch) throw new Error('File-authority publication metadata is missing; restore references/research-index.json before continuing.');
    publishWorkspaceFiles(root, {});
    return;
  }
  const index = JSON.parse(readFileSync(indexPath, 'utf8')) as WorkspaceResearchIndex;
  if (index.schemaVersion !== 1 || index.workspaceId !== project.workspaceId || !index.files || !index.pins) {
    throw new Error('File-authority research index is invalid or belongs to another workspace.');
  }
  const contentDirectory = join(root, '.git', 'beale', 'publication-content');
  mkdirSync(contentDirectory, { recursive: true });
  for (const [path, hash] of Object.entries(index.files)) {
    const absolute = join(root, path);
    assertWorkspaceChild(root, absolute);
    if (!safeRelative(path) || !existsSync(absolute) || workspaceFileHash(absolute) !== hash) {
      throw new Error(`${path}: canonical research does not match the recoverable research index.`);
    }
    const content = readFileSync(absolute);
    const baseline = join(contentDirectory, hash);
    if (!existsSync(baseline)) writeFileSync(baseline, content, { mode: 0o600 });
  }
  for (const [path, hash] of Object.entries({ ...index.pins, ...index.rawFiles })) {
    const absolute = join(root, path);
    assertWorkspaceChild(root, absolute);
    if (!safeRelative(path) || !existsSync(absolute) || workspaceFileHash(absolute) !== hash) {
      throw new Error(`${path}: retained research evidence does not match the recoverable research index.`);
    }
  }
  atomicWorkspaceWrite(root, '.git/beale/publication.json', JSON.stringify(index));
}

export const WORKSPACE_INSTRUCTIONS = `# Beale research workspace

This directory is one research workspace. Source repositories belong in the host-supplied external repository store.

- investigations/: stable candidate directories containing analysis, code, and fixtures; reuse identities across attempts.
- runbooks/: canonical executable procedures and self-contained reproduction packages.
- reports/: canonical reports and report-specific supporting material.
- memories/ and claims/: canonical file-authority records. Typed research tools trigger background synchronization; direct edits enter the derived index through validated import.
- evidence/: retained evidence and provenance. Cited evidence is immutable; corrections require a new artifact.
- references/: background material and the host-published research index.
- traces/: session summaries and untracked raw event exports.
- scratch/: disposable session experiments; cache/: rebuildable outputs and downloads. Both are excluded from Git.

Keep related scripts and fixtures with their investigation or runbook. Default shell execution uses session scratch; specify an external repository cwd for source work and an explicit investigation directory for persistent candidate work.
Keep the workspace top level clean. Unexpected files and directories are ignored by Git but detected directly by app-server; the agent is reminded every turn until it moves them into an approved directory.
App-server keeps its derived research index synchronized with these files and creates local Git checkpoints before research, at research milestones, every ten minutes with changes, and after execution ends. Do not bypass guards, rewrite history, discard changes, or push automatically. A checkpoint is history, not evidence validation. Git guard failures preserve work and must be surfaced.
Every commit ends with Investigation-ID and Session-ID trailers. Supply the actual IDs for manual research commits; use none only when there is no associated investigation or session.
Host commands retain the operator's privileges; these conventions are not filesystem isolation.
`;

function shellQuote(value: string): string { return "'" + value.replace(/'/gu, "'\\''") + "'"; }

export function installWorkspaceGitHook(root: string): void {
  const gitDir = join(root, ".git");
  if (!existsSync(gitDir) || !lstatSync(gitDir).isDirectory()) throw new Error("A research workspace requires its own local Git repository.");
  const hooks = join(gitDir, "hooks");
  mkdirSync(hooks, { recursive: true });
  const marker = "# Beale managed research guard";
  const source = fileURLToPath(import.meta.url).replace(/\\/gu, "/");
  const managedHooks = [['pre-commit', '--beale-workspace-precommit'], ['commit-msg', '--beale-workspace-commitmsg']] as const;
  for (const [name] of managedHooks) {
    const hook = join(hooks, name);
    if (existsSync(hook) && !readFileSync(hook, 'utf8').includes(marker)) throw new Error(`An unmanaged ${name} hook exists; preserve it and explicitly integrate the Beale guard before checkpointing.`);
  }
  for (const [name, argument] of managedHooks) {
    const hook = join(hooks, name);
    writeFileSync(hook, `#!/bin/sh\n${marker}\nexec env ELECTRON_RUN_AS_NODE=1 ${shellQuote(process.execPath.replace(/\\/gu, '/'))} ${shellQuote(source)} ${argument} "$@"\n`);
    chmodSync(hook, 0o755);
  }
  // Local-only configuration; no remote is created or used.
  git(root, ["config", "--local", "core.hooksPath", ".git/hooks"]);
}

let checkpointDeadline = 0;
function gitBytes(root: string, args: string[], environment: NodeJS.ProcessEnv = {}, input?: string): Buffer {
  const remaining = checkpointDeadline ? checkpointDeadline - Date.now() : 30_000;
  if (remaining <= 0) throw new Error("Workspace checkpoint exceeded its time budget; files were preserved.");
  const result = spawnSync("git", ["--literal-pathspecs", "-c", "core.quotepath=false", "-c", "core.autocrlf=false", "-c", "core.safecrlf=false", "-c", "gc.auto=0", ...args], {
    cwd: root, windowsHide: true, timeout: Math.min(30_000, remaining), maxBuffer: 32 * 1024 * 1024,
    env: { ...process.env, GIT_TERMINAL_PROMPT: "0", GIT_DIR: join(root, '.git'), GIT_WORK_TREE: root,
      GIT_INDEX_FILE: process.argv[2] === '--beale-workspace-precommit' ? process.env.GIT_INDEX_FILE : undefined,
      ...environment }, ...(input === undefined ? {} : { input }),
  });
  if (result.error || result.status !== 0) throw new Error(`Git ${args[0]} failed: ${result.error?.message ?? result.stderr.toString('utf8').trim()}`);
  return result.stdout;
}
function git(root: string, args: string[], environment: NodeJS.ProcessEnv = {}, input?: string): string {
  return gitBytes(root, args, environment, input).toString('utf8');
}

function safeRelative(path: string): boolean {
  return Boolean(path) && !isAbsolute(path) && !path.includes("\\") && !path.split("/").some((part) => !part || part === "." || part === ".." || part.toLowerCase() === ".git");
}

export function workspacePathProblem(path: string): string | null {
  if (!safeRelative(path)) return "path must be a relative workspace file";
  if (!path.includes("/")) return ROOT_FILES.has(path) ? null : "root files must be AGENTS.md, AGENTS.override.md, README.md, .gitignore, or workspace.json";
  const parts = path.split("/");
  if (!(WORKSPACE_DIRECTORIES as readonly string[]).includes(parts[0]!)) return "file must belong to an approved research directory";
  if (parts[0] === "scratch" || parts[0] === "cache") return "disposable files must not be committed";
  if (parts.some((part) => /^(?:node_modules|\.env(?:\..*)?|credentials?(?:\..*)?|id_rsa|id_ed25519)$/iu.test(part)) || /\.(?:sqlite(?:-wal|-shm)?|db|pem|key|p12|pfx)$/iu.test(path)) return "runtime databases and credential material must not be committed";
  if (path.startsWith("evidence/raw/") || (parts[0] === "traces" && (parts.includes("outputs") || /\.jsonl$/iu.test(path)))) return "raw captures are retained outside Git";
  return null;
}

function stagedFiles(root: string): Map<string, { mode: string; hash: string }> {
  const entries = new Map<string, { mode: string; hash: string }>();
  for (const entry of git(root, ["ls-files", "--stage", "-z"]).split("\0").filter(Boolean)) {
    const match = /^(\d+) ([a-f0-9]+) (\d)\t([\s\S]+)$/u.exec(entry);
    if (!match || match[3] !== "0") throw new Error("Resolve Git index conflicts before committing research.");
    entries.set(match[4]!, { mode: match[1]!, hash: match[2]! });
  }
  return entries;
}

function readIndex(root: string, spec: string): WorkspaceResearchIndex | null {
  let content: string;
  try { content = git(root, ["show", spec]); } catch { return null; }
  const value = JSON.parse(content) as WorkspaceResearchIndex;
  if (value.schemaVersion !== 1 || !value.files || !value.pins) throw new Error("Invalid research index; republish a complete canonical snapshot.");
  return value;
}

/** Validates blobs from the index, never unstaged working-tree substitutes. */
export function validateWorkspaceCommit(root: string): void {
  const entries = stagedFiles(root);
  const hashes = [...new Set([...entries.values()].map((entry) => entry.hash))];
  const sizes = new Map(git(root, ['cat-file', '--batch-check=%(objectname) %(objectsize)'], {}, hashes.join('\n') + '\n')
    .trim().split('\n').map((line) => { const [hash, size] = line.split(' '); return [hash!, Number(size)] as const; }));
  const contentHashes = new Map<string, string>();
  const special = new Map<string, string>();
  for (let start = 0; start < hashes.length;) {
    const batch: string[] = [];
    let total = 0;
    while (start < hashes.length && total < 8 * 1024 * 1024) {
      const hash = hashes[start++]!;
      const size = sizes.get(hash);
      if (size === undefined || !Number.isFinite(size) || size > MAX_TRACKED_BYTES) throw new Error('A staged file exceeds the 5 MiB tracked-file limit; retain it as a raw artifact with a manifest.');
      batch.push(hash); total += size;
    }
    const result = gitBytes(root, ['cat-file', '--batch'], {}, batch.join('\n') + '\n');
    let offset = 0;
    for (const hash of batch) {
      const header = result.indexOf(10, offset);
      if (header < 0) throw new Error('Incomplete Git blob response.');
      const content = result.subarray(header + 1, header + 1 + sizes.get(hash)!);
      offset = header + 1 + content.length + 1;
      contentHashes.set(hash, workspaceContentHash(content));
      const text = content.toString('utf8');
      if (content.subarray(0, 16).toString() === 'SQLite format 3\0' || /-----BEGIN (?:[A-Z ]+ )?PRIVATE KEY-----|\b(?:sk-proj-|sk-ant-)[A-Za-z0-9_-]{16,}/u.test(text)) throw new Error('A staged file contains runtime database or credential material.');
      if (hash === entries.get('workspace.json')?.hash || hash === entries.get(INDEX_PATH)?.hash) special.set(hash, text);
    }
  }
  const projectEntry = entries.get("workspace.json");
  if (!projectEntry) throw new Error("workspace.json cannot be removed.");
  const project = JSON.parse(special.get(projectEntry.hash)!) as WorkspaceProject;
  if ((project.schemaVersion !== 1 && project.schemaVersion !== WORKSPACE_PROJECT_VERSION)
    || typeof project.workspaceId !== 'string' || !project.workspaceId.trim()
    || project.checkpointIntervalMs !== WORKSPACE_CHECKPOINT_INTERVAL_MS
    || JSON.stringify(project.directories) !== JSON.stringify(WORKSPACE_DIRECTORIES)
    || (project.schemaVersion === WORKSPACE_PROJECT_VERSION && project.researchAuthority !== 'files')) {
    throw new Error("workspace.json must retain the supported layout, research authority, and identity.");
  }
  let previousProject: WorkspaceProject | undefined;
  try { previousProject = JSON.parse(git(root, ['show', 'HEAD:workspace.json'])) as WorkspaceProject; } catch { /* Initial commit. */ }
  if (previousProject && (previousProject.workspaceId !== project.workspaceId
    || previousProject.schemaVersion !== project.schemaVersion
    || previousProject.researchAuthority !== project.researchAuthority)) {
    throw new Error('The committed workspace identity cannot be changed, and research authority cannot be changed manually.');
  }
  for (const [path, entry] of entries) {
    const problem = workspacePathProblem(path);
    if (problem) throw new Error(`${path}: ${problem}.`);
    if (entry.mode !== "100644" && entry.mode !== "100755") throw new Error(`${path}: symlinks and nested repositories cannot be tracked research files.`);
  }
  const index = readIndex(root, `:${INDEX_PATH}`);
  const previous = readIndex(root, `HEAD:${INDEX_PATH}`);
  if (previous && !index) throw new Error("The canonical research index cannot be removed.");
  if (index) {
    if (index.workspaceId !== project.workspaceId) throw new Error("Research index belongs to a different workspace.");
    for (const [path, hash] of Object.entries({ ...index.files, ...index.pins, ...previous?.pins })) {
      const entry = entries.get(path);
      if (!safeRelative(path) || !entry || contentHashes.get(entry.hash) !== hash) throw new Error(`${path}: missing, modified, or incompletely published canonical research/evidence. Use validated research operations; retain referenced files.`);
    }
    const published = join(root, ".git", "beale", "publication.json");
    if (!existsSync(published) || readFileSync(published, "utf8") !== JSON.stringify(index)) throw new Error("Research snapshot is not the current completed app-server publication.");
    for (const [path, hash] of Object.entries({ ...index.rawFiles, ...previous?.rawFiles })) {
      const absolute = join(root, path);
      assertWorkspaceChild(root, absolute);
      if (!path.startsWith('evidence/raw/') || !existsSync(absolute) || workspaceFileHash(absolute) !== hash) throw new Error(`${path}: retained raw evidence is missing or changed.`);
    }
  }
}

function withProjectLock<T>(root: string, operation: () => T): T {
  const lock = join(root, ".git", "beale-checkpoint.lock");
  if (existsSync(lock)) {
    const owner = Number(readFileSync(lock, "utf8"));
    if (Number.isInteger(owner) && owner > 0) {
      try { process.kill(owner, 0); }
      catch (error) {
        if ((error as NodeJS.ErrnoException).code === "ESRCH") {
          recoverCheckpointIndex(root, owner);
          rmSync(lock);
        }
      }
    }
  }
  let descriptor: number;
  try { descriptor = openSync(lock, "wx"); } catch { throw new Error("Another workspace checkpoint or maintenance operation is in progress. Retry after it finishes."); }
  try { writeFileSync(descriptor, String(process.pid)); return operation(); }
  finally { closeSync(descriptor); rmSync(lock); }
}

function recoverCheckpointIndex(root: string, owner: number): void {
  const journal = join(root, '.git', 'beale', 'index-owner.json');
  if (!existsSync(journal)) return;
  const record = JSON.parse(readFileSync(journal, 'utf8')) as { pid: number; temporaryIndex: string };
  if (record.pid !== owner || !/^beale-index-[a-f0-9-]+$/u.test(record.temporaryIndex)) throw new Error('Checkpoint index recovery requires an intact ownership journal.');
  const temporary = join(root, '.git', record.temporaryIndex);
  rmSync(`${temporary}.lock`, { force: true });
  if (existsSync(temporary)) {
    let headTree: string | null = null;
    try { headTree = git(root, ['rev-parse', 'HEAD^{tree}']).trim(); } catch { /* No initial commit yet. */ }
    if (headTree && git(root, ['write-tree'], { GIT_INDEX_FILE: temporary }).trim() === headTree) copyFileSync(temporary, join(root, '.git', 'index'));
  }
  rmSync(join(root, '.git', 'index.lock'), { force: true });
  rmSync(temporary, { force: true });
  rmSync(journal);
}

/** No reset, stash, clean, remote operation, or working-tree rollback is used. */
export function checkpointWorkspace(root: string, reason: string, publish?: () => void, context: WorkspaceCommitContext = {}): WorkspaceCheckpointResult {
  if (!readWorkspaceProject(root)) return { status: "unmanaged", reason };
  try {
    checkpointDeadline = Date.now() + 60_000;
    return withProjectLock(root, () => {
      installWorkspaceGitHook(root);
      publish?.();
      const message = formatWorkspaceCommitMessage(reason, context);
      // Reserve the real index while preparing a separate index. Manual staging is never consumed.
      const indexLock = join(root, ".git", "index.lock");
      const lock = openSync(indexLock, "wx");
      let indexLockOpen = true;
      const temporaryIndex = join(root, ".git", `beale-index-${randomUUID()}`);
      const indexJournal = join(root, '.git', 'beale', 'index-owner.json');
      try {
        mkdirSync(dirname(indexJournal), { recursive: true });
        writeFileSync(indexJournal, JSON.stringify({ pid: process.pid, temporaryIndex: relative(join(root, '.git'), temporaryIndex) }));
        if (git(root, ["diff", "--cached", "--name-only"]).trim()) throw new Error("Manual staged changes are present. Commit or unstage them explicitly; automatic checkpoints preserve the index.");
        const indexPath = join(root, ".git", "index");
        if (existsSync(indexPath)) copyFileSync(indexPath, temporaryIndex);
        const env = { GIT_INDEX_FILE: temporaryIndex };
        const candidates = new Set([...git(root, ["ls-files", "-z"]).split("\0"), ...git(root, ["ls-files", "--others", "--exclude-standard", "-z"]).split("\0")].filter(Boolean));
        const paths = [...candidates].filter((path) => !workspacePathProblem(path));
        const invalid = [...candidates].filter((path) => workspacePathProblem(path));
        if (invalid.length) throw new Error(`Move unclassified files into the workspace layout before checkpointing: ${invalid.slice(0, 8).join(", ")}`);
        if (paths.length) git(root, ["add", "--all", "--pathspec-from-file=-", "--pathspec-file-nul"], env, paths.join("\0") + "\0");
        if (!git(root, ["diff", "--cached", "--name-only"], env).trim()) {
          git(root, ['hook', 'run', 'pre-commit'], env);
          const result: WorkspaceCheckpointResult = { status: 'unchanged', reason, commit: git(root, ['rev-parse', 'HEAD']).trim() };
          writeCheckpointStatus(root, result);
          return result;
        }
        git(root, ["-c", "user.name=Beale", "-c", "user.email=beale@example.invalid", "-c", "commit.gpgSign=false", "commit", "-m", message], env);
        copyFileSync(temporaryIndex, indexLock);
        closeSync(lock);
        indexLockOpen = false;
        renameSync(indexLock, indexPath);
        const commit = git(root, ["rev-parse", "HEAD"]).trim();
        writeCheckpointStatus(root, { status: "committed", commit, reason });
        return { status: "committed", commit, reason };
      } finally {
        if (indexLockOpen) closeSync(lock);
        rmSync(indexLock, { force: true });
        rmSync(temporaryIndex, { force: true });
        rmSync(`${temporaryIndex}.lock`, { force: true });
        rmSync(indexJournal, { force: true });
      }
    });
  } catch (error) {
    const result: WorkspaceCheckpointResult = { status: "failed", reason, error: error instanceof Error ? error.message : String(error) };
    writeCheckpointStatus(root, result);
    return result;
  } finally { checkpointDeadline = 0; }
}

export function writeCheckpointStatus(root: string, result: WorkspaceCheckpointResult): void {
  const directory = join(root, ".git", "beale");
  mkdirSync(directory, { recursive: true });
  atomicWorkspaceWrite(root, '.git/beale/checkpoint.json', JSON.stringify({ ...result, updatedAt: new Date().toISOString() }, null, 2) + "\n");
}

export function assertWorkspaceChild(root: string, path: string): string {
  const child = relative(resolve(root), resolve(path));
  if (!child || isAbsolute(child) || child === ".." || child.startsWith("../") || child.startsWith("..\\")) throw new Error("Path must remain inside the research workspace.");
  let parent = resolve(path);
  while (parent !== resolve(root)) {
    if (existsSync(parent) && lstatSync(parent).isSymbolicLink()) throw new Error("Managed workspace paths cannot traverse symlinks.");
    parent = dirname(parent);
  }
  return child.replace(/\\/gu, "/");
}

export function atomicWorkspaceWrite(root: string, path: string, content: string | Uint8Array): void {
  const destination = resolve(root, path);
  assertWorkspaceChild(root, destination);
  mkdirSync(dirname(destination), { recursive: true });
  const temporary = `${destination}.${randomUUID()}.tmp`;
  try { writeFileSync(temporary, content, { mode: 0o600 }); renameSync(temporary, destination); }
  finally { rmSync(temporary, { force: true }); }
}

/** Recovery copies are untracked and content-addressed; edits never erase the prior bytes. */
export function preserveWorkspaceFile(root: string, path: string): void {
  if (!readWorkspaceProject(root) || !existsSync(path)) return;
  const child = assertWorkspaceChild(root, path);
  const bytes = readFileSync(path);
  const hash = workspaceContentHash(bytes);
  const directory = join(root, ".git", "beale", "recovery");
  mkdirSync(directory, { recursive: true });
  const target = join(directory, hash);
  if (!existsSync(target)) writeFileSync(target, bytes, { flag: "wx", mode: 0o600 });
  writeFileSync(join(directory, `${Date.now()}-${randomUUID()}.json`), JSON.stringify({ path: child, hash }) + "\n");
}

export function publishWorkspaceFiles(root: string, files: Record<string, string>, pins: Record<string, string> = {}, rawFiles: Record<string, string> = {}): void {
  const project = readWorkspaceProject(root);
  if (!project) return;
  const state = join(root, ".git", "beale", "publication.json");
  const previous = existsSync(state) ? JSON.parse(readFileSync(state, "utf8")) as WorkspaceResearchIndex : null;
  if (previous && (!existsSync(join(root, INDEX_PATH)) || readFileSync(join(root, INDEX_PATH), 'utf8') !== JSON.stringify(previous, null, 2) + '\n')) throw new Error('The published research index was edited or removed; preserve the edit before republishing.');
  // Verify the entire previous publication before changing any file. Never overwrite manual edits.
  for (const [path, hash] of Object.entries(previous?.files ?? {})) {
    const absolute = join(root, path);
    assertWorkspaceChild(root, absolute);
    if (!existsSync(absolute) || workspaceContentHash(readFileSync(absolute)) !== hash) throw new Error(`${path}: canonical export was edited or removed; preserve/import the edit before republishing.`);
  }
  const hashes: Record<string, string> = {};
  for (const [path, content] of Object.entries(files).sort(([a], [b]) => a.localeCompare(b))) {
    if (workspacePathProblem(path)) throw new Error(`Invalid canonical export path: ${path}`);
    const destination = join(root, path);
    if (existsSync(destination) && !previous?.files[path] && readFileSync(destination, "utf8") !== content) throw new Error(`${path}: an existing file conflicts with canonical publication.`);
    hashes[path] = workspaceContentHash(content);
  }
  const index: WorkspaceResearchIndex = { schemaVersion: 1, workspaceId: project.workspaceId, files: hashes, pins: { ...previous?.pins, ...pins }, rawFiles: { ...previous?.rawFiles, ...rawFiles } };
  // Persist a recovery journal before publishing. A restart can finish an interrupted publication.
  const directory = dirname(state);
  mkdirSync(directory, { recursive: true });
  atomicWorkspaceWrite(root, '.git/beale/pending-publication.json', JSON.stringify({ files, index }));
  finishWorkspacePublication(root, files, index, previous);
}

function finishWorkspacePublication(root: string, files: Record<string, string>, index: WorkspaceResearchIndex, previous: WorkspaceResearchIndex | null): void {
  const contentDirectory = join(root, ".git", "beale", "publication-content");
  mkdirSync(contentDirectory, { recursive: true });
  for (const [path, content] of Object.entries(files)) {
    const hash = workspaceContentHash(content);
    writeFileSync(join(contentDirectory, hash), content, { mode: 0o600 });
    if (!existsSync(join(root, path)) || workspaceContentHash(readFileSync(join(root, path))) !== hash) atomicWorkspaceWrite(root, path, content);
  }
  for (const path of Object.keys(previous?.files ?? {})) {
    if (path in files || path in index.pins) continue;
    preserveWorkspaceFile(root, join(root, path));
    rmSync(join(root, path), { force: true });
  }
  atomicWorkspaceWrite(root, INDEX_PATH, JSON.stringify(index, null, 2) + "\n");
  const state = join(root, ".git", "beale", "publication.json");
  atomicWorkspaceWrite(root, '.git/beale/publication.json', JSON.stringify(index));
  rmSync(join(dirname(state), "pending-publication.json"), { force: true });
}

export function readPublishedWorkspaceFile(root: string, path: string): string {
  assertWorkspaceChild(root, join(root, path));
  const index = JSON.parse(readFileSync(join(root, ".git", "beale", "publication.json"), "utf8")) as WorkspaceResearchIndex;
  const hash = index.files[path];
  if (!hash || !/^[a-f0-9]{64}$/u.test(hash)) throw new Error("File is not a published canonical research record.");
  return readFileSync(join(root, ".git", "beale", "publication-content", hash), "utf8");
}

/** Returns only edits to app-server-managed research files; ordinary file-native work is not included. */
export function listWorkspaceResearchEdits(root: string): WorkspaceResearchEdit[] {
  if (workspaceResearchAuthority(root) !== "files") return [];
  const statePath = join(root, ".git", "beale", "publication.json");
  const indexPath = join(root, INDEX_PATH);
  if (!existsSync(statePath) || !existsSync(indexPath)) {
    throw new Error("File-authority workspace metadata is incomplete; restore the research index before continuing.");
  }
  const index = JSON.parse(readFileSync(statePath, "utf8")) as WorkspaceResearchIndex;
  if (readFileSync(indexPath, "utf8") !== JSON.stringify(index, null, 2) + "\n") {
    throw new Error("The research index was edited directly; restore it before importing individual research files.");
  }
  const edits: WorkspaceResearchEdit[] = [];
  for (const [path, hash] of Object.entries(index.files)) {
    const absolute = join(root, path);
    assertWorkspaceChild(root, absolute);
    if (!existsSync(absolute)) edits.push({ path, state: "deleted" });
    else if (workspaceFileHash(absolute) !== hash) edits.push({ path, state: "modified" });
  }
  for (const path of listManagedResearchRecordPaths(root)) {
    if (!(path in index.files)) edits.push({ path, state: 'created' });
  }
  return edits.sort((left, right) => left.path.localeCompare(right.path));
}

function listManagedResearchRecordPaths(root: string): string[] {
  const paths: string[] = [];
  const flat = (directory: string, expression: RegExp): void => {
    for (const entry of readdirSync(join(root, directory), { withFileTypes: true })) {
      if (entry.isFile() && expression.test(entry.name)) paths.push(`${directory}/${entry.name}`);
    }
  };
  const nested = (directory: string, names: readonly string[]): void => {
    for (const entry of readdirSync(join(root, directory), { withFileTypes: true })) {
      if (!entry.isDirectory()) continue;
      for (const name of names) if (existsSync(join(root, directory, entry.name, name))) paths.push(`${directory}/${entry.name}/${name}`);
    }
  };
  flat('claims', /\.json$/u);
  flat('memories', /\.md$/u);
  flat('evidence', /\.json$/u);
  nested('reports', ['report.md', 'record.json']);
  nested('runbooks', ['runbook.ipynb', 'record.json']);
  nested('investigations', ['record.json']);
  nested('traces', ['summary.md']);
  for (const name of ['scope.json', 'campaign-state.json']) if (existsSync(join(root, 'references', name))) paths.push(`references/${name}`);
  return paths;
}

export function isPublishedWorkspacePath(root: string, path: string): boolean {
  const state = join(root, '.git', 'beale', 'publication.json');
  if (!existsSync(state)) return false;
  const index = JSON.parse(readFileSync(state, 'utf8')) as WorkspaceResearchIndex;
  return path in index.files || path in index.pins || path in (index.rawFiles ?? {});
}

export function workspaceFileHash(path: string): string {
  const hash = createHash('sha256');
  const handle = openSync(path, 'r');
  const buffer = Buffer.allocUnsafe(1024 * 1024);
  try {
    let bytes: number;
    while ((bytes = readSync(handle, buffer, 0, buffer.length, null)) > 0) {
      if (checkpointDeadline && Date.now() > checkpointDeadline) throw new Error('Evidence verification exceeded the checkpoint time budget; retained files were preserved.');
      hash.update(buffer.subarray(0, bytes));
    }
    return hash.digest('hex');
  } finally { closeSync(handle); }
}

export function retainWorkspaceArtifact(root: string, path: string, source: string, hash: string): number {
  if (!/^[a-f0-9]{64}$/u.test(hash)) throw new Error('Evidence requires a SHA-256 content hash.');
  const destination = join(root, path);
  assertWorkspaceChild(root, destination);
  if (existsSync(destination)) {
    if (workspaceFileHash(destination) !== hash) throw new Error(`${path}: retained evidence was modified.`);
    return statSync(destination).size;
  }
  mkdirSync(dirname(destination), { recursive: true });
  const temporary = `${destination}.${randomUUID()}.tmp`;
  try {
    copyFileSync(source, temporary);
    if (workspaceFileHash(temporary) !== hash) throw new Error('Evidence changed during retention; retry after its writer finishes.');
    renameSync(temporary, destination);
    return statSync(destination).size;
  } finally { rmSync(temporary, { force: true }); }
}

/** Move only explicitly disposable locations; the journal supports operator recovery. */
export function quarantineWorkspaceDisposable(root: string, sessionId?: string): number {
  if (!readWorkspaceProject(root)) return 0;
  return withProjectLock(root, () => {
    const paths = sessionId ? [`scratch/${sessionId}`] : ["scratch", "cache"];
    const moves: Array<{ from: string; to: string }> = [];
    let movedFileCount = 0;
    const directory = join(root, ".git", "beale", "quarantine", randomUUID());
    for (const path of paths) {
      if (sessionId && !/^[a-zA-Z0-9][a-zA-Z0-9_.-]*$/u.test(sessionId)) throw new Error("Invalid session scratch identity.");
      const source = join(root, path);
      assertWorkspaceChild(root, source);
      if (!existsSync(source) || readdirSync(source).length === 0) continue;
      const destination = join(directory, String(moves.length));
      mkdirSync(directory, { recursive: true });
      moves.push({ from: path, to: relative(root, destination).replace(/\\/gu, "/") });
      writeFileSync(join(directory, "moves.json"), JSON.stringify(moves, null, 2) + "\n");
      renameSync(source, destination);
      mkdirSync(source, { recursive: true });
      const pending = [destination];
      while (pending.length) {
        for (const entry of readdirSync(pending.pop()!, { withFileTypes: true })) {
          if (entry.isDirectory() && !entry.isSymbolicLink()) pending.push(join(entry.parentPath, entry.name));
          else movedFileCount++;
        }
      }
    }
    return movedFileCount;
  });
}

export function recoverWorkspacePublication(root: string): void {
  const state = join(root, ".git", "beale", "publication.json");
  const pending = join(dirname(state), "pending-publication.json");
  if (!existsSync(pending)) return;
  const { files, index } = JSON.parse(readFileSync(pending, "utf8")) as { files: Record<string, string>; index: WorkspaceResearchIndex };
  const previous = existsSync(state) ? JSON.parse(readFileSync(state, "utf8")) as WorkspaceResearchIndex : null;
  for (const [path, content] of Object.entries(files)) {
    const absolute = join(root, path);
    assertWorkspaceChild(root, absolute);
    if (existsSync(absolute)) {
      const hash = workspaceContentHash(readFileSync(absolute));
      if (hash !== workspaceContentHash(content) && hash !== previous?.files[path]) throw new Error(`${path}: changed during interrupted publication; recovery preserves the edit.`);
    }
  }
  for (const [path, hash] of Object.entries(previous?.files ?? {})) {
    if (path in files || !existsSync(join(root, path))) continue;
    assertWorkspaceChild(root, join(root, path));
    if (workspaceFileHash(join(root, path)) !== hash) throw new Error(`${path}: changed during interrupted publication; recovery preserves the edit.`);
  }
  finishWorkspacePublication(root, files, index, previous);
}

if (process.argv[2] === "--beale-workspace-precommit" && resolve(process.argv[1] ?? "") === fileURLToPath(import.meta.url)) {
  checkpointDeadline = Date.now() + 30_000;
  try { validateWorkspaceCommit(process.cwd()); }
  catch (error) { process.stderr.write(`Beale pre-commit: ${error instanceof Error ? error.message : String(error)}\n`); process.exitCode = 1; }
}

if (process.argv[2] === '--beale-workspace-commitmsg' && resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) {
  try {
    const path = process.argv[3];
    if (!path) throw new Error('Git commit message path is required.');
    const message = readFileSync(path, 'utf8').replace(/\r\n/gu, '\n');
    if (!git(process.cwd(), ['stripspace', '--strip-comments'], {}, message).trim()) throw new Error('A research commit requires a nonempty message.');
    writeFileSync(path, formatWorkspaceCommitMessage(message));
  } catch (error) { process.stderr.write(`Beale commit-msg: ${error instanceof Error ? error.message : String(error)}\n`); process.exitCode = 1; }
}
