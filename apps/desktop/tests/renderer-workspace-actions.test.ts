import { readFileSync } from 'node:fs';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import type { RunDetail, WorkspaceSnapshot } from '@shared/types';
import { WORKSPACE_PRIMARY_DIRECTORY_MISSING_MESSAGE } from '../src/shared/ipc';
import { MissingWorkspaceDirectoryDialog } from '../src/renderer/features/workspaces/MissingWorkspaceDirectoryDialog';
import { runDetailNeedsEnrichment } from '../src/renderer/hooks/useRunDetailPolling';
import { researchSessionNeedsLoading } from '../src/renderer/hooks/useWorkspaceActions';
import { isWorkspacePrimaryDirectoryMissingError } from '../src/renderer/lib/errors';

describe('renderer workspace actions', () => {
  it('recognizes the wrapped missing-primary-directory IPC error', () => {
    expect(isWorkspacePrimaryDirectoryMissingError(
      new Error(`Error invoking remote method 'beale:open-registered-workspace': Error: ${WORKSPACE_PRIMARY_DIRECTORY_MISSING_MESSAGE}`)
    )).toBe(true);
    expect(isWorkspacePrimaryDirectoryMissingError(new Error('Workspace failed to open.'))).toBe(false);
  });

  it('offers registry removal when a workspace primary directory is missing', () => {
    const html = renderToStaticMarkup(createElement(MissingWorkspaceDirectoryDialog, {
      busy: false,
      workspace: { workspaceName: 'Apple Security' },
      onClose: () => undefined,
      onRemove: () => undefined,
    }));

    expect(html).toContain('Primary Directory Not Found');
    expect(html).toContain('The primary directory for <strong>Apple Security</strong> could not be found.');
    expect(html).toContain('Removing the workspace from Beale will not delete any files.');
    expect(html).toContain('Remove Workspace');
  });

  it('preserves live detail when opening a room from the already selected session', () => {
    const snapshot = {
      workspace: { workspacePath: 'C:\\research\\snapchat' }
    } as WorkspaceSnapshot;
    const workspace = { workspacePath: 'C:\\research\\snapchat' };
    const session = { runId: 'run_selected' };

    expect(researchSessionNeedsLoading(snapshot, 'run_selected', workspace, session)).toBe(false);
    expect(researchSessionNeedsLoading(snapshot, 'run_other', workspace, session)).toBe(true);
    expect(researchSessionNeedsLoading(snapshot, 'run_selected', { workspacePath: 'C:\\research\\other' }, session)).toBe(true);
  });

  it('enriches commentary memory after the initial session detail paint', () => {
    const detailWithoutMemory = {} as RunDetail;
    const detailWithMemory = { honeycrispMemory: {} } as RunDetail;
    expect(runDetailNeedsEnrichment(null, 'commentary')).toBe(false);
    expect(runDetailNeedsEnrichment(detailWithoutMemory, 'commentary')).toBe(true);
    expect(runDetailNeedsEnrichment(detailWithoutMemory, { mode: 'commentary', agentPath: null })).toBe(true);
    expect(runDetailNeedsEnrichment(detailWithMemory, 'commentary')).toBe(false);
    expect(runDetailNeedsEnrichment(detailWithoutMemory, 'full')).toBe(false);

    const serviceSource = readFileSync(new URL('../src/main/workspaceService.ts', import.meta.url), 'utf8');
    const detailStart = serviceSource.indexOf('  public async getRunDetailForClient(');
    const detailEnd = serviceSource.indexOf('  public getRunDetailVersion(', detailStart);
    const detailSource = serviceSource.slice(detailStart, detailEnd);
    expect(detailSource).toContain('if (isCommentaryRunDetailProjection(projection))');
    expect(detailSource.indexOf('if (isCommentaryRunDetailProjection(projection))'))
      .toBeLessThan(detailSource.indexOf('const withMemory = attachHoneycrispMemory('));
  });

  it('moves navigation state before awaiting a workspace or session open', () => {
    const source = readFileSync(new URL('../src/renderer/hooks/useWorkspaceActions.ts', import.meta.url), 'utf8');
    const workspaceStart = source.indexOf('  const openRegisteredWorkspace = useCallback(');
    const sessionStart = source.indexOf('  const openResearchSession = useCallback(', workspaceStart);
    const workspaceAction = source.slice(workspaceStart, sessionStart);
    const sessionEnd = source.indexOf('  const removeRegisteredWorkspace = useCallback(', sessionStart);
    const sessionAction = source.slice(sessionStart, sessionEnd);

    expect(workspaceAction.indexOf('setSelectedRunId(null);'))
      .toBeLessThan(workspaceAction.indexOf('void runWorkspaceAction('));
    expect(sessionAction.indexOf('setSelectedRunId(session.runId);'))
      .toBeLessThan(sessionAction.indexOf('void runWorkspaceAction('));
    expect(sessionAction).toContain('applySnapshot(next, session.runId);');

    const runtimeSource = readFileSync(new URL('../src/renderer/hooks/useWorkspaceRuntime.ts', import.meta.url), 'utf8');
    expect(runtimeSource).toContain('selectedRunIdOverride ?? selectRunId(current, next)');
  });

  it('uses an action response as the authoritative snapshot without immediate reloads', () => {
    const source = readFileSync(
      new URL('../src/renderer/App.tsx', import.meta.url),
      'utf8'
    );
    const start = source.indexOf('  const runAction = useCallback(');
    const end = source.indexOf('  const openNotification = useCallback(', start);
    const runActionSource = source.slice(start, end);

    expect(runActionSource).toContain('if (next) applySnapshot(next);');
    expect(runActionSource).toContain('else await loadSnapshot();');
    expect(runActionSource).not.toContain('loadWorkspaceRegistry');
  });

  it('does not reload or replay the registry during a pure workspace navigation', () => {
    const source = readFileSync(
      new URL('../src/renderer/hooks/useWorkspaceActions.ts', import.meta.url),
      'utf8'
    );
    const start = source.indexOf('  const openRegisteredWorkspace = useCallback(');
    const end = source.indexOf('  const openResearchSession = useCallback(', start);

    expect(source.slice(start, end)).toContain('{ reloadRegistry: false, missingDirectoryWorkspace: workspace }');

    const serviceSource = readFileSync(new URL('../src/main/workspaceService.ts', import.meta.url), 'utf8');
    const serviceStart = serviceSource.indexOf('  public openRegisteredWorkspace(');
    const serviceEnd = serviceSource.indexOf('  public removeRegisteredWorkspace(', serviceStart);
    const serviceAction = serviceSource.slice(serviceStart, serviceEnd);
    expect(serviceAction).toContain('this.open(workspace.workspacePath, false, false, undefined, undefined, false)');
    expect(serviceAction).toContain('registry.rememberWorkspaceOpened(registryWorkspaceId);');
  });

  it('defers cached workspace memory and secondary channel reads beyond the navigation paint', () => {
    const serviceSource = readFileSync(new URL('../src/main/workspaceService.ts', import.meta.url), 'utf8');
    expect(serviceSource).toContain('const WORKSPACE_MEMORY_SUMMARY_DEFER_MS = 250;');
    expect(serviceSource).toContain('this.workspaceMemorySummaryLoads.has(runtime.workspacePath)');
    expect(serviceSource.indexOf('this.workspaceMemorySummaryLoads.has(runtime.workspacePath)'))
      .toBeLessThan(serviceSource.indexOf('this.cachedMemorySummaryForRuntime(runtime)'));

    const appSource = readFileSync(new URL('../src/renderer/App.tsx', import.meta.url), 'utf8');
    expect(appSource).toContain('window.setTimeout(() => void load(), selectedRunId ? 750 : 0)');
  });
});
