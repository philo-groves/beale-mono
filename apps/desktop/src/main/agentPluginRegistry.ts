import { existsSync } from 'node:fs';
import { resolve } from 'node:path';
import { MANAGED_TOOL_PLUGIN_IDS } from '@beale/app-server-runtime/protocol';
import type { AgentPluginRecord, AgentPluginRegistryState } from '@shared/types';
import {
  getAppServerDarwinVmSetup,
  updateAppServerDarwinVmSetup,
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

  public getDarwinVmSetup(): import('@beale/app-server-runtime/protocol').DarwinVmSetupState {
    return getAppServerDarwinVmSetup(this.baseInput());
  }

  public updateDarwinVmSetup(update: import('@beale/app-server-runtime/protocol').DarwinVmSetupUpdate): import('@beale/app-server-runtime/protocol').DarwinVmSetupState {
    return updateAppServerDarwinVmSetup({ ...this.baseInput(), update });
  }

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
      id: `${id}-builtin`, path: defaultBuiltinPluginPath(id),
      installedAt: '2026-08-14T00:00:00.000Z', enabledByDefault: true
    })),
    {
      id: 'beale-introspection-builtin',
      path: defaultBuiltinPluginPath('beale-introspection'),
      installedAt: '2026-08-14T00:00:00.000Z'
    },
    {
      id: 'beale-terminator-builtin',
      path: defaultBuiltinPluginPath('beale-terminator'),
      installedAt: '2026-08-17T00:00:00.000Z',
      enabledByDefault: false
    }
  ];
}

function defaultBuiltinPluginPath(directoryName: string): string {
  const resourcesPath = (process as NodeJS.Process & { resourcesPath?: string }).resourcesPath;
  const candidates = [
    ...(resourcesPath
      ? [
          resolve(resourcesPath, 'app-server', 'resources', 'agent-plugins', directoryName),
          resolve(resourcesPath, 'agent-plugins', directoryName)
        ]
      : []),
    resolve(process.cwd(), '..', '..', 'app-server', 'resources', 'agent-plugins', directoryName),
    resolve(__dirname, '..', '..', '..', '..', 'app-server', 'resources', 'agent-plugins', directoryName)
  ];
  return candidates.find((candidate) => existsSync(candidate)) ?? candidates.at(-1)!;
}
