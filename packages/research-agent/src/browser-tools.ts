import { randomUUID } from "node:crypto";
import { nowIso } from "./ids.js";
import type { ResearchExecutableTool, ResearchToolExecutionContext, ResearchToolExecutionResult } from "./tool-registry.js";
import type { ResearchToolAction } from "./types.js";

const DEFAULT_ENDPOINT = "http://127.0.0.1:9222";
const CONNECT_TIMEOUT_MS = 10_000;
const COMMAND_TIMEOUT_MS = 120_000;
const MAX_EVENTS = 500;
const MAX_EVENT_CHARACTERS = 8_000_000;

interface CdpMessage {
  id?: number;
  method?: string;
  params?: unknown;
  result?: unknown;
  error?: { code?: number; message?: string; data?: unknown };
  sessionId?: string;
}

interface PendingCommand {
  resolve(value: CdpMessage): void;
  reject(error: Error): void;
}

interface BrowserConnection {
  socket: WebSocket;
  nextId: number;
  pending: Map<number, PendingCommand>;
  events: CdpMessage[];
  eventCharacters: number;
  droppedEvents: number;
  wake?: () => void;
}

/** A run-local CDP transport. No browser or credential state is persisted. */
export class BrowserCdpSession {
  readonly #connections = new Map<string, BrowserConnection>();
  readonly #defaultEndpoint: () => Promise<string>;
  readonly #researchSessionId: string | undefined;

  constructor(defaultEndpoint: () => Promise<string> = async () => DEFAULT_ENDPOINT, researchSessionId?: string) {
    this.#defaultEndpoint = defaultEndpoint;
    this.#researchSessionId = researchSessionId;
  }

  async contexts(signal?: AbortSignal): Promise<{ id: string; label: string }[]> {
    const response = await fetch(browserDiscoveryUrl('/contexts', browserHttpEndpoint(await this.#defaultEndpoint())), {
      signal: AbortSignal.any([signal ?? new AbortController().signal, AbortSignal.timeout(CONNECT_TIMEOUT_MS)])
    });
    if (!response.ok) throw new Error(`Embedded browser context listing failed with HTTP ${response.status}.`);
    const body: unknown = await response.json();
    if (!Array.isArray(body)) throw new Error('Embedded browser returned invalid contexts.');
    return body.filter((value): value is { id: string; label: string } =>
      isRecord(value) && typeof value.id === 'string' && typeof value.label === 'string'
    ).map(({ id, label }) => ({ id, label }));
  }

  async createContext(label: string, signal?: AbortSignal): Promise<{ id: string; label: string }> {
    const response = await fetch(browserDiscoveryUrl('/contexts', browserHttpEndpoint(await this.#defaultEndpoint())), {
      method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ label }),
      signal: AbortSignal.any([signal ?? new AbortController().signal, AbortSignal.timeout(CONNECT_TIMEOUT_MS)])
    });
    const body: unknown = await response.json();
    if (!response.ok) throw new Error(isRecord(body) && typeof body.error === 'string' ? body.error : `Browser context creation failed with HTTP ${response.status}.`);
    if (!isRecord(body) || typeof body.id !== 'string' || typeof body.label !== 'string') throw new Error('Embedded browser returned an invalid context.');
    return { id: body.id, label: body.label };
  }

  async closeContext(id: string, signal?: AbortSignal): Promise<void> {
    const response = await fetch(browserDiscoveryUrl(`/contexts/${encodeURIComponent(id)}`, browserHttpEndpoint(await this.#defaultEndpoint())), {
      method: 'DELETE', signal: AbortSignal.any([signal ?? new AbortController().signal, AbortSignal.timeout(CONNECT_TIMEOUT_MS)])
    });
    if (!response.ok) {
      const body: unknown = await response.json();
      throw new Error(isRecord(body) && typeof body.error === 'string' ? body.error : `Browser context close failed with HTTP ${response.status}.`);
    }
  }

  async openContext(id: string, signal?: AbortSignal): Promise<{ id: string; label: string }> {
    const response = await fetch(browserDiscoveryUrl(`/contexts/${encodeURIComponent(id)}/open`, browserHttpEndpoint(await this.#defaultEndpoint())), {
      method: 'POST', signal: AbortSignal.any([signal ?? new AbortController().signal, AbortSignal.timeout(CONNECT_TIMEOUT_MS)])
    });
    const body: unknown = await response.json();
    if (!response.ok) throw new Error(isRecord(body) && typeof body.error === 'string' ? body.error : `Browser context open failed with HTTP ${response.status}.`);
    if (!isRecord(body) || typeof body.id !== 'string' || typeof body.label !== 'string') throw new Error('Embedded browser returned an invalid context.');
    return { id: body.id, label: body.label };
  }

  async targets(endpoint?: string, signal?: AbortSignal): Promise<Record<string, unknown>[]> {
    const base = browserHttpEndpoint(endpoint ?? await this.#defaultEndpoint());
    const response = await fetch(browserDiscoveryUrl("/json/list", base), { signal: AbortSignal.any([signal ?? new AbortController().signal, AbortSignal.timeout(CONNECT_TIMEOUT_MS)]) });
    if (!response.ok) throw new Error(`CDP target discovery failed with HTTP ${response.status}.`);
    const body: unknown = await response.json();
    if (!Array.isArray(body)) throw new Error("CDP target discovery returned an invalid response.");
    const embeddedTarget = body.find((target) => {
      if (!isRecord(target) || typeof target.webSocketDebuggerUrl !== 'string') return false;
      try { return isEmbeddedBrowserSocket(new URL(target.webSocketDebuggerUrl)); } catch { return false; }
    });
    if (isRecord(embeddedTarget) && typeof embeddedTarget.webSocketDebuggerUrl === 'string') {
      await this.#acknowledgeEmbeddedSocket(new URL(embeddedTarget.webSocketDebuggerUrl), signal);
    }
    return body.filter(isRecord).map((target) => Object.fromEntries(
      ["id", "type", "title", "url", "description", "attached"].flatMap((key) =>
        key in target ? [[key, target[key]]] : []),
    ));
  }

  async connect(endpoint?: string, targetId?: string, signal?: AbortSignal): Promise<string> {
    let socketUrl: string;
    const parsed = new URL(endpoint ?? await this.#defaultEndpoint());
    if (parsed.protocol === "ws:" || parsed.protocol === "wss:") {
      if (targetId) throw new Error("targetId requires an HTTP CDP discovery endpoint.");
      socketUrl = parsed.href;
    } else {
      const base = browserHttpEndpoint(parsed.href);
      const discovery = browserDiscoveryUrl(targetId ? "/json/list" : "/json/version", base);
      const response = await fetch(discovery, { signal: AbortSignal.any([signal ?? new AbortController().signal, AbortSignal.timeout(CONNECT_TIMEOUT_MS)]) });
      if (!response.ok) throw new Error(`CDP discovery failed with HTTP ${response.status}.`);
      const body: unknown = await response.json();
      const entry = targetId && Array.isArray(body)
        ? body.find((candidate) => isRecord(candidate) && candidate.id === targetId)
        : body;
      if (!isRecord(entry) || typeof entry.webSocketDebuggerUrl !== "string") {
        throw new Error(targetId ? "CDP target was not found or has no debugger WebSocket." : "CDP browser WebSocket was not advertised.");
      }
      socketUrl = entry.webSocketDebuggerUrl;
    }
    const wsUrl = new URL(socketUrl);
    if (wsUrl.protocol !== "ws:" && wsUrl.protocol !== "wss:") throw new Error("CDP debugger URL must use ws or wss.");
    if (isEmbeddedBrowserSocket(wsUrl)) {
      await this.#acknowledgeEmbeddedSocket(wsUrl, signal);
      wsUrl.searchParams.set('sessionId', this.#researchSessionId!);
    }
    const socket = new WebSocket(wsUrl.href);
    try {
      await new Promise<void>((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error("CDP connection timed out.")), CONNECT_TIMEOUT_MS);
        const abort = () => reject(new Error("CDP connection was canceled."));
        signal?.addEventListener("abort", abort, { once: true });
        socket.addEventListener("open", () => { clearTimeout(timer); signal?.removeEventListener("abort", abort); resolve(); }, { once: true });
        socket.addEventListener("error", () => { clearTimeout(timer); signal?.removeEventListener("abort", abort); reject(new Error("CDP WebSocket connection failed.")); }, { once: true });
      });
    } catch (error) {
      socket.close();
      throw error;
    }
    const connection: BrowserConnection = { socket, nextId: 1, pending: new Map(), events: [], eventCharacters: 0, droppedEvents: 0 };
    const id = randomUUID();
    socket.addEventListener("message", (event) => this.#receive(connection, event.data));
    socket.addEventListener("close", () => this.#closed(id, connection));
    this.#connections.set(id, connection);
    return id;
  }

  async command(connectionId: string, method: string, params: Record<string, unknown> = {}, sessionId?: string, signal?: AbortSignal): Promise<CdpMessage> {
    const connection = this.#require(connectionId);
    if (!method.trim()) throw new Error("CDP method is required.");
    const id = connection.nextId++;
    return new Promise<CdpMessage>((resolve, reject) => {
      const timer = setTimeout(() => finish(new Error("CDP command timed out.")), COMMAND_TIMEOUT_MS);
      const abort = () => finish(new Error("CDP command was canceled."));
      const finish = (error?: Error, response?: CdpMessage) => {
        clearTimeout(timer);
        signal?.removeEventListener("abort", abort);
        connection.pending.delete(id);
        if (error) reject(error);
        else resolve(response!);
      };
      if (signal?.aborted) return finish(new Error("CDP command was canceled."));
      signal?.addEventListener("abort", abort, { once: true });
      connection.pending.set(id, { resolve: (value) => finish(undefined, value), reject: (error) => finish(error) });
      try {
        connection.socket.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
      } catch (error) {
        finish(error instanceof Error ? error : new Error(String(error)));
      }
    });
  }

  async events(connectionId: string, maxEvents = 50, waitMs = 0, signal?: AbortSignal): Promise<{ events: CdpMessage[]; droppedEvents: number }> {
    const connection = this.#require(connectionId);
    if (connection.events.length === 0 && waitMs > 0) {
      await new Promise<void>((resolve) => {
        const timer = setTimeout(done, Math.min(waitMs, 30_000));
        const abort = () => done();
        function done() { clearTimeout(timer); signal?.removeEventListener("abort", abort); delete connection.wake; resolve(); }
        connection.wake = done;
        signal?.addEventListener("abort", abort, { once: true });
        if (signal?.aborted) done();
      });
    }
    const events = connection.events.splice(0, Math.max(1, Math.min(maxEvents, 100)));
    connection.eventCharacters -= events.reduce((total, event) => total + JSON.stringify(event).length, 0);
    const droppedEvents = connection.droppedEvents;
    connection.droppedEvents = 0;
    return { events, droppedEvents };
  }

  disconnect(connectionId: string): void {
    const connection = this.#require(connectionId);
    connection.socket.close();
    this.#closed(connectionId, connection);
  }

  async cleanup(): Promise<void> {
    for (const id of [...this.#connections.keys()]) this.disconnect(id);
  }

  async #acknowledgeEmbeddedSocket(wsUrl: URL, signal?: AbortSignal): Promise<void> {
    if (!this.#researchSessionId) throw new Error('A research session is required to use the embedded browser.');
    const acknowledgementEndpoint = new URL('/sessions/acknowledge', wsUrl);
    acknowledgementEndpoint.protocol = wsUrl.protocol === 'wss:' ? 'https:' : 'http:';
    acknowledgementEndpoint.search = wsUrl.search;
    const acknowledgement = await fetch(acknowledgementEndpoint, {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ sessionId: this.#researchSessionId }),
      signal: AbortSignal.any([signal ?? new AbortController().signal, AbortSignal.timeout(COMMAND_TIMEOUT_MS)])
    });
    if (!acknowledgement.ok) throw new Error('Browser access was not acknowledged for this research session.');
  }

  #require(id: string): BrowserConnection {
    const connection = this.#connections.get(id);
    if (!connection) throw new Error("Unknown or closed CDP connection.");
    return connection;
  }

  #receive(connection: BrowserConnection, raw: unknown): void {
    try {
      const message: unknown = JSON.parse(String(raw));
      if (!isRecord(message)) return;
      const packet = message as CdpMessage;
      if (typeof packet.id === "number") {
        connection.pending.get(packet.id)?.resolve(packet);
      } else if (typeof packet.method === "string") {
        const characters = JSON.stringify(packet).length;
        while (connection.events.length > 0 && (connection.events.length >= MAX_EVENTS || connection.eventCharacters + characters > MAX_EVENT_CHARACTERS)) {
          connection.eventCharacters -= JSON.stringify(connection.events.shift()).length;
          connection.droppedEvents++;
        }
        if (characters > MAX_EVENT_CHARACTERS) connection.droppedEvents++;
        else {
          connection.events.push(packet);
          connection.eventCharacters += characters;
        }
        connection.wake?.();
      }
    } catch { /* Ignore malformed browser frames. */ }
  }

  #closed(id: string, connection: BrowserConnection): void {
    if (this.#connections.get(id) !== connection) return;
    this.#connections.delete(id);
    for (const pending of connection.pending.values()) pending.reject(new Error("CDP connection closed."));
    connection.pending.clear();
    connection.wake?.();
  }
}

function browserHttpEndpoint(endpoint: string): URL {
  const parsed = new URL(endpoint);
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") throw new Error("CDP discovery endpoint must use http or https.");
  return parsed;
}

function browserDiscoveryUrl(path: string, base: URL): URL {
  const discovery = new URL(path, base);
  discovery.search = base.search;
  return discovery;
}

function isEmbeddedBrowserSocket(url: URL): boolean {
  return url.hostname === '127.0.0.1'
    && url.pathname.startsWith('/devtools/page/')
    && url.searchParams.has('token');
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function createBrowserTools(session: BrowserCdpSession): ResearchExecutableTool[] {
  const tool = (name: string, description: string, properties: Record<string, unknown>, required: string[], run: (input: Record<string, unknown>, context?: ResearchToolExecutionContext) => Promise<unknown>): ResearchExecutableTool => {
    const parameters = { type: "object", properties, required, additionalProperties: false } as const;
    return {
      descriptor: { name, transportName: name.replaceAll(".", "_"), description, actionClasses: ["inspect", "experiment"], sideEffects: "process", requiredPermissions: ["browser:control"], inputSchema: parameters },
      parameters,
      async execute(action: ResearchToolAction, context?: ResearchToolExecutionContext): Promise<ResearchToolExecutionResult> {
        const startedAt = nowIso();
        try {
          const output = await run(action.input, context);
          if (name === "browser.command" && isRecord(output) && "error" in output) {
            return { action, status: "error", startedAt, completedAt: nowIso(), summary: "CDP command failed.", output, followUpActions: [] };
          }
          return { action, status: "complete", startedAt, completedAt: nowIso(), summary: `${name} completed.`, output, followUpActions: [] };
        } catch (error) {
          const message = error instanceof Error ? error.message : String(error);
          return { action, status: "error", startedAt, completedAt: nowIso(), summary: message, error: { message }, followUpActions: [] };
        }
      },
    };
  };
  const endpoint = { type: "string", description: `HTTP(S) CDP discovery endpoint or, for connect, a direct WebSocket URL. Defaults to Beale's embedded browser when available, otherwise ${DEFAULT_ENDPOINT}.` };
  const connectionId = { type: "string", description: "Connection ID returned by browser.connect." };
  return [
    tool("browser.contexts", "List Beale's isolated embedded browser contexts and their one-word labels. Open the Browser sidebar tab to attach their pages.", {}, [], async (_input, context) => ({ contexts: await session.contexts(context?.signal) })),
    tool("browser.context.create", "Create a separate embedded browser context with isolated cookies and storage. The label must be one word, up to 24 letters, numbers, hyphens, or underscores. Use browser.targets to find its page after the tab opens.", { label: { type: "string", minLength: 1, maxLength: 24, pattern: "^[A-Za-z0-9_][A-Za-z0-9_-]*$" } }, ["label"], async (input, context) => ({ context: await session.createContext(String(input.label), context?.signal) })),
    tool("browser.context.close", "Close a named embedded browser context and its page. The Default context cannot be closed.", { contextId: { type: "string" } }, ["contextId"], async (input, context) => { await session.closeContext(String(input.contextId), context?.signal); return { closed: true }; }),
    tool("browser.context.open", "Open an existing embedded browser context in the sidebar so this research session can attach to its page. Use browser.contexts to find its ID, then browser.targets and browser.connect. Browser access requires one acknowledgement per research session.", { contextId: { type: "string" } }, ["contextId"], async (input, context) => ({ context: await session.openContext(String(input.contextId), context?.signal) })),
    tool("browser.targets", "List debuggable targets. Beale's embedded page appears after its Browser tab opens; first access in a research session asks the user to acknowledge full CDP access.", { endpoint }, [], async (input, context) => ({ targets: await session.targets(typeof input.endpoint === "string" ? input.endpoint : undefined, context?.signal) })),
    tool("browser.connect", "Open a run-local CDP WebSocket. Open or reuse a Beale Browser context, then omit targetId for its default page or select a listed targetId. The user's acknowledgement is required once per research session. An explicit endpoint can connect to an external browser.", { endpoint, targetId: { type: "string" } }, [], async (input, context) => ({ connectionId: await session.connect(typeof input.endpoint === "string" ? input.endpoint : undefined, typeof input.targetId === "string" ? input.targetId : undefined, context?.signal) })),
    tool("browser.command", "Send any Chrome DevTools Protocol method and parameters. Pass sessionId for flattened target sessions. CDP errors are returned with their code and message.", { connectionId, method: { type: "string" }, params: { type: "object", additionalProperties: true }, sessionId: { type: "string" } }, ["connectionId", "method"], async (input, context) => {
      const response = await session.command(String(input.connectionId), String(input.method), isRecord(input.params) ? input.params : {}, typeof input.sessionId === "string" ? input.sessionId : undefined, context?.signal);
      return response.error ? { error: response.error } : { result: response.result === undefined ? {} : response.result, ...(response.sessionId ? { sessionId: response.sessionId } : {}) };
    }),
    tool("browser.events", "Read buffered CDP events in arrival order. Optionally wait up to 30 seconds. Reports dropped events if the 500-event buffer overflowed.", { connectionId, maxEvents: { type: "integer", minimum: 1, maximum: 100 }, waitMs: { type: "integer", minimum: 0, maximum: 30_000 } }, ["connectionId"], async (input, context) => session.events(String(input.connectionId), typeof input.maxEvents === "number" ? input.maxEvents : undefined, typeof input.waitMs === "number" ? input.waitMs : undefined, context?.signal)),
    tool("browser.disconnect", "Close a CDP connection and release its run-local state.", { connectionId }, ["connectionId"], async (input) => { session.disconnect(String(input.connectionId)); return { disconnected: true }; }),
  ];
}
