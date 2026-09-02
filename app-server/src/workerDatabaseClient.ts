import type { DatabaseSync } from 'node:sqlite';
import { deserialize } from 'node:v8';
import type { ResearchDatabaseFactory, ResearchDatabaseOpenOptions } from '@beale/research-agent';

const RESPONSE_HEADER_BYTES = 8;
const RESPONSE_BUFFER_BYTES = 16 * 1024 * 1024;
const RESPONSE_TIMEOUT_MS = 120_000;

export interface WorkerDatabaseRequest {
  connectionId?: number;
  operation: 'open' | 'exec' | 'all' | 'get' | 'run' | 'close';
  databasePath?: string;
  options?: ResearchDatabaseOpenOptions;
  sql?: string;
  parameters?: unknown[];
}

export interface WorkerDatabaseRequestMessage {
  type: 'database.request';
  request: WorkerDatabaseRequest;
  responseBuffer: SharedArrayBuffer;
}

interface WorkerDatabaseResponse {
  ok: boolean;
  value?: unknown;
  error?: { message: string; code?: string };
}

export function createWorkerResearchDatabaseFactory(
  postMessage: (message: WorkerDatabaseRequestMessage) => void,
): ResearchDatabaseFactory {
  const responseBuffer = new SharedArrayBuffer(RESPONSE_BUFFER_BYTES);
  let nextRequestId = 1;
  const request = <T>(input: WorkerDatabaseRequest): T => {
    const header = new Int32Array(responseBuffer, 0, 2);
    Atomics.store(header, 0, 0);
    Atomics.store(header, 1, 0);
    const requestId = nextRequestId++;
    postMessage({ type: 'database.request', request: input, responseBuffer });
    const waitResult = Atomics.wait(header, 0, 0, RESPONSE_TIMEOUT_MS);
    if (waitResult === 'timed-out') {
      throw new Error(`Timed out waiting for app-server database request ${requestId}.`);
    }
    const length = Atomics.load(header, 1);
    const payload = deserialize(Buffer.from(new Uint8Array(responseBuffer, RESPONSE_HEADER_BYTES, length))) as WorkerDatabaseResponse;
    if (!payload.ok) {
      const error = new Error(payload.error?.message ?? 'The app-server database request failed.');
      if (payload.error?.code) Object.assign(error, { code: payload.error.code });
      throw error;
    }
    return payload.value as T;
  };

  return (databasePath, options) => {
    const connectionId = request<number>({
      operation: 'open',
      databasePath,
      ...(options ? { options } : {})
    });
    let closed = false;
    const invoke = <T>(operation: 'exec' | 'all' | 'get' | 'run', sql: string, parameters?: unknown[]): T => {
      if (closed) throw new Error('The app-server database connection is closed.');
      return request<T>({ operation, connectionId, sql, ...(parameters ? { parameters } : {}) });
    };
    return {
      exec: (sql: string) => invoke<void>('exec', sql),
      prepare: (sql: string) => ({
        all: (...parameters: unknown[]) => invoke<unknown[]>('all', sql, parameters),
        get: (...parameters: unknown[]) => invoke<unknown>('get', sql, parameters),
        run: (...parameters: unknown[]) => invoke<unknown>('run', sql, parameters)
      }),
      close: () => {
        if (closed) return;
        request<void>({ operation: 'close', connectionId });
        closed = true;
      }
    } as unknown as DatabaseSync;
  };
}
