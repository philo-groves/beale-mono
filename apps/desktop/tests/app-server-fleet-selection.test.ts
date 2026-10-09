import { describe, expect, it, vi } from 'vitest';
import { AppServerRunEngine } from '../src/main/appServerRunEngine';

const host = vi.hoisted(() => ({ prepare: vi.fn() }));
vi.mock('../src/main/bealeAppServerClient', async (importOriginal) => ({
  ...await importOriginal<typeof import('../src/main/bealeAppServerClient')>(),
  invokeAppServerOperation: host.prepare
}));

describe('Fleet base selection for a new session', () => {
  it('tries selected bases in order and records the one that prepared the session', async () => {
    const run = { id: 'run-example', mode: 'research', budget: {
      machineId: 'tart:base-a', machineIds: ['tart:base-a', 'tart:base-b']
    } };
    const active = { context: { run }, stopped: false };
    const updateRunBudget = vi.fn((_runId: string, patch: { machineId: string }) => ({
      ...run, budget: { ...run.budget, ...patch }
    }));
    const finishPrelaunchStop = vi.fn();
    const subject = Object.assign(Object.create(AppServerRunEngine.prototype) as object, {
      db: { updateRunBudget }, finishPrelaunchStop
    }) as unknown as { startAppServerRun(params: unknown): Promise<void> };
    host.prepare.mockReset();
    host.prepare.mockRejectedValueOnce(new Error('Synthetic first base unavailable'));
    host.prepare.mockImplementationOnce(async () => {
      active.stopped = true;
      return { machineId: 'tart:base-b' };
    });

    await subject.startAppServerRun({ active, request: { launch: { workspaceId: 'workspace-example' } } });

    expect(host.prepare.mock.calls.map(([request]) => request.input.machineId)).toEqual(['tart:base-a', 'tart:base-b']);
    expect(updateRunBudget).toHaveBeenCalledWith('run-example', { machineId: 'tart:base-b' });
    expect(active.context.run.budget.machineId).toBe('tart:base-b');
    expect(finishPrelaunchStop).toHaveBeenCalledOnce();
  });

  it('keeps a continuing session on its recorded base', async () => {
    const subject = Object.assign(Object.create(AppServerRunEngine.prototype) as object, {
      db: { updateRunBudget: vi.fn() }
    }) as unknown as { startAppServerRun(params: unknown): Promise<void> };
    host.prepare.mockReset();
    host.prepare.mockRejectedValue(new Error('Synthetic current base unavailable'));
    const active = { context: { run: { id: 'run-example', mode: 'research', budget: {
      machineId: 'tart:base-b', machineIds: ['tart:base-a', 'tart:base-b']
    } } }, stopped: false };

    await expect(subject.startAppServerRun({ active, request: {
      launch: { workspaceId: 'workspace-example', continuation: { fallbackPrompt: 'Continue example research' } }
    } })).rejects.toThrow('Synthetic current base unavailable');
    expect(host.prepare.mock.calls.map(([request]) => request.input.machineId)).toEqual(['tart:base-b']);
  });
});
