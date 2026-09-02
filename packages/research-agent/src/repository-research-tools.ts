import { execFile } from "node:child_process";
import { stat } from "node:fs/promises";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { promisify } from "node:util";
import { nowIso } from "./ids.js";
import type {
  ResearchExecutableTool,
  ResearchToolExecutionResult,
} from "./tool-registry.js";
import type { ResearchToolAction } from "./types.js";

const execFileAsync = promisify(execFile);
const DEFAULT_HISTORY_RESULTS = 20;
const DEFAULT_HISTORY_MAX_BYTES = 32_000;
const DEFAULT_PRIOR_ART_RESULTS = 20;
const DEFAULT_PRIOR_ART_TIMEOUT_MS = 20_000;
const PRIOR_ART_SOURCES = ["nvd", "osv"] as const;

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
        reminder: repositoryHistoryReminder(),
        ...(review.details ? { scopeReview: review.details } : {}),
      };
    }
    this.#touched.add(key);
    return {
      firstTouch: true,
      repository,
      reminder: repositoryHistoryReminder(),
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

function repositoryHistoryReminder(): readonly string[] {
  return [
    "Record the repository origin, HEAD, tags or release identity, upstream relationship, and whether history is shallow.",
    "Inspect path history, blame, security-relevant fix commits, upstream changes, vendor forks or source drops, and version-to-version differences before treating the snapshot as novel.",
    "Search public CVEs, advisories, vendor security bulletins, release notes, security-content pages, and referenced fixes with prior_art.search; use repository, component, service, binary, package, and symbol aliases.",
    "For Apple components, include relevant Apple Open Source releases and upstream project history, then compare source drops or tags against the researched build when available.",
    "Record matches, likely variants, explicit no-match queries with dates, and deferred sources whose absence limits the novelty assessment.",
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

export interface PriorArtSearchToolOptions {
  fetch?: typeof fetch;
  timeoutMs?: number;
}

const PRIOR_ART_SEARCH_PARAMETERS = {
  type: "object",
  required: ["query"],
  properties: {
    query: { type: "string", description: "Component, repository, protocol, symbol, bug class, or advisory identifier." },
    aliases: { type: "array", items: { type: "string" }, maxItems: 3 },
    sources: { type: "array", items: { type: "string", enum: PRIOR_ART_SOURCES } },
    package: {
      type: "object",
      required: ["name", "ecosystem"],
      properties: { name: { type: "string" }, ecosystem: { type: "string" }, version: { type: "string" } },
    },
    commit: { type: "string", description: "Git commit hash for OSV commit lookup." },
    maxResults: { type: "number", minimum: 1, maximum: 50 },
  },
};

export function createPriorArtSearchTool(
  options: PriorArtSearchToolOptions = {},
): ResearchExecutableTool {
  const request = options.fetch ?? globalThis.fetch;
  return {
    descriptor: {
      name: "prior_art.search",
      transportName: "prior_art_search",
      description: "Search public vulnerability records through NVD keyword search and structured OSV package or commit lookup. Results are research leads with source URLs, not proof that the current revision is affected.",
      actionClasses: ["search", "inspect"],
      sideEffects: "network",
      requiredPermissions: ["network:public-advisory:read"],
      inputSchema: PRIOR_ART_SEARCH_PARAMETERS,
      metadata: { provider: "appServer.built_in", safetyProfile: "public-advisory-read" },
    },
    parameters: PRIOR_ART_SEARCH_PARAMETERS as NonNullable<ResearchExecutableTool["parameters"]>,
    async execute(action, context) {
      return completeOrError(action, async () => {
        if (!request) throw new Error("prior_art.search requires the host fetch runtime.");
        const query = requiredString(action.input.query, "query");
        const aliases = stringArray(action.input.aliases).slice(0, 3);
        const packageQuery = optionalRecord(action.input.package);
        const commit = optionalString(action.input.commit);
        const requestedSources = stringArray(action.input.sources);
        const sources = requestedSources.length > 0
          ? requestedSources.map((source) => requiredEnum(source, "sources[]", PRIOR_ART_SOURCES))
          : ["nvd" as const, ...(packageQuery || commit ? ["osv" as const] : [])];
        const maxResults = boundedInteger(action.input.maxResults, DEFAULT_PRIOR_ART_RESULTS, 1, 50);
        const controller = new AbortController();
        const timeout = setTimeout(() => controller.abort(), options.timeoutMs ?? DEFAULT_PRIOR_ART_TIMEOUT_MS);
        const abort = () => controller.abort();
        context?.signal?.addEventListener("abort", abort, { once: true });
        try {
          const searches: Promise<PriorArtSourceResult>[] = [];
          if (sources.includes("nvd")) {
            searches.push(searchNvd(request, [query, ...aliases], maxResults, controller.signal));
          }
          if (sources.includes("osv")) {
            if (!packageQuery && !commit) {
              searches.push(Promise.resolve({ source: "osv", records: [], error: "OSV requires package or commit input; keyword-only OSV search was skipped." }));
            } else {
              searches.push(searchOsv(request, packageQuery, commit, maxResults, controller.signal));
            }
          }
          const sourceResults = await Promise.all(searches);
          const records = dedupePriorArt(sourceResults.flatMap((source) => source.records)).slice(0, maxResults);
          if (records.length === 0 && sourceResults.every((source) => source.error)) {
            throw new Error(sourceResults.map((source) => `${source.source}: ${source.error}`).join("; "));
          }
          const output = {
            query,
            aliases,
            sources: sourceResults.map((source) => ({
              source: source.source,
              resultCount: source.records.length,
              ...(source.error ? { error: source.error } : {}),
            })),
            resultCount: records.length,
            records,
            disposition: records.length > 0 ? "matches_found" : "no_matches_found",
            caveat: "Public records are prior-art leads. Verify applicability against the inspected source revision, product version, reachability, and fix history.",
          };
          return result(action, `Prior-art search found ${records.length} normalized record(s).`, output, [
            records.length > 0
              ? "Compare referenced fixes and affected versions with the inspected repository before asserting novelty or applicability."
              : "Record the no-match query and date on any promoted candidate; absence from these sources does not establish novelty.",
          ]);
        } finally {
          clearTimeout(timeout);
          context?.signal?.removeEventListener("abort", abort);
        }
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

async function runGit(root: string, args: readonly string[]): Promise<string> {
  const { stdout } = await execFileAsync("git", ["-c", `safe.directory=${root}`, "-C", root, ...args], {
    encoding: "utf8",
    maxBuffer: 2 * 1024 * 1024,
    timeout: 30_000,
    windowsHide: true,
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

interface PriorArtRecord {
  id: string;
  aliases: string[];
  source: "nvd" | "osv";
  summary: string;
  published: string | null;
  modified: string | null;
  affected: string[];
  references: string[];
  url: string;
}

interface PriorArtSourceResult {
  source: "nvd" | "osv";
  records: PriorArtRecord[];
  error?: string;
}

async function searchNvd(
  request: typeof fetch,
  terms: readonly string[],
  maxResults: number,
  signal: AbortSignal,
): Promise<PriorArtSourceResult> {
  try {
    const responses = await Promise.all(uniqueStrings(terms).slice(0, 4).map(async (term) => {
      const url = new URL("https://services.nvd.nist.gov/rest/json/cves/2.0");
      url.searchParams.set("keywordSearch", term);
      url.searchParams.set("resultsPerPage", String(Math.min(maxResults, 50)));
      const response = await request(url, { headers: { Accept: "application/json", "User-Agent": "app-server prior-art research" }, signal });
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      return await response.json() as unknown;
    }));
    const records = responses.flatMap(normalizeNvdResponse);
    return { source: "nvd", records: dedupePriorArt(records).slice(0, maxResults) };
  } catch (error) {
    return { source: "nvd", records: [], error: errorMessage(error) };
  }
}

async function searchOsv(
  request: typeof fetch,
  packageQuery: Record<string, unknown> | null,
  commit: string | null,
  maxResults: number,
  signal: AbortSignal,
): Promise<PriorArtSourceResult> {
  try {
    const body = commit
      ? { commit }
      : {
          package: {
            name: requiredString(packageQuery?.name, "package.name"),
            ecosystem: requiredString(packageQuery?.ecosystem, "package.ecosystem"),
          },
          ...(optionalString(packageQuery?.version) ? { version: optionalString(packageQuery?.version)! } : {}),
        };
    const response = await request("https://api.osv.dev/v1/query", {
      method: "POST",
      headers: { Accept: "application/json", "Content-Type": "application/json", "User-Agent": "app-server prior-art research" },
      body: JSON.stringify(body),
      signal,
    });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    return { source: "osv", records: normalizeOsvResponse(await response.json() as unknown).slice(0, maxResults) };
  } catch (error) {
    return { source: "osv", records: [], error: errorMessage(error) };
  }
}

function normalizeNvdResponse(value: unknown): PriorArtRecord[] {
  const record = optionalRecord(value);
  const vulnerabilities = Array.isArray(record?.vulnerabilities) ? record.vulnerabilities : [];
  return vulnerabilities.flatMap((entry) => {
    const cve = optionalRecord(optionalRecord(entry)?.cve);
    const id = optionalString(cve?.id);
    if (!id) return [];
    const descriptions = Array.isArray(cve?.descriptions) ? cve.descriptions : [];
    const description = descriptions.map(optionalRecord).find((item) => item?.lang === "en") ?? optionalRecord(descriptions[0]);
    const references = Array.isArray(cve?.references) ? cve.references.flatMap((item) => optionalString(optionalRecord(item)?.url) ?? []) : [];
    const configurations = Array.isArray(cve?.configurations) ? cve.configurations : [];
    const affected = configurations.flatMap((configuration) => extractNvdCriteria(configuration)).slice(0, 20);
    return [{
      id,
      aliases: [],
      source: "nvd" as const,
      summary: optionalString(description?.value) ?? "",
      published: optionalString(cve?.published),
      modified: optionalString(cve?.lastModified),
      affected,
      references: uniqueStrings(references).slice(0, 12),
      url: `https://nvd.nist.gov/vuln/detail/${encodeURIComponent(id)}`,
    }];
  });
}

function extractNvdCriteria(value: unknown): string[] {
  if (Array.isArray(value)) return value.flatMap(extractNvdCriteria);
  const record = optionalRecord(value);
  if (!record) return [];
  return [
    ...(optionalString(record.criteria) ? [optionalString(record.criteria)!] : []),
    ...Object.values(record).flatMap(extractNvdCriteria),
  ];
}

function normalizeOsvResponse(value: unknown): PriorArtRecord[] {
  const record = optionalRecord(value);
  const vulnerabilities = Array.isArray(record?.vulns) ? record.vulns : [];
  return vulnerabilities.flatMap((entry) => {
    const item = optionalRecord(entry);
    const id = optionalString(item?.id);
    if (!id) return [];
    const affected = Array.isArray(item?.affected) ? item.affected.flatMap((candidate) => {
      const packageRecord = optionalRecord(optionalRecord(candidate)?.package);
      const name = optionalString(packageRecord?.name);
      const ecosystem = optionalString(packageRecord?.ecosystem);
      return name ? [`${ecosystem ? `${ecosystem}:` : ""}${name}`] : [];
    }) : [];
    const references = Array.isArray(item?.references)
      ? item.references.flatMap((candidate) => optionalString(optionalRecord(candidate)?.url) ?? [])
      : [];
    return [{
      id,
      aliases: stringArray(item?.aliases),
      source: "osv" as const,
      summary: optionalString(item?.summary) ?? optionalString(item?.details) ?? "",
      published: optionalString(item?.published),
      modified: optionalString(item?.modified),
      affected: uniqueStrings(affected),
      references: uniqueStrings(references).slice(0, 12),
      url: `https://osv.dev/vulnerability/${encodeURIComponent(id)}`,
    }];
  });
}

function dedupePriorArt(records: readonly PriorArtRecord[]): PriorArtRecord[] {
  const seen = new Set<string>();
  return records.filter((record) => {
    const identities = [record.id, ...record.aliases].map((value) => value.toLowerCase());
    if (identities.some((identity) => seen.has(identity))) return false;
    identities.forEach((identity) => seen.add(identity));
    return true;
  });
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

function uniqueStrings(values: readonly string[]): string[] {
  return [...new Set(values.map((value) => value.trim()).filter(Boolean))];
}

function stringArray(value: unknown): string[] {
  return Array.isArray(value) ? uniqueStrings(value.flatMap((item) => typeof item === "string" ? [item] : [])) : [];
}

function optionalString(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function requiredString(value: unknown, field: string): string {
  const result = optionalString(value);
  if (!result) throw new Error(`${field} must be a non-empty string.`);
  return result;
}

function optionalRecord(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : null;
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
