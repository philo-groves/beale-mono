import { afterEach, describe, expect, it, vi } from 'vitest';
import type { AppServerMemorySummary, RunDetail, RunDetailUpdate, RunStatus } from '@shared/types';
import { WorkspaceService } from '../src/main/workspaceService';
import { AppServerReadTransportError } from '../src/main/bealeAppServerClient';
import { getAppServerRunDetailUpdateForClient } from '../src/main/appServerSessionBoundary';
import { mergeRunDetailUpdate } from '../src/renderer/view-models/runDetailUpdates';

vi.mock('../src/main/appServerSessionBoundary', async (importOriginal) => ({
  ...await importOriginal<typeof import('../src/main/appServerSessionBoundary')>(),
  getAppServerRunDetailUpdateForClient: vi.fn()
}));

// Replace host/storage boundaries only; exercise the service's actual refresh
// scheduling, failure handling, projection, and renderer merge.
interface ServiceBoundaries {
  requireRuntimeForRunId(): { db: { getActiveScope(): null } };
  reconcileCanonicalTerminalRun(): void;
  memorySummaryForRuntimeAsync(): Promise<AppServerMemorySummary>;
}

const cursor = { afterTraceSequence: -1, afterTranscriptCount: 0 };
const memory = { nodes: [], activeCatalogHash: 'catalog-example' } as unknown as AppServerMemorySummary;

function setup(status: RunStatus = 'active') {
  const service = new WorkspaceService();
  const boundaries = service as unknown as ServiceBoundaries;
  vi.spyOn(boundaries, 'requireRuntimeForRunId').mockReturnValue({ db: { getActiveScope: () => null } });
  vi.spyOn(boundaries, 'reconcileCanonicalTerminalRun').mockImplementation(() => undefined);
  const refresh = vi.spyOn(boundaries, 'memorySummaryForRuntimeAsync').mockResolvedValue(memory);
  const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
  const update = {
    run: { id: 'run-example', status },
    version: { runId: 'run-example', version: 'version-example', databaseMs: 0, generatedAt: '2026-01-01T00:00:00Z' },
    attempts: [], traceEvents: [], artifacts: [], verifierContracts: [], verifierRuns: [],
    modelSessions: [], contextCompactions: [], policyEvents: [], exports: [],
    transcriptMessages: [{
      id: 'message-example', runId: 'run-example', attemptId: null, traceEventId: null,
      role: 'assistant', contentMarkdown: 'Example session update.', source: 'openai',
      createdAt: '2026-01-01T00:00:00Z', metadata: {}
    }]
  } as unknown as RunDetailUpdate;
  vi.mocked(getAppServerRunDetailUpdateForClient).mockResolvedValue(update);
  return { service, refresh, warn, update };
}

function resetError(operation: 'session.read' | 'memory.summary' = 'memory.summary') {
  return new AppServerReadTransportError(operation, 3, Object.assign(new Error('read ECONNRESET'), { code: 'ECONNRESET' }));
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.clearAllMocks();
  vi.useRealTimers();
});

describe('session update memory enrichment', () => {
  it.each(['active', 'completed'] as const)('preserves transcript and cached memory, backs off, and recovers for %s sessions', async (status) => {
    vi.useFakeTimers();
    const { service, refresh, warn, update } = setup(status);
    refresh.mockRejectedValueOnce(resetError());

    const result = await service.getRunDetailUpdateForClient('run-example', cursor, undefined, 'commentary');
    expect(result.transcriptMessages).toEqual(update.transcriptMessages);
    expect(result.version).toEqual(update.version);
    expect(result.appServerMemory).toBeUndefined();
    const previous: RunDetail = { ...update, transcriptMessages: [], appServerMemory: memory };
    const merged = mergeRunDetailUpdate(previous, result);
    expect(merged.appServerMemory).toBe(memory);
    expect(merged.transcriptMessages).toEqual(update.transcriptMessages);
    expect(warn).toHaveBeenCalledWith(expect.stringContaining('memory.summary read failed after 3 attempt(s): fetch failed (ECONNRESET)'));

    await vi.advanceTimersByTimeAsync(4_999);
    await service.getRunDetailUpdateForClient('run-example', cursor);
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(getAppServerRunDetailUpdateForClient).toHaveBeenCalledTimes(2);
    expect(warn).toHaveBeenCalledTimes(1);

    await vi.advanceTimersByTimeAsync(1);
    const recovered = await service.getRunDetailUpdateForClient('run-example', cursor);
    expect(recovered.appServerMemory).toBe(memory);
    expect(refresh).toHaveBeenCalledTimes(2);
  });

  it('keeps primary session failures visible and never attempts enrichment for them', async () => {
    const { service, refresh } = setup();
    const error = resetError('session.read');
    vi.mocked(getAppServerRunDetailUpdateForClient).mockRejectedValueOnce(error);
    await expect(service.getRunDetailUpdateForClient('run-example', cursor)).rejects.toBe(error);
    expect(refresh).not.toHaveBeenCalled();
  });

  it('does not hide non-transport memory failures or delay their next attempt', async () => {
    const { service, refresh, warn } = setup();
    const error = new Error('Invalid example memory response.');
    refresh.mockRejectedValueOnce(error);
    await expect(service.getRunDetailUpdateForClient('run-example', cursor)).rejects.toBe(error);
    await expect(service.getRunDetailUpdateForClient('run-example', cursor)).resolves.toHaveProperty('appServerMemory', memory);
    expect(warn).not.toHaveBeenCalled();
  });

  it.each([false, true])('honors caller cancellation after shared enrichment settles (failure: %s)', async (failure) => {
    const { service, refresh, warn } = setup();
    const controller = new AbortController();
    const reason = new Error('Example caller cancelled.');
    refresh.mockImplementationOnce(async () => {
      controller.abort(reason);
      if (failure) throw resetError();
      return memory;
    });
    await expect(service.getRunDetailUpdateForClient('run-example', cursor, controller.signal)).rejects.toBe(reason);
    expect(warn).not.toHaveBeenCalled();
  });
});
