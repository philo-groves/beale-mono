import { randomBytes } from 'node:crypto';
import { Worker } from 'node:worker_threads';
import { AppServerWorkerDatabaseBroker } from './workerDatabaseBroker.js';
import type { WorkerDatabaseRequestMessage } from './workerDatabaseClient.js';

const MAX_STDERR_CHARS = 8_000;

export interface SpawnAppServerSessionOptions {
  sessionId: string;
  args?: readonly string[];
  env?: NodeJS.ProcessEnv;
}

export interface AppServerSession {
  sessionId: string;
  onEvent(listener: (event: Record<string, unknown>) => void): () => void;
  sendControl(control: Record<string, unknown>): void;
  stderrTail(): string;
  waitExit(): Promise<{ code: number | null; stderr: string }>;
  stop(): void;
}

export function generateSessionToken(): string {
  return randomBytes(24).toString('base64url');
}

export function appServerWorkerEnvironment(
  additions: NodeJS.ProcessEnv = {},
  electronVersion: string | undefined = process.versions.electron
): NodeJS.ProcessEnv {
  return {
    ...process.env,
    ...additions,
    ...(electronVersion ? { ELECTRON_RUN_AS_NODE: '1' } : {})
  };
}

export function spawnAppServerSession(options: SpawnAppServerSessionOptions): Promise<AppServerSession> {
  const workerEnvironment = appServerWorkerEnvironment(options.env ?? {});
  const databasePath = workerEnvironment.APP_SERVER_DATABASE_PATH?.trim();
  if (!databasePath) throw new Error('App-server runtime workers require app-server-owned database storage.');
  const databaseBroker = new AppServerWorkerDatabaseBroker(databasePath);
  const worker = new Worker(new URL('./runtimeWorker.js', import.meta.url), {
    workerData: {
      args: ['--hosted-session', '--session-id', options.sessionId, ...(options.args ?? [])],
      env: Object.fromEntries(Object.entries(workerEnvironment).flatMap(([name, value]) => (
        typeof value === 'string' ? [[name, value] as const] : []
      )))
    },
    stdout: true,
    stderr: true
  });
  let stderr = '';
  worker.stderr.setEncoding('utf8');
  worker.stderr.on('data', (chunk: string) => {
    stderr += chunk;
    if (stderr.length > MAX_STDERR_CHARS) stderr = stderr.slice(stderr.length - MAX_STDERR_CHARS);
  });
  const listeners = new Set<(event: Record<string, unknown>) => void>();
  const pendingEvents: Record<string, unknown>[] = [];
  let resolvedCode: number | null = null;
  let failureMessage = '';
  const exitPromise = new Promise<{ code: number | null; stderr: string }>((resolve) => {
    worker.on('message', (message: unknown) => {
      if (!message || typeof message !== 'object' || Array.isArray(message)) return;
      const record = message as Record<string, unknown>;
      if (record.type === 'database.request') {
        databaseBroker.handle(message as WorkerDatabaseRequestMessage);
      } else if (record.type === 'event' && record.event && typeof record.event === 'object' && !Array.isArray(record.event)) {
        const event = record.event as Record<string, unknown>;
        if (listeners.size === 0) pendingEvents.push(event);
        else for (const listener of listeners) listener(event);
      } else if (record.type === 'complete') {
        resolvedCode = typeof record.exitCode === 'number' ? record.exitCode : 0;
      } else if (record.type === 'failed') {
        failureMessage = typeof record.error === 'string' ? record.error : 'app-server runtime worker failed.';
        resolvedCode = 1;
      }
    });
    worker.once('exit', (code) => {
      databaseBroker.close();
      resolve({ code: resolvedCode ?? code, stderr: failureMessage || stderr });
    });
  });
  const session: AppServerSession = {
    sessionId: options.sessionId,
    onEvent: (listener) => {
      listeners.add(listener);
      for (const event of pendingEvents.splice(0)) listener(event);
      return () => listeners.delete(listener);
    },
    sendControl: (control) => worker.postMessage({ type: 'control', control }),
    stderrTail: () => stderr,
    waitExit: () => exitPromise,
    stop: () => {
      worker.postMessage({ type: 'stop' });
      const timeout = setTimeout(() => void worker.terminate(), 3_000);
      timeout.unref();
    }
  };
  return new Promise((resolve, reject) => {
    worker.once('online', () => resolve(session));
    worker.once('error', reject);
  });
}
