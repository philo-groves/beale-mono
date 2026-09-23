import { parentPort, workerData } from 'node:worker_threads';
import { checkpointWorkspace, checkpointWorkspaceResearch, importWorkspaceResearchFile, initializeWorkspaceProject, installResearchDatabaseFactory, isImportableWorkspaceResearchPath, listWorkspaceResearchEdits, quarantineWorkspaceDisposable, rebuildWorkspaceResearchIndex, recoverWorkspacePublication, releaseWorkspaceResearchIndex, resolveStoredResearchProfile, workspaceCheckpointRepairPlan, workspaceResearchAuthority, workspaceResearchFileExpectedRevision, workspaceResearchIndexNeedsRebuild, writeCheckpointStatus, type WorkspacePublicationOptions } from '@beale/app-server-runtime/runtime-services';
import { createWorkerResearchDatabaseFactory } from './workerDatabaseClient.js';

if ('initializeInput' in workerData) {
  try { parentPort?.postMessage({ result: initializeWorkspaceProject(workerData.initializeInput.workspaceRoot, workerData.initializeInput.workspaceId) }); }
  catch (error) { parentPort?.postMessage({ error: error instanceof Error ? error.message : String(error) }); }
} else if ('maintenanceInput' in workerData) {
  try {
    const { invokeAppServerProtocol } = await import('./appServerProtocolClient.js');
    parentPort?.postMessage({ result: await invokeAppServerProtocol('maintenance.run', { args: [], input: workerData.maintenanceInput }) });
  } catch (error) { parentPort?.postMessage({ error: error instanceof Error ? error.message : String(error) }); }
} else if ('repairPreviewInput' in workerData) {
  try { parentPort?.postMessage({ result: workspaceCheckpointRepairPlan(workerData.repairPreviewInput.workspaceRoot) }); }
  catch (error) { parentPort?.postMessage({ error: error instanceof Error ? error.message : String(error) }); }
} else {
const input = workerData as { options: WorkspacePublicationOptions; reason: string; edit?: { path: string; expectedRevision: number }; exportResearch?: boolean; researchIndexAction?: 'rebuild' | 'release'; cleanupSession?: string; repairFingerprint?: string };
if (!parentPort) throw new Error('The workspace checkpoint worker requires a parent port.');
const checkpointPort = parentPort;
installResearchDatabaseFactory(createWorkerResearchDatabaseFactory((message) => checkpointPort.postMessage(message)));
try {
  const fileAuthority = workspaceResearchAuthority(input.options.workspaceRoot) === 'files';
  if (fileAuthority) recoverWorkspacePublication(input.options.workspaceRoot);
  if (fileAuthority && (input.researchIndexAction === 'rebuild' || workspaceResearchIndexNeedsRebuild(input.options.workspaceRoot))) {
    const researchIndex = rebuildWorkspaceResearchIndex(input.options);
    if (input.researchIndexAction === 'rebuild') {
      parentPort?.postMessage({ status: 'unchanged', reason: input.reason, researchIndex });
    }
  }
  if (input.researchIndexAction === 'rebuild') {
    // The result was posted above. Keep this branch separate from publication so
    // an explicit cache rebuild cannot rewrite or commit workspace files.
  } else {
  const edits = fileAuthority && !input.edit ? listWorkspaceResearchEdits(input.options.workspaceRoot) : [];
  if (edits.some((edit) => edit.state === 'created')) {
    throw new Error(`Use typed research creation for new file-authority records: ${edits.filter((edit) => edit.state === 'created').map((edit) => edit.path).join(', ')}`);
  }
  if (edits.some((edit) => edit.state === 'deleted')) {
    throw new Error(`Canonical file-authority records cannot be deleted directly: ${edits.filter((edit) => edit.state === 'deleted').map((edit) => edit.path).join(', ')}`);
  }
  const modified = edits.filter((edit) => edit.state === 'modified');
  if (!input.edit && modified.length > 1) {
    throw new Error(`Import one direct file-authority edit at a time so revisioned updates remain atomic: ${modified.map((edit) => edit.path).join(', ')}`);
  }
  const unsupported = modified.filter((edit) => !isImportableWorkspaceResearchPath(edit.path));
  if (unsupported.length > 0) {
    throw new Error(`Use typed research operations for these file-authority records: ${unsupported.map((edit) => edit.path).join(', ')}`);
  }
  if (input.edit || modified.length > 0) {
    const profile = await resolveStoredResearchProfile(input.options);
    if (input.edit) importWorkspaceResearchFile(input.options, input.edit.path, input.edit.expectedRevision, profile);
    else if (modified[0]) importWorkspaceResearchFile(input.options, modified[0].path,
      workspaceResearchFileExpectedRevision(input.options.workspaceRoot, modified[0].path), profile);
  }
  // File-authority workspaces republish the derived query index at every
  // checkpoint. Legacy schema-v1 workspaces retain explicit export semantics.
  const result = fileAuthority || input.edit || input.exportResearch
    ? checkpointWorkspaceResearch(input.options, input.reason, input.repairFingerprint)
    : checkpointWorkspace(input.options.workspaceRoot, input.reason, undefined, {
        ...(input.options.sessionId ? { sessionId: input.options.sessionId } : {}),
        ...(input.options.investigationId ? { investigationId: input.options.investigationId } : {}),
      }, input.repairFingerprint);
  if (input.cleanupSession && (result.status === 'committed' || result.status === 'unchanged')) quarantineWorkspaceDisposable(input.options.workspaceRoot, input.cleanupSession);
  const researchIndex = input.researchIndexAction === 'release' && (result.status === 'committed' || result.status === 'unchanged')
    ? releaseWorkspaceResearchIndex(input.options) : undefined;
  parentPort?.postMessage({ ...result, ...(input.edit ? { imported: true } : {}), ...(researchIndex ? { researchIndex } : {}) });
  }
} catch (error) {
  const result = { status: 'failed' as const, reason: input.reason, error: error instanceof Error ? error.message : String(error) };
  writeCheckpointStatus(input.options.workspaceRoot, result);
  parentPort?.postMessage(result);
}
}
