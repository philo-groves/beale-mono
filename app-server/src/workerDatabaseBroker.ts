import { resolve } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { serialize } from 'node:v8';
import type { WorkerDatabaseRequest, WorkerDatabaseRequestMessage } from './workerDatabaseClient.js';

const RESPONSE_HEADER_BYTES = 8;

interface WorkerDatabaseResponse {
  ok: boolean;
  value?: unknown;
  error?: { message: string; code?: string };
}

export class AppServerWorkerDatabaseBroker {
  private readonly connections = new Map<number, DatabaseSync>();
  private nextConnectionId = 1;

  public constructor(private readonly allowedDatabasePath: string) {}

  public handle(message: WorkerDatabaseRequestMessage): void {
    let response: WorkerDatabaseResponse;
    try {
      response = { ok: true, value: this.execute(message.request) };
    } catch (error) {
      response = {
        ok: false,
        error: {
          message: error instanceof Error ? error.message : String(error),
          ...(error && typeof error === 'object' && 'code' in error && typeof error.code === 'string'
            ? { code: error.code }
            : {})
        }
      };
    }
    writeWorkerDatabaseResponse(message.responseBuffer, response);
  }

  public close(): void {
    for (const database of this.connections.values()) {
      try { database.close(); } catch { /* The worker may already have closed it. */ }
    }
    this.connections.clear();
  }

  private execute(request: WorkerDatabaseRequest): unknown {
    if (request.operation === 'open') {
      const databasePath = requiredText(request.databasePath, 'databasePath');
      if (!sameDatabasePath(databasePath, this.allowedDatabasePath)) {
        throw new Error('The runtime worker requested storage outside its app-server-owned database.');
      }
      const connectionId = this.nextConnectionId++;
      this.connections.set(
        connectionId,
        request.options ? new DatabaseSync(databasePath, request.options) : new DatabaseSync(databasePath)
      );
      return connectionId;
    }

    const connectionId = requiredConnectionId(request.connectionId);
    const database = this.connections.get(connectionId);
    if (!database) throw new Error(`The app-server database connection ${connectionId} is not open.`);
    if (request.operation === 'close') {
      database.close();
      this.connections.delete(connectionId);
      return undefined;
    }
    const sql = requiredText(request.sql, 'sql');
    if (request.operation === 'exec') {
      database.exec(sql);
      return undefined;
    }
    const statement = database.prepare(sql);
    const parameters = request.parameters ?? [];
    if (request.operation === 'all') {
      return (statement.all as (...values: unknown[]) => unknown[])(...parameters);
    }
    if (request.operation === 'get') {
      return (statement.get as (...values: unknown[]) => unknown)(...parameters);
    }
    return (statement.run as (...values: unknown[]) => unknown)(...parameters);
  }
}

function writeWorkerDatabaseResponse(buffer: SharedArrayBuffer, response: WorkerDatabaseResponse): void {
  const header = new Int32Array(buffer, 0, 2);
  let bytes = serialize(response);
  const capacity = buffer.byteLength - RESPONSE_HEADER_BYTES;
  if (bytes.byteLength > capacity) {
    bytes = serialize({
      ok: false,
      error: { message: `The app-server database response exceeds ${capacity} bytes.` }
    } satisfies WorkerDatabaseResponse);
  }
  new Uint8Array(buffer, RESPONSE_HEADER_BYTES, bytes.byteLength).set(bytes);
  Atomics.store(header, 1, bytes.byteLength);
  Atomics.store(header, 0, 1);
  Atomics.notify(header, 0);
}

function sameDatabasePath(candidate: string, allowed: string): boolean {
  if (candidate === ':memory:' || allowed === ':memory:') return candidate === allowed;
  return normalizePath(candidate) === normalizePath(allowed);
}

function normalizePath(path: string): string {
  const normalized = resolve(path);
  return process.platform === 'win32' ? normalized.toLowerCase() : normalized;
}

function requiredConnectionId(value: number | undefined): number {
  if (!Number.isInteger(value) || (value ?? 0) <= 0) throw new Error('A valid app-server database connection ID is required.');
  return value!;
}

function requiredText(value: string | undefined, name: string): string {
  if (!value?.trim()) throw new Error(`${name} is required.`);
  return value;
}
