import { spawn, execFile, type ChildProcess } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, readdirSync } from 'node:fs';
import { mkdir } from 'node:fs/promises';
import { createWriteStream } from 'node:fs';
import { homedir } from 'node:os';
import { join, resolve } from 'node:path';
import { pipeline } from 'node:stream/promises';
import { Readable } from 'node:stream';
import { promisify } from 'node:util';
import { WebSocket } from 'ws';

const execFileAsync = promisify(execFile);
const ROOT = join(homedir(), '.beale', 'fleet-browser');
const BOOT_TIMEOUT_MS = 120_000;
const MAX_BROWSER_FRAME_BYTES = 700_000;
const MAX_AGENT_FRAME_BYTES = 8_000_000;
const MAX_DOWNLOAD_BYTES = 350_000_000;
const SESSION_ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/u;

interface ChromeTarget { id: string; type: string; title: string; url: string; webSocketDebuggerUrl: string }
interface BrowserSession {
  token: string;
  endpoint: string;
  child: ChildProcess | null;
  viewers: Set<WebSocket>;
  page: WebSocket | null;
  pageLaunching: Promise<void> | null;
  targetId: string | null;
  commandId: number;
  controller: WebSocket | null;
  leaseUntil: number;
  lastFrame: string | null;
  lastUrl: string;
  lastWidth: number;
  lastHeight: number;
  pendingCommands: Map<number, (result: Record<string, unknown>) => void>;
}

function record(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

function safeSessionId(value: string): string {
  if (!SESSION_ID.test(value)) throw new Error('Invalid Fleet browser session identity.');
  return value;
}

function send(socket: WebSocket, value: unknown): void {
  if (socket.readyState === WebSocket.OPEN && socket.bufferedAmount < 2 * MAX_BROWSER_FRAME_BYTES) {
    socket.send(JSON.stringify(value));
  }
}

/** Browser state stays on the session VM; this class only relays pixels and control. */
export class FleetBrowserService {
  private readonly sessions = new Map<string, BrowserSession>();
  private readonly launching = new Map<string, Promise<BrowserSession>>();

  public constructor(private readonly endpointOverride?: (sessionId: string) => Promise<string>) {}

  public tokenFor(sessionId: string): string {
    safeSessionId(sessionId);
    let session = this.sessions.get(sessionId);
    if (!session) {
      session = { token: randomBytes(24).toString('base64url'), endpoint: '', child: null,
        viewers: new Set(), page: null, pageLaunching: null, targetId: null, commandId: 1, controller: null,
        leaseUntil: 0, lastFrame: null, lastUrl: 'about:blank', lastWidth: 1280,
        lastHeight: 800, pendingCommands: new Map() };
      this.sessions.set(sessionId, session);
    }
    return session.token;
  }

  public authorized(sessionId: string, token: string): boolean {
    return this.sessions.get(safeSessionId(sessionId))?.token === token && Boolean(token);
  }

  public hasSession(sessionId: string): boolean {
    return this.sessions.has(safeSessionId(sessionId));
  }

  public async discovery(sessionId: string, kind: 'list' | 'version' | 'contexts', baseUrl: string): Promise<unknown> {
    const session = await this.ensure(sessionId);
    if (kind === 'contexts') return [{ id: 'default', label: 'Default' }];
    const targets = await this.targets(session);
    const socketBase = new URL(baseUrl);
    socketBase.protocol = socketBase.protocol === 'https:' ? 'wss:' : 'ws:';
    const mapped = targets.map((target) => ({ id: target.id, type: target.type, title: target.title,
      url: target.url, description: 'Fleet VM browser page',
      webSocketDebuggerUrl: `${socketBase.origin}/v1/fleet-browser/${encodeURIComponent(sessionId)}/devtools/page/${encodeURIComponent(target.id)}?token=${session.token}` }));
    if (kind === 'list') return mapped;
    const first = mapped[0];
    if (!first) throw new Error('Fleet browser has no page target.');
    return { Browser: 'Beale Fleet browser', webSocketDebuggerUrl: first.webSocketDebuggerUrl };
  }

  public async agentSocket(sessionId: string, targetId: string, client: WebSocket): Promise<void> {
    const early: Array<{ data: Buffer; binary: boolean }> = [];
    let earlyBytes = 0;
    const onEarly = (data: unknown, binary: boolean): void => {
      const frame = Buffer.from(String(data));
      earlyBytes += frame.byteLength;
      if (earlyBytes > MAX_AGENT_FRAME_BYTES) { client.close(1009); return; }
      early.push({ data: frame, binary });
    };
    client.on('message', onEarly);
    const session = await this.ensure(sessionId);
    if (client.readyState !== WebSocket.OPEN) return;
    const target = (await this.targets(session)).find((entry) => entry.id === targetId);
    if (!target) { client.close(1008, 'Browser page is unavailable'); return; }
    const upstream = new WebSocket(target.webSocketDebuggerUrl, { maxPayload: MAX_AGENT_FRAME_BYTES });
    const queued: Array<{ data: Buffer; binary: boolean }> = [];
    let queuedBytes = 0;
    const forward = (data: unknown, binary: boolean): void => {
      const frame = Buffer.from(String(data));
      if (frame.byteLength > MAX_AGENT_FRAME_BYTES) { client.close(1009); return; }
      if (!binary && session.controller && session.leaseUntil > Date.now()) {
        try {
          const command: unknown = JSON.parse(String(data));
          if (record(command) && typeof command.method === 'string' && /^(?:Input\.|Page\.(?:navigate|reload|goBack|goForward))/u.test(command.method)) {
            send(client, { id: command.id, error: { code: -32000, message: 'The researcher is controlling this browser page.' } });
            return;
          }
        } catch { /* Chrome validates malformed commands. */ }
      }
      if (upstream.readyState === WebSocket.OPEN) upstream.send(frame, { binary });
      else if (upstream.readyState === WebSocket.CONNECTING && queuedBytes + frame.byteLength <= MAX_AGENT_FRAME_BYTES) {
        queued.push({ data: frame, binary }); queuedBytes += frame.byteLength;
      } else client.close(1011, 'Browser CDP connection unavailable');
    };
    client.off('message', onEarly);
    client.on('message', forward);
    for (const entry of early) forward(entry.data, entry.binary);
    upstream.on('open', () => { for (const entry of queued) upstream.send(entry.data, { binary: entry.binary }); queued.length = 0; });
    upstream.on('message', (data, binary) => { if (client.readyState === WebSocket.OPEN) client.send(data, { binary }); });
    upstream.on('close', () => { if (client.readyState === WebSocket.OPEN) client.close(1001); });
    upstream.on('error', () => { if (client.readyState === WebSocket.OPEN) client.close(1011); });
    client.on('close', () => upstream.close());
  }

  public async viewerSocket(sessionId: string, client: WebSocket): Promise<void> {
    const session = await this.ensure(sessionId);
    if (client.readyState !== WebSocket.OPEN) return;
    session.viewers.add(client);
    try { await this.ensurePageStream(session); }
    catch (error) { session.viewers.delete(client); throw error; }
    if (client.readyState !== WebSocket.OPEN) { session.viewers.delete(client); return; }
    send(client, { type: 'browser.ready', url: session.lastUrl, width: session.lastWidth, height: session.lastHeight });
    if (session.lastFrame) send(client, JSON.parse(session.lastFrame) as unknown);
    client.on('message', (raw) => { void this.handleViewerMessage(session, client, raw.toString()).catch((error: unknown) => {
      send(client, { type: 'browser.error', message: error instanceof Error ? error.message : String(error) });
    }); });
    client.on('close', () => {
      session.viewers.delete(client);
      if (session.controller === client) { session.controller = null; session.leaseUntil = 0; }
      if (session.viewers.size === 0 && session.page?.readyState === WebSocket.OPEN) {
        void this.command(session, 'Page.stopScreencast').catch(() => undefined);
      }
    });
  }

  public close(): void {
    for (const session of this.sessions.values()) {
      for (const viewer of session.viewers) viewer.close(1001);
      session.page?.close();
      session.child?.kill();
    }
    this.sessions.clear();
  }

  private async handleViewerMessage(session: BrowserSession, client: WebSocket, raw: string): Promise<void> {
    if (raw.length > 16_384) throw new Error('Browser input is too large.');
    const message: unknown = JSON.parse(raw);
    if (!record(message) || typeof message.type !== 'string' || !Number.isSafeInteger(message.id)) throw new Error('Invalid browser input.');
    if (message.type === 'browser.release') {
      if (session.controller === client) { session.controller = null; session.leaseUntil = 0; }
      send(client, { type: 'browser.ack', id: message.id });
      return;
    }
    if (session.controller && session.controller !== client && session.leaseUntil > Date.now()) throw new Error('Another researcher controls this browser page.');
    session.controller = client;
    session.leaseUntil = Date.now() + 10_000;
    if (message.type === 'browser.navigate') {
      if (typeof message.url !== 'string' || !allowedPageUrl(message.url)) throw new Error('Only HTTP and HTTPS page URLs are supported.');
      await this.command(session, 'Page.navigate', { url: message.url });
    } else if (message.type === 'browser.mouse') {
      if (!['mousePressed', 'mouseReleased', 'mouseMoved', 'mouseWheel'].includes(String(message.eventType))
        || !finiteCoordinate(message.x, session.lastWidth) || !finiteCoordinate(message.y, session.lastHeight)) throw new Error('Invalid browser pointer event.');
      await this.command(session, 'Input.dispatchMouseEvent', { type: message.eventType, x: message.x, y: message.y,
        ...(typeof message.button === 'string' ? { button: message.button } : {}),
        ...(typeof message.buttons === 'number' ? { buttons: message.buttons } : {}),
        ...(typeof message.clickCount === 'number' ? { clickCount: message.clickCount } : {}),
        ...(message.eventType === 'mouseWheel' ? { deltaX: Number(message.deltaX) || 0, deltaY: Number(message.deltaY) || 0 } : {}) });
    } else if (message.type === 'browser.key') {
      if (!['keyDown', 'keyUp', 'rawKeyDown', 'char'].includes(String(message.eventType))
        || typeof message.key !== 'string' || message.key.length > 40
        || typeof message.code !== 'string' || message.code.length > 40) throw new Error('Invalid browser key event.');
      await this.command(session, 'Input.dispatchKeyEvent', { type: message.eventType, key: message.key, code: message.code,
        text: typeof message.text === 'string' ? message.text.slice(0, 8) : '',
        modifiers: Number.isInteger(message.modifiers) ? message.modifiers : 0 });
    } else if (message.type === 'browser.text') {
      if (typeof message.text !== 'string' || message.text.length > 4096) throw new Error('Invalid browser text input.');
      await this.command(session, 'Input.insertText', { text: message.text });
    } else throw new Error('Unsupported browser input.');
    send(client, { type: 'browser.ack', id: message.id });
  }

  private async ensure(sessionId: string): Promise<BrowserSession> {
    safeSessionId(sessionId);
    this.tokenFor(sessionId);
    const session = this.sessions.get(sessionId)!;
    if (session.endpoint && await browserHealthy(session.endpoint)) return session;
    let pending = this.launching.get(sessionId);
    if (!pending) {
      pending = this.launch(sessionId, session).finally(() => this.launching.delete(sessionId));
      this.launching.set(sessionId, pending);
    }
    return pending;
  }

  private async launch(sessionId: string, session: BrowserSession): Promise<BrowserSession> {
    if (this.endpointOverride) {
      session.endpoint = await this.endpointOverride(sessionId);
      return session;
    }
    const profile = join(ROOT, 'sessions', sessionId, 'profile');
    mkdirSync(profile, { recursive: true, mode: 0o700 });
    const activePort = join(profile, 'DevToolsActivePort');
    const existing = readChromePort(activePort);
    if (existing && await browserHealthy(`http://127.0.0.1:${existing}`)) {
      session.endpoint = `http://127.0.0.1:${existing}`;
      return session;
    }
    const executable = await chromeExecutable();
    const child = spawn(executable, ['--headless=new', '--remote-debugging-address=127.0.0.1', '--remote-debugging-port=0',
      `--user-data-dir=${profile}`, '--no-first-run', '--no-default-browser-check', '--window-size=1280,800',
      ...(process.getuid?.() === 0 ? ['--no-sandbox'] : []), 'about:blank'],
    { stdio: 'ignore', windowsHide: true });
    let launchError: Error | null = null;
    child.on('error', (error) => { launchError = error; });
    session.child = child;
    const deadline = Date.now() + BOOT_TIMEOUT_MS;
    while (Date.now() < deadline) {
      if (launchError) throw launchError;
      if (child.exitCode !== null) throw new Error('Fleet browser exited before its CDP endpoint became available.');
      const port = readChromePort(activePort);
      if (port && await browserHealthy(`http://127.0.0.1:${port}`)) {
        session.endpoint = `http://127.0.0.1:${port}`;
        return session;
      }
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 250));
    }
    child.kill();
    throw new Error('Fleet browser did not start within two minutes.');
  }

  private async targets(session: BrowserSession): Promise<ChromeTarget[]> {
    const response = await fetch(`${session.endpoint}/json/list`, { signal: AbortSignal.timeout(10_000) });
    if (!response.ok) throw new Error('Fleet browser target discovery failed.');
    const value: unknown = await response.json();
    if (!Array.isArray(value)) throw new Error('Fleet browser returned invalid targets.');
    return value.filter((entry): entry is ChromeTarget => record(entry) && entry.type === 'page'
      && typeof entry.id === 'string' && typeof entry.webSocketDebuggerUrl === 'string'
      && typeof entry.title === 'string' && typeof entry.url === 'string');
  }

  private async ensurePageStream(session: BrowserSession): Promise<void> {
    if (!session.pageLaunching) {
      session.pageLaunching = this.openPageStream(session).finally(() => { session.pageLaunching = null; });
    }
    return session.pageLaunching;
  }

  private async openPageStream(session: BrowserSession): Promise<void> {
    if (session.page?.readyState === WebSocket.OPEN) {
      await this.command(session, 'Page.startScreencast', { format: 'jpeg', quality: 60, maxWidth: 1280, maxHeight: 800 });
      return;
    }
    const page = (await this.targets(session))[0];
    if (!page) throw new Error('Fleet browser has no page target.');
    session.targetId = page.id;
    session.lastUrl = page.url;
    const socket = new WebSocket(page.webSocketDebuggerUrl, { maxPayload: MAX_AGENT_FRAME_BYTES });
    await new Promise<void>((resolveOpen, rejectOpen) => {
      socket.once('open', resolveOpen);
      socket.once('error', rejectOpen);
    });
    session.page = socket;
    socket.on('message', (raw) => {
      let value: unknown;
      try { value = JSON.parse(String(raw)); } catch { return; }
      if (!record(value)) return;
      if (typeof value.id === 'number') {
        const resolveCommand = session.pendingCommands.get(value.id);
        session.pendingCommands.delete(value.id);
        resolveCommand?.(value);
      }
      if (value.method === 'Page.frameNavigated' && record(value.params) && record(value.params.frame)
        && typeof value.params.frame.url === 'string') {
        session.lastUrl = value.params.frame.url;
        for (const viewer of session.viewers) send(viewer, { type: 'browser.location', url: session.lastUrl });
      }
      if (value.method === 'Page.screencastFrame' && record(value.params)) {
        const { data, metadata, sessionId } = value.params;
        if (typeof sessionId === 'number' && session.page?.readyState === WebSocket.OPEN) {
          session.page.send(JSON.stringify({ id: session.commandId++, method: 'Page.screencastFrameAck', params: { sessionId } }));
        }
        if (typeof data !== 'string' || data.length > MAX_BROWSER_FRAME_BYTES * 1.37) return;
        if (record(metadata)) {
          if (typeof metadata.deviceWidth === 'number') session.lastWidth = metadata.deviceWidth;
          if (typeof metadata.deviceHeight === 'number') session.lastHeight = metadata.deviceHeight;
        }
        const frame = JSON.stringify({ type: 'browser.frame', mime: 'image/jpeg', data,
          width: session.lastWidth, height: session.lastHeight });
        session.lastFrame = frame;
        for (const viewer of session.viewers) {
          if (viewer.readyState === WebSocket.OPEN && viewer.bufferedAmount < MAX_BROWSER_FRAME_BYTES) viewer.send(frame);
        }
      }
    });
    socket.on('close', () => {
      if (session.page === socket) session.page = null;
      for (const resolveCommand of session.pendingCommands.values()) resolveCommand({ error: { message: 'Browser CDP connection closed.' } });
      session.pendingCommands.clear();
      for (const viewer of session.viewers) send(viewer, { type: 'browser.disconnected' });
    });
    await this.command(session, 'Page.enable');
    await this.command(session, 'Page.startScreencast', { format: 'jpeg', quality: 60, maxWidth: 1280, maxHeight: 800 });
  }

  private async command(session: BrowserSession, method: string, params: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
    if (!session.page || session.page.readyState !== WebSocket.OPEN) throw new Error('Fleet browser is disconnected.');
    const id = session.commandId++;
    return new Promise((resolveCommand, rejectCommand) => {
      const timer = setTimeout(() => { session.pendingCommands.delete(id); rejectCommand(new Error('Fleet browser command timed out.')); }, 30_000);
      session.pendingCommands.set(id, (response) => {
        clearTimeout(timer);
        if (response.error) rejectCommand(new Error(record(response.error) && typeof response.error.message === 'string'
          ? response.error.message : 'Fleet browser command failed.'));
        else resolveCommand(response);
      });
      session.page!.send(JSON.stringify({ id, method, params }));
    });
  }
}

function allowedPageUrl(value: string): boolean {
  try { const url = new URL(value); return url.protocol === 'http:' || url.protocol === 'https:'; }
  catch { return false; }
}

function finiteCoordinate(value: unknown, max: number): boolean {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 && value <= max;
}

function readChromePort(path: string): number | null {
  try {
    const first = readFileSync(path, 'utf8').split(/\r?\n/u)[0];
    const port = Number(first);
    return Number.isInteger(port) && port > 0 && port <= 65_535 ? port : null;
  } catch { return null; }
}

async function browserHealthy(endpoint: string): Promise<boolean> {
  try { return (await fetch(`${endpoint}/json/version`, { signal: AbortSignal.timeout(1_000) })).ok; }
  catch { return false; }
}

async function chromeExecutable(): Promise<string> {
  const override = process.env.BEALE_FLEET_CHROME_PATH?.trim();
  if (override) {
    const path = resolve(override);
    if (!existsSync(path)) throw new Error('BEALE_FLEET_CHROME_PATH does not exist in the guest.');
    return path;
  }
  const candidates = process.platform === 'darwin'
    ? ['/Applications/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing',
      '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome']
    : process.platform === 'win32'
      ? [join(process.env.PROGRAMFILES ?? '', 'Google', 'Chrome', 'Application', 'chrome.exe'),
        join(process.env['PROGRAMFILES(X86)'] ?? '', 'Microsoft', 'Edge', 'Application', 'msedge.exe')]
      : ['/usr/bin/google-chrome', '/usr/bin/chromium', '/usr/bin/chromium-browser'];
  const installed = candidates.find((path) => existsSync(path));
  if (installed) return installed;
  const cached = join(ROOT, 'binaries');
  if (existsSync(cached)) {
    for (const version of readdirSync(cached).sort().reverse()) {
      const platformName = process.platform === 'darwin' ? process.arch === 'arm64' ? 'mac-arm64' : 'mac-x64'
        : process.platform === 'win32' ? 'win64' : 'linux64';
      const path = process.platform === 'darwin'
        ? join(cached, version, `chrome-${platformName}`, 'Google Chrome for Testing.app', 'Contents', 'MacOS', 'Google Chrome for Testing')
        : process.platform === 'win32' ? join(cached, version, 'chrome-win64', 'chrome.exe')
          : join(cached, version, 'chrome-linux64', 'chrome');
      if (existsSync(path)) return path;
    }
  }
  const platform = process.platform === 'darwin' ? process.arch === 'arm64' ? 'mac-arm64' : 'mac-x64'
    : process.platform === 'win32' ? 'win64' : process.arch === 'x64' ? 'linux64' : null;
  if (!platform) throw new Error('Fleet browser has no Chrome for Testing build for this guest platform.');
  const manifestResponse = await fetch('https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json',
    { signal: AbortSignal.timeout(30_000) });
  if (!manifestResponse.ok) throw new Error('Fleet could not find a Chrome for Testing build.');
  const manifest: unknown = await manifestResponse.json();
  const stable = record(manifest) && record(manifest.channels) && record(manifest.channels.Stable) ? manifest.channels.Stable : null;
  const downloads = stable && record(stable.downloads) && Array.isArray(stable.downloads.chrome) ? stable.downloads.chrome : [];
  const selected = downloads.find((entry: unknown) => record(entry) && entry.platform === platform && typeof entry.url === 'string');
  if (!record(selected) || typeof selected.url !== 'string' || typeof stable?.version !== 'string') throw new Error('Chrome for Testing does not list this VM platform.');
  const url = new URL(selected.url);
  if (url.protocol !== 'https:' || url.hostname !== 'storage.googleapis.com'
    || !url.pathname.startsWith('/chrome-for-testing-public/')) throw new Error('Unexpected Chrome for Testing download URL.');
  const directory = join(ROOT, 'binaries', stable.version);
  const executable = process.platform === 'darwin'
    ? join(directory, `chrome-${platform}`, 'Google Chrome for Testing.app', 'Contents', 'MacOS', 'Google Chrome for Testing')
    : process.platform === 'win32' ? join(directory, 'chrome-win64', 'chrome.exe')
      : join(directory, 'chrome-linux64', 'chrome');
  if (existsSync(executable)) return executable;
  await mkdir(directory, { recursive: true });
  const archive = join(directory, 'chrome.zip');
  const response = await fetch(url, { signal: AbortSignal.timeout(10 * 60_000) });
  if (!response.ok || !response.body) throw new Error('Fleet could not download Chrome for Testing.');
  const length = Number(response.headers.get('content-length') ?? 0);
  if (length > MAX_DOWNLOAD_BYTES) throw new Error('Fleet browser archive exceeds the download limit.');
  let downloaded = 0;
  const chunks = Readable.fromWeb(response.body as never);
  chunks.on('data', (chunk: Buffer) => {
    downloaded += chunk.length;
    if (downloaded > MAX_DOWNLOAD_BYTES) chunks.destroy(new Error('Fleet browser archive exceeds the download limit.'));
  });
  await pipeline(chunks, createWriteStream(archive, { mode: 0o600 }));
  if (process.platform === 'win32') {
    const command = `Expand-Archive -LiteralPath '${archive.replaceAll("'", "''")}' -DestinationPath '${directory.replaceAll("'", "''")}' -Force`;
    await execFileAsync('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', command], { timeout: 5 * 60_000 });
  } else {
    await execFileAsync('unzip', ['-q', archive, '-d', directory], { timeout: 5 * 60_000 });
  }
  if (!existsSync(executable)) throw new Error('Fleet browser archive did not contain Chrome.');
  return executable;
}
