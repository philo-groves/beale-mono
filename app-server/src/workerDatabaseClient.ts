import type { DatabaseSync } from 'node:sqlite';
import { deserialize } from 'node:v8';
import type { ResearchDatabaseFactory, ResearchDatabaseOpenOptions } from '@beale/research-agent';

const RESPONSE_HEADER_BYTES = 8;
const DEFAULT_RESPONSE_BUFFER_BYTES = 16 * 1024 * 1024;
const DEFAULT_MAXIMUM_RESPONSE_BUFFER_BYTES = 256 * 1024 * 1024;
const MINIMUM_RESPONSE_BUFFER_BYTES = 1024;
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
  error?: { message: string; code?: string; requiredBytes?: number };
}

export interface WorkerResearchDatabaseFactoryOptions {
  initialResponseBufferBytes?: number;
  maximumResponseBufferBytes?: number;
}

export function createWorkerResearchDatabaseFactory(
  postMessage: (message: WorkerDatabaseRequestMessage) => void,
  options: WorkerResearchDatabaseFactoryOptions = {},
): ResearchDatabaseFactory {
  const initialResponseBufferBytes = responseBufferSize(
    options.initialResponseBufferBytes,
    DEFAULT_RESPONSE_BUFFER_BYTES,
    'initialResponseBufferBytes',
  );
  const maximumResponseBufferBytes = responseBufferSize(
    options.maximumResponseBufferBytes,
    DEFAULT_MAXIMUM_RESPONSE_BUFFER_BYTES,
    'maximumResponseBufferBytes',
  );
  if (maximumResponseBufferBytes < initialResponseBufferBytes) {
    throw new Error('maximumResponseBufferBytes must be at least initialResponseBufferBytes.');
  }
  let responseBuffer = new SharedArrayBuffer(initialResponseBufferBytes);
  let nextRequestId = 1;
  const request = <T>(input: WorkerDatabaseRequest): T => {
    for (;;) {
      const activeResponseBuffer = responseBuffer;
      const header = new Int32Array(activeResponseBuffer, 0, 2);
      Atomics.store(header, 0, 0);
      Atomics.store(header, 1, 0);
      const requestId = nextRequestId++;
      postMessage({ type: 'database.request', request: input, responseBuffer: activeResponseBuffer });
      const waitResult = Atomics.wait(header, 0, 0, RESPONSE_TIMEOUT_MS);
      if (waitResult === 'timed-out') {
        throw new Error(`Timed out waiting for app-server database request ${requestId}.`);
      }
      const length = Atomics.load(header, 1);
      const payload = deserialize(Buffer.from(
        new Uint8Array(activeResponseBuffer, RESPONSE_HEADER_BYTES, length),
      )) as WorkerDatabaseResponse;
      if (payload.ok) return payload.value as T;

      const requiredBytes = payload.error?.code === 'BEALE_DATABASE_RESPONSE_BUFFER_TOO_SMALL'
        ? payload.error.requiredBytes
        : undefined;
      if (isRetryableRead(input.operation) && Number.isInteger(requiredBytes) && (requiredBytes ?? 0) > 0) {
        const nextSize = expandedResponseBufferSize(
          activeResponseBuffer.byteLength,
          requiredBytes!,
          maximumResponseBufferBytes,
        );
        if (nextSize !== null) {
          responseBuffer = new SharedArrayBuffer(nextSize);
          continue;
        }
      }

      const error = new Error(payload.error?.message ?? 'The app-server database request failed.');
      if (payload.error?.code) Object.assign(error, { code: payload.error.code });
      throw error;
    }
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

function isRetryableRead(operation: WorkerDatabaseRequest['operation']): boolean {
  return operation === 'all' || operation === 'get';
}

function responseBufferSize(value: number | undefined, fallback: number, name: string): number {
  const normalized = value ?? fallback;
  if (!Number.isSafeInteger(normalized) || normalized < MINIMUM_RESPONSE_BUFFER_BYTES) {
    throw new Error(`${name} must be an integer of at least ${MINIMUM_RESPONSE_BUFFER_BYTES} bytes.`);
  }
  return normalized;
}

function expandedResponseBufferSize(
  currentBytes: number,
  requiredPayloadBytes: number,
  maximumBytes: number,
): number | null {
  const requiredBytes = requiredPayloadBytes + RESPONSE_HEADER_BYTES;
  if (requiredBytes > maximumBytes) return null;
  let candidate = currentBytes;
  while (candidate < requiredBytes) candidate = Math.min(maximumBytes, candidate * 2);
  return candidate > currentBytes ? candidate : null;
}
