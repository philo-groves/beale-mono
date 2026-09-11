import { parentPort, workerData } from 'node:worker_threads';
import { checkpointWorkspaceResearch, importWorkspaceResearchFile, initializeWorkspaceProject, installResearchDatabaseFactory, quarantineWorkspaceDisposable, resolveStoredResearchProfile, writeCheckpointStatus, type WorkspacePublicationOptions } from '@beale/app-server-runtime/runtime-services';
import { createWorkerResearchDatabaseFactory } from './workerDatabaseClient.js';

if ('initializeInput' in workerData) {
  try { parentPort?.postMessage({ result: initializeWorkspaceProject(workerData.initializeInput.workspaceRoot, workerData.initializeInput.workspaceId) }); }
  catch (error) { parentPort?.postMessage({ error: error instanceof Error ? error.message : String(error) }); }
} else if ('maintenanceInput' in workerData) {
  try {
    const { invokeAppServerProtocol } = await import('./appServerProtocolClient.js');
    parentPort?.postMessage({ result: await invokeAppServerProtocol('maintenance.run', { args: [], input: workerData.maintenanceInput }) });
  } catch (error) { parentPort?.postMessage({ error: error instanceof Error ? error.message : String(error) }); }
} else {
const input = workerData as { options: WorkspacePublicationOptions; reason: string; edit?: { path: string; expectedRevision: number }; cleanupSession?: string };
if (!parentPort) throw new Error('The workspace checkpoint worker requires a parent port.');
const checkpointPort = parentPort;
installResearchDatabaseFactory(createWorkerResearchDatabaseFactory((message) => checkpointPort.postMessage(message)));
try {
  if (input.edit) importWorkspaceResearchFile(input.options, input.edit.path, input.edit.expectedRevision, await resolveStoredResearchProfile(input.options));
  const result = checkpointWorkspaceResearch(input.options, input.reason);
  if (input.cleanupSession && (result.status === 'committed' || result.status === 'unchanged')) quarantineWorkspaceDisposable(input.options.workspaceRoot, input.cleanupSession);
  parentPort?.postMessage({ ...result, ...(input.edit ? { imported: true } : {}) });
} catch (error) {
  const result = { status: 'failed' as const, reason: input.reason, error: error instanceof Error ? error.message : String(error) };
  writeCheckpointStatus(input.options.workspaceRoot, result);
  parentPort?.postMessage(result);
}
}
