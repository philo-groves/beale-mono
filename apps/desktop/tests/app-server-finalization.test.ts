import { describe, expect, it, vi } from 'vitest';
import type { AppServerCatalogEntry } from '../src/main/bealeAppServerClient';
import { waitForTerminalAppServerSession } from '../src/main/appServerRunEngine';

describe('app-server session finalization', () => {
  it('retries a transient missing catalog response before accepting terminal state', async () => {
    const completed: AppServerCatalogEntry = {
      sessionId: 'run-example',
      state: 'completed',
      startedAt: '2026-09-13T00:00:00.000Z',
      endedAt: '2026-09-13T00:01:00.000Z',
      exitCode: 0,
      diagnostic: null,
      clientConnected: false,
      replay: { bufferedFrames: 0, bufferedBytes: 0, droppedFrames: 0 }
    };
    const read = vi.fn<() => Promise<AppServerCatalogEntry | null>>()
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce(completed);

    await expect(waitForTerminalAppServerSession(read, { timeoutMs: 100, intervalMs: 0 }))
      .resolves.toEqual(completed);
    expect(read).toHaveBeenCalledTimes(2);
  });
});
