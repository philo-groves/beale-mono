import { homedir } from 'node:os';
import { readFileSync } from 'node:fs';
import { basename, join, resolve } from 'node:path';
import type {
  AppServerSessionSummary,
} from './appServerCliClient';
import { invokeAppServerRegistryState, invokeAppServerRegistryStateAsync } from './appServerCliClient';
import type {
  ComputerUsePermissionMode,
  ComputerUseSettings,
  DebuggingSettings,
  DeveloperSettings,
  MemorySettings,
  MemoryTypeDescriptions,
  ProviderAuthenticationMethod,
  ProviderModelDefaults,
  ProviderSettings,
  ResearchModelProviderId,
  ResearchProfileId,
  ResearchSessionSummary,
  ShellOptions,
  WorkspaceDirectorySelection,
  WorkspaceMemoryBackendId,
  WorkspaceOnboardingDefaults,
  WorkspaceRegistryEntry,
  WorkspaceRegistryState,
  WorkspaceSnapshot,
} from '../shared/types';

interface RegistryBootstrap {
  state: WorkspaceRegistryState;
  profilingEnabled: boolean;
  developerSettings: DeveloperSettings;
  providerSettings: ProviderSettings;
  memorySettings: MemorySettings;
  shellOptions: ShellOptions;
  debuggingSettings: DebuggingSettings;
  computerUseSettings: ComputerUseSettings;
  lastKnownWorkspace: WorkspaceRegistryEntry | null;
}

function defaultWorkspaceRegistryDirectory(): string {
  return process.env.BEALE_WORKSPACE_REGISTRY_DIR?.trim() || join(homedir(), '.beale');
}

/**
 * Synchronous compatibility facade for existing Desktop callers. Persistence is
 * owned and executed by the app-server; this object only keeps a local read cache.
 */
export class WorkspaceRegistry {
  public readonly registryPath: string;
  public readonly internalWorkspaceDirectory: string;
  private readonly shellOptionsPath: string;
  private readonly registryDirectory: string;
  private state: WorkspaceRegistryState;
  private profilingEnabled: boolean;
  private developerSettings: DeveloperSettings;
  private providerSettings: ProviderSettings;
  private memorySettings: MemorySettings;
  private shellOptions: ShellOptions;
  private debuggingSettings: DebuggingSettings;
  private computerUseSettings: ComputerUseSettings;
  private lastKnownWorkspace: WorkspaceRegistryEntry | null;

  public constructor(registryDirectory = defaultWorkspaceRegistryDirectory()) {
    this.registryDirectory = resolve(registryDirectory);
    this.registryPath = join(this.registryDirectory, 'workspace-registry.sqlite');
    this.internalWorkspaceDirectory = join(this.registryDirectory, 'internal-workspaces');
    this.shellOptionsPath = join(this.registryDirectory, 'shell-options.json');
    const bootstrap = this.invoke<RegistryBootstrap>('initialize');
    this.state = bootstrap.state;
    this.profilingEnabled = bootstrap.profilingEnabled;
    this.developerSettings = bootstrap.developerSettings;
    this.providerSettings = bootstrap.providerSettings;
    this.memorySettings = bootstrap.memorySettings;
    this.shellOptions = bootstrap.shellOptions;
    this.debuggingSettings = bootstrap.debuggingSettings;
    this.computerUseSettings = bootstrap.computerUseSettings;
    this.lastKnownWorkspace = bootstrap.lastKnownWorkspace;
  }

  public close(): void {}

  public getState(): WorkspaceRegistryState {
    return this.state;
  }

  public markResearchSessionViewed(sessionId: string, viewedAt?: string): void {
    this.invoke<void>('markResearchSessionViewed', optionalSecondArgument(sessionId, viewedAt));
    this.invalidateState();
  }

  public archiveResearchSession(sessionId: string, archivedAt?: string): void {
    this.invoke<void>('archiveResearchSession', optionalSecondArgument(sessionId, archivedAt));
    this.invalidateState();
  }

  public restoreResearchSession(sessionId: string): void {
    this.invoke<void>('restoreResearchSession', [sessionId]);
    this.invalidateState();
  }

  public listArchivedQuickChats(limit = 200): ResearchSessionSummary[] {
    return this.invoke<ResearchSessionSummary[]>('listArchivedQuickChats', [limit]);
  }

  public getProfilingEnabled(): boolean {
    return this.profilingEnabled;
  }

  public setProfilingEnabled(enabled: boolean): void {
    this.invoke<void>('setProfilingEnabled', [enabled]);
    this.profilingEnabled = enabled;
  }

  public getDeveloperSettings(): DeveloperSettings {
    return this.developerSettings;
  }

  public getDeveloperModeEnabled(): boolean {
    return this.developerSettings.developerModeEnabled;
  }

  public setDeveloperModeEnabled(enabled: boolean): DeveloperSettings {
    this.developerSettings = this.invoke<DeveloperSettings>('setDeveloperModeEnabled', [enabled]);
    return this.developerSettings;
  }

  public getProviderSettings(): ProviderSettings {
    return this.providerSettings;
  }

  public setDefaultProviderId(providerId: ResearchModelProviderId | null): ProviderSettings {
    return this.updateProviderSettings('setDefaultProviderId', [providerId]);
  }

  public setProviderModelDefaults(
    providerId: ResearchModelProviderId,
    defaults: ProviderModelDefaults,
  ): ProviderSettings {
    return this.updateProviderSettings('setProviderModelDefaults', [providerId, defaults]);
  }

  public setProviderOptionalModelEnabled(
    providerId: ResearchModelProviderId,
    modelId: string,
    enabled: boolean,
  ): ProviderSettings {
    return this.updateProviderSettings('setProviderOptionalModelEnabled', [providerId, modelId, enabled]);
  }

  public setProviderCyberPolicyRiskAcknowledged(
    providerId: ResearchModelProviderId,
    acknowledged: boolean,
  ): ProviderSettings {
    return this.updateProviderSettings('setProviderCyberPolicyRiskAcknowledged', [providerId, acknowledged]);
  }

  public setProviderPreferredAuthenticationMethod(
    providerId: ResearchModelProviderId,
    method: ProviderAuthenticationMethod | null,
  ): ProviderSettings {
    return this.updateProviderSettings('setProviderPreferredAuthenticationMethod', [providerId, method]);
  }

  public getMemorySettings(): MemorySettings {
    return this.memorySettings;
  }

  public setMemoryTypeDescriptions(descriptions: MemoryTypeDescriptions): MemorySettings {
    this.memorySettings = this.invoke<MemorySettings>('setMemoryTypeDescriptions', [descriptions]);
    return this.memorySettings;
  }

  public setWorkspaceMemoryBackend(
    registryWorkspaceId: string,
    memoryBackend: WorkspaceMemoryBackendId,
  ): WorkspaceRegistryEntry {
    const workspace = this.invoke<WorkspaceRegistryEntry>('setWorkspaceMemoryBackend', [registryWorkspaceId, memoryBackend]);
    this.replaceWorkspace(workspace);
    return workspace;
  }

  public getShellOptions(): ShellOptions {
    return this.shellOptions;
  }

  public setShellOptions(options: ShellOptions): ShellOptions {
    this.shellOptions = this.invoke<ShellOptions>('setShellOptions', [options]);
    return this.shellOptions;
  }

  public getShellOptionsPath(): string {
    this.invoke<string>('getShellOptionsPath');
    return this.shellOptionsPath;
  }

  public inspectDirectory(path: string): WorkspaceDirectorySelection {
    return this.invoke<WorkspaceDirectorySelection>('inspectDirectory', [path]);
  }

  public getWorkspace(registryWorkspaceId: string): WorkspaceRegistryEntry | null {
    return this.invoke<WorkspaceRegistryEntry | null>('getWorkspace', [registryWorkspaceId]);
  }

  public getWorkspaceByPath(path: string): WorkspaceRegistryEntry | null {
    return this.invoke<WorkspaceRegistryEntry | null>('getWorkspaceByPath', [path]);
  }

  public getDebuggingSettings(): DebuggingSettings {
    return this.debuggingSettings;
  }

  public setTracesEnabled(enabled: boolean): DebuggingSettings {
    this.debuggingSettings = this.invoke<DebuggingSettings>('setTracesEnabled', [enabled]);
    return this.debuggingSettings;
  }

  public getComputerUseSettings(): ComputerUseSettings {
    return this.computerUseSettings;
  }

  public setComputerUsePermissionMode(permissionMode: ComputerUsePermissionMode): ComputerUseSettings {
    this.computerUseSettings = this.invoke<ComputerUseSettings>('setComputerUsePermissionMode', [permissionMode]);
    return this.computerUseSettings;
  }

  public getWorkspaceByDirectory(path: string): WorkspaceRegistryEntry | null {
    return this.invoke<WorkspaceRegistryEntry | null>('getWorkspaceByDirectory', [path]);
  }

  public setWorkspaceDirectories(registryWorkspaceId: string, directories: readonly string[]): WorkspaceRegistryEntry {
    const workspace = this.invoke<WorkspaceRegistryEntry>('setWorkspaceDirectories', [registryWorkspaceId, directories]);
    this.replaceWorkspace(workspace);
    return workspace;
  }

  public getLastKnownWorkspace(): WorkspaceRegistryEntry | null {
    return this.lastKnownWorkspace;
  }

  public rememberWorkspaceOpened(registryWorkspaceId: string, openedAt?: string): void {
    this.invoke<void>('rememberWorkspaceOpened', optionalSecondArgument(registryWorkspaceId, openedAt));
    this.invalidateState();
    this.lastKnownWorkspace = this.invoke<WorkspaceRegistryEntry | null>('getLastKnownWorkspace');
  }

  public removeRegisteredWorkspace(registryWorkspaceId: string): WorkspaceRegistryEntry | null {
    const removed = this.invoke<WorkspaceRegistryEntry | null>('removeRegisteredWorkspace', [registryWorkspaceId]);
    if (removed) {
      this.state = {
        ...this.state,
        workspaces: this.state.workspaces.filter(({ id }) => id !== registryWorkspaceId),
        researchSessions: this.state.researchSessions.filter(({ registryWorkspaceId: id }) => id !== registryWorkspaceId),
        archivedResearchSessions: this.state.archivedResearchSessions?.filter(({ registryWorkspaceId: id }) => id !== registryWorkspaceId),
      };
      if (this.lastKnownWorkspace?.id === registryWorkspaceId) this.lastKnownWorkspace = null;
    }
    return removed;
  }

  public syncWorkspaceFromStorage(
    input: {
      workspacePath: string;
      databasePath: string;
      artifactRoot: string;
      researchProfileId: ResearchProfileId;
    },
    options: { rememberLast?: boolean } = {},
  ): void {
    const existing = this.state.workspaces.find((workspace) => workspace.workspacePath === input.workspacePath);
    const synchronized = this.invoke<{
      state: WorkspaceRegistryState;
      lastKnownWorkspace: WorkspaceRegistryEntry | null;
    }>('syncWorkspaceFromStorage', [{
      ...input,
      workspaceDirectories: existing?.workspaceDirectories ?? [input.workspacePath],
      rememberLast: options.rememberLast ?? true,
    }]);
    this.state = synchronized.state;
    this.lastKnownWorkspace = synchronized.lastKnownWorkspace;
  }

  public syncResearchSession(
    researchProfileId: ResearchProfileId,
    workspacePath: string,
    workspaceId: string,
    row: WorkspaceSnapshot['runs'][number],
  ): boolean {
    const synced = this.invoke<boolean>('syncResearchSession', [researchProfileId, workspacePath, workspaceId, row]);
    if (synced) this.invalidateState();
    return synced;
  }

  public touchResearchSessionActivity(
    researchProfileId: ResearchProfileId,
    workspaceId: string,
    runId: string,
    updatedAt: string,
  ): boolean {
    const touched = this.invoke<boolean>('touchResearchSessionActivity', [researchProfileId, workspaceId, runId, updatedAt]);
    if (touched) this.invalidateState();
    return touched;
  }

  public touchResearchSessionActivityCached(
    workspaceId: string,
    runId: string,
    updatedAt: string,
  ): boolean {
    const sessionIndex = this.state.researchSessions.findIndex((session) => (
      session.workspaceId === workspaceId && session.runId === runId
    ));
    if (sessionIndex < 0) return false;
    const current = this.state.researchSessions[sessionIndex]!;
    if (current.updatedAt >= updatedAt) return true;
    const researchSessions = [...this.state.researchSessions];
    researchSessions[sessionIndex] = { ...current, updatedAt };
    researchSessions.sort((left, right) => right.updatedAt.localeCompare(left.updatedAt));
    this.state = { ...this.state, researchSessions };
    return true;
  }

  public reconcileAppServerSessions(
    researchProfileId: ResearchProfileId,
    workspaceId: string,
    sessions: readonly AppServerSessionSummary[],
  ): void {
    this.invoke<void>('reconcileAppServerSessions', [researchProfileId, workspaceId, sessions]);
    this.invalidateState();
  }

  public async reconcileAppServerSessionsAsync(
    researchProfileId: ResearchProfileId,
    workspaceId: string,
    sessions: readonly AppServerSessionSummary[],
  ): Promise<void> {
    await this.invokeAsync<void>('reconcileAppServerSessions', [researchProfileId, workspaceId, sessions]);
  }

  public markAppServerSessionsInterrupted(
    researchProfileId: ResearchProfileId,
    workspaceId: string,
    runIds: readonly string[],
    updatedAt?: string,
  ): void {
    const args: unknown[] = [researchProfileId, workspaceId, runIds];
    if (updatedAt !== undefined) args.push(updatedAt);
    this.invoke<void>('markAppServerSessionsInterrupted', args);
    this.invalidateState();
  }

  public async markAppServerSessionsInterruptedAsync(
    researchProfileId: ResearchProfileId,
    workspaceId: string,
    runIds: readonly string[],
    updatedAt?: string,
  ): Promise<void> {
    const args: unknown[] = [researchProfileId, workspaceId, runIds];
    if (updatedAt !== undefined) args.push(updatedAt);
    await this.invokeAsync<void>('markAppServerSessionsInterrupted', args);
  }

  public async refreshStateAsync(): Promise<void> {
    const currentSessions = new Map(this.state.researchSessions.map((session) => [session.runId, session]));
    const refreshed = await this.invokeAsync<WorkspaceRegistryState>('getState');
    const researchSessions = refreshed.researchSessions.map((session) => {
      const current = currentSessions.get(session.runId);
      return current && current.updatedAt > session.updatedAt ? current : session;
    }).sort((left, right) => right.updatedAt.localeCompare(left.updatedAt));
    this.state = { ...refreshed, researchSessions };
  }

  private invoke<T>(action: string, args: readonly unknown[] = []): T {
    return invokeAppServerRegistryState<T>(this.registryDirectory, action, args);
  }

  private async invokeAsync<T>(action: string, args: readonly unknown[] = []): Promise<T> {
    return await invokeAppServerRegistryStateAsync<T>(this.registryDirectory, action, args);
  }

  private invalidateState(): void {
    this.state = this.invoke<WorkspaceRegistryState>('getState');
  }

  private updateProviderSettings(action: string, args: readonly unknown[]): ProviderSettings {
    this.providerSettings = this.invoke<ProviderSettings>(action, args);
    return this.providerSettings;
  }

  private replaceWorkspace(workspace: WorkspaceRegistryEntry): void {
    this.state = {
      ...this.state,
      workspaces: this.state.workspaces.map((candidate) => candidate.id === workspace.id ? workspace : candidate),
    };
    if (this.lastKnownWorkspace?.id === workspace.id) this.lastKnownWorkspace = workspace;
  }
}

function optionalSecondArgument(first: string, second: string | undefined): unknown[] {
  return second === undefined ? [first] : [first, second];
}

export function isUntrackedResourceSession(row: WorkspaceSnapshot['runs'][number]): boolean {
  const context = row.run.budget.resourceContext;
  return Boolean(
    context
    && typeof context === 'object'
    && !Array.isArray(context)
    && (context as Record<string, unknown>).kind === 'report'
  );
}

export function defaultsForWorkspaceDirectory(workspacePath: string): WorkspaceOnboardingDefaults {
  const resolvedWorkspacePath = resolve(workspacePath);
  let descriptionMarkdown = '';
  try {
    descriptionMarkdown = readFileSync(join(resolvedWorkspacePath, 'AGENTS.md'), 'utf8');
  } catch {
    // Workspaces without guidance start with an empty portable description.
  }
  const normalizedName = basename(resolvedWorkspacePath)
    .replace(/[-_]+/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();
  return {
    workspacePath: resolvedWorkspacePath,
    workspaceDirectories: [resolvedWorkspacePath],
    workspaceName: normalizedName
      ? normalizedName.replace(/\b\w/g, (letter) => letter.toUpperCase())
      : 'Untitled Workspace',
    scopeOwner: '',
    descriptionMarkdown,
    rules: [],
    expiresAt: null,
    assets: [],
  };
}
