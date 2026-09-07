import { createHash } from "node:crypto";
import { nowIso } from "./ids.js";
import { boundedPublicResponse, publicDocumentUrl, readPublicDocument, type PublicDocumentOptions } from "./public-document-tools.js";
import type { ResearchExecutableTool } from "./tool-registry.js";
import type { ResourcePriorArtContext } from "./resource-prior-art.js";

export type PriorArtSearchToolOptions = PublicDocumentOptions & { history?: ResourcePriorArtContext };
export const PRIOR_ART_SOURCES = ["nvd", "osv", "github_issues", "github_releases", "documents"] as const;
export type PriorArtSource = typeof PRIOR_ART_SOURCES[number];

export interface PriorArtRecord {
  id: string;
  aliases: string[];
  source: PriorArtSource;
  kind: "advisory" | "issue" | "pull_request" | "release" | "document";
  summary: string;
  published: string | null;
  modified: string | null;
  affected: string[];
  /** Original structured applicability data, including version bounds and range events. */
  affectedDetails: unknown[];
  references: string[];
  url: string;
  detailUrl: string;
  /** Source-specific fields retained without flattening or cross-source deduplication. */
  sourceData: Record<string, unknown>;
}

interface PageState { offset: number; page: number; token?: string; hash?: string }
interface SearchStream { source: PriorArtSource; query: string; url?: string }
interface SearchRequest { query: string; aliases: string[]; sources: PriorArtSource[]; repository: string | null; package: Record<string, unknown> | null; commit: string | null; urls: string[] }
interface SearchCursor { version: 1; fingerprint: string; states: Array<PageState | null>; limitations: Array<string | null>; returnedSoFar: number }
interface SourcePage { records: PriorArtRecord[]; next: PageState | null; url: string; limited?: string }
export interface PriorArtSourceCoverage {
  source: PriorArtSource;
  query: string;
  url?: string;
  status: "complete" | "partial" | "error" | "not_searched";
  resultCount: number;
  error?: string;
  limitation?: string;
}
export interface PriorArtSearchResult {
  query: string;
  aliases: string[];
  fetchedAt: string;
  sources: PriorArtSourceCoverage[];
  records: PriorArtRecord[];
  resultCount: number;
  returnedSoFar: number;
  nextCursor: string | null;
  complete: boolean;
  disposition: "matches_found" | "no_matches_found" | "incomplete" | "sources_unavailable";
  caveat: string;
  savedHistoryId?: string;
  request?: Record<string, unknown>;
}

const PARAMETERS = {
  type: "object", required: ["query"], properties: {
    resourceId: { type: "string", description: "Resource ID from resource.catalog. Required in workspace sessions; saves the search and source coverage for later recall." },
    revision: { type: "string", description: "Optional resource build, version, or commit this search concerns." },
    query: { type: "string", description: "Advisory keywords or literal phrase for repository/document search. Use * to browse release or document pages." },
    aliases: { type: "array", items: { type: "string" }, maxItems: 3, description: "Additional NVD keyword queries." },
    sources: { type: "array", items: { type: "string", enum: PRIOR_ART_SOURCES }, uniqueItems: true },
    package: { type: "object", required: ["name", "ecosystem"], properties: { name: { type: "string" }, ecosystem: { type: "string" }, version: { type: "string" } } },
    commit: { type: "string", description: "Commit identity for OSV lookup." },
    repository: { type: "string", description: "Public GitHub owner/repository for issue, pull-request, and release search." },
    urls: { type: "array", items: { type: "string" }, maxItems: 5, description: "Specific public bulletin, release-note, feed, or mailing-list archive pages to search. Only these documents are searched; links are available through prior_art.fetch." },
    maxResults: { type: "integer", minimum: 1, maximum: 50, description: "Requested upper limit. Context pages return at most 20 cards; nextCursor retains the rest." },
    cursor: { type: "string", description: "nextCursor from this query. Keep query, sources, aliases, repository, package, commit, and URLs unchanged; page size may change." },
  },
};

export function createPriorArtSearchTool(options: PriorArtSearchToolOptions = {}): ResearchExecutableTool {
  const parameters = options.history ? { ...PARAMETERS, required: ["query", "resourceId"] } : PARAMETERS;
  return {
    descriptor: { name: "prior_art.search", transportName: "prior_art_search", description: "Search NVD/OSV advisories, public GitHub issues and pull requests, release notes, or specified public documents. Follow nextCursor for remaining results; per-source coverage distinguishes incomplete or failed searches from no matches.", actionClasses: ["search", "inspect"], sideEffects: "network", requiredPermissions: ["network:public-advisory:read"], inputSchema: parameters, metadata: { provider: "appServer.built_in", safetyProfile: "public-advisory-read" } },
    parameters: parameters as NonNullable<ResearchExecutableTool["parameters"]>,
    async execute(action, context) {
      const startedAt = nowIso();
      try {
        const resourceId = options.history?.store.requireResource(action.input.resourceId);
        const query = parseRequest(action.input);
        const maxResults = Math.min(20, integer(action.input.maxResults, 20, 1, 50));
        const streams = makeStreams(query);
        const fingerprint = digest({ ...query, resourceId, revision: action.input.revision });
        const cursor = decodeCursor(action.input.cursor, fingerprint, streams.length);
        const states = cursor?.states ?? streams.map(() => ({ offset: 0, page: 1 }));
        const limitations = cursor?.limitations ?? streams.map(() => null);
        const deadline = AbortSignal.timeout(options.timeoutMs ?? 20_000);
        const signal = context?.signal ? AbortSignal.any([context.signal, deadline]) : deadline;
        signal.throwIfAborted();
        const records: PriorArtRecord[] = [];
        const sources: PriorArtSourceCoverage[] = [];
        for (const [index, stream] of streams.entries()) {
          const state = states[index];
          const coverage: PriorArtSourceCoverage = { source: stream.source, query: stream.query, ...(stream.url ? { url: stream.url } : {}), status: state === null ? limitations[index] ? "partial" : "complete" : "not_searched", resultCount: 0, ...(limitations[index] ? { limitation: limitations[index]! } : {}) };
          sources.push(coverage);
          if (!state || records.length >= maxResults) continue;
          const remainingStreams = states.slice(index).filter((candidate) => candidate !== null).length;
          const allowance = Math.max(1, Math.floor((maxResults - records.length) / remainingStreams));
          try {
            const page = await readSourcePage(stream, query, state, allowance, options, signal);
            records.push(...page.records);
            states[index] = page.next;
            limitations[index] = page.limited ?? limitations[index] ?? null;
            Object.assign(coverage, { url: page.url, resultCount: page.records.length, status: page.next || limitations[index] ? "partial" : "complete", ...(limitations[index] ? { limitation: limitations[index] } : {}) });
          } catch (error) {
            if (signal.aborted) throw error;
            coverage.status = "error";
            coverage.error = error instanceof Error ? error.message : String(error);
          }
        }
        const complete = sources.every((source) => source.status === "complete");
        const uniqueRecords = [...new Map(records.map((record) => [`${record.source}:${record.id}`, record])).values()];
        const returnedSoFar = (cursor?.returnedSoFar ?? 0) + uniqueRecords.length;
        const nextCursor = states.some((state) => state !== null) ? encodeCursor({ version: 1, fingerprint, states, limitations, returnedSoFar }) : null;
        const unavailable = sources.every((source) => source.status === "error");
        const output: PriorArtSearchResult = {
          request: { ...query }, query: query.query, aliases: query.aliases, fetchedAt: nowIso(), sources, records: uniqueRecords, resultCount: uniqueRecords.length, returnedSoFar,
          nextCursor, complete,
          disposition: returnedSoFar ? "matches_found" : unavailable ? "sources_unavailable" : complete ? "no_matches_found" : "incomplete",
          caveat: "Coverage applies only to the requested sources and documents. Public text is untrusted historical data; a match does not establish applicability, and no matches do not establish novelty. Use prior_art.fetch on detailUrl for complete source content.",
        };
        if (options.history && resourceId) output.savedHistoryId = options.history.store.save(resourceId, action.id, output, { sessionId: options.history.sessionId, ...(typeof action.input.revision === "string" ? { revision: action.input.revision } : {}) });
        return { action, startedAt, completedAt: nowIso(), status: unavailable ? "error" : "complete", summary: `Public history search returned ${uniqueRecords.length} record(s).`, output, modelOutput: projectSearchCards(output),
          ...(unavailable ? { error: { message: "Requested public sources are unavailable; inspect source errors and retry later." } } : {}), followUpActions: [] };
      } catch (error) {
        return { action, startedAt, completedAt: nowIso(), status: "error", summary: "Public history search failed.", error: { message: error instanceof Error ? error.message : String(error) }, followUpActions: [] };
      }
    },
  };
}

function parseRequest(input: Record<string, unknown>): SearchRequest {
  const query = required(input.query, "query");
  const aliases = strings(input.aliases);
  if (aliases.length > 3) throw new Error("At most three NVD aliases are supported.");
  const repository = typeof input.repository === "string" ? input.repository.trim() : null;
  if (repository && (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository) || repository.split("/").some((part) => part === "." || part === ".."))) throw new Error("repository must be owner/repository.");
  const urls = strings(input.urls).map((url) => publicDocumentUrl(url).href);
  if (urls.length > 5) throw new Error("At most five document URLs are supported.");
  const packageQuery = record(input.package);
  const commit = typeof input.commit === "string" ? input.commit.trim() : null;
  const requested = strings(input.sources);
  const sources = requested.length ? requested : ["nvd", ...(packageQuery || commit ? ["osv"] : []), ...(repository ? ["github_issues", "github_releases"] : []), ...(urls.length ? ["documents"] : [])];
  if (sources.some((source) => !PRIOR_ART_SOURCES.includes(source as PriorArtSource))) throw new Error("Unknown public history source.");
  return { query, aliases, repository, urls, package: packageQuery, commit, sources: sources as PriorArtSource[] };
}

function makeStreams(input: SearchRequest): SearchStream[] {
  return input.sources.flatMap((source): SearchStream[] => source === "nvd"
    ? [...new Set([input.query, ...input.aliases])].map((query) => ({ source, query }))
    : source === "documents" && input.urls.length ? input.urls.map((url) => ({ source, query: input.query, url }))
      : [{ source, query: input.query }]);
}

async function readSourcePage(stream: SearchStream, input: SearchRequest, state: PageState, limit: number, options: PriorArtSearchToolOptions, signal: AbortSignal): Promise<SourcePage> {
  const fetchJson = async (url: string, body?: unknown) => {
    const loaded = await boundedPublicResponse(options.fetch ?? globalThis.fetch, url, signal, {
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      headers: { Accept: "application/json", ...(body === undefined ? {} : { "Content-Type": "application/json" }) },
    });
    return { value: JSON.parse(new TextDecoder().decode(loaded.bytes)) as unknown, response: loaded.response, url: loaded.url };
  };
  if (stream.source === "nvd") {
    const url = new URL("https://services.nvd.nist.gov/rest/json/cves/2.0");
    url.searchParams.set("keywordSearch", stream.query);
    url.searchParams.set("startIndex", String(state.offset));
    url.searchParams.set("resultsPerPage", String(limit));
    const { value } = await fetchJson(url.href);
    const data = requiredRecord(value, "NVD response");
    const items = requiredArray(data.vulnerabilities, "NVD vulnerabilities");
    const nextOffset = state.offset + items.length;
    const total = typeof data.totalResults === "number" ? data.totalResults : null;
    if (items.length > limit) throw new Error("NVD returned more records than requested.");
    return { url: url.href, records: items.map(normalizeNvd),
      next: total !== null && nextOffset < total ? { offset: nextOffset, page: 1 } : null,
      ...(total === null ? { limited: "NVD omitted totalResults; pagination coverage is unknown." } : {}),
      ...(items.length === 0 && total !== null && nextOffset < total ? { limited: "NVD returned an empty non-terminal page; retry later." } : {}),
    };
  }
  if (stream.source === "osv") {
    if (!input.package && !input.commit) throw new Error("OSV requires package or commit input; it has no keyword-only search.");
    const body = input.commit ? { commit: input.commit } : {
      package: { name: required(input.package?.name, "package.name"), ecosystem: required(input.package?.ecosystem, "package.ecosystem") },
      ...(typeof input.package?.version === "string" ? { version: input.package.version } : {}),
    };
    const { value, url } = await fetchJson("https://api.osv.dev/v1/query", { ...body, ...(state.token ? { page_token: state.token } : {}) });
    const data = requiredRecord(value, "OSV response");
    const items = data.vulns === undefined ? [] : requiredArray(data.vulns, "OSV vulnerabilities");
    const hash = digest(data);
    verifyPageHash(state, hash);
    const page = items.slice(state.offset, state.offset + limit);
    const offset = state.offset + page.length;
    const nextToken = typeof data.next_page_token === "string" && data.next_page_token ? data.next_page_token : null;
    return { url, records: page.map(normalizeOsv), next: offset < items.length ? { ...state, offset, hash } : nextToken ? { offset: 0, page: 1, token: nextToken } : null };
  }
  if (stream.source === "documents") {
    if (!stream.url) throw new Error("Document search requires explicit urls; no website was searched.");
    const document = await readPublicDocument(stream.url, options, signal);
    const match = stream.query === "*" ? 0 : document.text.toLowerCase().indexOf(stream.query.toLowerCase());
    return { url: document.url, next: null, records: match < 0 ? [] : [{
      id: document.url, source: "documents", kind: "document", aliases: [], summary: document.text.slice(Math.max(0, match - 200), match + 1200),
      published: null, modified: document.lastModified, affected: [], affectedDetails: [], references: [], url: document.url, detailUrl: document.url,
      sourceData: { title: document.title, contentHash: document.contentHash, fetchedAt: document.fetchedAt, contentType: document.contentType, totalCharacters: document.text.length, totalLinks: document.links.length },
    }] };
  }
  if (!input.repository) throw new Error("GitHub history search requires repository as owner/repository.");
  const url = stream.source === "github_issues" ? new URL("https://api.github.com/search/issues") : new URL(`https://api.github.com/repos/${input.repository}/releases`);
  // Treat keywords as a phrase rather than allowing them to override repository scope.
  if (stream.source === "github_issues") {
    url.searchParams.set("q", `${JSON.stringify(stream.query)} repo:${input.repository} is:public`);
    url.searchParams.set("sort", "created");
    url.searchParams.set("order", "asc");
  }
  url.searchParams.set("page", String(state.page));
  url.searchParams.set("per_page", "50");
  const { value, response } = await fetchJson(url.href);
  const data = stream.source === "github_issues" ? requiredRecord(value, "GitHub search") : null;
  const items = requiredArray(data ? data.items : value, "GitHub records");
  const hash = digest(items);
  verifyPageHash(state, hash);
  const records: PriorArtRecord[] = [];
  let offset = state.offset;
  while (offset < items.length && records.length < limit) {
    const item = requiredRecord(items[offset++], "GitHub record");
    if (stream.source === "github_releases" && stream.query !== "*" && !`${item.name ?? ""}\n${item.body ?? ""}\n${item.tag_name ?? ""}`.toLowerCase().includes(stream.query.toLowerCase())) continue;
    const detailUrl = required(item.url, "GitHub API record URL");
    records.push({ id: `${input.repository}:${item.id}`, aliases: [], source: stream.source,
      kind: stream.source === "github_releases" ? "release" : item.pull_request ? "pull_request" : "issue",
      summary: `${item.title ?? item.name ?? item.tag_name ?? ""}\n${item.body ?? ""}`,
      published: text(item.published_at) ?? text(item.created_at), modified: text(item.updated_at), affected: [], affectedDetails: [],
      references: [], url: text(item.html_url) ?? detailUrl, detailUrl,
      sourceData: { state: item.state, number: item.number, labels: item.labels, tagName: item.tag_name, targetCommitish: item.target_commitish, prerelease: item.prerelease, pullRequest: item.pull_request, commentsUrl: item.comments_url, body: item.body },
    });
  }
  const hasNext = /<[^>]+>;\s*rel="next"/.test(response.headers.get("link") ?? "");
  const limited = data?.incomplete_results === true || (typeof data?.total_count === "number" && data.total_count > 1000);
  return { url: url.href, records, next: offset < items.length ? { ...state, offset, hash }
    : hasNext && !(stream.source === "github_issues" && state.page >= 20) ? { page: state.page + 1, offset: 0 } : null,
    ...(limited ? { limited: "GitHub reports incomplete results or more than its 1,000-result search limit; narrow the query." } : {}),
  };
}

function normalizeNvd(value: unknown): PriorArtRecord {
  const item = requiredRecord(requiredRecord(value, "NVD item").cve, "NVD CVE");
  const id = required(item.id, "NVD CVE id");
  const descriptions = Array.isArray(item.descriptions) ? item.descriptions.map((value) => record(value)) : [];
  const configurations = Array.isArray(item.configurations) ? item.configurations : [];
  return { id, aliases: [], source: "nvd", kind: "advisory", summary: text(descriptions.find((value) => value?.lang === "en")?.value) ?? text(descriptions[0]?.value) ?? "",
    published: text(item.published), modified: text(item.lastModified), affected: nvdCriteria(configurations), affectedDetails: configurations,
    references: referenceUrls(item.references), url: `https://nvd.nist.gov/vuln/detail/${encodeURIComponent(id)}`,
    detailUrl: `https://services.nvd.nist.gov/rest/json/cves/2.0?cveId=${encodeURIComponent(id)}`,
    sourceData: { vulnStatus: item.vulnStatus, sourceIdentifier: item.sourceIdentifier, descriptions: item.descriptions, metrics: item.metrics, weaknesses: item.weaknesses, references: item.references },
  };
}
function normalizeOsv(value: unknown): PriorArtRecord {
  const item = requiredRecord(value, "OSV record");
  const id = required(item.id, "OSV id");
  const affectedDetails = Array.isArray(item.affected) ? item.affected : [];
  return { id, aliases: strings(item.aliases), source: "osv", kind: "advisory", summary: text(item.summary) ?? text(item.details) ?? "",
    published: text(item.published), modified: text(item.modified), affected: affectedDetails.flatMap((value) => {
      const pkg = record(record(value)?.package);
      return typeof pkg?.name === "string" ? [`${typeof pkg.ecosystem === "string" ? `${pkg.ecosystem}:` : ""}${pkg.name}`] : [];
    }), affectedDetails, references: referenceUrls(item.references), url: `https://osv.dev/vulnerability/${encodeURIComponent(id)}`,
    detailUrl: `https://api.osv.dev/v1/vulns/${encodeURIComponent(id)}`,
    sourceData: { details: item.details, withdrawn: item.withdrawn, severity: item.severity, related: item.related, databaseSpecific: item.database_specific, references: item.references, schemaVersion: item.schema_version },
  };
}
function nvdCriteria(value: unknown): string[] {
  if (Array.isArray(value)) return value.flatMap(nvdCriteria);
  const object = record(value);
  return object ? [...(typeof object.criteria === "string" ? [object.criteria] : []), ...Object.values(object).flatMap(nvdCriteria)] : [];
}
function referenceUrls(value: unknown): string[] { return Array.isArray(value) ? [...new Set(value.flatMap((item) => text(record(item)?.url) ?? []))] : []; }
function digest(value: unknown): string {
  const canonical = (item: unknown): unknown => Array.isArray(item) ? item.map(canonical)
    : record(item) ? Object.fromEntries(Object.entries(item as Record<string, unknown>).sort(([left], [right]) => left.localeCompare(right)).map(([key, entry]) => [key, canonical(entry)]))
      : item;
  return createHash("sha256").update(JSON.stringify(canonical(value))).digest("hex");
}
function verifyPageHash(state: PageState, hash: string): void { if (state.hash && state.hash !== hash) throw new Error("Source page changed during pagination; restart without cursor."); }
function encodeCursor(cursor: SearchCursor): string { return Buffer.from(JSON.stringify(cursor)).toString("base64url"); }
function decodeCursor(value: unknown, fingerprint: string, count: number): SearchCursor | null {
  if (value === undefined) return null;
  if (typeof value !== "string" || value.length > 24_000) throw new Error("Invalid public history cursor.");
  const parsed = requiredRecord(JSON.parse(Buffer.from(value, "base64url").toString("utf8")), "cursor");
  if (parsed.version !== 1 || parsed.fingerprint !== fingerprint || !Array.isArray(parsed.states) || parsed.states.length !== count) throw new Error("Cursor does not match this public history query; restart without cursor.");
  if (!Array.isArray(parsed.limitations) || parsed.limitations.length !== count || parsed.limitations.some((value) => value !== null && (typeof value !== "string" || value.length > 500))) throw new Error("Invalid public history cursor coverage.");
  const states = parsed.states.map((value): PageState | null => {
    if (value === null) return null;
    const state = requiredRecord(value, "cursor state");
    for (const key of ["token", "hash"]) if (state[key] !== undefined && (typeof state[key] !== "string" || state[key].length > 8_000)) throw new Error("Invalid public history cursor state.");
    return { offset: integer(state.offset, 0, 0, 1_000_000_000), page: integer(state.page, 1, 1, 1_000_000),
      ...(typeof state.token === "string" ? { token: state.token } : {}), ...(typeof state.hash === "string" ? { hash: state.hash } : {}) };
  });
  return { version: 1, fingerprint, states, limitations: parsed.limitations as Array<string | null>, returnedSoFar: integer(parsed.returnedSoFar, 0, 0, Number.MAX_SAFE_INTEGER) };
}

export function projectSearchCards(output: PriorArtSearchResult): unknown {
  // Keep every returned record discoverable in model context. Full normalized
  // data remains in the canonical tool result; detailUrl supports paged reads.
  const detailBudget = Math.min(1000, Math.floor(10_000 / Math.max(1, output.records.length)));
  const summaryBudget = Math.min(500, Math.floor(6_000 / Math.max(1, output.records.length)));
  return { ...output, records: output.records.map((item) => ({
    id: item.id, aliases: item.aliases, source: item.source, kind: item.kind,
    summary: item.summary.slice(0, summaryBudget), summaryTruncated: item.summary.length > summaryBudget,
    published: item.published, modified: item.modified, url: item.url, detailUrl: item.detailUrl,
    references: item.references.slice(0, 2), referenceCount: item.references.length,
    affected: item.affected.slice(0, 3), affectedCount: item.affected.length,
    ...(JSON.stringify(item.affectedDetails, null, 2).length <= detailBudget ? { affectedDetails: item.affectedDetails } : {}),
    withdrawn: item.sourceData.withdrawn ?? null,
    detailsDeferred: true,
  })), recall: "Read detailUrl with prior_art.fetch for full descriptions, affected ranges, metrics, references, and source-specific data. Deferred or omitted fields are not absent evidence." };
}
function record(value: unknown): Record<string, unknown> | null { return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : null; }
function requiredRecord(value: unknown, name: string): Record<string, unknown> { const result = record(value); if (!result) throw new Error(`Invalid ${name}.`); return result; }
function requiredArray(value: unknown, name: string): unknown[] { if (!Array.isArray(value)) throw new Error(`Invalid ${name}.`); return value; }
function text(value: unknown): string | null { return typeof value === "string" ? value : null; }
function required(value: unknown, name: string): string { if (typeof value !== "string" || !value.trim()) throw new Error(`${name} must be a non-empty string.`); return value.trim(); }
function strings(value: unknown): string[] { return Array.isArray(value) ? [...new Set(value.flatMap((item) => typeof item === "string" && item.trim() ? [item.trim()] : []))] : []; }
function integer(value: unknown, fallback: number, minimum: number, maximum: number): number { if (value === undefined) return fallback; if (typeof value !== "number" || !Number.isSafeInteger(value) || value < minimum || value > maximum) throw new Error("Invalid public history page bounds."); return value; }
