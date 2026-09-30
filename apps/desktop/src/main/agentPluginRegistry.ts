import { existsSync, readdirSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { MANAGED_TOOL_PLUGIN_IDS } from '@beale/app-server-runtime/protocol';
import type { AgentPluginRecord, AgentPluginRegistryState } from '@shared/types';
import {
  addAppServerPluginFromFilesystem,
  addAppServerPluginFromRepository,
  getAppServerPluginRuntime,
  listAppServerPlugins,
  removeAppServerPlugin,
  setAppServerPluginEnabled,
  type AppServerAgentPluginRuntime,
  type AppServerBuiltinPlugin
} from './appServerCliClient';

export interface AgentPluginRegistryOptions {
  builtinPlugins?: AppServerBuiltinPlugin[];
  runtimeEnvironment?: (plugin: AgentPluginRecord) => Record<string, string>;
}

export type AgentPluginAppServerRuntime = AppServerAgentPluginRuntime;

export class AgentPluginRegistry {
  public constructor(
    private readonly registryDirectory: string,
    private readonly options: AgentPluginRegistryOptions = {}
  ) {}

  public getState(): AgentPluginRegistryState {
    return listAppServerPlugins(this.baseInput());
  }

  public getAppServerRuntime(): AgentPluginAppServerRuntime {
    const state = this.getState();
    const runtimeEnvironment = Object.fromEntries(state.plugins.map((plugin) => [
      plugin.id,
      this.options.runtimeEnvironment?.(plugin) ?? {}
    ]));
    return getAppServerPluginRuntime({ ...this.baseInput(), runtimeEnvironment });
  }

  public addFromFilesystem(pluginRoot: string): AgentPluginRegistryState {
    return addAppServerPluginFromFilesystem({ ...this.baseInput(), pluginRoot });
  }

  public addFromRepository(repositoryUrl: string): Promise<AgentPluginRegistryState> {
    return addAppServerPluginFromRepository({ ...this.baseInput(), repositoryUrl });
  }

  public setEnabled(pluginId: string, enabled: boolean): AgentPluginRegistryState {
    return setAppServerPluginEnabled({ ...this.baseInput(), pluginId, enabled });
  }

  public remove(pluginId: string): AgentPluginRegistryState {
    return removeAppServerPlugin({ ...this.baseInput(), pluginId });
  }

  private baseInput(): Record<string, unknown> {
    return {
      registryDirectory: this.registryDirectory,
      builtinPlugins: this.options.builtinPlugins ?? defaultBuiltinPlugins()
    };
  }
}

function defaultBuiltinPlugins(): AppServerBuiltinPlugin[] {
  return [
    ...MANAGED_TOOL_PLUGIN_IDS.map((id) => ({
      id: `${id}-builtin`, path: defaultHarnessFeaturePath(id),
      installedAt: '2026-08-14T00:00:00.000Z', enabledByDefault: true
    })),
    {
      id: 'beale-introspection-builtin',
      path: defaultHarnessFeaturePath('beale-introspection'),
      installedAt: '2026-08-14T00:00:00.000Z'
    },
    ...bundledManagedPlugins(),
  ];
}

function bundledManagedPlugins(): AppServerBuiltinPlugin[] {
  const root = defaultManagedPluginPath('');
  if (!existsSync(root)) return [];
  return readdirSync(root, { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && existsSync(join(root, entry.name, 'plugin.json')))
    .map((entry) => ({
      id: `${entry.name}-builtin`,
      path: join(root, entry.name),
      installedAt: '2026-09-23T00:00:00.000Z',
      enabledByDefault: entry.name === 'meta-skills'
    }));
}

function defaultHarnessFeaturePath(directoryName: string): string {
  const resourcesPath = (process as NodeJS.Process & { resourcesPath?: string }).resourcesPath;
  const candidates = [
    ...(resourcesPath
      ? [
          resolve(resourcesPath, 'app-server', 'resources', 'harness-features', directoryName),
          resolve(resourcesPath, 'harness-features', directoryName)
        ]
      : []),
    resolve(process.cwd(), '..', '..', 'app-server', 'resources', 'harness-features', directoryName),
    resolve(__dirname, '..', '..', '..', '..', 'app-server', 'resources', 'harness-features', directoryName)
  ];
  return candidates.find((candidate) => existsSync(candidate)) ?? candidates.at(-1)!;
}

function defaultManagedPluginPath(directoryName: string): string {
  const resourcesPath = (process as NodeJS.Process & { resourcesPath?: string }).resourcesPath;
  const candidates = [
    ...(resourcesPath
      ? [
          resolve(resourcesPath, 'managed-plugins', directoryName),
          resolve(resourcesPath, 'app-server', 'resources', 'managed-plugins', directoryName)
        ]
      : []),
    resolve(process.cwd(), '..', '..', 'managed-plugins', directoryName),
    resolve(__dirname, '..', '..', '..', '..', 'managed-plugins', directoryName)
  ];
  return candidates.find((candidate) => existsSync(candidate)) ?? candidates.at(-1)!;
}
