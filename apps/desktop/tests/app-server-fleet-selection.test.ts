import { describe, expect, it, vi } from 'vitest';
import { AppServerRunEngine } from '../src/main/appServerRunEngine';

const host = vi.hoisted(() => ({ prepare: vi.fn() }));
vi.mock('../src/main/bealeAppServerClient', async (importOriginal) => ({
  ...await importOriginal<typeof import('../src/main/bealeAppServerClient')>(),
  invokeAppServerOperation: host.prepare
}));

describe('Fleet machine selection for a session', () => {
  it('prepares only the recorded machine even when an older run contains a candidate list', async () => {
    const subject = Object.create(AppServerRunEngine.prototype) as {
      startAppServerRun(params: unknown): Promise<void>;
    };
    host.prepare.mockReset();
    host.prepare.mockRejectedValue(new Error('Synthetic selected base unavailable'));
    const active = { context: { run: { id: 'run-example', mode: 'research', budget: {
      machineId: 'tart:base-a', machineIds: ['tart:base-a', 'tart:base-b']
    } } }, stopped: false };

    await expect(subject.startAppServerRun({ active, request: {
      launch: { workspaceId: 'workspace-example', provider: { id: 'openai-codex', model: 'model-example' },
        collaboration: { providers: [{ provider: 'xai', model: 'collaborator-example', enabled: true },
          { provider: 'zai', model: 'disabled-example', enabled: false }] } }
    } })).rejects.toThrow('Synthetic selected base unavailable');
    expect(host.prepare.mock.calls.map(([request]) => request.input.machineId)).toEqual(['tart:base-a']);
    expect(host.prepare.mock.calls[0]?.[0].input.providerModels).toEqual([
      { providerId: 'openai-codex', modelId: 'model-example' },
      { providerId: 'xai', modelId: 'collaborator-example' },
    ]);
  });
});
