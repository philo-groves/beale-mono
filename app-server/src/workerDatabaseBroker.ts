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

interface CoordinatedDatabaseRequest {
  readonly kind: 'worker';
  readonly broker: AppServerWorkerDatabaseBroker;
  readonly databaseKey: string;
  readonly message: WorkerDatabaseRequestMessage;
  readonly execute: () => boolean;
  readonly reject: (message: string) => void;
}

interface CoordinatedHostDatabaseRequest {
  readonly kind: 'host';
  readonly databaseKey: string;
  readonly execute: () => void;
}

type PendingDatabaseRequest = CoordinatedDatabaseRequest | CoordinatedHostDatabaseRequest;

interface DatabaseTransactionOwner {
  readonly broker: AppServerWorkerDatabaseBroker;
  readonly connectionId: number;
}

interface CoordinatedDatabaseState {
  owner: DatabaseTransactionOwner | null;
  readonly pending: PendingDatabaseRequest[];
  draining: boolean;
}

/**
 * Serializes brokered connections while one runtime worker owns an explicit
 * transaction. Without this lease, a second worker can block the app-server
 * event loop inside SQLite while the first worker is waiting for that same
 * event loop to deliver COMMIT or ROLLBACK.
 */
export class AppServerWorkerDatabaseCoordinator {
  private readonly databases = new Map<string, CoordinatedDatabaseState>();

  public dispatch(request: CoordinatedDatabaseRequest): void {
    const state = this.stateFor(request.databaseKey);
    const connectionId = requestConnectionId(request.message.request);
    if (state.owner && !sameOwner(state.owner, request.broker, connectionId)) {
      state.pending.push(request);
      return;
    }
    this.execute(request, state, connectionId);
    if (!state.owner && state.pending.length === 0) this.databases.delete(request.databaseKey);
  }

  public runWhenAvailable<T>(databasePath: string, operation: () => Promise<T> | T): Promise<T> {
    const databaseKey = normalizedDatabaseKey(databasePath);
    return new Promise<T>((resolvePromise, rejectPromise) => {
      const request: CoordinatedHostDatabaseRequest = {
        kind: 'host',
        databaseKey,
        execute: () => {
          try {
            Promise.resolve(operation()).then(resolvePromise, rejectPromise);
          } catch (error) {
            rejectPromise(error);
          }
        }
      };
      const state = this.stateFor(databaseKey);
      if (state.owner) {
        state.pending.push(request);
      } else {
        request.execute();
        if (state.pending.length === 0) this.databases.delete(databaseKey);
      }
    });
  }

  public unregister(broker: AppServerWorkerDatabaseBroker, databaseKey: string): void {
    const state = this.databases.get(databaseKey);
    if (!state) return;

    const retained: PendingDatabaseRequest[] = [];
    for (const request of state.pending) {
      if (request.kind === 'worker' && request.broker === broker) {
        request.reject('The app-server database broker closed before the queued operation could run.');
      } else {
        retained.push(request);
      }
    }
    state.pending.splice(0, state.pending.length, ...retained);
    if (state.owner?.broker === broker) state.owner = null;
    this.drain(databaseKey, state);
  }

  private execute(
    request: CoordinatedDatabaseRequest,
    state: CoordinatedDatabaseState,
    connectionId: number | null
  ): void {
    const transactionBoundary = explicitTransactionBoundary(request.message.request);
    const succeeded = request.execute();
    if (!succeeded) return;

    if (transactionBoundary === 'begin' && connectionId !== null) {
      state.owner = { broker: request.broker, connectionId };
      return;
    }
    const ownedConnection = state.owner && sameOwner(state.owner, request.broker, connectionId);
    if (ownedConnection && (transactionBoundary === 'end' || request.message.request.operation === 'close')) {
      state.owner = null;
      this.drain(request.databaseKey, state);
    }
  }

  private drain(databaseKey: string, state: CoordinatedDatabaseState): void {
    if (state.draining) return;
    state.draining = true;
    try {
      while (!state.owner && state.pending.length > 0) {
        const request = state.pending.shift()!;
        if (request.kind === 'host') request.execute();
        else this.execute(request, state, requestConnectionId(request.message.request));
      }
    } finally {
      state.draining = false;
      if (!state.owner && state.pending.length === 0) this.databases.delete(databaseKey);
    }
  }

  private stateFor(databaseKey: string): CoordinatedDatabaseState {
    let state = this.databases.get(databaseKey);
    if (!state) {
      state = { owner: null, pending: [], draining: false };
      this.databases.set(databaseKey, state);
    }
    return state;
  }
}

export class AppServerWorkerDatabaseBroker {
  private readonly connections = new Map<number, DatabaseSync>();
  private readonly databaseKey: string;
  private closed = false;
  private nextConnectionId = 1;

  public constructor(
    private readonly allowedDatabasePath: string,
    private readonly coordinator = new AppServerWorkerDatabaseCoordinator()
  ) {
    this.databaseKey = normalizedDatabaseKey(allowedDatabasePath);
  }

  public handle(message: WorkerDatabaseRequestMessage): void {
    if (this.closed) {
      writeWorkerDatabaseResponse(message.responseBuffer, {
        ok: false,
        error: { message: 'The app-server database broker is closed.' }
      });
      return;
    }
    this.coordinator.dispatch({
      kind: 'worker',
      broker: this,
      databaseKey: this.databaseKey,
      message,
      execute: () => this.executeAndRespond(message),
      reject: (errorMessage) => writeWorkerDatabaseResponse(message.responseBuffer, {
        ok: false,
        error: { message: errorMessage }
      })
    });
  }

  public close(): void {
    if (this.closed) return;
    this.closed = true;
    for (const database of this.connections.values()) {
      try { database.close(); } catch { /* The worker may already have closed it. */ }
    }
    this.connections.clear();
    this.coordinator.unregister(this, this.databaseKey);
  }

  private executeAndRespond(message: WorkerDatabaseRequestMessage): boolean {
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
    return response.ok;
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

function normalizedDatabaseKey(path: string): string {
  return path === ':memory:' ? path : normalizePath(path);
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

function requestConnectionId(request: WorkerDatabaseRequest): number | null {
  return Number.isInteger(request.connectionId) && (request.connectionId ?? 0) > 0
    ? request.connectionId!
    : null;
}

function sameOwner(
  owner: DatabaseTransactionOwner,
  broker: AppServerWorkerDatabaseBroker,
  connectionId: number | null
): boolean {
  return owner.broker === broker && owner.connectionId === connectionId;
}

function explicitTransactionBoundary(request: WorkerDatabaseRequest): 'begin' | 'end' | null {
  if (request.operation !== 'exec' || !request.sql) return null;
  const sql = request.sql.trim().replace(/;\s*$/, '').trim();
  if (/^BEGIN(?:\s+(?:DEFERRED|IMMEDIATE|EXCLUSIVE))?(?:\s+TRANSACTION)?$/i.test(sql)) return 'begin';
  if (/^(?:(?:COMMIT|END)(?:\s+TRANSACTION)?|ROLLBACK(?:\s+TRANSACTION)?)$/i.test(sql)) return 'end';
  return null;
}
