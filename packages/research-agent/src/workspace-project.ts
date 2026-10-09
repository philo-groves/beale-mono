import { createHash, randomUUID } from "node:crypto";
import { closeSync, copyFileSync, existsSync, lstatSync, mkdirSync, openSync, readFileSync, readSync, readdirSync, renameSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";

export const WORKSPACE_PROJECT_VERSION = 2;
export const WORKSPACE_DIRECTORIES = ["investigations", "runbooks", "reports", "evidence", "references", "memories", "claims", "traces", "scratch", "cache"] as const;
const ROOT_FILES = new Set(["AGENTS.md", "AGENTS.override.md", "README.md", ".gitignore", "workspace.json"]);
const WORKSPACE_ROOT_INTERNAL_ENTRIES = new Set([".git", ".beale"]);
const INDEX_PATH = "references/research-index.json";
function publicationDirectory(root: string): string {
  const legacy = join(root, ".git", "beale");
  return existsSync(join(legacy, "publication.json")) ? legacy : join(root, ".beale", "publication");
}
function publicationRelativePath(root: string, name: string): string {
  return relative(root, join(publicationDirectory(root), name)).replace(/\\/gu, "/");
}
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
  /** Legacy workspace metadata; no longer schedules Git checkpoints. */
  checkpointIntervalMs?: number;
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
  status: "unchanged" | "failed" | "unmanaged";
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
  sessionId?: string;
}

export interface WorkspaceResearchEdit {
  path: string;
  state: "created" | "modified" | "deleted";
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
      else if (!/^traces\/.*(?:\.jsonl$|\/outputs\/)|(?:^|\/)evidence\/raw\//u.test(child) && workspacePathProblem(child)) health.unclassifiedFileCount++;
    }
  }
  health.partial ||= stack.length > 0;
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

/** Scans the filesystem directly so unexpected root entries remain visible to the agent. */
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

/** A dedicated research directory is never adopted from an existing source repository. */
export function initializeWorkspaceProject(root: string, workspaceId: string, adoptStagedIndex = false): WorkspaceProject {
  root = resolve(root);
  mkdirSync(root, { recursive: true });
  const existing = readWorkspaceProject(root);
  if (existing) {
    if (existing.workspaceId !== workspaceId) throw new Error("Workspace identity does not match workspace.json.");
    mkdirSync(join(root, "references", "research"), { recursive: true });
    for (const name of ['pre-commit', 'commit-msg']) {
      const hook = join(root, '.git', 'hooks', name);
      if (existsSync(hook) && readFileSync(hook, 'utf8').includes('# Beale managed research guard')) rmSync(hook);
    }
    if (workspaceResearchAuthority(root) === 'files') restoreWorkspacePublicationMetadata(root, existing, adoptStagedIndex);
    return existing;
  }
  if (existsSync(join(root, ".git"))) throw new Error("Choose a dedicated research directory; keep source repositories outside the workspace.");
  const unexpected = readdirSync(root).filter((name) => name !== ".beale" && !ROOT_FILES.has(name));
  if (unexpected.length) throw new Error("New research workspaces must be empty apart from AGENTS.md, README.md, and workspace configuration. Import reference material after creation.");
  const project: WorkspaceProject = {
    schemaVersion: WORKSPACE_PROJECT_VERSION,
    workspaceId,
    directories: WORKSPACE_DIRECTORIES,
    researchAuthority: "files",
  };
  for (const directory of WORKSPACE_DIRECTORIES) {
    mkdirSync(join(root, directory), { recursive: true });
    if (directory !== "scratch" && directory !== "cache") writeFileSync(join(root, directory, ".gitkeep"), "");
  }
  mkdirSync(join(root, "references", "research"), { recursive: true });
  if (!existsSync(join(root, "AGENTS.md"))) writeFileSync(join(root, "AGENTS.md"), WORKSPACE_INSTRUCTIONS);
  if (!existsSync(join(root, "README.md"))) writeFileSync(join(root, "README.md"), "# Beale research workspace\n\nResearch files are managed by the Beale app-server. Source repositories remain outside this directory.\n");
  atomicWorkspaceWrite(root, "workspace.json", JSON.stringify(project, null, 2) + "\n");
  publishWorkspaceFiles(root, {});
  return project;
}

/** Rebuilds only disposable publication metadata from the canonical working files. */
function restoreWorkspacePublicationMetadata(root: string, project: WorkspaceProject, adoptStagedIndex = false): void {
  const statePath = join(publicationDirectory(root), 'publication.json');
  if (existsSync(statePath) && !adoptStagedIndex) return;
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
  const contentDirectory = join(publicationDirectory(root), 'publication-content');
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
  atomicWorkspaceWrite(root, publicationRelativePath(root, 'publication.json'), JSON.stringify(index));
}

export const WORKSPACE_INSTRUCTIONS = `# Beale research workspace

This directory is one research workspace. Source repositories belong in the host-supplied external repository store.

- investigations/: stable candidate directories containing analysis, code, and fixtures; reuse identities across attempts.
- runbooks/: canonical executable procedures and self-contained reproduction packages.
- reports/: canonical reports and report-specific supporting material.
- memories/ and claims/: canonical file-authority records. Typed research tools trigger background synchronization; direct edits enter the derived index through validated import.
- evidence/: retained evidence and provenance. Cited evidence is immutable; corrections require a new artifact.
- references/: background material and the host-published research index.
- references/research/: agent-written research documentation. Keep synthesis in Markdown files here and cite canonical claims, memories, runbooks, and evidence instead of copying their bodies.
- traces/: session summaries and raw event exports.
- scratch/: disposable session experiments; cache/: rebuildable outputs and downloads.

Keep related scripts and fixtures with their investigation or runbook. Default shell execution uses session scratch; specify an external repository cwd for source work and an explicit investigation directory for persistent candidate work.
Keep generated candidate artifacts and build output in their relevant investigation or evidence directories. The app-server keeps canonical research records and the derived research index synchronized without updating a workspace Git repository.
Keep the workspace top level clean. Unexpected files and directories are detected directly by app-server; the agent is reminded every turn until it moves them into an approved directory.
Host commands retain the operator's privileges; these conventions are not filesystem isolation.
`;

function safeRelative(path: string): boolean {
  return Boolean(path) && !isAbsolute(path) && !path.includes("\\") && !path.split("/").some((part) => !part || part === "." || part === ".." || part.toLowerCase() === ".git");
}

export function workspacePathProblem(path: string): string | null {
  if (!safeRelative(path)) return "path must be a relative workspace file";
  if (!path.includes("/")) return ROOT_FILES.has(path) ? null : "root files must be AGENTS.md, AGENTS.override.md, README.md, .gitignore, or workspace.json";
  const parts = path.split("/");
  if (!(WORKSPACE_DIRECTORIES as readonly string[]).includes(parts[0]!)) return "file must belong to an approved research directory";
  if (parts[0] === "scratch" || parts[0] === "cache") return "disposable files are outside canonical research";
  if (parts.some((part) => /^(?:node_modules|\.env(?:\..*)?|credentials?(?:\..*)?|id_rsa|id_ed25519)$/iu.test(part)) || /\.(?:sqlite(?:-wal|-shm)?|db|pem|key|p12|pfx)$/iu.test(path)) return "runtime databases and credential material are outside canonical research";
  if (/(?:^|\/)evidence\/raw\//u.test(path) || (parts[0] === "traces" && (parts.includes("outputs") || /\.jsonl$/iu.test(path)))) return "raw captures are outside canonical research";
  return null;
}

function withProjectLock<T>(root: string, operation: () => T): T {
  const lock = join(root, ".beale", "maintenance.lock");
  mkdirSync(dirname(lock), { recursive: true });
  if (existsSync(lock)) {
    const owner = Number(readFileSync(lock, "utf8"));
    if (Number.isInteger(owner) && owner > 0) {
      try { process.kill(owner, 0); }
      catch (error) {
        if ((error as NodeJS.ErrnoException).code === "ESRCH") {
          rmSync(lock);
        }
      }
    }
  }
  let descriptor: number;
  try { descriptor = openSync(lock, "wx"); } catch { throw new Error("Another workspace maintenance operation is in progress. Retry after it finishes."); }
  try { writeFileSync(descriptor, String(process.pid)); return operation(); }
  finally { closeSync(descriptor); rmSync(lock); }
}

/** Preserve the workspace publication boundary without updating a local Git repository. */
export function checkpointWorkspace(root: string, reason: string, publish?: () => void): WorkspaceCheckpointResult {
  if (!readWorkspaceProject(root)) return { status: "unmanaged", reason };
  try {
    publish?.();
    return { status: "unchanged", reason };
  } catch (error) {
    return { status: "failed", reason, error: error instanceof Error ? error.message : String(error) };
  }
}

export function writeCheckpointStatus(root: string, result: WorkspaceCheckpointResult): void {
  const directory = join(root, ".beale", "publication");
  mkdirSync(directory, { recursive: true });
  atomicWorkspaceWrite(root, '.beale/publication/checkpoint.json', JSON.stringify({ ...result, updatedAt: new Date().toISOString() }, null, 2) + "\n");
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

/** Recovery copies are content-addressed; edits never erase the prior bytes. */
export function preserveWorkspaceFile(root: string, path: string): void {
  if (!readWorkspaceProject(root) || !existsSync(path)) return;
  const child = assertWorkspaceChild(root, path);
  const bytes = readFileSync(path);
  const hash = workspaceContentHash(bytes);
  const directory = join(publicationDirectory(root), "recovery");
  mkdirSync(directory, { recursive: true });
  const target = join(directory, hash);
  if (!existsSync(target)) writeFileSync(target, bytes, { flag: "wx", mode: 0o600 });
  writeFileSync(join(directory, `${Date.now()}-${randomUUID()}.json`), JSON.stringify({ path: child, hash }) + "\n");
}

export function publishWorkspaceFiles(root: string, files: Record<string, string>, pins: Record<string, string> = {}, rawFiles: Record<string, string> = {}, trustedEditedHashes: ReadonlyMap<string, string> = new Map()): void {
  const project = readWorkspaceProject(root);
  if (!project) return;
  const state = join(publicationDirectory(root), "publication.json");
  const previous = existsSync(state) ? JSON.parse(readFileSync(state, "utf8")) as WorkspaceResearchIndex : null;
  const publishedFiles = { ...files };
  // A completed publication makes pinned evidence immutable. Publication may
  // render database paths differently after a referenced workspace file is
  // revised, but it must not rewrite the already-published evidence snapshot.
  const pinnedReplacements = Object.entries(previous?.pins ?? {}).flatMap(([path, hash]) => {
    if (!(path in publishedFiles) || workspaceContentHash(publishedFiles[path]!) === hash) return [];
    return [[path, hash] as const];
  });
  if (pinnedReplacements.length > 0) {
    for (const [path, hash] of pinnedReplacements) {
      const publishedContent = join(publicationDirectory(root), 'publication-content', hash);
      const content = existsSync(publishedContent) ? readFileSync(publishedContent, 'utf8') : undefined;
      if (content === undefined || workspaceContentHash(content) !== hash) {
        throw new Error(`${path}: published evidence does not match its canonical pin.`);
      }
      publishedFiles[path] = content;
    }
  }
  if (previous && (!existsSync(join(root, INDEX_PATH)) || readFileSync(join(root, INDEX_PATH), 'utf8') !== JSON.stringify(previous, null, 2) + '\n')) throw new Error('The published research index was edited or removed; preserve the edit before republishing.');
  // Verify the entire previous publication before changing any file. Typed snapshots
  // may differ from the last publication only while their verified bytes are intact.
  for (const [path, hash] of Object.entries(previous?.files ?? {})) {
    const absolute = join(root, path);
    assertWorkspaceChild(root, absolute);
    if (!existsSync(absolute)) throw new Error(`${path}: canonical export was edited or removed; preserve/import the edit before republishing.`);
    const currentHash = workspaceContentHash(readFileSync(absolute));
    if (currentHash !== hash && (!(path in publishedFiles) || trustedEditedHashes.get(path) !== currentHash)) {
      throw new Error(`${path}: canonical export was edited or removed; preserve/import the edit before republishing.`);
    }
  }
  const hashes: Record<string, string> = {};
  for (const [path, content] of Object.entries(publishedFiles).sort(([a], [b]) => a.localeCompare(b))) {
    if (workspacePathProblem(path)) throw new Error(`Invalid canonical export path: ${path}`);
    const destination = join(root, path);
    if (existsSync(destination) && !previous?.files[path] && readFileSync(destination, "utf8") !== content
      && trustedEditedHashes.get(path) !== workspaceContentHash(readFileSync(destination))) {
      throw new Error(`${path}: an existing file conflicts with canonical publication.`);
    }
    hashes[path] = workspaceContentHash(content);
  }
  const index: WorkspaceResearchIndex = { schemaVersion: 1, workspaceId: project.workspaceId, files: hashes, pins: { ...pins, ...previous?.pins }, rawFiles: { ...previous?.rawFiles, ...rawFiles } };
  // Persist a recovery journal before publishing. A restart can finish an interrupted publication.
  const directory = dirname(state);
  mkdirSync(directory, { recursive: true });
  atomicWorkspaceWrite(root, publicationRelativePath(root, 'pending-publication.json'), JSON.stringify({ files: publishedFiles, index }));
  finishWorkspacePublication(root, publishedFiles, index, previous);
}

function finishWorkspacePublication(root: string, files: Record<string, string>, index: WorkspaceResearchIndex, previous: WorkspaceResearchIndex | null): void {
  const contentDirectory = join(publicationDirectory(root), "publication-content");
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
  const state = join(publicationDirectory(root), "publication.json");
  atomicWorkspaceWrite(root, publicationRelativePath(root, 'publication.json'), JSON.stringify(index));
  rmSync(join(dirname(state), "pending-publication.json"), { force: true });
}

export function readPublishedWorkspaceFile(root: string, path: string): string {
  assertWorkspaceChild(root, join(root, path));
  const index = JSON.parse(readFileSync(join(publicationDirectory(root), "publication.json"), "utf8")) as WorkspaceResearchIndex;
  const hash = index.files[path];
  if (!hash || !/^[a-f0-9]{64}$/u.test(hash)) throw new Error("File is not a published canonical research record.");
  return readFileSync(join(publicationDirectory(root), "publication-content", hash), "utf8");
}

/** Returns only edits to app-server-managed research files; ordinary file-native work is not included. */
export function listWorkspaceResearchEdits(root: string): WorkspaceResearchEdit[] {
  if (workspaceResearchAuthority(root) !== "files") return [];
  const statePath = join(publicationDirectory(root), "publication.json");
  const indexPath = join(root, INDEX_PATH);
  if (!existsSync(statePath) || !existsSync(indexPath)) {
    throw new Error("File-authority workspace metadata is incomplete; restore the research index before continuing.");
  }
  const index = JSON.parse(readFileSync(statePath, "utf8")) as WorkspaceResearchIndex;
  if (readFileSync(indexPath, "utf8") !== JSON.stringify(index, null, 2) + "\n") {
    throw new Error("The research index was edited directly; restore it before importing individual research files.");
  }
  const edits: WorkspaceResearchEdit[] = [];
  const managed = { ...index.pins, ...index.files };
  for (const [path, hash] of Object.entries(managed)) {
    const absolute = join(root, path);
    assertWorkspaceChild(root, absolute);
    if (!existsSync(absolute)) edits.push({ path, state: "deleted" });
    else if (workspaceFileHash(absolute) !== hash) edits.push({ path, state: "modified" });
  }
  for (const path of listManagedResearchRecordPaths(root)) {
    if (!(path in managed)) edits.push({ path, state: 'created' });
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
  // Evidence manifests become canonical only after publication pins them in
  // the research index. A researcher may place candidate verifier output in
  // evidence/ before citing it; treating every JSON file there as a newly
  // created canonical record makes that candidate poison every later session
  // publication. Published evidence is already covered by `managed` above.
  nested('reports', ['report.md', 'record.json']);
  nested('runbooks', ['runbook.ipynb', 'record.json']);
  nested('investigations', ['record.json']);
  nested('traces', ['summary.md']);
  for (const name of ['scope.json', 'campaign-state.json']) if (existsSync(join(root, 'references', name))) paths.push(`references/${name}`);
  const topicsDirectory = join(root, 'references', 'topics');
  if (existsSync(topicsDirectory)) {
    for (const entry of readdirSync(topicsDirectory, { withFileTypes: true })) {
      if (entry.isFile() && /^[a-zA-Z0-9][a-zA-Z0-9_.-]*\.json$/u.test(entry.name)) paths.push(`references/topics/${entry.name}`);
    }
  }
  return paths;
}

export function isPublishedWorkspacePath(root: string, path: string): boolean {
  const state = join(publicationDirectory(root), 'publication.json');
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
    const directory = join(root, ".beale", "quarantine", randomUUID());
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
  const state = join(publicationDirectory(root), "publication.json");
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
