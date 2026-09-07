import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { stat } from "node:fs/promises";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { promisify } from "node:util";
import { nowIso } from "./ids.js";
import { researchKitFirstTouchGuidance } from "./research-kit-guidance.js";
import type {
  ResearchExecutableTool,
  ResearchToolExecutionResult,
} from "./tool-registry.js";
import type { ResearchToolAction } from "./types.js";
export { createPriorArtSearchTool, type PriorArtSearchToolOptions } from "./prior-art-tools.js";

const execFileAsync = promisify(execFile);
const DEFAULT_HISTORY_RESULTS = 20;
const DEFAULT_HISTORY_MAX_BYTES = 32_000;

export interface RepositoryIdentity {
  root: string;
  head: string;
  branch: string | null;
  shallow: boolean;
  remotes: Array<{ name: string; url: string }>;
}

export interface RepositoryFirstTouchNotice {
  firstTouch: true;
  repository: RepositoryIdentity;
  reminder: readonly string[];
  scopeReview?: Record<string, unknown>;
}

export interface RepositoryResearchSessionOptions {
  researchKitId?: string;
  beforeFirstTouch?: (repository: RepositoryIdentity) => Promise<{
    approved: boolean;
    firstTouch: boolean;
    details?: Record<string, unknown>;
  }>;
}

/** Session-scoped state shared by repository-aware tools and agents. */
export class RepositoryResearchSession {
  readonly #identities: RepositoryIdentity[] = [];
  readonly #lookups = new Map<string, Promise<RepositoryIdentity | null>>();
  readonly #touched = new Set<string>();
  readonly #headChecks = new Map<string, { checkedAt: number; identity: RepositoryIdentity }>();

  public constructor(private readonly options: RepositoryResearchSessionOptions = {}) {}

  public invalidate(root: string): void {
    const index = this.#identities.findIndex((identity) => identity.root === root);
    if (index >= 0) this.#identities.splice(index, 1);
    this.#headChecks.delete(root);
    this.#lookups.clear();
  }

  public async identify(path: string): Promise<RepositoryIdentity | null> {
    const absolutePath = resolve(path);
    const known = this.#identities.find((identity) => containedBy(identity.root, absolutePath));
    if (known) return known;
    const start = await directoryForPath(absolutePath);
    const existing = this.#lookups.get(start);
    if (existing) return existing;
    const lookup = inspectRepository(start).then((identity) => {
      if (identity && !this.#identities.some((candidate) => candidate.root === identity.root)) {
        this.#identities.push(identity);
      }
      return identity;
    });
    this.#lookups.set(start, lookup);
    const identity = await lookup;
    if (!identity) this.#lookups.delete(start);
    return identity;
  }

  public async touch(path: string): Promise<RepositoryFirstTouchNotice | null> {
    const identified = await this.identify(path);
    if (!identified) return null;
    const repository = await this.refreshIdentity(identified);
    const key = `${repository.root}\n${repository.head}`;
    if (this.#touched.has(key)) return null;
    if (this.options.beforeFirstTouch) {
      const review = await this.options.beforeFirstTouch(repository);
      if (!review.approved) return null;
      this.#touched.add(key);
      if (!review.firstTouch) return null;
      return {
        firstTouch: true,
        repository,
        reminder: repositoryHistoryReminder(this.options.researchKitId),
        ...(review.details ? { scopeReview: review.details } : {}),
      };
    }
    this.#touched.add(key);
    return {
      firstTouch: true,
      repository,
      reminder: repositoryHistoryReminder(this.options.researchKitId),
    };
  }

  private async refreshIdentity(identity: RepositoryIdentity): Promise<RepositoryIdentity> {
    const cached = this.#headChecks.get(identity.root);
    if (cached && Date.now() - cached.checkedAt < 1_000) return cached.identity;
    const head = await tryRunGit(identity.root, ["rev-parse", "HEAD"]);
    let current = identity;
    if (head && head !== identity.head) {
      current = await inspectRepository(identity.root) ?? identity;
      const index = this.#identities.findIndex((candidate) => candidate.root === identity.root);
      if (index >= 0) this.#identities[index] = current;
    }
    this.#headChecks.set(identity.root, { checkedAt: Date.now(), identity: current });
    return current;
  }
}

function repositoryHistoryReminder(researchKitId?: string): readonly string[] {
  return [
    "Record the repository origin, HEAD, tags or release identity, upstream relationship, and whether history is shallow.",
    "Inspect path history, blame, security-relevant fix commits, upstream changes, vendor forks or source drops, and version-to-version differences before treating the snapshot as novel.",
    "Use prior_art.search for NVD/OSV records, public GitHub issue and release history, and specific document URLs. Read vendor bulletins, release notes, security-content pages, and mailing-list archive links with prior_art.fetch; these tools do not search the entire web or crawl sites.",
    "For vendor-maintained components, include official source releases and upstream project history, then compare source drops or tags against the researched build when available.",
    "When local history is incomplete, repository.fetch_history can explicitly fetch a named configured remote or deepen a shallow clone without checking out files. Record any remote or branch not fetched as a coverage limit.",
    ...researchKitFirstTouchGuidance(researchKitId),
    "Follow returned continuation cursors and record searched sources, dates, incomplete coverage, errors, and deferred sources separately from completed no-match queries.",
  ];
}

export interface RepositoryHistoryToolOptions {
  roots?: readonly string[];
  researchSession?: RepositoryResearchSession;
  maxBytes?: number;
}

const REPOSITORY_HISTORY_PARAMETERS = {
  type: "object",
  required: ["operation"],
  properties: {
    operation: {
      type: "string",
      enum: ["overview", "log", "blame", "search_changes", "diff"],
    },
    root: {
      type: "string",
      description: "Configured root label/path, an absolute repository path, or a child repository path under a configured root.",
    },
    path: { type: "string", description: "Optional repository-relative path." },
    query: { type: "string", description: "Required for search_changes; searched as a literal added or removed string." },
    from: { type: "string", description: "Required for diff." },
    to: { type: "string", description: "Required for diff." },
    startLine: { type: "number", minimum: 1 },
    endLine: { type: "number", minimum: 1 },
    maxResults: { type: "number", minimum: 1, maximum: 100 },
  },
};

export function createRepositoryHistoryTool(
  options: RepositoryHistoryToolOptions = {},
): ResearchExecutableTool {
  const roots = uniquePaths(options.roots ?? []);
  const researchSession = options.researchSession ?? new RepositoryResearchSession();
  return {
    descriptor: {
      name: "repository.history",
      transportName: "repository_history",
      description: "Inspect local Git provenance and change history without modifying the worktree. Supports repository overview, path log, line blame, literal change search, and revision diff. Reports shallow history explicitly.",
      actionClasses: ["search", "inspect"],
      sideEffects: "read",
      requiredPermissions: ["filesystem:read"],
      inputSchema: REPOSITORY_HISTORY_PARAMETERS,
      artifactLocations: roots,
      metadata: { provider: "appServer.built_in", safetyProfile: "host-filesystem-read" },
    },
    parameters: REPOSITORY_HISTORY_PARAMETERS as NonNullable<ResearchExecutableTool["parameters"]>,
    async execute(action) {
      return completeOrError(action, async () => {
        const input = action.input;
        const operation = requiredEnum(input.operation, "operation", ["overview", "log", "blame", "search_changes", "diff"] as const);
        const root = await resolveHistoryRepository(roots, optionalString(input.root), researchSession);
        const repository = await researchSession.identify(root);
        if (!repository) throw new Error(`repository.history requires a Git worktree: ${root}`);
        const path = normalizeRepositoryPath(repository.root, optionalString(input.path));
        const maxResults = boundedInteger(input.maxResults, DEFAULT_HISTORY_RESULTS, 1, 100);
        const maxBytes = options.maxBytes ?? DEFAULT_HISTORY_MAX_BYTES;
        const firstTouch = await researchSession.touch(repository.root);
        const output = await executeHistoryOperation({
          operation,
          repository,
          path,
          query: optionalString(input.query),
          from: optionalString(input.from),
          to: optionalString(input.to),
          startLine: boundedOptionalInteger(input.startLine, 1),
          endLine: boundedOptionalInteger(input.endLine, 1),
          maxResults,
          maxBytes,
        });
        const projected = {
          ...output,
          ...(firstTouch ? { repositoryFirstTouch: firstTouch } : {}),
        };
        return result(action, `${operation} history inspection completed for ${basename(repository.root)}.`, projected, firstTouch
          ? ["Complete the repository-first-touch provenance, advisory, release-note, upstream, and vendor-source history baseline before broad exploration."]
          : []);
      });
    },
  };
}

export interface RepositoryFetchHistoryInput {
  root?: string;
  remote: string;
  ref?: string;
  deepen?: number;
  unshallow?: boolean;
}

export function createRepositoryFetchHistoryTool(options: RepositoryHistoryToolOptions = {}): ResearchExecutableTool {
  const session = options.researchSession ?? new RepositoryResearchSession();
  const parameters = {
    type: "object", required: ["remote"], properties: {
      root: { type: "string" },
      remote: { type: "string", description: "Existing configured remote name; never an arbitrary URL or new remote." },
      ref: { type: "string", description: "Optional branch, tag, or commit to fetch. Ref mappings and force prefixes are not accepted." },
      deepen: { type: "integer", minimum: 1, maximum: 100_000, description: "Additional history depth for a shallow clone; mutually exclusive with unshallow." },
      unshallow: { type: "boolean", description: "Fetch the available full history for a shallow clone." },
    },
  };
  return {
    descriptor: { name: "repository.fetch_history", transportName: "repository_fetch_history", description: "Explicitly fetch history from a named configured remote, optionally deepening or unshallowing the clone. Updates Git objects and remote-tracking metadata without checkout, merge, reset, submodule recursion, or changing worktree files.", actionClasses: ["inspect"], sideEffects: "network", requiredPermissions: ["filesystem:write", "network:repository:read"], inputSchema: parameters },
    parameters: parameters as NonNullable<ResearchExecutableTool["parameters"]>,
    async execute(action, context) {
      return completeOrError(action, async () => {
        context?.signal?.throwIfAborted();
        const root = await resolveHistoryRepository(options.roots ?? [], optionalString(action.input.root), session);
        const before = await inspectRepository(root);
        if (!before) throw new Error("A Git worktree is required.");
        const remote = requiredString(action.input.remote, "remote");
        if (!before.remotes.some((candidate) => candidate.name === remote)) throw new Error("remote must name an existing configured repository remote.");
        const ref = optionalString(action.input.ref);
        if (ref && (ref.startsWith("-") || /[:*?\[\]\\\s~^+]/.test(ref) || ref.includes("..") || ref.includes("@{"))) throw new Error("ref must be one branch, tag, or commit without ref mappings or force prefixes.");
        const deepen = action.input.deepen;
        if (deepen !== undefined && (typeof deepen !== "number" || !Number.isSafeInteger(deepen) || deepen < 1 || deepen > 100_000)) throw new Error("deepen must be an integer between 1 and 100000.");
        const unshallow = action.input.unshallow === true;
        if (deepen !== undefined && unshallow) throw new Error("Choose deepen or unshallow, not both.");
        if ((deepen !== undefined || unshallow) && !before.shallow) throw new Error("This repository is not shallow; omit deepen and unshallow.");
        const refspec = ref ? `${ref}:refs/beale/history/${createHash("sha256").update(`${remote}\n${ref}`).digest("hex").slice(0, 24)}`
          : `refs/heads/*:refs/remotes/${remote}/*`;
        const args = ["fetch", "--no-recurse-submodules", "--no-tags", "--no-write-fetch-head", "--no-auto-maintenance", "--refmap=",
          ...(typeof deepen === "number" ? [`--deepen=${deepen}`] : []), ...(unshallow ? ["--unshallow"] : []), "--", remote, refspec];
        try {
          await runGit(root, args, context?.signal);
        } catch {
          context?.signal?.throwIfAborted();
          // Git diagnostics may echo credential-bearing remote configuration.
          throw new Error("Git history fetch failed or timed out. Check the named remote's availability and authentication on the host.");
        } finally {
          // Fetch may change the shallow boundary without changing HEAD, even on a partial failure.
          session.invalidate(root);
        }
        const after = await inspectRepository(root);
        if (!after) throw new Error("Repository identity became unavailable after fetching history.");
        return result(action, "Repository history fetched.", {
          remote, ref, before: { head: before.head, shallow: before.shallow }, after: { head: after.head, shallow: after.shallow },
          fetchedRef: ref ? refspec.slice(refspec.indexOf(":") + 1) : `refs/remotes/${remote}/*`,
          scope: ref ? "requested_ref" : "remote_branches",
          caveat: "Coverage is limited to the named remote and requested refs; other upstream branches or source drops may remain absent.",
        }, []);
      });
    },
  };
}

interface HistoryInput {
  operation: "overview" | "log" | "blame" | "search_changes" | "diff";
  repository: RepositoryIdentity;
  path: string | null;
  query: string | null;
  from: string | null;
  to: string | null;
  startLine: number | null;
  endLine: number | null;
  maxResults: number;
  maxBytes: number;
}

async function executeHistoryOperation(input: HistoryInput): Promise<Record<string, unknown>> {
  const common = { repository: input.repository, path: input.path };
  if (input.operation === "overview") {
    const recent = parseCommits(await runGit(input.repository.root, [
      "log", `--max-count=${input.maxResults}`, "--date=iso-strict", "--format=%H%x1f%h%x1f%cI%x1f%an%x1f%s%x1e",
    ]));
    const tags = parseTags(await runGit(input.repository.root, [
      "for-each-ref", "--sort=-creatordate", `--count=${input.maxResults}`, "--format=%(refname:short)%1f%(creatordate:iso-strict)%1f%(objectname)", "refs/tags",
    ]));
    return { operation: input.operation, ...common, recentCommits: recent, tags };
  }
  if (input.operation === "log") {
    const args = ["log", "--all", `--max-count=${input.maxResults}`, "--date=iso-strict", "--format=%H%x1f%h%x1f%cI%x1f%an%x1f%s%x1e"];
    if (input.path) args.push("--", input.path);
    return { operation: input.operation, ...common, commits: parseCommits(await runGit(input.repository.root, args)) };
  }
  if (input.operation === "blame") {
    if (!input.path) throw new Error("repository.history blame requires path.");
    const args = ["blame", "--line-porcelain"];
    if (input.startLine || input.endLine) {
      const start = input.startLine ?? input.endLine ?? 1;
      const end = input.endLine ?? start;
      if (end < start) throw new Error("endLine must be greater than or equal to startLine.");
      args.push("-L", `${start},${end}`);
    }
    args.push("--", input.path);
    return { operation: input.operation, ...common, blame: bounded(await runGit(input.repository.root, args), input.maxBytes) };
  }
  if (input.operation === "search_changes") {
    if (!input.query) throw new Error("repository.history search_changes requires query.");
    const args = ["log", "--all", `--max-count=${input.maxResults}`, "--date=iso-strict", "--format=commit %H%nDate: %cI%nAuthor: %an%nSubject: %s", "-p", "-S", input.query];
    if (input.path) args.push("--", input.path);
    return { operation: input.operation, ...common, query: input.query, changes: bounded(await runGit(input.repository.root, args), input.maxBytes) };
  }
  if (!input.from || !input.to) throw new Error("repository.history diff requires from and to revisions.");
  const from = await resolveCommit(input.repository.root, input.from, "from");
  const to = await resolveCommit(input.repository.root, input.to, "to");
  const args = ["diff", "--find-renames", "--stat", from, to];
  if (input.path) args.push("--", input.path);
  const statOutput = await runGit(input.repository.root, args);
  const patchArgs = ["diff", "--find-renames", "--unified=3", from, to];
  if (input.path) patchArgs.push("--", input.path);
  return {
    operation: input.operation,
    ...common,
    from,
    to,
    stat: bounded(statOutput, Math.min(input.maxBytes, 8_000)),
    patch: bounded(await runGit(input.repository.root, patchArgs), input.maxBytes),
  };
}

async function resolveCommit(root: string, revision: string, field: string): Promise<string> {
  try {
    return await runGit(root, ["rev-parse", "--verify", "--quiet", "--end-of-options", `${revision}^{commit}`]);
  } catch {
    throw new Error(`repository.history ${field} must resolve to a commit.`);
  }
}

async function resolveHistoryRepository(
  roots: readonly string[],
  requested: string | null,
  session: RepositoryResearchSession,
): Promise<string> {
  const candidates: string[] = [];
  if (requested) {
    if (isAbsolute(requested)) candidates.push(requested);
    for (const root of roots) {
      if (root === requested || basename(root).toLowerCase() === requested.toLowerCase()) candidates.push(root);
      candidates.push(join(root, requested));
    }
  } else {
    candidates.push(...roots);
  }
  for (const candidate of uniquePaths(candidates)) {
    const identity = await session.identify(candidate);
    if (identity) return identity.root;
  }
  if (!requested && roots.length > 1) {
    throw new Error("repository.history root is required when multiple repository contexts are configured.");
  }
  throw new Error(`repository.history could not resolve a Git worktree${requested ? ` for: ${requested}` : ""}.`);
}

async function inspectRepository(start: string): Promise<RepositoryIdentity | null> {
  const root = await tryRunGit(start, ["rev-parse", "--show-toplevel"]);
  if (!root) return null;
  const [head, branch, shallow, remoteOutput] = await Promise.all([
    tryRunGit(root, ["rev-parse", "HEAD"]),
    tryRunGit(root, ["symbolic-ref", "--quiet", "--short", "HEAD"]),
    tryRunGit(root, ["rev-parse", "--is-shallow-repository"]),
    tryRunGit(root, ["remote", "-v"]),
  ]);
  if (!head) return null;
  const remotes = uniqueRemoteLines(remoteOutput ?? "");
  return { root: resolve(root), head, branch, shallow: shallow === "true", remotes };
}

async function runGit(root: string, args: readonly string[], signal?: AbortSignal): Promise<string> {
  const { stdout } = await execFileAsync("git", ["-c", `safe.directory=${root}`, "-C", root, ...args], {
    encoding: "utf8",
    maxBuffer: 2 * 1024 * 1024,
    timeout: 30_000,
    windowsHide: true,
    ...(signal ? { signal } : {}),
  });
  return stdout.trim();
}

async function tryRunGit(root: string, args: readonly string[]): Promise<string | null> {
  try {
    const output = await runGit(root, args);
    return output || null;
  } catch {
    return null;
  }
}

function parseCommits(value: string): Array<{ hash: string; shortHash: string; committedAt: string; author: string; subject: string }> {
  return value.split("\x1e").flatMap((entry) => {
    const fields = entry.trim().split("\x1f");
    const [hash, shortHash, committedAt, author, subject] = fields;
    return hash && shortHash && committedAt && author && subject
      ? [{ hash, shortHash, committedAt, author, subject }]
      : [];
  });
}

function parseTags(value: string): Array<{ name: string; createdAt: string; object: string }> {
  return value.split(/\r?\n/).flatMap((entry) => {
    const [name, createdAt, object] = entry.trim().split("\x1f");
    return name && createdAt && object ? [{ name, createdAt, object }] : [];
  });
}

function uniqueRemoteLines(value: string): Array<{ name: string; url: string }> {
  const seen = new Set<string>();
  return value.split(/\r?\n/).flatMap((line) => {
    const match = /^(\S+)\s+(\S+)\s+\((?:fetch|push)\)$/.exec(line.trim());
    if (!match?.[1] || !match[2]) return [];
    const key = `${match[1]}\n${match[2]}`;
    if (seen.has(key)) return [];
    seen.add(key);
    return [{ name: match[1], url: match[2] }];
  });
}

async function directoryForPath(path: string): Promise<string> {
  const pathStat = await stat(path).catch(() => null);
  return pathStat?.isFile() ? dirname(path) : path;
}

function normalizeRepositoryPath(root: string, path: string | null): string | null {
  if (!path) return null;
  const absolute = isAbsolute(path) ? resolve(path) : resolve(root, path);
  if (!containedBy(root, absolute)) throw new Error("repository.history path must remain inside the selected repository.");
  return relative(root, absolute).split(sep).join("/") || ".";
}

function containedBy(root: string, candidate: string): boolean {
  const value = relative(resolve(root), resolve(candidate));
  return value === "" || (value !== ".." && !value.startsWith(`..${sep}`) && !isAbsolute(value));
}

function uniquePaths(values: readonly string[]): string[] {
  return [...new Set(values.filter(Boolean).map((value) => resolve(value)))];
}

function optionalString(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function requiredString(value: unknown, field: string): string {
  const result = optionalString(value);
  if (!result) throw new Error(`${field} must be a non-empty string.`);
  return result;
}

function requiredEnum<const T extends readonly string[]>(value: unknown, field: string, allowed: T): T[number] {
  if (typeof value === "string" && allowed.includes(value)) return value as T[number];
  throw new Error(`${field} must be one of: ${allowed.join(", ")}.`);
}

function boundedInteger(value: unknown, fallback: number, minimum: number, maximum: number): number {
  if (typeof value !== "number" || !Number.isFinite(value)) return fallback;
  return Math.max(minimum, Math.min(maximum, Math.floor(value)));
}

function boundedOptionalInteger(value: unknown, minimum: number): number | null {
  return typeof value === "number" && Number.isFinite(value) ? Math.max(minimum, Math.floor(value)) : null;
}

function bounded(value: string, maxCharacters: number): string {
  return value.length <= maxCharacters ? value : `${value.slice(0, Math.max(0, maxCharacters - 1))}…`;
}

function errorMessage(error: unknown): string {
  if (error instanceof Error && error.name === "AbortError") return "Public advisory search timed out or was interrupted.";
  return error instanceof Error ? error.message : String(error);
}

async function completeOrError(
  action: ResearchToolAction,
  run: () => Promise<ResearchToolExecutionResult>,
): Promise<ResearchToolExecutionResult> {
  try {
    return await run();
  } catch (error) {
    const timestamp = nowIso();
    return {
      action,
      status: "error",
      startedAt: timestamp,
      completedAt: timestamp,
      summary: `${action.toolName} failed.`,
      error: { message: errorMessage(error) },
      followUpActions: [],
    };
  }
}

function result(
  action: ResearchToolAction,
  summary: string,
  output: unknown,
  followUpActions: readonly string[],
): ResearchToolExecutionResult {
  const timestamp = nowIso();
  return {
    action,
    status: "complete",
    startedAt: timestamp,
    completedAt: timestamp,
    summary,
    output,
    modelOutput: output,
    artifactRefs: [],
    followUpActions,
  };
}
