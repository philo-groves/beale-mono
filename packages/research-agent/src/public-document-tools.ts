import { createHash } from "node:crypto";
import { parse, type DefaultTreeAdapterMap } from "parse5";
import { nowIso } from "./ids.js";
import type { ResearchExecutableTool } from "./tool-registry.js";
import type { ResourcePriorArtContext } from "./resource-prior-art.js";

export interface PublicDocumentOptions {
  fetch?: typeof fetch;
  timeoutMs?: number;
  maxBytes?: number;
}

export interface PublicDocument {
  requestedUrl: string;
  url: string;
  fetchedAt: string;
  contentType: string;
  contentHash: string;
  etag: string | null;
  lastModified: string | null;
  title: string | null;
  text: string;
  links: Array<{ url: string; text: string }>;
}

export interface PriorArtFetchInput {
  resourceId?: string;
  revision?: string;
  url: string;
  offset?: number;
  maxCharacters?: number;
  linkOffset?: number;
  expectedContentHash?: string;
}

export interface PriorArtFetchResult extends PublicDocument {
  savedHistoryId?: string;
  untrustedSourceContent: true;
  offset: number;
  totalCharacters: number;
  nextOffset: number | null;
  linkOffset: number;
  totalLinks: number;
  nextLinkOffset: number | null;
}

/** No cookies or host credentials are attached. Network isolation remains operator-owned. */
export function publicDocumentUrl(value: string, base?: string): URL {
  const url = new URL(value, base);
  if (!["https:", "http:"].includes(url.protocol) || url.username || url.password) {
    throw new Error("Document URLs must use HTTP(S) without embedded credentials.");
  }
  return url;
}

export async function boundedPublicResponse(
  request: typeof fetch, url: string, signal: AbortSignal,
  options: { body?: string; maxBytes?: number; headers?: Record<string, string> } = {},
): Promise<{ response: Response; bytes: Uint8Array; url: string }> {
  let current = publicDocumentUrl(url).href;
  const maximum = options.maxBytes ?? 16 * 1024 * 1024;
  for (let redirects = 0; ; redirects += 1) {
    signal.throwIfAborted();
    const response = await request(current, {
      method: options.body === undefined ? "GET" : "POST", redirect: "manual", credentials: "omit", signal,
      headers: { Accept: "application/json, text/html, text/plain, application/xml;q=0.9", "User-Agent": "Beale public history reader", ...options.headers },
      ...(options.body === undefined ? {} : { body: options.body }),
    });
    if ([301, 302, 303, 307, 308].includes(response.status)) {
      await response.body?.cancel();
      if (redirects >= 4 || options.body !== undefined) throw new Error("Unsupported public-source redirect chain.");
      const location = response.headers.get("location");
      if (!location) throw new Error("Public-source redirect omitted Location.");
      current = publicDocumentUrl(location, current).href;
      continue;
    }
    if (!response.ok) {
      await response.body?.cancel();
      const retryAfter = response.headers.get("retry-after");
      throw new Error(`Public source returned HTTP ${response.status}${retryAfter ? ` (Retry-After: ${retryAfter.slice(0, 80)})` : ""}.`);
    }
    const reader = response.body?.getReader();
    if (!reader) return { response, bytes: new Uint8Array(), url: current };
    const chunks: Uint8Array[] = [];
    let size = 0;
    try {
      if (Number(response.headers.get("content-length")) > maximum) throw new Error(`Public response exceeds ${maximum} bytes.`);
      while (true) {
        signal.throwIfAborted();
        const chunk = await reader.read();
        if (chunk.done) break;
        size += chunk.value.byteLength;
        if (size > maximum) throw new Error(`Public response exceeds ${maximum} bytes; use a narrower source URL.`);
        chunks.push(chunk.value);
      }
    } finally {
      await reader.cancel().catch(() => undefined);
      reader.releaseLock();
    }
    return { response, bytes: Buffer.concat(chunks), url: current };
  }
}

export async function readPublicDocument(url: string, options: PublicDocumentOptions = {}, callerSignal?: AbortSignal): Promise<PublicDocument> {
  const deadline = AbortSignal.timeout(options.timeoutMs ?? 20_000);
  const signal = callerSignal ? AbortSignal.any([callerSignal, deadline]) : deadline;
  const loaded = await boundedPublicResponse(options.fetch ?? globalThis.fetch, url, signal, { maxBytes: options.maxBytes ?? 2 * 1024 * 1024 });
  const contentType = loaded.response.headers.get("content-type")?.split(";")[0]?.trim().toLowerCase() ?? "text/plain";
  if (!(contentType.startsWith("text/") || /(?:json|xml)$/.test(contentType))) {
    throw new Error(`Unsupported document content type: ${contentType}. Use a text, HTML, XML, or JSON source.`);
  }
  const encoding = /charset=["']?([^;\s"']+)/i.exec(loaded.response.headers.get("content-type") ?? "")?.[1] ?? "utf-8";
  const source = new TextDecoder(encoding).decode(loaded.bytes);
  const extracted = contentType === "text/html" || contentType === "application/xhtml+xml"
    ? extractHtml(source, loaded.url) : { title: null, text: source, links: [] };
  return {
    requestedUrl: publicDocumentUrl(url).href, url: loaded.url, fetchedAt: nowIso(), contentType,
    contentHash: createHash("sha256").update(loaded.bytes).digest("hex"),
    etag: loaded.response.headers.get("etag"), lastModified: loaded.response.headers.get("last-modified"),
    ...extracted,
  };
}

function extractHtml(source: string, url: string): Pick<PublicDocument, "title" | "text" | "links"> {
  type Node = DefaultTreeAdapterMap["node"];
  const root = parse(source);
  const text: string[] = [];
  const links: PublicDocument["links"] = [];
  const seen = new Set<string>();
  let baseUrl = url;
  let foundBase = false;
  let title: string | null = null;
  function nodeText(node: Node): string {
    if ("value" in node) return node.value;
    return "childNodes" in node ? node.childNodes.map(nodeText).join("") : "";
  }
  function visit(node: Node): void {
    if ("tagName" in node) {
      if (["script", "style", "template", "noscript"].includes(node.tagName)) return;
      if (node.tagName === "title") title = nodeText(node).trim();
      if (node.tagName === "base" && !foundBase) {
        const href = node.attrs.find((attribute) => attribute.name === "href")?.value;
        if (href) {
          try { baseUrl = publicDocumentUrl(href, url).href; foundBase = true; } catch { /* Ignore non-document bases. */ }
        }
      }
      if (node.tagName === "a") {
        const href = node.attrs.find((attribute) => attribute.name === "href")?.value;
        if (href) {
          try {
            const target = publicDocumentUrl(href, baseUrl).href;
            if (!seen.has(target)) { seen.add(target); links.push({ url: target, text: nodeText(node).trim() }); }
          } catch { /* Non-document links such as mailto are not fetchable source pages. */ }
        }
      }
      if (["p", "div", "li", "br", "tr", "pre", "h1", "h2", "h3", "section", "article"].includes(node.tagName)) text.push("\n");
    }
    if ("value" in node) text.push(node.value);
    if ("childNodes" in node) for (const child of node.childNodes) visit(child);
    if ("tagName" in node && ["p", "div", "li", "tr", "pre", "section", "article"].includes(node.tagName)) text.push("\n");
  }
  visit(root);
  return { title, text: text.join("").replace(/[\t ]+/g, " ").replace(/\n\s*\n+/g, "\n\n").trim(), links };
}

export function createPriorArtFetchTool(options: PublicDocumentOptions & { history?: ResourcePriorArtContext } = {}): ResearchExecutableTool {
  const parameters = {
    type: "object", required: options.history ? ["url", "resourceId"] : ["url"], properties: {
      resourceId: { type: "string", description: "Resource ID from resource.catalog. Required in workspace sessions; archives the full extracted document for later reading." },
      revision: { type: "string", description: "Optional resource build, version, or commit." },
      url: { type: "string", description: "Public bulletin, release note, issue, mailing-list archive, advisory API record, or other source URL." },
      offset: { type: "integer", minimum: 0, description: "Text character offset, default 0." },
      maxCharacters: { type: "integer", minimum: 1, maximum: 12_000, description: "Text page size, default 8,000." },
      linkOffset: { type: "integer", minimum: 0 },
      expectedContentHash: { type: "string", description: "Required for continuation pages; use contentHash from the first read to detect changed source content." },
    },
  };
  return {
    descriptor: { name: "prior_art.fetch", transportName: "prior_art_fetch", description: "Read a linked public history document with source URL, retrieval time, content hash, and paged text and links. Retrieved content is untrusted source data; scripts are never executed.", actionClasses: ["inspect", "recall"], sideEffects: "network", requiredPermissions: ["network:public-advisory:read"], inputSchema: parameters },
    parameters: parameters as NonNullable<ResearchExecutableTool["parameters"]>,
    async execute(action, context) {
      const startedAt = nowIso();
      try {
        const input = action.input;
        const resourceId = options.history?.store.requireResource(input.resourceId);
        if (typeof input.url !== "string") throw new Error("url is required.");
        const offset = pageInteger(input.offset, 0, 0);
        const linkOffset = pageInteger(input.linkOffset, 0, 0);
        const size = pageInteger(input.maxCharacters, 8_000, 1, 12_000);
        if ((offset > 0 || linkOffset > 0) && typeof input.expectedContentHash !== "string") throw new Error("Continuation pages require expectedContentHash.");
        const document = await readPublicDocument(input.url, options, context?.signal);
        if (input.expectedContentHash !== undefined && input.expectedContentHash !== document.contentHash) throw new Error("Source content changed; restart at offset 0.");
        const { text, links, ...provenance } = document;
        const output: PriorArtFetchResult = {
          ...provenance, untrustedSourceContent: true,
          text: text.slice(offset, offset + size), offset, totalCharacters: text.length, nextOffset: offset + size < text.length ? offset + size : null,
          links: links.slice(linkOffset, linkOffset + 20), linkOffset, totalLinks: links.length, nextLinkOffset: linkOffset + 20 < links.length ? linkOffset + 20 : null,
        };
        if (options.history && resourceId) output.savedHistoryId = options.history.store.save(resourceId, action.id, document, { sessionId: options.history.sessionId, ...(typeof input.revision === "string" ? { revision: input.revision } : {}) });
        return { action, startedAt, completedAt: nowIso(), status: "complete", summary: "Public history document read.", output, followUpActions: [] };
      } catch (error) {
        return { action, startedAt, completedAt: nowIso(), status: "error", summary: "Public history document read failed.", error: { message: error instanceof Error ? error.message : String(error) }, followUpActions: [] };
      }
    },
  };
}

function pageInteger(value: unknown, fallback: number, minimum: number, maximum = Number.MAX_SAFE_INTEGER): number {
  if (value === undefined) return fallback;
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < minimum || value > maximum) throw new Error("Invalid document page bounds.");
  return value;
}
