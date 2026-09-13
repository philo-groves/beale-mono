import { randomBytes } from 'node:crypto';
import { Worker } from 'node:worker_threads';
import {
  AppServerWorkerDatabaseBroker,
  AppServerWorkerDatabaseCoordinator
} from './workerDatabaseBroker.js';
import type { WorkerDatabaseRequestMessage } from './workerDatabaseClient.js';

const MAX_STDERR_CHARS = 8_000;

export interface SpawnAppServerSessionOptions {
  sessionId: string;
  args?: readonly string[];
  env?: NodeJS.ProcessEnv;
  databaseCoordinator?: AppServerWorkerDatabaseCoordinator;
}

export interface AppServerSession {
  sessionId: string;
  onEvent(listener: (event: Record<string, unknown>) => void): () => void;
  sendControl(control: Record<string, unknown>): void;
  stderrTail(): string;
  /** Resolves only after the worker has completed runtime initialization. */
  waitReady?(): Promise<void>;
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
  const databaseBroker = new AppServerWorkerDatabaseBroker(databasePath, options.databaseCoordinator);
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
  let readySettled = false;
  let resolveReady!: () => void;
  let rejectReady!: (error: Error) => void;
  const readyPromise = new Promise<void>((resolve, reject) => {
    resolveReady = resolve;
    rejectReady = reject;
  });
  // A worker can fail between coming online and the host attaching its
  // readiness observer. Keep that early rejection handled while preserving it
  // for waitReady callers.
  void readyPromise.catch(() => undefined);
  let stopRequested = false;
  let forceStopTimeout: ReturnType<typeof setTimeout> | null = null;
  const settleReady = (): void => {
    if (readySettled) return;
    readySettled = true;
    resolveReady();
  };
  const failReady = (message: string): void => {
    if (readySettled) return;
    readySettled = true;
    rejectReady(new Error(message));
  };
  const dispatchEvent = (event: Record<string, unknown>): void => {
    if (listeners.size === 0) pendingEvents.push(event);
    else for (const listener of listeners) listener(event);
  };
  const startupEvent = (phase: string, text: string): Record<string, unknown> => ({
    schemaVersion: 1,
    kind: 'model.output',
    timestamp: new Date().toISOString(),
    payload: {
      eventId: `startup:${options.sessionId}:${phase}`,
      phase: 'completed',
      messagePhase: 'commentary',
      text,
      agentPath: '/root',
      responseId: `startup-${options.sessionId}`,
      itemId: `startup:${phase}`
    }
  });
  const exitPromise = new Promise<{ code: number | null; stderr: string }>((resolve) => {
    worker.on('message', (message: unknown) => {
      if (!message || typeof message !== 'object' || Array.isArray(message)) return;
      const record = message as Record<string, unknown>;
      if (record.type === 'database.request') {
        databaseBroker.handle(message as WorkerDatabaseRequestMessage);
      } else if (record.type === 'event' && record.event && typeof record.event === 'object' && !Array.isArray(record.event)) {
        dispatchEvent(record.event as Record<string, unknown>);
      } else if (record.type === 'startup' && typeof record.phase === 'string' && typeof record.message === 'string') {
        dispatchEvent(startupEvent(record.phase, record.message));
      } else if (record.type === 'ready') {
        dispatchEvent(startupEvent('ready', 'Research runtime is ready. Waiting for the model’s first response.'));
        settleReady();
      } else if (record.type === 'complete') {
        resolvedCode = typeof record.exitCode === 'number' ? record.exitCode : 0;
      } else if (record.type === 'failed') {
        failureMessage = typeof record.error === 'string' ? record.error : 'app-server runtime worker failed.';
        resolvedCode = 1;
        failReady(failureMessage);
      }
    });
    worker.once('exit', (code) => {
      if (forceStopTimeout) clearTimeout(forceStopTimeout);
      forceStopTimeout = null;
      databaseBroker.close();
      failReady(failureMessage || stderr || `app-server runtime worker exited before initialization completed (code ${code ?? 'unknown'}).`);
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
    waitReady: () => readyPromise,
    waitExit: () => exitPromise,
    stop: () => {
      if (stopRequested) return;
      stopRequested = true;
      worker.postMessage({ type: 'stop' });
      forceStopTimeout = setTimeout(() => void worker.terminate(), 3_000);
      forceStopTimeout.unref();
    }
  };
  return new Promise((resolve, reject) => {
    worker.once('online', () => resolve(session));
    worker.once('error', (error) => {
      failReady(error.message);
      reject(error);
    });
  });
}
