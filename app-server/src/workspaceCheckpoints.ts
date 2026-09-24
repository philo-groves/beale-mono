import { Worker } from 'node:worker_threads';
import { resolve as resolvePath } from 'node:path';
import type { WorkspaceCheckpointRepairPlan, WorkspaceCheckpointResult, WorkspacePublicationOptions } from '@beale/app-server-runtime/runtime-services';
import {
  AppServerWorkerDatabaseBroker,
  type AppServerWorkerDatabaseCoordinator
} from './workerDatabaseBroker.js';
import type { WorkerDatabaseRequestMessage } from './workerDatabaseClient.js';

const queues = new Map<string, Promise<WorkspaceCheckpointResult>>();
export const workspaceOperationKey = (root: string): string => process.platform === 'win32' ? resolvePath(root).toLowerCase() : resolvePath(root);

export function runWorkspaceMaintenance(input: unknown): Promise<unknown> {
  return runWorkspaceSetupWorker({ maintenanceInput: input });
}

export function initializeWorkspaceProjectAsync(workspaceRoot: string, workspaceId: string): Promise<unknown> {
  return runWorkspaceSetupWorker({ initializeInput: { workspaceRoot, workspaceId } });
}

export function previewWorkspaceCheckpointRepair(workspaceRoot: string): Promise<WorkspaceCheckpointRepairPlan> {
  return runWorkspaceSetupWorker({ repairPreviewInput: { workspaceRoot } }) as Promise<WorkspaceCheckpointRepairPlan>;
}

function runWorkspaceSetupWorker(workerData: unknown): Promise<unknown> {
  return new Promise((resolve, reject) => {
    const worker = new Worker(new URL('./workspaceCheckpointWorker.js', import.meta.url), { workerData });
    let received = false;
    worker.once('message', (message: { result?: unknown; error?: string }) => {
      received = true;
      if (message.error) reject(new Error(message.error));
      else resolve(message.result);
    });
    worker.once('error', reject);
    worker.once('exit', (code) => { if (!received) reject(new Error(`Workspace setup/maintenance worker exited with code ${code}.`)); });
  });
}

/** Git and export I/O must never block delivery of stop controls on the host event loop. */
export function runWorkspaceCheckpoint(
  options: WorkspacePublicationOptions,
  reason: string,
  edit?: { path: string; expectedRevision: number },
  cleanupSession?: string,
  databaseCoordinator?: AppServerWorkerDatabaseCoordinator,
  settings?: { exportResearch?: boolean; researchIndexAction?: 'rebuild' | 'release'; repairFingerprint?: string },
): Promise<WorkspaceCheckpointResult> {
  const key = workspaceOperationKey(options.workspaceRoot);
  const previous = queues.get(key) ?? Promise.resolve();
  const operation = previous.catch(() => undefined).then(() => new Promise<WorkspaceCheckpointResult>((resolve) => {
    let worker: Worker;
    try { worker = new Worker(new URL('./workspaceCheckpointWorker.js', import.meta.url), { workerData: { options, reason, edit, exportResearch: settings?.exportResearch === true, researchIndexAction: settings?.researchIndexAction, repairFingerprint: settings?.repairFingerprint, cleanupSession } }); }
    catch (error) { resolve({ status: 'failed', reason, error: error instanceof Error ? error.message : String(error) }); return; }
    const databaseBroker = new AppServerWorkerDatabaseBroker(options.databasePath, databaseCoordinator);
    let result: WorkspaceCheckpointResult | undefined;
    worker.on('message', (message: WorkspaceCheckpointResult | WorkerDatabaseRequestMessage) => {
      if (message && typeof message === 'object' && 'type' in message && message.type === 'database.request') {
        databaseBroker.handle(message);
      } else {
        result = message as WorkspaceCheckpointResult;
      }
    });
    worker.once('error', (error) => { result = { status: 'failed', reason, error: error.message }; });
    worker.once('exit', (code) => {
      databaseBroker.close();
      resolve(result ?? { status: 'failed', reason, error: `Checkpoint worker exited with code ${code}; working files were preserved.` });
    });
  }));
  queues.set(key, operation);
  const clear = () => { if (queues.get(key) === operation) queues.delete(key); };
  void operation.then(clear, clear);
  return operation;
}
