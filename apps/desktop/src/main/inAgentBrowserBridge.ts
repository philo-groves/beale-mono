import { randomUUID } from 'node:crypto';
import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http';
import { rmSync } from 'node:fs';
import { mkdir, writeFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { dirname, join } from 'node:path';
import type { WebContents } from 'electron';
import { WebSocket, WebSocketServer } from 'ws';
import { browserContextLabel, DEFAULT_BROWSER_CONTEXT, MAX_BROWSER_CONTEXTS } from '../shared/browserContexts';
import type { BrowserContextSummary, BrowserContextsUpdate } from '../shared/browserContexts';

const DISCOVERY_FILE = join(homedir(), '.beale', 'desktop-browser.json');
const MAX_MESSAGE_BYTES = 8_000_000;

export function allowedBrowserUrl(value: string): boolean {
  try {
    const protocol = new URL(value).protocol;
    return protocol === 'http:' || protocol === 'https:' || value === 'about:blank';
  } catch {
    return false;
  }
}

export function allowedBrowserCommand(method: string): boolean {
  return Boolean(method.trim()) && !/^(?:Target|Browser|SystemInfo|IO)\./u.test(method);
}

interface CdpRequest {
  id: number;
  method: string;
  params?: Record<string, unknown>;
  sessionId?: string;
}

interface BrowserContextState {
  summary: BrowserContextSummary;
  guest: WebContents | null;
  client: WebSocket | null;
}

function sendJson(response: ServerResponse, status: number, payload: unknown): void {
  response.writeHead(status, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' });
  response.end(JSON.stringify(payload));
}

async function readJsonBody(request: IncomingMessage): Promise<unknown> {
  let body = '';
  for await (const chunk of request) {
    body += String(chunk);
    if (body.length > 1024) throw new Error('Browser context request is too large.');
  }
  return JSON.parse(body) as unknown;
}

function cdpRequest(value: unknown): CdpRequest | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const record = value as Record<string, unknown>;
  if (!Number.isSafeInteger(record.id) || typeof record.method !== 'string') return null;
  if (record.params !== undefined && (!record.params || typeof record.params !== 'object' || Array.isArray(record.params))) return null;
  if (record.sessionId !== undefined && typeof record.sessionId !== 'string') return null;
  return record as unknown as CdpRequest;
}

/** Exposes only isolated embedded browser guests to the app-server's run-local CDP client. */
export class InAgentBrowserBridge {
  readonly #token = randomUUID();
  readonly #server: Server;
  readonly #sockets = new WebSocketServer({ noServer: true, maxPayload: MAX_MESSAGE_BYTES });
  readonly #contexts = new Map<string, BrowserContextState>([[DEFAULT_BROWSER_CONTEXT.id, {
    summary: DEFAULT_BROWSER_CONTEXT,
    guest: null,
    client: null
  }]]);
  #port = 0;
  readonly #discoveryFile: string;
  readonly #onChange: (update: BrowserContextsUpdate) => void;
  readonly #onAcknowledge: (sessionId: string) => Promise<boolean>;
  readonly #acknowledgedSessions = new Set<string>();
  readonly #pendingAcknowledgements = new Map<string, Promise<boolean>>();

  constructor(
    discoveryFile = DISCOVERY_FILE,
    onChange: (update: BrowserContextsUpdate) => void = () => undefined,
    onAcknowledge: (sessionId: string) => Promise<boolean> = async () => false
  ) {
    this.#discoveryFile = discoveryFile;
    this.#onChange = onChange;
    this.#onAcknowledge = onAcknowledge;
    this.#server = createServer((request, response) => {
      void this.#handleRequest(request, response).catch(() => {
        if (!response.headersSent) response.writeHead(500);
        response.end();
      });
    });
    this.#server.on('upgrade', (request, socket, head) => {
      const url = new URL(request.url ?? '/', 'http://127.0.0.1');
      const targetId = url.pathname.startsWith('/devtools/page/') ? url.pathname.slice('/devtools/page/'.length) : '';
      const context = [...this.#contexts.values()].find((candidate) =>
        candidate.guest && !candidate.guest.isDestroyed() && String(candidate.guest.id) === targetId
      );
      const sessionId = url.searchParams.get('sessionId');
      if (url.searchParams.get('token') !== this.#token || !context || !sessionId || !this.#acknowledgedSessions.has(sessionId)) {
        socket.destroy();
        return;
      }
      this.#sockets.handleUpgrade(request, socket, head, (client) => this.#connect(context, client));
    });
  }

  listContexts(): BrowserContextSummary[] {
    return [...this.#contexts.values()].map((context) => context.summary);
  }

  contextIdForPartition(partition: string): string | null {
    return [...this.#contexts.values()].find((context) => context.summary.partition === partition)?.summary.id ?? null;
  }

  createContext(value: unknown): BrowserContextSummary {
    const label = browserContextLabel(value);
    if (!label) throw new Error('Browser context labels must be one word of at most 24 letters, numbers, hyphens, or underscores.');
    if (this.#contexts.size >= MAX_BROWSER_CONTEXTS) throw new Error(`At most ${MAX_BROWSER_CONTEXTS} browser contexts can be open.`);
    if (this.listContexts().some((context) => context.label.toLowerCase() === label.toLowerCase())) {
      throw new Error('A browser context with that label already exists.');
    }
    const id = randomUUID();
    const summary = { id, label, partition: `beale-in-agent-browser-${id}` };
    this.#contexts.set(id, { summary, guest: null, client: null });
    this.#onChange({ contexts: this.listContexts(), createdId: id });
    return summary;
  }

  renameContext(id: string, value: unknown): BrowserContextSummary {
    const context = this.#contexts.get(id);
    if (!context) throw new Error('Browser context was not found.');
    const label = browserContextLabel(value);
    if (!label) throw new Error('Browser context labels must be one word of at most 24 letters, numbers, hyphens, or underscores.');
    if (this.listContexts().some((candidate) => candidate.id !== id && candidate.label.toLowerCase() === label.toLowerCase())) {
      throw new Error('A browser context with that label already exists.');
    }
    context.summary = { ...context.summary, label };
    this.#onChange({ contexts: this.listContexts() });
    return context.summary;
  }

  async openContext(id: string): Promise<BrowserContextSummary> {
    const context = this.#contexts.get(id);
    if (!context) throw new Error('Browser context was not found.');
    this.#onChange({ contexts: this.listContexts(), openedId: id });
    if (!context.guest || context.guest.isDestroyed()) {
      await new Promise<void>((resolve, reject) => {
        const deadline = Date.now() + 5_000;
        const check = (): void => {
          if (context.guest && !context.guest.isDestroyed()) resolve();
          else if (Date.now() >= deadline) reject(new Error('The browser context did not open in the sidebar.'));
          else setTimeout(check, 50);
        };
        check();
      });
    }
    return context.summary;
  }

  async acknowledgeSession(sessionId: unknown): Promise<boolean> {
    if (typeof sessionId !== 'string' || !/^[A-Za-z0-9_.:-]{1,128}$/u.test(sessionId)) {
      throw new Error('Invalid research session identity.');
    }
    if (this.#acknowledgedSessions.has(sessionId)) return true;
    let pending = this.#pendingAcknowledgements.get(sessionId);
    if (!pending) {
      pending = this.#onAcknowledge(sessionId).then((accepted) => {
        if (accepted) this.#acknowledgedSessions.add(sessionId);
        return accepted;
      }).finally(() => this.#pendingAcknowledgements.delete(sessionId));
      this.#pendingAcknowledgements.set(sessionId, pending);
    }
    return pending;
  }

  removeContext(id: string): void {
    if (id === DEFAULT_BROWSER_CONTEXT.id) throw new Error('The default browser context cannot be removed.');
    const context = this.#contexts.get(id);
    if (!context) throw new Error('Browser context was not found.');
    this.#contexts.delete(id);
    const guest = context.guest;
    this.#detach(context);
    if (guest && !guest.isDestroyed()) void guest.session?.clearStorageData();
    this.#onChange({ contexts: this.listContexts(), removedId: id });
  }

  async start(): Promise<void> {
    await new Promise<void>((resolve, reject) => {
      this.#server.once('error', reject);
      this.#server.listen(0, '127.0.0.1', () => {
        this.#server.off('error', reject);
        resolve();
      });
    });
    const address = this.#server.address();
    if (!address || typeof address === 'string') throw new Error('Browser bridge did not bind a TCP port.');
    this.#port = address.port;
    await mkdir(dirname(this.#discoveryFile), { recursive: true });
    await writeFile(this.#discoveryFile, JSON.stringify({ endpoint: `http://127.0.0.1:${this.#port}?token=${this.#token}` }), { mode: 0o600 });
  }

  attach(contextId: string, guest: WebContents): void {
    const context = this.#contexts.get(contextId);
    if (!context) throw new Error('Browser context was not found.');
    this.#detach(context);
    context.guest = guest;
    guest.setWindowOpenHandler(() => ({ action: 'deny' }));
    guest.on('will-navigate', (event, url) => {
      if (!allowedBrowserUrl(url)) event.preventDefault();
    });
    guest.on('will-redirect', (event, url) => {
      if (!allowedBrowserUrl(url)) event.preventDefault();
    });
    guest.on('destroyed', () => {
      if (context.guest === guest) this.#detach(context);
    });
  }

  stop(): void {
    for (const context of this.#contexts.values()) this.#detach(context);
    rmSync(this.#discoveryFile, { force: true });
    this.#sockets.close();
    if (this.#server.listening) this.#server.close();
  }

  async #handleRequest(request: IncomingMessage, response: ServerResponse): Promise<void> {
    const url = new URL(request.url ?? '/', 'http://127.0.0.1');
    if (url.searchParams.get('token') !== this.#token) {
      response.writeHead(403).end();
      return;
    }
    const targets = [...this.#contexts.values()].flatMap((context) => {
      const guest = context.guest;
      if (!guest || guest.isDestroyed()) return [];
      return [{
        id: String(guest.id),
        type: 'page',
        title: `Browser [${context.summary.label}] ${guest.getTitle()}`.trim(),
        description: `Beale browser context: ${context.summary.label}`,
        url: guest.getURL(),
        webSocketDebuggerUrl: `ws://127.0.0.1:${this.#port}/devtools/page/${guest.id}?token=${this.#token}`
      }];
    });
    if (request.method === 'GET' && url.pathname === '/json/list') {
      sendJson(response, 200, targets);
      return;
    }
    if (request.method === 'GET' && url.pathname === '/json/version') {
      const defaultGuest = this.#contexts.get(DEFAULT_BROWSER_CONTEXT.id)?.guest;
      const target = targets.find((candidate) => candidate.id === String(defaultGuest?.id)) ?? targets[0];
      if (!target) response.writeHead(404).end();
      else sendJson(response, 200, { Browser: 'Beale embedded browser', webSocketDebuggerUrl: target.webSocketDebuggerUrl });
      return;
    }
    if (request.method === 'GET' && url.pathname === '/contexts') {
      sendJson(response, 200, this.listContexts().map(({ id, label }) => ({ id, label })));
      return;
    }
    try {
      if (request.method === 'POST' && url.pathname === '/sessions/acknowledge') {
        const input = await readJsonBody(request);
        const sessionId = input && typeof input === 'object' && 'sessionId' in input ? input.sessionId : null;
        const acknowledged = await this.acknowledgeSession(sessionId);
        sendJson(response, acknowledged ? 200 : 403, { acknowledged });
        return;
      }
      if (request.method === 'POST' && url.pathname === '/contexts') {
        const input = await readJsonBody(request);
        const label = input && typeof input === 'object' && 'label' in input ? input.label : null;
        const { id, label: createdLabel } = this.createContext(label);
        sendJson(response, 201, { id, label: createdLabel });
        return;
      }
      if (request.method === 'PATCH' && url.pathname.startsWith('/contexts/')) {
        const input = await readJsonBody(request);
        const label = input && typeof input === 'object' && 'label' in input ? input.label : null;
        const { id, label: renamedLabel } = this.renameContext(decodeURIComponent(url.pathname.slice('/contexts/'.length)), label);
        sendJson(response, 200, { id, label: renamedLabel });
        return;
      }
      if (request.method === 'POST' && url.pathname.startsWith('/contexts/') && url.pathname.endsWith('/open')) {
        const id = decodeURIComponent(url.pathname.slice('/contexts/'.length, -'/open'.length));
        const { label } = await this.openContext(id);
        sendJson(response, 200, { id, label });
        return;
      }
      if (request.method === 'DELETE' && url.pathname.startsWith('/contexts/')) {
        this.removeContext(decodeURIComponent(url.pathname.slice('/contexts/'.length)));
        sendJson(response, 200, { removed: true });
        return;
      }
    } catch (error) {
      sendJson(response, 400, { error: error instanceof Error ? error.message : String(error) });
      return;
    }
    response.writeHead(404).end();
  }

  #connect(context: BrowserContextState, client: WebSocket): void {
    const guest = context.guest;
    if (!guest || guest.isDestroyed()) {
      client.close();
      return;
    }
    context.client?.close();
    context.client = client;
    try {
      if (!guest.debugger.isAttached()) guest.debugger.attach('1.3');
    } catch {
      client.close();
      return;
    }
    const onMessage = (_event: Electron.Event, method: string, params: unknown, sessionId?: string): void => {
      if (client.readyState === WebSocket.OPEN) client.send(JSON.stringify({ method, params, ...(sessionId ? { sessionId } : {}) }));
    };
    const onDetach = (): void => client.close();
    guest.debugger.on('message', onMessage);
    guest.debugger.on('detach', onDetach);
    client.on('message', (raw) => {
      let request: CdpRequest | null = null;
      try { request = cdpRequest(JSON.parse(String(raw))); } catch { /* Invalid CDP frame. */ }
      if (!request) return;
      const { id, method, params, sessionId } = request;
      if (context.client !== client || guest.isDestroyed()) {
        client.close();
        return;
      }
      if (!allowedBrowserCommand(method)) {
        client.send(JSON.stringify({ id, error: { code: -32601, message: 'CDP domain is unavailable for the embedded browser.' } }));
        return;
      }
      if (method === 'Page.navigate' && (typeof params?.url !== 'string' || !allowedBrowserUrl(params.url))) {
        client.send(JSON.stringify({ id, error: { code: -32602, message: 'Only HTTP, HTTPS, and about:blank page URLs are supported.' } }));
        return;
      }
      try {
        void guest.debugger.sendCommand(method, params ?? {}, sessionId).then(
          (result) => {
            if (client.readyState === WebSocket.OPEN) client.send(JSON.stringify({ id, result }));
          },
          (error: unknown) => {
            if (client.readyState === WebSocket.OPEN) client.send(JSON.stringify({ id, error: { code: -32000, message: error instanceof Error ? error.message : String(error) } }));
          }
        );
      } catch (error) {
        if (client.readyState === WebSocket.OPEN) client.send(JSON.stringify({ id, error: { code: -32000, message: error instanceof Error ? error.message : String(error) } }));
      }
    });
    client.on('close', () => {
      const activeClient = context.client === client;
      if (activeClient) context.client = null;
      if (guest.isDestroyed()) return;
      guest.debugger.off('message', onMessage);
      guest.debugger.off('detach', onDetach);
      if (activeClient && guest.debugger.isAttached()) guest.debugger.detach();
    });
  }

  #detach(context: BrowserContextState): void {
    context.client?.close();
    context.client = null;
    const guest = context.guest;
    if (guest && !guest.isDestroyed() && guest.debugger.isAttached()) guest.debugger.detach();
    context.guest = null;
  }
}
