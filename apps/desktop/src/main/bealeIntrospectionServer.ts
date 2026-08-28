import { randomBytes, randomInt } from 'node:crypto';
import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http';

export interface BealeIntrospectionEndpoint {
  url: string;
  token: string;
}

export type BealeIntrospectionToolHandler = (
  tool: string,
  args: Record<string, unknown>,
  signal: AbortSignal
) => Promise<unknown> | unknown;

const DEFAULT_REQUEST_TIMEOUT_MS = 25_000;
const MAX_REQUEST_TIMEOUT_MS = 30_000;

export class BealeIntrospectionServer {
  private server: Server | null = null;
  private endpoint: BealeIntrospectionEndpoint | null = null;
  private readonly token = randomBytes(32).toString('hex');

  public constructor(private readonly handleTool: BealeIntrospectionToolHandler) {}

  public ensureStarted(): BealeIntrospectionEndpoint {
    if (this.endpoint) return this.endpoint;
    const server = createServer((request, response) => {
      void this.handleRequest(request, response);
    });
    const port = randomInt(49_152, 65_536);
    server.on('error', () => {
      if (this.server === server) {
        this.server = null;
        this.endpoint = null;
      }
    });
    server.listen(port, '127.0.0.1');
    this.server = server;
    this.endpoint = {
      url: `http://127.0.0.1:${port}`,
      token: this.token
    };
    return this.endpoint;
  }

  public async ensureReady(): Promise<BealeIntrospectionEndpoint> {
    let lastError: Error | null = null;
    for (let attempt = 0; attempt < 5; attempt += 1) {
      const endpoint = this.ensureStarted();
      const server = this.server;
      if (!server) continue;
      if (server.listening) return endpoint;
      try {
        await waitForListening(server);
        return endpoint;
      } catch (error) {
        lastError = error instanceof Error ? error : new Error(String(error));
      }
    }
    throw lastError ?? new Error('Beale introspection server failed to start.');
  }

  public stop(): void {
    this.endpoint = null;
    const server = this.server;
    this.server = null;
    server?.close();
  }

  private async handleRequest(request: IncomingMessage, response: ServerResponse): Promise<void> {
    const controller = new AbortController();
    const deadline = requestDeadline(request);
    const deadlineTimer = setTimeout(() => controller.abort(), Math.max(0, deadline - Date.now()));
    deadlineTimer.unref();
    const abort = (): void => controller.abort();
    request.once('aborted', abort);
    response.once('close', () => {
      if (!response.writableEnded) abort();
    });
    try {
      if (request.method !== 'POST' || request.url !== '/tool') {
        writeJson(response, 404, { ok: false, error: 'Unknown Beale introspection route.' });
        return;
      }
      if (request.headers.authorization !== `Bearer ${this.token}`) {
        writeJson(response, 401, { ok: false, error: 'Invalid Beale introspection token.' });
        return;
      }
      const payload = await readJsonBody(request);
      if (!payload || typeof payload !== 'object' || Array.isArray(payload)) {
        writeJson(response, 400, { ok: false, error: 'Request body must be a JSON object.' });
        return;
      }
      const tool = typeof payload.tool === 'string' ? payload.tool.trim() : '';
      const args = payload.args && typeof payload.args === 'object' && !Array.isArray(payload.args)
        ? payload.args as Record<string, unknown>
        : {};
      if (!tool) {
        writeJson(response, 400, { ok: false, error: 'Request body must include a tool name.' });
        return;
      }
      await new Promise<void>((resolve) => setImmediate(resolve));
      if (controller.signal.aborted || Date.now() >= deadline) {
        writeJson(response, 408, { ok: false, error: 'Beale introspection request expired before execution.' });
        return;
      }
      const result = await this.handleTool(tool, args, controller.signal);
      controller.signal.throwIfAborted();
      writeJson(response, 200, { ok: true, result });
    } catch (error) {
      if (controller.signal.aborted) {
        writeJson(response, 408, { ok: false, error: 'Beale introspection request was canceled.' });
        return;
      }
      writeJson(response, 500, {
        ok: false,
        error: error instanceof Error ? error.message : String(error)
      });
    } finally {
      clearTimeout(deadlineTimer);
      request.off('aborted', abort);
    }
  }
}

function waitForListening(server: Server): Promise<void> {
  return new Promise((resolve, reject) => {
    const cleanup = (): void => {
      server.off('listening', onListening);
      server.off('error', onError);
      server.off('close', onClose);
    };
    const onListening = (): void => {
      cleanup();
      resolve();
    };
    const onError = (error: Error): void => {
      cleanup();
      reject(error);
    };
    const onClose = (): void => {
      cleanup();
      reject(new Error('Beale introspection server closed before it became ready.'));
    };
    server.once('listening', onListening);
    server.once('error', onError);
    server.once('close', onClose);
  });
}

function requestDeadline(request: IncomingMessage): number {
  const now = Date.now();
  const raw = request.headers['x-beale-introspection-deadline'];
  const requested = typeof raw === 'string' ? Number(raw) : Number.NaN;
  if (!Number.isFinite(requested)) return now + DEFAULT_REQUEST_TIMEOUT_MS;
  return Math.min(requested, now + MAX_REQUEST_TIMEOUT_MS);
}

function readJsonBody(request: IncomingMessage): Promise<Record<string, unknown> | null> {
  const maxBytes = 1024 * 1024;
  let size = 0;
  const chunks: Buffer[] = [];
  return new Promise((resolve, reject) => {
    request.on('data', (chunk: Buffer) => {
      size += chunk.length;
      if (size > maxBytes) {
        reject(new Error('Beale introspection request body is too large.'));
        request.destroy();
        return;
      }
      chunks.push(chunk);
    });
    request.on('end', () => {
      if (chunks.length === 0) {
        resolve({});
        return;
      }
      try {
        resolve(JSON.parse(Buffer.concat(chunks).toString('utf8')) as Record<string, unknown>);
      } catch {
        reject(new Error('Request body must be valid JSON.'));
      }
    });
    request.on('error', reject);
  });
}

function writeJson(response: ServerResponse, statusCode: number, payload: Record<string, unknown>): void {
  if (response.destroyed || response.writableEnded) return;
  const body = `${JSON.stringify(payload)}\n`;
  response.writeHead(statusCode, {
    'content-type': 'application/json; charset=utf-8',
    'content-length': Buffer.byteLength(body)
  });
  response.end(body);
}
