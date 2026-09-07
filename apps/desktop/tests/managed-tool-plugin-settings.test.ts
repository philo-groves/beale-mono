import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it, vi } from 'vitest';
import { MANAGED_TOOL_PLUGINS } from '@beale/app-server-runtime/protocol';
import { AgentPluginRegistry } from '../src/main/agentPluginRegistry';
import { listAppServerPlugins, getAppServerPluginRuntime } from '../src/main/appServerCliClient';

vi.mock('../src/main/appServerCliClient', () => ({
  listAppServerPlugins: vi.fn(() => ({ plugins: [] })),
  getAppServerPluginRuntime: vi.fn(() => ({ managedPluginIds: [] })),
}));

describe('managed tool plugin settings boundary', () => {
  it('sends bundled defaults with matching usage guidance to the canonical host', () => {
    const registry = new AgentPluginRegistry('example-registry');
    registry.getState();
    const input = vi.mocked(listAppServerPlugins).mock.calls.at(-1)![0];
    const defaults = input.builtinPlugins as Array<{ id: string; path: string; enabledByDefault?: boolean }>;
    for (const plugin of MANAGED_TOOL_PLUGINS) {
      const builtin = defaults.find((candidate) => candidate.id === `${plugin.id}-builtin`)!;
      expect(builtin.enabledByDefault).toBe(true);
      const manifest = JSON.parse(readFileSync(resolve(builtin.path, 'plugin.json'), 'utf8'));
      expect(manifest.description).toBe(plugin.description);
    }
    expect(defaults.some((plugin) => /workspace|execution/.test(plugin.id))).toBe(false);
    expect(registry.getAppServerRuntime().managedPluginIds).toEqual([]);
    expect(getAppServerPluginRuntime).toHaveBeenCalledWith(expect.objectContaining({ builtinPlugins: defaults }));
  });
});
