import { beforeEach, describe, expect, it, vi } from 'vitest';
import { AppServerRunEngine } from '../src/main/appServerRunEngine';
import { WorkspaceService } from '../src/main/workspaceService';
import type { BealeAppServerDiscovery } from '../src/main/bealeAppServerClient';

const host = vi.hoisted(() => ({ discover: vi.fn(), stop: vi.fn() }));
vi.mock('../src/main/bealeAppServerClient', async (importOriginal) => ({
  ...await importOriginal<typeof import('../src/main/bealeAppServerClient')>(),
  ensureBealeAppServerRunning: host.discover,
  stopAppServerSession: host.stop
}));

const record = { url: 'http://127.0.0.1:12345', operatorToken: 'example-token' } as BealeAppServerDiscovery;
interface ActiveHarness {
  appServerRecord: BealeAppServerDiscovery | null;
  appServerSessionId: string | null;
  launchReady?: Promise<void>;
  stopped: boolean;
  stopReason: string | null;
  budgetTimer: null;
  forceStopTimer: null;
}
interface EngineHarness {
  activeRuns: Map<string, ActiveHarness>;
  stop(runId: string): Promise<void>;
}
function engine(active?: ActiveHarness): EngineHarness {
  return Object.assign(Object.create(AppServerRunEngine.prototype) as EngineHarness, {
    activeRuns: new Map(active ? [['session-example', active]] : [])
  });
}
function activeRun(): ActiveHarness {
  return { appServerRecord: record, appServerSessionId: 'session-example', stopped: false,
    stopReason: null, budgetTimer: null, forceStopTimer: null };
}
function deferred() {
  let resolve!: () => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<void>((accept, fail) => { resolve = accept; reject = fail; });
  return { promise, resolve, reject };
}

beforeEach(() => { vi.clearAllMocks(); host.discover.mockResolvedValue(record); host.stop.mockResolvedValue(undefined); });

describe('authoritative session stop', () => {
  it('stops a host session even when Desktop has no local attachment', async () => {
    await engine().stop('session-example');
    expect(host.stop).toHaveBeenCalledWith(record, 'session-example');
  });

  it('uses the host endpoint without depending on WebSocket delivery', async () => {
    const active = activeRun();
    await engine(active).stop('session-example');
    expect(host.stop).toHaveBeenCalledWith(record, 'session-example');
    expect(active.stopped).toBe(true);
    expect(active.stopReason).toBe('user');
  });

  it('waits for an in-flight launch before stopping its resulting host session', async () => {
    const active = activeRun();
    active.appServerSessionId = null;
    const ready = deferred();
    active.launchReady = ready.promise;
    const stopping = engine(active).stop('session-example');
    await Promise.resolve();
    expect(host.stop).not.toHaveBeenCalled();
    active.appServerSessionId = 'session-example';
    ready.resolve();
    await stopping;
    expect(host.stop).toHaveBeenCalledWith(record, 'session-example');
  });

  it('surfaces rejection and permits retry without claiming the session stopped', async () => {
    const active = activeRun();
    const subject = engine(active);
    host.stop.mockRejectedValueOnce(new Error('Example host unavailable'));
    await expect(subject.stop('session-example')).rejects.toThrow('Example host unavailable');
    expect(active.stopped).toBe(false);
    await subject.stop('session-example');
    expect(host.stop).toHaveBeenCalledTimes(2);
    expect(active.stopped).toBe(true);
  });

  it('still stops the host when local attachment fails after launch', async () => {
    const active = activeRun();
    active.launchReady = Promise.reject(new Error('Example attachment failed'));
    await engine(active).stop('session-example');
    expect(host.stop).toHaveBeenCalledWith(record, 'session-example');
    expect(active.stopped).toBe(true);
  });

  it('coalesces pending stops and blocks queued continuation while acceptance is pending', async () => {
    const active = Object.assign(activeRun(), {
      context: { run: { id: 'session-example' } },
      queuedContinuations: new Map([['steer-example', { instruction: 'Example follow-up', requestId: 'steer-example' }]])
    });
    const subject = engine(active) as EngineHarness & {
      extendRun: ReturnType<typeof vi.fn>;
      launchQueuedContinuation(active: ActiveHarness): void;
    };
    subject.extendRun = vi.fn();
    const acknowledgement = deferred();
    host.stop.mockReturnValueOnce(acknowledgement.promise);
    const first = subject.stop('session-example');
    const second = subject.stop('session-example');
    subject.launchQueuedContinuation(active);
    expect(subject.extendRun).not.toHaveBeenCalled();
    expect(first).toBe(second);
    expect(host.stop).toHaveBeenCalledTimes(1);
    acknowledgement.resolve();
    await first;
    subject.launchQueuedContinuation(active);
    expect(subject.extendRun).not.toHaveBeenCalled();
  });

  for (const rejected of [false, true]) {
    it(`does not persist a stopped run before host ${rejected ? 'rejection' : 'acceptance'}`, async () => {
      const acknowledgement = deferred();
      const db = {
        getRun: () => ({ id: 'session-example', budget: { runEngine: 'app-server' } }),
        getRunDetail: () => ({ attempts: [{ id: 'attempt-example' }] }),
        updateAttemptState: vi.fn(), updateRunStatus: vi.fn(), appendTraceEvent: vi.fn()
      };
      const runtime = { db, workspacePath: 'workspace-example', appServerEngine: { stop: () => acknowledgement.promise } };
      const service = Object.assign(Object.create(WorkspaceService.prototype) as object, {
        requireRuntimeForRunId: () => runtime, workspacePath: 'workspace-example',
        emitChange: vi.fn(), requireSnapshot: () => ({ workspace: { id: 'workspace-example' } })
      }) as unknown as Pick<WorkspaceService, 'steerRunForClient'>;
      const stopping = service.steerRunForClient({ type: 'stop', runId: 'session-example' });
      expect(db.updateRunStatus).not.toHaveBeenCalled();
      expect(db.updateAttemptState).not.toHaveBeenCalled();
      if (rejected) {
        acknowledgement.reject(new Error('Example stop rejected'));
        await expect(stopping).rejects.toThrow('Example stop rejected');
        expect(db.updateRunStatus).not.toHaveBeenCalled();
      } else {
        acknowledgement.resolve();
        await stopping;
        expect(db.updateRunStatus).toHaveBeenCalledWith('session-example', 'stopped', 'Stopped by user steering.');
      }
    });
  }
});
