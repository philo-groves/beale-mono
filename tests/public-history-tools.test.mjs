import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { promisify } from "node:util";
import test from "node:test";
import {
  createPriorArtSearchTool, createPriorArtFetchTool, createRepositoryFetchHistoryTool,
  createRepositoryHistoryTool, createResearchToolRegistry, managedToolPluginId,
  projectModelToolResult, RepositoryResearchSession,
} from "../packages/research-agent/dist/index.js";

const json = (value, headers = {}) => new Response(JSON.stringify(value), { headers: { "content-type": "application/json", ...headers } });
const action = (toolName, input) => ({ id: "history-example", toolName, actionClass: toolName === "prior_art.search" ? "search" : "inspect", input });
const search = (fetch) => {
  const tool = createPriorArtSearchTool({ fetch });
  return (input, context) => tool.execute(action("prior_art.search", input), context);
};
const osvRecord = (id) => ({
  id, aliases: ["CVE-2099-10001"], summary: "Example package documentation issue.", details: "Example complete advisory description.",
  published: "2099-01-01T00:00:00Z", modified: "2099-01-02T00:00:00Z", withdrawn: "2099-01-03T00:00:00Z",
  affected: [{ package: { name: "example-package", ecosystem: "npm", purl: "pkg:npm/example-package" },
    ranges: [{ type: "GIT", repo: "https://github.com/example-org/example-repository", events: [{ introduced: "revision-example-one" }, { fixed: "revision-example-two" }, { last_affected: "revision-example-three" }, { limit: "revision-example-four" }] }],
    versions: ["1.0.0", "1.0.1"], ecosystem_specific: { example: true }, database_specific: { example: "data" } }],
  severity: [{ type: "CVSS_V3", score: "example-vector" }],
  references: Array.from({ length: 15 }, (_, i) => ({ type: "ADVISORY", url: `https://example.test/advisory/${i}` })),
});

test("public advisory search retains structured applicability and separate source provenance", async () => {
  const osv = osvRecord("OSV-EXAMPLE-ONE");
  const configurations = [{ operator: "AND", negate: false, nodes: [{ cpeMatch: [{ vulnerable: true, criteria: "cpe:2.3:a:example:package:*:*:*:*:*:*:*:*", versionStartIncluding: "1.0", versionEndExcluding: "1.1" }] }] }];
  const run = search(async (url) => String(url).includes("nvd.nist.gov") ? json({ totalResults: 1, vulnerabilities: [{ cve: {
    id: "CVE-2099-10001", descriptions: [{ lang: "en", value: "Example advisory." }], configurations, metrics: { example: "metric" },
  } }] }) : json({ vulns: [osv] }));
  const result = await run({ query: "example", sources: ["nvd", "osv"], package: { name: "example-package", ecosystem: "npm" } });
  assert.equal(result.status, "complete");
  assert.equal(result.output.complete, true);
  assert.equal(result.output.records.length, 2, "CVE aliases must not discard the richer OSV source");
  assert.deepEqual(result.output.records[0].affectedDetails, configurations);
  assert.deepEqual(result.output.records[1].affectedDetails, osv.affected);
  assert.deepEqual(result.output.records[1].sourceData.severity, osv.severity);
  assert.equal(result.output.records[1].sourceData.withdrawn, osv.withdrawn);
  assert.equal(result.output.records[1].references.length, 15);
  assert.match(result.output.records[1].detailUrl, /api.osv.dev\/v1\/vulns/);
});

test("NVD aliases paginate independently without skipping result pages", async () => {
  const calls = [];
  const run = search(async (raw) => {
    const url = new URL(raw);
    const query = url.searchParams.get("keywordSearch");
    const offset = Number(url.searchParams.get("startIndex"));
    calls.push([query, offset]);
    return json({ startIndex: offset, totalResults: 3, vulnerabilities: [{ cve: { id: `${query}-${offset}`, descriptions: [] } }] });
  });
  const input = { query: "example-one", aliases: ["example-two"], sources: ["nvd"], maxResults: 2 };
  const ids = [];
  let cursor;
  do {
    const result = await run({ ...input, ...(cursor ? { cursor } : {}) });
    assert.equal(result.status, "complete");
    ids.push(...result.output.records.map((item) => item.id));
    cursor = result.output.nextCursor;
  } while (cursor);
  assert.equal(new Set(ids).size, 6);
  assert.deepEqual(calls, [["example-one", 0], ["example-two", 0], ["example-one", 1], ["example-two", 1], ["example-one", 2], ["example-two", 2]]);
});

test("OSV continuation preserves unread records inside native pages and handles empty token pages", async () => {
  const bodies = [];
  const run = search(async (_url, options) => {
    const body = JSON.parse(options.body);
    bodies.push(body);
    return json(body.page_token === "page-example-two" ? { next_page_token: "page-example-three" }
      : body.page_token === "page-example-three" ? { vulns: [osvRecord("OSV-EXAMPLE-THREE")] }
        : { vulns: [osvRecord("OSV-EXAMPLE-ONE"), osvRecord("OSV-EXAMPLE-TWO")], next_page_token: "page-example-two" });
  });
  const input = { query: "example", sources: ["osv"], package: { name: "example-package", ecosystem: "npm", version: "1.0" }, maxResults: 1 };
  const ids = [];
  let cursor;
  let pages = 0;
  do {
    const result = await run({ ...input, ...(cursor ? { cursor } : {}) });
    assert.equal(result.status, "complete");
    assert.equal(result.output.disposition, "matches_found", "an empty continuation must not erase earlier matches");
    ids.push(...result.output.records.map((item) => item.id));
    cursor = result.output.nextCursor;
    pages += 1;
  } while (cursor && pages < 8);
  assert.equal(pages, 4);
  assert.deepEqual(ids, ["OSV-EXAMPLE-ONE", "OSV-EXAMPLE-TWO", "OSV-EXAMPLE-THREE"]);
  assert.equal(bodies[0].version, "1.0");
  assert.equal(bodies[1].page_token, undefined, "unread records retain the current native page");
  assert.equal(bodies[2].page_token, "page-example-two");
});

test("pagination rejects changed queries and changed source pages", async () => {
  let changed = false;
  const run = search(async () => json({ vulns: [osvRecord(changed ? "OSV-EXAMPLE-CHANGED" : "OSV-EXAMPLE-ONE"), osvRecord("OSV-EXAMPLE-TWO")] }));
  const input = { query: "example", commit: "revision-example", sources: ["osv"], maxResults: 1 };
  const first = await run(input);
  assert.equal((await run({ ...input, query: "different", cursor: first.output.nextCursor })).status, "error");
  changed = true;
  const later = await run({ ...input, cursor: first.output.nextCursor });
  assert.equal(later.status, "error");
  assert.match(later.output.sources[0].error, /Source page changed/);
});

test("source failures, missing inputs, and pending sources never become completed no-match searches", async () => {
  const run = search(async (url) => String(url).includes("nvd.nist.gov") ? new Response(null, { status: 429, headers: { "retry-after": "60" } }) : json({ vulns: [] }));
  const partial = await run({ query: "example", sources: ["nvd", "osv"], commit: "revision-example" });
  assert.equal(partial.output.complete, false);
  assert.equal(partial.output.disposition, "incomplete");
  assert.match(partial.output.sources[0].error, /429.*60/);
  assert.ok(partial.output.nextCursor);
  const missing = await run({ query: "example", sources: ["osv", "documents", "github_issues"] });
  assert.equal(missing.status, "error");
  assert.equal(missing.output.disposition, "sources_unavailable");
  assert.ok(missing.output.sources.every((source) => source.status === "error"));
});

test("GitHub issues include PRs, paginate, and retain search coverage limits across other sources", async () => {
  const calls = [];
  const run = search(async (raw) => {
    const url = new URL(raw); calls.push(url);
    if (url.hostname === "example.test") return new Response("No matching record.", { headers: { "content-type": "text/plain" } });
    return json({ incomplete_results: true, total_count: 1001, items: [{ id: 1, title: "Example issue", body: "Example discussion.", url: "https://api.github.com/repos/example-org/example-repository/issues/1", html_url: "https://github.com/example-org/example-repository/issues/1", pull_request: { url: "https://api.github.com/repos/example-org/example-repository/pulls/1" } }] });
  });
  const input = { query: "example", repository: "example-org/example-repository", urls: ["https://example.test/archive"], sources: ["github_issues", "documents"], maxResults: 1 };
  const first = await run(input);
  assert.equal(first.output.records[0].kind, "pull_request");
  assert.match(calls[0].searchParams.get("q"), /repo:example-org\/example-repository is:public/);
  assert.equal(first.output.sources[1].status, "not_searched");
  const next = await run({ ...input, cursor: first.output.nextCursor });
  assert.equal(next.output.complete, false, "provider limits persist after that stream is finished");
  assert.equal(next.output.disposition, "matches_found");
  assert.match(next.output.sources[0].limitation, /1,000/);
});

test("release searches continue past unmatched pages and retain release identities", async () => {
  const run = search(async (raw) => {
    const page = new URL(raw).searchParams.get("page");
    return json([{ id: Number(page), name: page === "1" ? "Maintenance" : "Example release", body: "Release notes.", tag_name: `v${page}`, url: `https://api.github.com/repos/example-org/example-repository/releases/${page}`, html_url: `https://github.com/example-org/example-repository/releases/tag/v${page}` }], page === "1" ? { link: '<https://api.github.com/example?page=2>; rel="next"' } : {});
  });
  const input = { query: "example", repository: "example-org/example-repository", sources: ["github_releases"] };
  const first = await run(input);
  assert.equal(first.output.disposition, "incomplete");
  const second = await run({ ...input, cursor: first.output.nextCursor });
  assert.equal(second.output.complete, true);
  assert.equal(second.output.records[0].sourceData.tagName, "v2");
});

test("partial GitHub coverage remains partial after a later complete native page", async () => {
  const run = search(async (raw) => {
    const page = new URL(raw).searchParams.get("page");
    return json({ incomplete_results: page === "1", total_count: 2, items: [{ id: Number(page), title: "Example issue", url: `https://api.github.com/repos/example-org/example-repository/issues/${page}` }] },
      page === "1" ? { link: '<https://api.github.com/example?page=2>; rel="next"' } : {});
  });
  const input = { query: "example", repository: "example-org/example-repository", sources: ["github_issues"] };
  const first = await run(input);
  const next = await run({ ...input, cursor: first.output.nextCursor });
  assert.equal(next.output.nextCursor, null);
  assert.equal(next.output.complete, false);
  assert.match(next.output.sources[0].limitation, /incomplete/);
});

test("HTML source links retain base URLs and fragment provenance", async () => {
  const tool = createPriorArtFetchTool({ fetch: async () => new Response('<html><head><base href="https://example.test/archive/"></head><body><a href="bulletin#entry">Entry</a></body></html>', { headers: { "content-type": "text/html" } }) });
  const result = await tool.execute(action("prior_art.fetch", { url: "https://example.test/index" }));
  assert.deepEqual(result.output.links, [{ url: "https://example.test/archive/bulletin#entry", text: "Entry" }]);
});

test("document search covers only supplied pages and links to complete source text", async () => {
  const run = search(async () => new Response("<html><title>Example bulletin</title><body><p>Example release correction.</p><a href='/archive'>Archive</a></body></html>", { headers: { "content-type": "text/html" } }));
  const result = await run({ query: "release correction", sources: ["documents"], urls: ["https://example.test/bulletin"] });
  assert.equal(result.output.complete, true);
  assert.equal(result.output.records[0].kind, "document");
  assert.equal(result.output.records[0].sourceData.title, "Example bulletin");
  assert.equal(result.output.records[0].detailUrl, "https://example.test/bulletin");
  assert.equal(result.output.records[0].sourceData.contentHash.length, 64);
});

test("document fetch follows bounded redirects, extracts text, pages links, and detects source changes", async () => {
  let changed = false;
  const calls = [];
  const tool = createPriorArtFetchTool({ fetch: async (url, options) => {
    calls.push([url, options]);
    return url.endsWith("/old") ? new Response(null, { status: 302, headers: { location: "/bulletin" } }) : new Response(
      `<html><title>Example &amp; bulletin</title><style>hidden-style</style><script>hidden-script</script><body><p>${changed ? "Changed" : "Original"} example text.</p>${Array.from({ length: 23 }, (_, i) => `<a href='/source/${i}'>Source ${i}</a>`).join("")}</body></html>`,
      { headers: { "content-type": "text/html; charset=utf-8", etag: "example-etag" } });
  } });
  const first = await tool.execute(action("prior_art.fetch", { url: "https://example.test/old", maxCharacters: 10 }));
  assert.equal(first.status, "complete");
  assert.equal(first.output.url, "https://example.test/bulletin");
  assert.equal(first.output.title, "Example & bulletin");
  assert.equal(first.output.untrustedSourceContent, true);
  assert.equal(first.output.nextLinkOffset, 20);
  assert.equal(first.output.etag, "example-etag");
  const next = await tool.execute(action("prior_art.fetch", { url: first.output.url, offset: 10, linkOffset: 20, expectedContentHash: first.output.contentHash }));
  assert.equal(next.status, "complete");
  assert.equal(next.output.links.length, 3);
  assert.doesNotMatch(first.output.text + next.output.text, /hidden-script|hidden-style/);
  assert.ok(calls.every(([, options]) => options.credentials === "omit" && options.headers.authorization === undefined && options.headers.cookie === undefined));
  changed = true;
  const stale = await tool.execute(action("prior_art.fetch", { url: first.output.url, offset: 10, expectedContentHash: first.output.contentHash }));
  assert.equal(stale.status, "error");
  assert.match(stale.error.message, /Source content changed/);
});

test("document fetch rejects binary, oversized, credential-bearing, and cancelled requests", async () => {
  const tooLarge = createPriorArtFetchTool({ maxBytes: 5, fetch: async () => new Response("ten-bytes!", { headers: { "content-type": "text/plain" } }) });
  assert.match((await tooLarge.execute(action("prior_art.fetch", { url: "https://example.test/page" }))).error.message, /exceeds 5 bytes/);
  const binary = createPriorArtFetchTool({ fetch: async () => new Response("example", { headers: { "content-type": "application/pdf" } }) });
  assert.match((await binary.execute(action("prior_art.fetch", { url: "https://example.test/page" }))).error.message, /Unsupported document content type/);
  let calls = 0;
  const tool = createPriorArtFetchTool({ fetch: async () => { calls += 1; return new Response("Example"); } });
  for (const url of ["file:///example", "https://user:password@example.test/page"]) assert.equal((await tool.execute(action("prior_art.fetch", { url }))).status, "error");
  const signal = AbortSignal.abort(new Error("Example cancellation."));
  assert.equal((await tool.execute(action("prior_art.fetch", { url: "https://example.test/page" }), { signal })).status, "error");
  assert.equal(calls, 0);
});

test("large advisory bodies remain retrievable without flooding model search cards", async () => {
  const large = osvRecord("OSV-EXAMPLE-LARGE");
  large.details = "Example long description. ".repeat(10_000);
  const result = await search(async () => json({ vulns: [large] }))({ query: "example", sources: ["osv"], commit: "revision-example" });
  assert.equal(result.output.records[0].sourceData.details, large.details);
  const text = projectModelToolResult(result).content.filter((item) => item.type === "text").map((item) => item.text).join("");
  assert.ok(text.length < 10_000);
  assert.doesNotMatch(text, /Tool result truncated/);
  assert.match(text, /detailsDeferred/);
  assert.match(text, /api.osv.dev/);
});

test("large requested result counts use smaller context pages without dropping records", async () => {
  const run = search(async () => json({ vulns: Array.from({ length: 50 }, (_, index) => osvRecord(`OSV-EXAMPLE-${index}`)) }));
  const input = { query: "example", sources: ["osv"], commit: "revision-example", maxResults: 50 };
  const ids = [];
  let cursor;
  do {
    const result = await run({ ...input, ...(cursor ? { cursor } : {}) });
    const text = projectModelToolResult(result).content.filter((item) => item.type === "text").map((item) => item.text).join("");
    assert.doesNotMatch(text, /Tool result truncated/);
    assert.ok(result.output.records.length <= 20);
    ids.push(...result.output.records.map((item) => item.id));
    cursor = result.output.nextCursor;
  } while (cursor);
  assert.equal(new Set(ids).size, 50);
});

test("explicit repository history fetch deepens a clone while preserving HEAD and dirty files", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "beale-history-fetch-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const git = async (...args) => (await promisify(execFile)("git", args, { windowsHide: true })).stdout.trim();
  const origin = join(root, "origin");
  const clone = join(root, "clone");
  await git("init", "--quiet", origin);
  for (let i = 0; i < 3; i += 1) {
    await writeFile(join(origin, "example.txt"), `Example version ${i}.\n`);
    await git("-C", origin, "add", "example.txt");
    await git("-C", origin, "-c", "user.name=Example", "-c", "user.email=example@example.test", "commit", "--quiet", "-m", `Example revision ${i}`);
  }
  await git("clone", "--quiet", "--depth=1", pathToFileURL(origin).href, clone);
  await writeFile(join(clone, "example.txt"), "Example uncommitted edit.\n");
  const head = await git("-C", clone, "rev-parse", "HEAD");
  const session = new RepositoryResearchSession();
  const history = createRepositoryHistoryTool({ roots: [clone], researchSession: session });
  const fetch = createRepositoryFetchHistoryTool({ roots: [clone], researchSession: session });
  assert.equal((await history.execute(action("repository.history", { operation: "overview" }))).output.repository.shallow, true);
  assert.equal((await fetch.execute(action("repository.fetch_history", { remote: "unknown" }))).status, "error");
  assert.equal((await fetch.execute(action("repository.fetch_history", { remote: "origin", ref: "HEAD:refs/heads/unwanted" }))).status, "error");
  const result = await fetch.execute(action("repository.fetch_history", { remote: "origin", unshallow: true }));
  assert.equal(result.status, "complete", result.error?.message);
  assert.equal(result.output.after.shallow, false);
  assert.equal(await git("-C", clone, "rev-parse", "HEAD"), head);
  assert.equal(await readFile(join(clone, "example.txt"), "utf8"), "Example uncommitted edit.\n");
  assert.equal((await history.execute(action("repository.history", { operation: "overview" }))).output.repository.shallow, false);
  assert.equal(await git("-C", clone, "rev-list", "--count", "HEAD"), "3");
});

test("new public history tools belong to Provenance and retain accurate side effects", () => {
  const tools = [createPriorArtFetchTool(), createRepositoryFetchHistoryTool()];
  for (const tool of tools) {
    assert.equal(managedToolPluginId(tool.descriptor.name), "beale-provenance");
    assert.equal(tool.descriptor.sideEffects, "network");
    assert.ok(createResearchToolRegistry([tool]).find(tool.descriptor.transportName));
  }
  assert.ok(tools[1].descriptor.requiredPermissions.includes("filesystem:write"));
});
