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

  async targets(endpoint = DEFAULT_ENDPOINT, signal?: AbortSignal): Promise<Record<string, unknown>[]> {
    const base = browserHttpEndpoint(endpoint);
    const response = await fetch(new URL("/json/list", base), { signal: AbortSignal.any([signal ?? new AbortController().signal, AbortSignal.timeout(CONNECT_TIMEOUT_MS)]) });
    if (!response.ok) throw new Error(`CDP target discovery failed with HTTP ${response.status}.`);
    const body: unknown = await response.json();
    if (!Array.isArray(body)) throw new Error("CDP target discovery returned an invalid response.");
    return body.filter(isRecord).map((target) => Object.fromEntries(
      ["id", "type", "title", "url", "description", "attached"].flatMap((key) =>
        key in target ? [[key, target[key]]] : []),
    ));
  }

  async connect(endpoint = DEFAULT_ENDPOINT, targetId?: string, signal?: AbortSignal): Promise<string> {
    let socketUrl: string;
    const parsed = new URL(endpoint);
    if (parsed.protocol === "ws:" || parsed.protocol === "wss:") {
      if (targetId) throw new Error("targetId requires an HTTP CDP discovery endpoint.");
      socketUrl = parsed.href;
    } else {
      const base = browserHttpEndpoint(endpoint);
      const discovery = new URL(targetId ? "/json/list" : "/json/version", base);
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
  const endpoint = { type: "string", description: `HTTP(S) CDP discovery endpoint or, for connect, a direct WebSocket URL. Defaults to ${DEFAULT_ENDPOINT}.` };
  const connectionId = { type: "string", description: "Connection ID returned by browser.connect." };
  return [
    tool("browser.targets", "List debuggable targets from a compatible browser's CDP discovery endpoint.", { endpoint }, [], async (input, context) => ({ targets: await session.targets(typeof input.endpoint === "string" ? input.endpoint : undefined, context?.signal) })),
    tool("browser.connect", "Open a run-local CDP WebSocket. Omit targetId for the browser endpoint, which supports Target.attachToTarget with flatten=true. A targetId connects directly to that page target.", { endpoint, targetId: { type: "string" } }, [], async (input, context) => ({ connectionId: await session.connect(typeof input.endpoint === "string" ? input.endpoint : undefined, typeof input.targetId === "string" ? input.targetId : undefined, context?.signal) })),
    tool("browser.command", "Send any Chrome DevTools Protocol method and parameters. Pass sessionId for flattened target sessions. CDP errors are returned with their code and message.", { connectionId, method: { type: "string" }, params: { type: "object", additionalProperties: true }, sessionId: { type: "string" } }, ["connectionId", "method"], async (input, context) => {
      const response = await session.command(String(input.connectionId), String(input.method), isRecord(input.params) ? input.params : {}, typeof input.sessionId === "string" ? input.sessionId : undefined, context?.signal);
      return response.error ? { error: response.error } : { result: response.result === undefined ? {} : response.result, ...(response.sessionId ? { sessionId: response.sessionId } : {}) };
    }),
    tool("browser.events", "Read buffered CDP events in arrival order. Optionally wait up to 30 seconds. Reports dropped events if the 500-event buffer overflowed.", { connectionId, maxEvents: { type: "integer", minimum: 1, maximum: 100 }, waitMs: { type: "integer", minimum: 0, maximum: 30_000 } }, ["connectionId"], async (input, context) => session.events(String(input.connectionId), typeof input.maxEvents === "number" ? input.maxEvents : undefined, typeof input.waitMs === "number" ? input.waitMs : undefined, context?.signal)),
    tool("browser.disconnect", "Close a CDP connection and release its run-local state.", { connectionId }, ["connectionId"], async (input) => { session.disconnect(String(input.connectionId)); return { disconnected: true }; }),
  ];
}
