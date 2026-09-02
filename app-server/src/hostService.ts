import { existsSync } from 'node:fs';
import { mkdir, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { randomUUID } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import {
  BEALE_APP_SERVER_CONTROL_VERSION,
  APP_SERVER_SESSION_LAUNCH_VERSION,
  type BealeAppServerCanonicalResult,
  type BealeAppServerProviderCatalog,
  type BealeMemoryNotificationFeed,
  type BealeWorkspaceMemoryCatalog,
  type BealeWorkspaceClaimSecurityTracking,
  type BealeWorkspaceMemoryNode,
  type BealeAppServerWorkspaceList,
  decodeAppServerSessionLaunchRequest,
  type AppServerProtocolOperation,
  type AppServerSessionLaunchRequest
} from '@beale/app-server-runtime/protocol';
import { getProviderModelCatalog } from '@beale/app-server-runtime/runtime-services';
import {
  AppServerHostRegistry,
  type AppServerHostRegistryOptions,
  type AppServerHostStorage,
  type AppServerHostWorkspace
} from './hostRegistry.js';
import {
  invokeAppServerProtocol,
  type InvokeAppServerProtocolOptions
} from './appServerProtocolClient.js';
import {
  resolveAppServerCodexAuthFile,
  type ResolvedAppServerSessionLaunch
} from './sessionLaunch.js';
import { longSessionRecoveryFallbackPrompt } from './sessionRecovery.js';

type ProtocolInvoker = <T>(
  operation: AppServerProtocolOperation,
  options: InvokeAppServerProtocolOptions
) => Promise<T>;

interface StoredRestartLaunchDescriptor {
  schemaVersion: 1;
  eligible: boolean;
  launch: AppServerSessionLaunchRequest['launch'];
}

interface AppServerSessionSummaryProjection {
  id?: unknown;
  workspaceId?: unknown;
  status?: unknown;
  prompt?: unknown;
  provider?: unknown;
  model?: unknown;
  reasoningEffort?: unknown;
  workflowId?: unknown;
  profile?: unknown;
  metadata?: unknown;
  attempts?: unknown;
  createdAt?: unknown;
  updatedAt?: unknown;
}

interface AppServerSessionUpdateProjection {
  session?: AppServerSessionSummaryProjection;
}

interface AppServerPluginRuntimeProjection {
  skillDirs?: unknown;
  selectedSkillIds?: unknown;
  mcpConfigPath?: unknown;
  allowedMcpServers?: unknown;
}

interface AppServerProviderSemanticsProjection {
  defaultSmallModels?: unknown;
  sessionTitleEffort?: unknown;
  shellReviewEffort?: unknown;
}

export interface PreparedAppServerSession {
  sessionId: string;
  attemptId: string;
  launch: ResolvedAppServerSessionLaunch;
}

export interface RecoveredAppServerSession {
  request: AppServerSessionLaunchRequest;
  prepared: PreparedAppServerSession;
}

export interface AppServerStartupRecoveryResult {
  recovered: RecoveredAppServerSession[];
  interruptedSessions: number;
  skippedSessions: number;
  errors: string[];
}

export interface DueAppServerAutomation {
  request: AppServerSessionLaunchRequest;
  dueAt: string;
}

export interface AppServerHostServiceOptions extends AppServerHostRegistryOptions {
  registry?: AppServerHostRegistry;
  invokeProtocol?: ProtocolInvoker;
}

export class AppServerHostService {
  private readonly registry: AppServerHostRegistry;
  private readonly invokeProtocol: ProtocolInvoker;
  private pluginRuntimePromise: Promise<ResolvedAppServerSessionLaunch['pluginRuntime'] | null> | null = null;
  private providerSemanticsPromise: Promise<{
    defaultSmallModels: Record<string, string>;
    sessionTitleEffort: string;
    shellReviewEffort: string;
  }> | null = null;

  public constructor(options: AppServerHostServiceOptions = {}) {
    this.registry = options.registry ?? new AppServerHostRegistry(options);
    this.invokeProtocol = options.invokeProtocol ?? invokeAppServerProtocol;
  }

  public listWorkspaces(): BealeAppServerWorkspaceList {
    return {
      controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
      workspaces: this.registry.listWorkspaces()
    };
  }

  public providerCatalog(): BealeAppServerProviderCatalog {
    const settings = this.registry.providerSettings();
    const connectedProviderIds = new Set([
      ...(settings.defaultProviderId ? [settings.defaultProviderId] : []),
      ...Object.keys(settings.modelDefaults),
      ...Object.keys(settings.authenticationPreferences)
    ]);
    return {
      controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
      defaultProviderId: settings.defaultProviderId,
      providers: getProviderModelCatalog()
        .filter((catalog) => connectedProviderIds.has(catalog.providerId))
        .map((catalog) => {
          const defaults = settings.modelDefaults[catalog.providerId];
          return {
            providerId: catalog.providerId,
            providerName: catalog.providerName,
            defaultLeadModel: defaults?.leadModel ?? null,
            defaultSubagentModel: defaults?.smallModel ?? null,
            defaultReasoningEffort: defaults?.reasoningEffort ?? null,
            models: catalog.models.map((model) => ({
              id: model.id,
              name: model.name,
              reasoning: model.reasoning,
              effortLevels: [...model.effortLevels]
            }))
          };
        })
    };
  }

  public async executeOperation(request: {
    operation: AppServerProtocolOperation;
    args?: readonly string[];
    input?: unknown;
    profileId?: string;
    signal?: AbortSignal;
  }): Promise<unknown> {
    const workspaceIdentifier = workspaceIdentifierFromInput(request.input);
    const workspace = workspaceIdentifier
      ? this.registry.resolveWorkspace(workspaceIdentifier)
      : null;
    if (workspace && request.profileId && request.profileId !== workspace.researchProfileId) {
      throw new Error(`Workspace ${workspace.workspaceId} uses research profile ${workspace.researchProfileId}, not ${request.profileId}.`);
    }
    if (workspace?.memoryBackend === 'disabled'
      && (request.operation.startsWith('dreaming.')
        || request.operation.startsWith('investigation.')
        || request.operation.startsWith('claim.'))) {
      throw new Error(`Workspace ${workspace.workspaceId} has memory disabled.`);
    }
    const storageProfileId = workspace?.researchProfileId ?? request.profileId;
    const storage = storageProfileId ? this.registry.storageForProfile(storageProfileId) : null;
    const operationInput = workspace && storage
      ? request.operation.startsWith('suggestion.')
        ? this.hostedSuggestionInput(request.operation, request.input, workspace, storage)
        : request.operation === 'prompt.expand'
          ? this.hostedPromptExpansionInput(request.input, workspace, storage)
          : request.input
      : request.input;
    const result = await this.invokeProtocol(request.operation, {
      args: request.args ?? [],
      ...(operationInput !== undefined ? { input: operationInput } : {}),
      ...(storage ? { storage } : {}),
      ...(request.signal ? { signal: request.signal } : {})
    });
    if (workspace?.memoryBackend !== 'disabled') return result;
    if (request.operation === 'memory.summary') return withoutWorkspaceMemory(result);
    if (request.operation === 'memory.notification_feed'
      && result && typeof result === 'object' && !Array.isArray(result)) {
      return { ...(result as Record<string, unknown>), nodes: [] };
    }
    return result;
  }

  private hostedSuggestionInput(
    operation: AppServerProtocolOperation,
    input: unknown,
    workspace: AppServerHostWorkspace,
    storage: AppServerHostStorage
  ): Record<string, unknown> {
    if (!isRecord(input)) throw new Error('Research goal suggestion input is required.');
    if (operation === 'suggestion.select') {
      return {
        ...input,
        workspaceId: workspace.workspaceId,
        databasePath: storage.databasePath
      };
    }
    if (operation !== 'suggestion.generate') return { ...input };
    const settings = this.registry.providerSettings();
    const providerId = settings.defaultProviderId;
    if (!providerId) throw new Error('No Lead provider is configured for this Beale host.');
    const defaults = settings.modelDefaults[providerId];
    const codexAuthFile = providerId === 'openai-codex'
      ? resolveAppServerCodexAuthFile()
      : undefined;
    return {
      ...input,
      workspaceId: workspace.workspaceId,
      workspaceRoot: workspace.workspacePath,
      databasePath: storage.databasePath,
      artifactDirectoryPath: storage.artifactDirectoryPath,
      researchProfileId: workspace.researchProfileId,
      memoryEnabled: workspace.memoryBackend !== 'disabled',
      provider: {
        id: providerId,
        ...(defaults?.smallModel ? { smallModel: defaults.smallModel } : {}),
        ...(defaults?.reasoningEffort ? { reasoningEffort: defaults.reasoningEffort } : {}),
        ...(codexAuthFile ? { codexAuthFile } : {}),
        authenticationPreferences: settings.authenticationPreferences
      }
    };
  }

  private hostedPromptExpansionInput(
    input: unknown,
    workspace: AppServerHostWorkspace,
    storage: AppServerHostStorage
  ): Record<string, unknown> {
    if (!isRecord(input)) throw new Error('Research prompt expansion input is required.');
    const settings = this.registry.providerSettings();
    const requestedProvider = isRecord(input.provider) ? nonEmpty(input.provider.id) : undefined;
    const providerId = requestedProvider ?? settings.defaultProviderId;
    if (!providerId) throw new Error('No Lead provider is configured for this Beale host.');

    const connectedProviderIds = new Set([
      ...(settings.defaultProviderId ? [settings.defaultProviderId] : []),
      ...Object.keys(settings.modelDefaults),
      ...Object.keys(settings.authenticationPreferences)
    ]);
    if (!connectedProviderIds.has(providerId)) {
      throw new Error(`Provider ${providerId} is not connected to this Beale host.`);
    }
    const catalog = getProviderModelCatalog().find((candidate) => candidate.providerId === providerId);
    if (!catalog) throw new Error(`Unsupported research model provider: ${providerId}.`);
    const requestedModel = isRecord(input.provider) ? nonEmpty(input.provider.model) : undefined;
    if (requestedModel && !catalog.models.some((candidate) => candidate.id === requestedModel)) {
      throw new Error(`Model ${requestedModel} is not available from connected provider ${providerId}.`);
    }
    const defaults = settings.modelDefaults[providerId];
    const model = requestedModel ?? defaults?.leadModel;
    if (!model) throw new Error(`No Lead model is configured for provider ${providerId}.`);
    const codexAuthFile = providerId === 'openai-codex'
      ? resolveAppServerCodexAuthFile()
      : undefined;
    return {
      ...input,
      workspaceId: workspace.workspaceId,
      workspaceRoot: workspace.workspacePath,
      databasePath: storage.databasePath,
      artifactDirectoryPath: storage.artifactDirectoryPath,
      researchProfileId: workspace.researchProfileId,
      memoryEnabled: workspace.memoryBackend !== 'disabled',
      provider: {
        id: providerId,
        model,
        ...(defaults?.reasoningEffort ? { reasoningEffort: defaults.reasoningEffort } : {}),
        ...(codexAuthFile ? { codexAuthFile } : {}),
        authenticationPreferences: settings.authenticationPreferences
      }
    };
  }

  public async prepareSession(
    request: AppServerSessionLaunchRequest,
    generatedSessionId: string
  ): Promise<PreparedAppServerSession> {
    const sessionId = request.sessionId ?? generatedSessionId;
    const workspace = this.requireWorkspace(request.launch.workspaceId);
    const attemptId = request.launch.attemptId ?? `attempt-${randomUUID()}`;
    const requestedProfileId = request.launch.researchProfileId?.trim();
    if (requestedProfileId && requestedProfileId !== workspace.researchProfileId) {
      throw new Error(
        `Workspace ${workspace.workspaceId} uses research profile ${workspace.researchProfileId}, not ${requestedProfileId}.`
      );
    }
    const profileId = workspace.researchProfileId || 'security-research';
    const storage = this.registry.storageForProfile(profileId);
    const providerSettings = this.registry.providerSettings();
    const providerSemantics = await this.resolveProviderSemantics();
    const providerId = request.launch.provider?.id?.trim()
      || providerSettings.defaultProviderId;
    if (!providerId) {
      throw new Error('No Lead provider is configured for this Beale host.');
    }
    const providerDefaults = providerSettings.modelDefaults[providerId];
    const model = request.launch.provider?.model?.trim()
      || providerDefaults?.leadModel;
    const reasoningEffort = request.launch.provider?.reasoningEffort?.trim()
      || providerDefaults?.reasoningEffort;
    const fastMode = request.launch.provider?.fastMode === true;
    if (fastMode && providerId !== 'openai-codex') {
      throw new Error('Fast mode is available only when OpenAI is the Lead provider.');
    }
    const shellReviewModels = {
      ...providerSemantics.defaultSmallModels,
      ...Object.fromEntries(Object.entries(providerSettings.modelDefaults).flatMap(([id, defaults]) => (
        defaults.smallModel ? [[id, defaults.smallModel] as const] : []
      )))
    };
    const runDirectory = join(workspace.workspacePath, '.beale', 'app-server-runs');
    const continuation = request.launch.continuation;
    const fileStem = continuation ? `${sessionId}.${attemptId}` : sessionId;
    const capturePath = join(runDirectory, `${fileStem}.capture.json`);
    await mkdir(runDirectory, { recursive: true });
    const collaborationConfigPath = request.launch.collaboration
      ? join(runDirectory, `${fileStem}.collaboration.json`)
      : undefined;
    if (collaborationConfigPath) {
      await writePrivateJson(collaborationConfigPath, request.launch.collaboration);
    }
    const resumeFallbackPromptPath = continuation
      ? join(runDirectory, `${fileStem}.resume-fallback.md`)
      : undefined;
    if (resumeFallbackPromptPath && continuation) {
      await writeFile(resumeFallbackPromptPath, continuation.fallbackPrompt, { encoding: 'utf8', mode: 0o600 });
    }
    const resumeCapturePath = continuation?.resumeAttemptId
      ? join(
          runDirectory,
          continuation.resumeFromInitialAttempt
            ? `${sessionId}.capture.json`
            : `${sessionId}.${continuation.resumeAttemptId}.capture.json`
        )
      : undefined;
    const introspection = validatedIntrospectionEndpoint(request.launch.introspection);
    const loadedPluginRuntime = introspection?.runtimeMode === 'isolated'
      ? await this.loadIntrospectionPluginRuntime()
      : await this.resolvePluginRuntime();
    const pluginRuntime = loadedPluginRuntime
      ? {
          ...loadedPluginRuntime,
          ...(!introspection && loadedPluginRuntime.allowedMcpServers
            ? {
                allowedMcpServers: loadedPluginRuntime.allowedMcpServers.filter(
                  (name) => name !== 'beale-introspection.beale'
                )
              }
            : {})
        }
      : null;
    const restartLaunch = restartLaunchDescriptor(request, {
      providerId,
      ...(model ? { model } : {}),
      ...(reasoningEffort ? { reasoningEffort } : {}),
      ...(fastMode ? { fastMode: true } : {}),
      profileId
    });
    await this.ensureCanonicalSession({
      sessionId,
      attemptId,
      workspace,
      storage,
      prompt: request.launch.promptMarkdown,
      providerId,
      ...(model ? { model } : {}),
      ...(reasoningEffort ? { reasoningEffort } : {}),
      ...(request.launch.workflowId ? { workflowId: request.launch.workflowId } : {}),
      profileId,
      ...(request.launch.researchProfileHash
        ? { profileHash: request.launch.researchProfileHash }
        : {}),
      continuation: Boolean(continuation),
      restartLaunch,
      ...(continuation?.resumeAttemptId ? { parentAttemptId: continuation.resumeAttemptId } : {})
    });
    return {
      sessionId,
      attemptId,
      launch: {
        workspaceRoot: workspace.workspacePath,
        workspaceDirectories: workspace.workspaceDirectories,
        capturePath,
        attemptId,
        promptMarkdown: request.launch.promptMarkdown,
        ...(request.launch.goal ? { goal: request.launch.goal } : {}),
        provider: {
          id: providerId,
          ...(model ? { model } : {}),
          ...(reasoningEffort ? { reasoningEffort } : {}),
          ...(fastMode ? { fastMode: true } : {}),
          riskAcknowledgements: providerSettings.riskAcknowledgements,
          authenticationPreferences: providerSettings.authenticationPreferences,
          ...(request.launch.generateTitle
            ? {
                title: {
                  ...(providerDefaults?.smallModel || providerSemantics.defaultSmallModels[providerId]
                    ? { model: providerDefaults?.smallModel ?? providerSemantics.defaultSmallModels[providerId]! }
                    : {}),
                  effort: providerSemantics.sessionTitleEffort
                }
              }
            : {}),
          shellReview: {
            models: shellReviewModels,
            effort: providerSemantics.shellReviewEffort
          }
        },
        shellSafetyMode: request.launch.shellSafetyMode?.trim() || 'auto_review',
        ...(existsSync(this.registry.shellOptionsPath) ? { shellOptionsPath: this.registry.shellOptionsPath } : {}),
        ...(collaborationConfigPath ? { collaborationConfigPath } : {}),
        ...(resumeCapturePath ? { resumeCapturePath } : {}),
        ...(resumeFallbackPromptPath ? { resumeFallbackPromptPath } : {}),
        ...(request.launch.workflowId ? { workflowId: request.launch.workflowId } : {}),
        researchProfileId: profileId,
        ...(request.launch.researchProfileHash
          ? { researchProfileHash: request.launch.researchProfileHash }
          : {}),
        profileAware: Boolean(profileId),
        memoryBackend: workspace.memoryBackend,
        ...(pluginRuntime ? { pluginRuntime } : {}),
        ...(introspection
          ? { introspection: { url: introspection.url, token: introspection.token } }
          : {}),
        ...(!profileId && this.registry.memoryTypeDescriptions()
          ? { memoryTypeDescriptions: this.registry.memoryTypeDescriptions()! }
          : {}),
        storage
      }
    };
  }

  public async prepareSessionRecovery(input: {
    request: AppServerSessionLaunchRequest;
    sessionId: string;
    previousAttemptId: string;
    previousAttemptWasInitial: boolean;
    fallbackPrompt: string;
  }): Promise<PreparedAppServerSession> {
    const workspace = this.requireWorkspace(input.request.launch.workspaceId);
    const storage = this.registry.storageForProfile(workspace.researchProfileId || 'security-research');
    const existing = await this.invokeProtocol<AppServerSessionSummaryProjection>('session.get', {
      args: ['session', 'get', '--session-id', input.sessionId],
      storage
    });
    if (nonEmpty(existing.status) === 'active') {
      await this.invokeProtocol('session.transition', {
        args: ['session', 'transition', '--session-id', input.sessionId],
        storage,
        input: {
          status: 'paused',
          summary: 'Paused after an unexpected app-server worker failure.',
          attemptId: input.previousAttemptId,
          metadata: {
            longSessionRecovery: true,
            interruptedAttemptId: input.previousAttemptId
          }
        }
      });
    }
    return await this.prepareSession({
      ...input.request,
      sessionId: input.sessionId,
      launch: {
        ...input.request.launch,
        attemptId: `attempt-${randomUUID()}`,
        generateTitle: false,
        continuation: {
          resumeAttemptId: input.previousAttemptId,
          resumeFromInitialAttempt: input.previousAttemptWasInitial,
          fallbackPrompt: input.fallbackPrompt
        }
      }
    }, input.sessionId);
  }

  public async recoverInterruptedSessions(): Promise<AppServerStartupRecoveryResult> {
    const recovered: RecoveredAppServerSession[] = [];
    const errors: string[] = [];
    let interruptedSessions = 0;
    let skippedSessions = 0;
    for (const workspace of this.registry.listWorkspaces()) {
      const storage = this.registry.storageForProfile(workspace.researchProfileId || 'security-research');
      let report: { sessionIds?: unknown; interruptedSessions?: unknown };
      try {
        report = await this.invokeProtocol('session.recover_interrupted', {
          args: ['session', 'recover-interrupted', '--workspace-id', workspace.workspaceId],
          storage,
          input: { reason: 'app_server_startup', at: new Date().toISOString() }
        }) as { sessionIds?: unknown; interruptedSessions?: unknown };
      } catch (error) {
        errors.push(`${workspace.workspaceId}: ${errorMessage(error)}`);
        continue;
      }
      interruptedSessions += Number.isInteger(report.interruptedSessions)
        ? Number(report.interruptedSessions)
        : 0;
      const sessionIds = stringArray(report.sessionIds);
      for (const sessionId of sessionIds) {
        try {
          const session = await this.invokeProtocol<AppServerSessionSummaryProjection>('session.get', {
            args: ['session', 'get', '--session-id', sessionId],
            storage
          });
          const restart = decodeRestartLaunchDescriptor(session.metadata);
          const previousAttempt = interruptedAttempt(session.attempts);
          if (!restart || !restart.eligible || !previousAttempt) {
            skippedSessions += 1;
            continue;
          }
          const request: AppServerSessionLaunchRequest = {
            launchVersion: APP_SERVER_SESSION_LAUNCH_VERSION,
            sessionId,
            launch: restart.launch
          };
          const diagnostic = 'The prior app-server process ended while this session was active.';
          const prepared = await this.prepareSessionRecovery({
            request,
            sessionId,
            previousAttemptId: previousAttempt.id,
            previousAttemptWasInitial: previousAttempt.parentAttemptId === null,
            fallbackPrompt: longSessionRecoveryFallbackPrompt(restart.launch.promptMarkdown, diagnostic)
          });
          recovered.push({ request, prepared });
        } catch (error) {
          skippedSessions += 1;
          errors.push(`${sessionId}: ${errorMessage(error)}`);
        }
      }
    }
    return { recovered, interruptedSessions, skippedSessions, errors };
  }

  public async dueAutomations(at = new Date()): Promise<DueAppServerAutomation[]> {
    const due: DueAppServerAutomation[] = [];
    for (const summary of this.registry.listWorkspaces()) {
      const workspace = this.registry.resolveWorkspace(summary.workspaceId);
      if (!workspace) continue;
      const storage = this.registry.storageForProfile(workspace.researchProfileId || 'security-research');
      let sessions: AppServerSessionSummaryProjection[];
      try {
        sessions = await this.invokeProtocol<AppServerSessionSummaryProjection[]>('session.list_summaries', {
          args: ['session', 'list-summaries', '--workspace-id', workspace.workspaceId, '--limit', '500'],
          storage
        });
      } catch {
        continue;
      }
      for (const session of sessions) {
        const candidate = dueAutomation(session, workspace, at);
        if (candidate) due.push(candidate);
      }
    }
    return due.sort((left, right) => left.dueAt.localeCompare(right.dueAt));
  }

  public async recordSessionControlState(input: {
    request: AppServerSessionLaunchRequest;
    sessionId: string;
    attemptId: string;
    state: 'active' | 'paused' | 'stopped';
  }): Promise<void> {
    const workspace = this.requireWorkspace(input.request.launch.workspaceId);
    const storage = this.registry.storageForProfile(workspace.researchProfileId || 'security-research');
    const summary = input.state === 'paused'
      ? 'Paused by the user.'
      : input.state === 'stopped'
        ? 'Stopped by the user.'
        : 'Resumed by the user.';
    await this.invokeProtocol('session.transition', {
      args: ['session', 'transition', '--session-id', input.sessionId],
      storage,
      input: {
        status: input.state,
        summary,
        attemptId: input.attemptId,
        metadata: {
          manualControlState: input.state,
          manualControlAt: new Date().toISOString()
        }
      }
    });
  }

  public async workspaceMemory(
    workspaceIdentifier: string
  ): Promise<BealeAppServerCanonicalResult<BealeWorkspaceMemoryCatalog>> {
    const workspace = this.requireWorkspace(workspaceIdentifier);
    const storage = this.registry.storageForProfile(workspace.researchProfileId);
    const result = await this.invokeProtocol<unknown>('memory.summary', {
      args: ['knowledge', 'summary'],
      input: {
        workspaceId: workspace.workspaceId,
        workspaceRoot: workspace.workspacePath,
        researchProfileId: workspace.researchProfileId
      },
      storage
    });
    return canonicalResult(workspace, pathFreeWorkspaceMemoryCatalog(
      workspace.workspaceId,
      workspace.memoryBackend === 'disabled' ? withoutWorkspaceMemory(result) : result
    ));
  }

  public async workspaceMemoryNotifications(
    workspaceIdentifier: string,
    sessionId?: string
  ): Promise<BealeAppServerCanonicalResult<BealeMemoryNotificationFeed>> {
    const workspace = this.requireWorkspace(workspaceIdentifier);
    const storage = this.registry.storageForProfile(workspace.researchProfileId);
    const result = await this.invokeProtocol<BealeMemoryNotificationFeed>('memory.notification_feed', {
      args: ['knowledge', 'notification-feed'],
      input: {
        workspaceId: workspace.workspaceId,
        workspaceRoot: workspace.workspacePath,
        researchProfileId: workspace.researchProfileId,
        ...(sessionId ? { sessionId } : {})
      },
      storage
    });
    return canonicalResult(workspace, workspace.memoryBackend === 'disabled'
      ? { ...result, nodes: [] }
      : result);
  }

  public async workspaceSessions(
    workspaceIdentifier: string,
    limit = 200
  ): Promise<BealeAppServerCanonicalResult> {
    const workspace = this.requireWorkspace(workspaceIdentifier);
    const result = await this.invokeProtocol<unknown>('session.list_summaries', {
      args: [
        'session', 'list-summaries',
        '--workspace-id', workspace.workspaceId,
        '--limit', String(boundedInteger(limit, 1, 500))
      ],
      storage: this.registry.storageForProfile(workspace.researchProfileId)
    });
    return canonicalResult(workspace, result);
  }

  public async workspaceChannels(
    workspaceIdentifier: string,
    limit = 200,
    archived = false
  ): Promise<BealeAppServerCanonicalResult> {
    const workspace = this.requireWorkspace(workspaceIdentifier);
    const result = await this.invokeProtocol<unknown>('channel.list', {
      args: ['channel', 'list', '--workspace-id', workspace.workspaceId, '--limit', String(boundedInteger(limit, 1, 500))],
      input: { workspaceId: workspace.workspaceId, archived },
      storage: this.registry.storageForProfile(workspace.researchProfileId)
    });
    return canonicalResult(workspace, result);
  }

  public async workspaceChannel(
    workspaceIdentifier: string,
    channel: string,
    messageLimit = 500
  ): Promise<BealeAppServerCanonicalResult> {
    return this.channelOperation(workspaceIdentifier, 'channel.get', channel, {
      channel,
      messageLimit
    }, ['--message-limit', String(boundedInteger(messageLimit, 1, 2_000))]);
  }

  public async createWorkspaceChannel(
    workspaceIdentifier: string,
    input: Record<string, unknown>
  ): Promise<BealeAppServerCanonicalResult> {
    return this.channelOperation(workspaceIdentifier, 'channel.create', null, input);
  }

  public async postWorkspaceChannelMessage(
    workspaceIdentifier: string,
    channel: string,
    input: Record<string, unknown>
  ): Promise<BealeAppServerCanonicalResult> {
    return this.channelOperation(workspaceIdentifier, 'channel.post', channel, { ...input, channel });
  }

  public async deleteWorkspaceChannel(
    workspaceIdentifier: string,
    channel: string
  ): Promise<BealeAppServerCanonicalResult> {
    return this.channelOperation(workspaceIdentifier, 'channel.delete', channel, { channel });
  }

  public async archiveWorkspaceChannel(
    workspaceIdentifier: string,
    channel: string
  ): Promise<BealeAppServerCanonicalResult> {
    return this.channelOperation(workspaceIdentifier, 'channel.archive', channel, { channel });
  }

  public async restoreWorkspaceChannel(
    workspaceIdentifier: string,
    channel: string
  ): Promise<BealeAppServerCanonicalResult> {
    return this.channelOperation(workspaceIdentifier, 'channel.restore', channel, { channel });
  }

  public async sessionUpdate(
    workspaceIdentifier: string,
    sessionId: string,
    options: { afterEventId?: string; tail?: boolean; limit?: number; maxBytes?: number }
  ): Promise<BealeAppServerCanonicalResult> {
    const workspace = this.requireWorkspace(workspaceIdentifier);
    const storage = this.registry.storageForProfile(workspace.researchProfileId);
    const result = await this.invokeProtocol<AppServerSessionUpdateProjection>('session.get_update', {
      args: [
        'session', 'get-update', '--session-id', sessionId,
        ...(options.afterEventId ? ['--after-event-id', options.afterEventId] : []),
        ...(options.tail ? ['--tail'] : []),
        ...(options.limit ? ['--limit', String(boundedInteger(options.limit, 1, 2_000))] : []),
        ...(options.maxBytes ? ['--max-bytes', String(boundedInteger(options.maxBytes, 1, 4_000_000))] : [])
      ],
      storage
    });
    this.requireSessionWorkspaceProjection(sessionId, workspace, result.session);
    return canonicalResult(workspace, result);
  }

  public async sessionEvents(
    workspaceIdentifier: string,
    sessionId: string,
    options: { stream?: string; afterEventId?: string; tail?: boolean; limit?: number; maxBytes?: number }
  ): Promise<BealeAppServerCanonicalResult> {
    const stream = options.stream === 'transcript' || options.stream === 'trace' || options.stream === 'commentary'
      ? options.stream
      : 'all';
    return this.sessionQuery(workspaceIdentifier, sessionId, 'session.events', [
      'session', 'events', '--session-id', sessionId,
      '--stream', stream,
      ...(options.afterEventId ? ['--after-event-id', options.afterEventId] : []),
      ...(options.tail ? ['--tail'] : []),
      ...(options.limit ? ['--limit', String(boundedInteger(options.limit, 1, 2_000))] : []),
      ...(options.maxBytes ? ['--max-bytes', String(boundedInteger(options.maxBytes, 1, 4_000_000))] : [])
    ]);
  }

  public async sessionEventDetails(
    workspaceIdentifier: string,
    sessionId: string,
    eventIds: readonly string[]
  ): Promise<BealeAppServerCanonicalResult> {
    if (eventIds.length === 0 || eventIds.length > 200) {
      throw new Error('Between 1 and 200 event ids are required.');
    }
    return this.sessionQuery(workspaceIdentifier, sessionId, 'session.event_details', [
      'session', 'event-details', '--session-id', sessionId,
      ...eventIds.flatMap((eventId) => ['--event-id', eventId])
    ]);
  }

  public async sessionCollaboration(
    workspaceIdentifier: string,
    sessionId: string,
    messageLimit = 200
  ): Promise<BealeAppServerCanonicalResult> {
    return this.sessionQuery(workspaceIdentifier, sessionId, 'session.collaboration', [
      'session', 'collaboration', '--session-id', sessionId,
      '--message-limit', String(boundedInteger(messageLimit, 1, 1_000))
    ]);
  }

  public async sessionCaptures(
    workspaceIdentifier: string,
    sessionId: string
  ): Promise<BealeAppServerCanonicalResult> {
    return this.sessionQuery(workspaceIdentifier, sessionId, 'session.captures', [
      'session', 'captures', '--session-id', sessionId
    ]);
  }

  private async sessionQuery(
    workspaceIdentifier: string,
    sessionId: string,
    operation: AppServerProtocolOperation,
    args: readonly string[]
  ): Promise<BealeAppServerCanonicalResult> {
    const workspace = this.requireWorkspace(workspaceIdentifier);
    const storage = this.registry.storageForProfile(workspace.researchProfileId);
    await this.requireSessionWorkspace(sessionId, workspace, storage);
    const result = await this.invokeProtocol<unknown>(operation, {
      args,
      storage
    });
    return canonicalResult(workspace, result);
  }

  private async channelOperation(
    workspaceIdentifier: string,
    operation: AppServerProtocolOperation,
    channel: string | null,
    input: Record<string, unknown>,
    extraArgs: readonly string[] = []
  ): Promise<BealeAppServerCanonicalResult> {
    const workspace = this.requireWorkspace(workspaceIdentifier);
    const result = await this.invokeProtocol<unknown>(operation, {
      args: [
        'channel', operation.slice('channel.'.length), '--workspace-id', workspace.workspaceId,
        ...(channel ? ['--channel', channel] : []),
        ...extraArgs
      ],
      input: { ...input, workspaceId: workspace.workspaceId, ...(channel ? { channel } : {}) },
      storage: this.registry.storageForProfile(workspace.researchProfileId)
    });
    return canonicalResult(workspace, result);
  }

  private async requireSessionWorkspace(
    sessionId: string,
    workspace: AppServerHostWorkspace,
    storage: AppServerHostStorage
  ): Promise<AppServerSessionSummaryProjection> {
    const update = await this.invokeProtocol<AppServerSessionUpdateProjection>('session.get_update', {
      args: [
        'session', 'get-update', '--session-id', sessionId,
        '--tail', '--limit', '1', '--max-bytes', '1024'
      ],
      storage
    });
    return this.requireSessionWorkspaceProjection(sessionId, workspace, update.session);
  }

  private requireSessionWorkspaceProjection(
    sessionId: string,
    workspace: AppServerHostWorkspace,
    session: AppServerSessionSummaryProjection | undefined
  ): AppServerSessionSummaryProjection {
    const projectedSessionId = nonEmpty(session?.id);
    const sessionWorkspaceId = nonEmpty(session?.workspaceId);
    if (!session || !projectedSessionId || projectedSessionId !== sessionId) {
      throw new Error(`app-server session projection did not match requested session ${sessionId}.`);
    }
    if (!sessionWorkspaceId || sessionWorkspaceId !== workspace.workspaceId) {
      throw new Error(`app-server session ${sessionId} does not belong to workspace ${workspace.workspaceId}.`);
    }
    return session;
  }

  private requireWorkspace(identifier: string): AppServerHostWorkspace {
    const workspace = this.registry.resolveWorkspace(identifier);
    if (!workspace) throw new Error(`Beale workspace is not registered: ${identifier}`);
    return workspace;
  }

  private async resolvePluginRuntime(): Promise<ResolvedAppServerSessionLaunch['pluginRuntime'] | null> {
    this.pluginRuntimePromise ??= this.loadPluginRuntime();
    return this.pluginRuntimePromise;
  }

  private async loadPluginRuntime(): Promise<ResolvedAppServerSessionLaunch['pluginRuntime'] | null> {
    try {
      const runtime = await this.invokeProtocol<AppServerPluginRuntimeProjection>('plugin.runtime', {
        args: ['harness', 'plugin-runtime'],
        input: {
          registryDirectory: this.registry.registryDirectory,
          builtinPlugins: defaultBuiltinPlugins()
        }
      });
      return {
        skillDirectories: stringArray(runtime.skillDirs),
        selectedSkillIds: stringArray(runtime.selectedSkillIds),
        ...(nonEmpty(runtime.mcpConfigPath) ? { mcpConfigPath: nonEmpty(runtime.mcpConfigPath)! } : {}),
        allowedMcpServers: stringArray(runtime.allowedMcpServers)
      };
    } catch {
      return null;
    }
  }

  private async loadIntrospectionPluginRuntime(): Promise<ResolvedAppServerSessionLaunch['pluginRuntime'] | null> {
    const plugin = builtinPlugin('beale-introspection-builtin', 'beale-introspection', false);
    if (!existsSync(plugin.path)) {
      throw new Error('Beale introspection plugin is unavailable for Quick Chat.');
    }
    const runtime = await this.invokeProtocol<AppServerPluginRuntimeProjection>('plugin.runtime', {
      args: ['harness', 'plugin-runtime'],
      input: {
        registryDirectory: join(this.registry.registryDirectory, 'quick-chat-plugin-runtime'),
        builtinPlugins: [plugin]
      }
    });
    const mcpConfigPath = nonEmpty(runtime.mcpConfigPath);
    if (!mcpConfigPath || !stringArray(runtime.allowedMcpServers).includes('beale-introspection.beale')) {
      throw new Error('Beale introspection plugin did not provide the Quick Chat tool runtime.');
    }
    return {
      skillDirectories: stringArray(runtime.skillDirs),
      selectedSkillIds: stringArray(runtime.selectedSkillIds),
      mcpConfigPath,
      allowedMcpServers: stringArray(runtime.allowedMcpServers)
    };
  }

  private async resolveProviderSemantics(): Promise<{
    defaultSmallModels: Record<string, string>;
    sessionTitleEffort: string;
    shellReviewEffort: string;
  }> {
    this.providerSemanticsPromise ??= this.loadProviderSemantics();
    return this.providerSemanticsPromise;
  }

  private async loadProviderSemantics(): Promise<{
    defaultSmallModels: Record<string, string>;
    sessionTitleEffort: string;
    shellReviewEffort: string;
  }> {
    const descriptor = await this.invokeProtocol<AppServerProviderSemanticsProjection>('provider.describe', {
      args: ['harness', 'provider-describe'],
      input: {}
    });
    return {
      defaultSmallModels: stringRecord(descriptor.defaultSmallModels),
      sessionTitleEffort: nonEmpty(descriptor.sessionTitleEffort) ?? 'medium',
      shellReviewEffort: nonEmpty(descriptor.shellReviewEffort) ?? 'medium'
    };
  }

  private async ensureCanonicalSession(input: {
    sessionId: string;
    attemptId: string;
    workspace: AppServerHostWorkspace;
    storage: AppServerHostStorage;
    prompt: string;
    providerId: string;
    model?: string;
    reasoningEffort?: string;
    workflowId?: string;
    profileId: string;
    profileHash?: string;
    continuation: boolean;
    restartLaunch: StoredRestartLaunchDescriptor;
    parentAttemptId?: string;
  }): Promise<void> {
    let existing: AppServerSessionSummaryProjection | null = null;
    try {
      existing = await this.invokeProtocol<AppServerSessionSummaryProjection>('session.get', {
        args: ['session', 'get', '--session-id', input.sessionId],
        storage: input.storage
      });
    } catch (error) {
      if (!/Session not found/iu.test(errorMessage(error))) throw error;
    }
    if (!existing) {
      await this.invokeProtocol('session.create', {
        args: ['session', 'create'],
        storage: input.storage,
        input: {
          id: input.sessionId,
          workspaceId: input.workspace.workspaceId,
          attemptId: input.attemptId,
          title: generatedTitle(input.prompt),
          prompt: input.prompt,
          provider: input.providerId,
          model: input.model ?? 'default',
          reasoningEffort: input.reasoningEffort ?? 'medium',
          workflowId: input.workflowId ?? null,
          profile: {
            id: input.profileId,
            ...(input.profileHash ? { hash: input.profileHash } : {})
          },
          metadata: {
            source: 'beale-app-server',
            appServerRestartLaunch: input.restartLaunch
          }
        }
      });
      return;
    }
    const existingWorkspaceId = nonEmpty(existing.workspaceId);
    if (!existingWorkspaceId || existingWorkspaceId !== input.workspace.workspaceId) {
      throw new Error(
        `app-server session ${input.sessionId} does not belong to workspace ${input.workspace.workspaceId}.`
      );
    }
    if (!attemptIds(existing.attempts).has(input.attemptId)) {
      await this.invokeProtocol('session.begin_attempt', {
        args: ['session', 'begin-attempt', '--session-id', input.sessionId],
        storage: input.storage,
        input: {
          attemptId: input.attemptId,
          ...(input.parentAttemptId ? { parentAttemptId: input.parentAttemptId } : {}),
          summary: input.continuation
            ? 'Continuing the current app-server research session.'
            : 'Starting the app-server research session.'
        }
      });
    }
    await this.invokeProtocol('session.transition', {
      args: ['session', 'transition', '--session-id', input.sessionId],
      storage: input.storage,
      input: {
        status: 'active',
        summary: input.continuation
          ? 'Continuing the current app-server research session.'
          : 'Starting the app-server research session.',
        attemptId: input.attemptId,
        metadata: { appServerRestartLaunch: input.restartLaunch },
        configuration: {
          prompt: input.prompt,
          provider: input.providerId,
          model: input.model ?? 'default',
          reasoningEffort: input.reasoningEffort ?? 'medium',
          workflowId: input.workflowId ?? null
        }
      }
    });
  }
}

function canonicalResult<T>(
  workspace: AppServerHostWorkspace,
  result: T
): BealeAppServerCanonicalResult<T> {
  const {
    workspacePath: _workspacePath,
    workspaceDirectories: _workspaceDirectories,
    memoryBackend: _memoryBackend,
    ...summary
  } = workspace;
  return { controlVersion: BEALE_APP_SERVER_CONTROL_VERSION, workspace: summary, result };
}

function withoutWorkspaceMemory(value: unknown): unknown {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return value;
  const summary = value as Record<string, unknown>;
  const campaign = summary.campaign && typeof summary.campaign === 'object' && !Array.isArray(summary.campaign)
    ? summary.campaign as Record<string, unknown>
    : {};
  return {
    ...summary,
    nodeCount: 0,
    edgeCount: 0,
    evidenceRefCount: 0,
    latestNodeUpdatedAt: null,
    nodeTypeCounts: {},
    nodeStatusCounts: {},
    nodeProvenanceCounts: {},
    nodes: [],
    edges: [],
    leads: [],
    findings: [],
    campaign: {
      ...campaign,
      nodes: [],
      edges: [],
      coverageGaps: [],
      contradictions: [],
      nextActions: [],
      tracks: [],
      activeTrackId: null,
      replayMetrics: undefined,
      momentum: { state: 'empty', reason: 'Workspace memory is disabled.', supportingNodeIds: [] },
      counts: { leads: 0, findings: 0, verifiedFindings: 0, disclosedFindings: 0, coverageGaps: 0, contradictions: 0 }
    },
    dreaming: {
      available: false,
      scope: 'workspace',
      hiddenNodeCount: 0,
      restorableChangeCount: 0,
      lastRun: null,
      changes: []
    }
  };
}

function pathFreeWorkspaceMemoryCatalog(
  workspaceId: string,
  value: unknown
): BealeWorkspaceMemoryCatalog {
  const summary = value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
  const nodes = Array.isArray(summary.nodes)
    ? summary.nodes.flatMap(pathFreeWorkspaceMemoryNode)
    : [];
  const leads = Array.isArray(summary.leads) ? summary.leads.flatMap(pathFreeWorkspaceClaim) : [];
  const findings = Array.isArray(summary.findings) ? summary.findings.flatMap(pathFreeWorkspaceClaim) : [];
  return {
    schemaVersion: 4,
    workspaceId,
    status: nonEmpty(summary.status) ?? (nodes.length > 0 ? 'ready' : 'empty'),
    nodeCount: typeof summary.nodeCount === 'number' && Number.isFinite(summary.nodeCount)
      ? Math.max(0, Math.trunc(summary.nodeCount))
      : nodes.length,
    nodeTypeCounts: numericRecord(summary.nodeTypeCounts),
    nodes,
    leads,
    findings
  };
}

function pathFreeWorkspaceClaim(value: unknown): BealeWorkspaceMemoryCatalog['findings'] {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return [];
  const claim = value as Record<string, unknown>;
  const id = nonEmpty(claim.id);
  const projection = claim.projection === 'lead' || claim.projection === 'finding' ? claim.projection : null;
  const classification = nonEmpty(claim.classification);
  const title = nonEmpty(claim.title);
  if (!id || !projection || !classification || !title) return [];
  const evidence = Array.isArray(claim.evidence) ? claim.evidence : [];
  return [{
    id,
    sessionIds: stringArray([claim.originSessionId, ...evidence.flatMap((entry) => (
      entry && typeof entry === 'object' && !Array.isArray(entry) ? [(entry as Record<string, unknown>).sessionId] : []
    ))]),
    projection,
    maturity: nonEmpty(claim.maturity) ?? 'proposed',
    freshness: nonEmpty(claim.freshness) ?? 'current',
    workflow: nonEmpty(claim.workflow) ?? 'open',
    rating: researchClaimRating(claim.rating),
    classification,
    componentClaimIds: stringArray(claim.componentClaimIds),
    duplicateClaims: Array.isArray(claim.duplicateClaims)
      ? claim.duplicateClaims.flatMap(pathFreeWorkspaceClaimDuplicate)
      : [],
    title,
    summary: typeof claim.summary === 'string' ? claim.summary : '',
    impact: typeof claim.impact === 'string' ? claim.impact : '',
    securityTracking: pathFreeClaimSecurityTracking(claim.securityTracking),
    confidence: typeof claim.confidence === 'number' && Number.isFinite(claim.confidence)
      ? Math.max(0, Math.min(1, claim.confidence)) : 0,
    evidenceCount: evidence.length,
    createdAt: nonEmpty(claim.createdAt) ?? new Date(0).toISOString(),
    updatedAt: nonEmpty(claim.updatedAt) ?? new Date(0).toISOString(),
    revision: typeof claim.revision === 'number' && Number.isInteger(claim.revision) && claim.revision > 0
      ? claim.revision : 1
  }];
}

function pathFreeWorkspaceClaimDuplicate(
  value: unknown
): BealeWorkspaceMemoryCatalog['findings'][number]['duplicateClaims'] {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return [];
  const claim = value as Record<string, unknown>;
  const id = nonEmpty(claim.id);
  const projection = claim.projection === 'lead' || claim.projection === 'finding' ? claim.projection : null;
  const classification = nonEmpty(claim.classification);
  const title = nonEmpty(claim.title);
  const status = nonEmpty(claim.status);
  const markedAt = nonEmpty(claim.markedAt);
  if (!id || !projection || !classification || !title || !status || !markedAt) return [];
  return [{
    id,
    projection,
    maturity: nonEmpty(claim.maturity) ?? 'proposed',
    rating: researchClaimRating(claim.rating),
    classification,
    title,
    status,
    revision: typeof claim.revision === 'number' && Number.isInteger(claim.revision) && claim.revision > 0
      ? claim.revision : 1,
    markedAt
  }];
}

function researchClaimRating(value: unknown): BealeWorkspaceMemoryCatalog['findings'][number]['rating'] {
  return value === 'low' || value === 'medium' || value === 'high' || value === 'critical'
    ? value
    : 'informational';
}

function pathFreeClaimSecurityTracking(value: unknown): BealeWorkspaceClaimSecurityTracking | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const tracking = value as Record<string, unknown>;
  const reachability = tracking.reachability && typeof tracking.reachability === 'object' && !Array.isArray(tracking.reachability)
    ? tracking.reachability as Record<string, unknown>
    : {};
  const state = ['not_assessed', 'unreachable', 'conditional', 'reachable'].includes(String(reachability.state))
    ? reachability.state as BealeWorkspaceClaimSecurityTracking['reachability']['state']
    : 'not_assessed';
  const riskTreatment = ['unreviewed', 'remediate', 'mitigated', 'accepted', 'transferred'].includes(String(tracking.riskTreatment))
    ? tracking.riskTreatment as BealeWorkspaceClaimSecurityTracking['riskTreatment']
    : 'unreviewed';
  const cvssAssessments = Array.isArray(tracking.cvssAssessments) ? tracking.cvssAssessments.flatMap((entry) => {
    if (!entry || typeof entry !== 'object' || Array.isArray(entry)) return [];
    const assessment = entry as Record<string, unknown>;
    const version: '4.0' | '3.1' | null = assessment.version === '4.0' || assessment.version === '3.1'
      ? assessment.version
      : null;
    const nomenclature = ['CVSS-B', 'CVSS-BT', 'CVSS-BE', 'CVSS-BTE', 'CVSS:3.1'].includes(String(assessment.nomenclature))
      ? assessment.nomenclature as BealeWorkspaceClaimSecurityTracking['cvssAssessments'][number]['nomenclature']
      : null;
    const vector = nonEmpty(assessment.vector);
    const assessedAt = nonEmpty(assessment.assessedAt);
    if (!version || !nomenclature || !vector || !assessedAt || typeof assessment.score !== 'number' || !Number.isFinite(assessment.score)) return [];
    return [{ version, vector, score: Math.max(0, Math.min(10, assessment.score)), nomenclature, assessedAt }];
  }) : [];
  const affectedVersions = Array.isArray(tracking.affectedVersions) ? tracking.affectedVersions.flatMap((entry) => {
    if (!entry || typeof entry !== 'object' || Array.isArray(entry)) return [];
    const version = entry as Record<string, unknown>;
    const range = nonEmpty(version.range);
    return range ? [{ assetId: nonEmpty(version.assetId), range, fixedVersion: nonEmpty(version.fixedVersion) }] : [];
  }) : [];
  const externalReferences = Array.isArray(tracking.externalReferences) ? tracking.externalReferences.flatMap((entry) => {
    if (!entry || typeof entry !== 'object' || Array.isArray(entry)) return [];
    const reference = entry as Record<string, unknown>;
    const kind = nonEmpty(reference.kind);
    const identifier = nonEmpty(reference.identifier);
    return kind && identifier ? [{ kind, identifier, url: nonEmpty(reference.url) }] : [];
  }) : [];
  return {
    reachability: {
      state,
      conditions: typeof reachability.conditions === 'string' ? reachability.conditions : '',
      assessedAt: nonEmpty(reachability.assessedAt)
    },
    riskTreatment,
    cvssAssessments,
    affectedAssetIds: stringArray(tracking.affectedAssetIds),
    affectedVersions,
    externalReferences
  };
}

function pathFreeWorkspaceMemoryNode(value: unknown): BealeWorkspaceMemoryNode[] {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return [];
  const node = value as Record<string, unknown>;
  const id = nonEmpty(node.id);
  const type = nonEmpty(node.type);
  const title = nonEmpty(node.title);
  if (!id || !type || !title) return [];
  return [{
    id,
    sessionIds: stringArray(node.sessionIds),
    type,
    title,
    summary: typeof node.summary === 'string' ? node.summary : '',
    status: nonEmpty(node.status) ?? 'unknown',
    confidence: typeof node.confidence === 'number' && Number.isFinite(node.confidence)
      ? Math.max(0, Math.min(1, node.confidence))
      : 0,
    tags: stringArray(node.tags),
    createdAt: nonEmpty(node.createdAt) ?? new Date(0).toISOString(),
    updatedAt: nonEmpty(node.updatedAt) ?? new Date(0).toISOString(),
    revision: typeof node.revision === 'number' && Number.isInteger(node.revision) && node.revision > 0
      ? node.revision
      : 1
  }];
}

function numericRecord(value: unknown): Record<string, number> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
  return Object.fromEntries(Object.entries(value).flatMap(([key, entry]) => (
    key.trim() && typeof entry === 'number' && Number.isFinite(entry) && entry >= 0
      ? [[key, Math.trunc(entry)] as const]
      : []
  )));
}

async function writePrivateJson(path: string, value: unknown): Promise<void> {
  await writeFile(path, `${JSON.stringify(value, null, 2)}\n`, { encoding: 'utf8', mode: 0o600 });
}

function defaultBuiltinPlugins(): Array<{ id: string; path: string; installedAt: string; enabledByDefault?: boolean }> {
  return [
    builtinPlugin('beale-introspection-builtin', 'beale-introspection', false),
    builtinPlugin('beale-terminator-builtin', 'beale-terminator', true)
  ].flatMap((plugin) => existsSync(plugin.path) ? [plugin] : []);
}

function builtinPlugin(
  id: string,
  directory: string,
  disabledByDefault: boolean
): { id: string; path: string; installedAt: string; enabledByDefault?: boolean } {
  return {
    id,
    path: builtinPluginPath(directory),
    installedAt: '2026-08-21T00:00:00.000Z',
    ...(disabledByDefault ? { enabledByDefault: false } : {})
  };
}

function builtinPluginPath(directory: string): string {
  const resourcesPath = (process as NodeJS.Process & { resourcesPath?: string }).resourcesPath;
  const candidates = [
    ...(resourcesPath
      ? [
          resolve(resourcesPath, 'agent-plugins', directory),
          resolve(resourcesPath, 'resources', 'agent-plugins', directory)
        ]
      : []),
    fileURLToPath(new URL(`../resources/agent-plugins/${directory}`, import.meta.url)),
    resolve(process.cwd(), 'resources', 'agent-plugins', directory)
  ];
  return candidates.find((candidate) => existsSync(candidate)) ?? candidates.at(-1)!;
}

function generatedTitle(prompt: string): string {
  const normalized = prompt.trim().replace(/\s+/gu, ' ');
  return normalized.slice(0, 120) || 'Beale research session';
}

function validatedIntrospectionEndpoint(
  endpoint: AppServerSessionLaunchRequest['launch']['introspection']
): { url: string; token: string; runtimeMode: 'isolated' | 'standard' } | undefined {
  if (!endpoint) return undefined;
  const endpointUrl = new URL(endpoint.url);
  if (
    endpointUrl.protocol !== 'http:'
    || endpointUrl.hostname !== '127.0.0.1'
    || endpointUrl.username
    || endpointUrl.password
    || endpointUrl.search
    || endpointUrl.hash
    || !endpoint.token.trim()
  ) {
    throw new Error('Beale introspection requires an authenticated loopback endpoint.');
  }
  return {
    url: endpointUrl.toString().replace(/\/$/u, ''),
    token: endpoint.token,
    runtimeMode: endpoint.runtimeMode ?? 'isolated'
  };
}

function restartLaunchDescriptor(
  request: AppServerSessionLaunchRequest,
  resolved: {
    providerId: string;
    model?: string;
    reasoningEffort?: string;
    fastMode?: boolean;
    profileId: string;
  }
): StoredRestartLaunchDescriptor {
  return {
    schemaVersion: 1,
    eligible: request.launch.introspection === undefined
      || request.launch.introspection.runtimeMode === 'standard',
    launch: {
      workspaceId: request.launch.workspaceId,
      promptMarkdown: request.launch.promptMarkdown,
      ...(request.launch.goal ? { goal: request.launch.goal } : {}),
      provider: {
        id: resolved.providerId,
        ...(resolved.model ? { model: resolved.model } : {}),
        ...(resolved.reasoningEffort ? { reasoningEffort: resolved.reasoningEffort } : {}),
        ...(resolved.fastMode ? { fastMode: true } : {})
      },
      shellSafetyMode: request.launch.shellSafetyMode?.trim() || 'auto_review',
      ...(request.launch.workflowId ? { workflowId: request.launch.workflowId } : {}),
      researchProfileId: request.launch.researchProfileId?.trim() || resolved.profileId,
      ...(request.launch.researchProfileHash
        ? { researchProfileHash: request.launch.researchProfileHash }
        : {}),
      ...(request.launch.collaboration ? { collaboration: request.launch.collaboration } : {})
    }
  };
}

type AutomationIntervalUnit = 'minutely' | 'hourly' | 'daily' | 'weekly' | 'monthly';

interface AutomationInterval {
  type: AutomationIntervalUnit;
  interval: number;
}

function dueAutomation(
  session: AppServerSessionSummaryProjection,
  workspace: AppServerHostWorkspace,
  at: Date
): DueAppServerAutomation | null {
  const sessionId = nonEmpty(session.id);
  const workspaceId = nonEmpty(session.workspaceId);
  const promptMarkdown = nonEmpty(session.prompt);
  const status = nonEmpty(session.status);
  if (!sessionId || workspaceId !== workspace.workspaceId || !promptMarkdown) return null;
  if (status !== 'completed' && status !== 'failed' && status !== 'stopped') return null;
  const metadata = isRecord(session.metadata) ? session.metadata : {};
  const storedRun = isRecord(metadata.bealeRun) ? metadata.bealeRun : {};
  const budget = isRecord(storedRun.budget) ? storedRun.budget : {};
  const schedule = automationInterval(budget.repeatSchedule);
  if (!schedule) return null;
  const latestStartedAt = latestAutomationAttemptStartedAt(session.attempts) ?? nonEmpty(session.createdAt);
  if (!latestStartedAt) return null;
  const dueAt = nextAutomationRunAt(schedule, latestStartedAt);
  if (!dueAt || dueAt.getTime() > at.getTime()) return null;

  const profile = isRecord(session.profile) ? session.profile : {};
  const providerId = nonEmpty(session.provider);
  const model = nonEmpty(session.model);
  const reasoningEffort = nonEmpty(session.reasoningEffort);
  const researchProfileId = nonEmpty(profile.id) ?? workspace.researchProfileId;
  const researchProfileHash = nonEmpty(profile.hash);
  const workflowId = nonEmpty(session.workflowId);
  const collaboration = isRecord(budget.collaboration) ? budget.collaboration : null;
  const fastMode = budget.fastMode === true && providerId === 'openai-codex';
  const shellSafetyMode = nonEmpty(metadata.shellSafetyMode)
    ?? nonEmpty(storedRun.shellSafetyMode)
    ?? 'auto_review';
  const goalEnabled = budget.goalEnabled === true;
  const goalObjective = nonEmpty(budget.goalObjective) ?? promptMarkdown;

  return {
    dueAt: dueAt.toISOString(),
    request: {
      launchVersion: APP_SERVER_SESSION_LAUNCH_VERSION,
      sessionId,
      launch: {
        workspaceId,
        promptMarkdown,
        ...(goalEnabled ? { goal: { objective: goalObjective } } : {}),
        ...(providerId || model || reasoningEffort || fastMode ? {
          provider: {
            ...(providerId ? { id: providerId } : {}),
            ...(model ? { model } : {}),
            ...(reasoningEffort ? { reasoningEffort } : {}),
            ...(fastMode ? { fastMode: true } : {})
          }
        } : {}),
        shellSafetyMode,
        ...(workflowId ? { workflowId } : {}),
        ...(researchProfileId ? { researchProfileId } : {}),
        ...(researchProfileHash ? { researchProfileHash } : {}),
        ...(collaboration ? { collaboration } : {}),
        generateTitle: false
      }
    }
  };
}

function automationInterval(value: unknown): AutomationInterval | null {
  if (!isRecord(value)) return null;
  if (value.type !== 'minutely' && value.type !== 'hourly' && value.type !== 'daily'
    && value.type !== 'weekly' && value.type !== 'monthly') return null;
  const interval = typeof value.interval === 'number' && Number.isFinite(value.interval)
    ? Math.max(1, Math.min(99, Math.floor(value.interval)))
    : 1;
  return { type: value.type, interval };
}

function latestAutomationAttemptStartedAt(value: unknown): string | null {
  if (!Array.isArray(value)) return null;
  return value.reduce<string | null>((latest, attempt) => {
    if (!isRecord(attempt)) return latest;
    const startedAt = nonEmpty(attempt.startedAt);
    if (!startedAt || !Number.isFinite(Date.parse(startedAt))) return latest;
    return !latest || startedAt > latest ? startedAt : latest;
  }, null);
}

export function nextAutomationRunAt(schedule: AutomationInterval, startedAt: string): Date | null {
  const started = new Date(startedAt);
  if (!Number.isFinite(started.getTime())) return null;
  const next = new Date(started);
  if (schedule.type === 'monthly') {
    next.setUTCMonth(next.getUTCMonth() + schedule.interval);
    return next;
  }
  const unitMs = schedule.type === 'minutely'
    ? 60_000
    : schedule.type === 'hourly'
      ? 3_600_000
      : schedule.type === 'daily'
        ? 86_400_000
        : 604_800_000;
  return new Date(started.getTime() + (schedule.interval * unitMs));
}

function decodeRestartLaunchDescriptor(value: unknown): StoredRestartLaunchDescriptor | null {
  if (!isRecord(value)) return null;
  const stored = value.appServerRestartLaunch;
  if (!isRecord(stored) || stored.schemaVersion !== 1 || typeof stored.eligible !== 'boolean') return null;
  try {
    const decoded = decodeAppServerSessionLaunchRequest({
      launchVersion: APP_SERVER_SESSION_LAUNCH_VERSION,
      sessionId: 'startup-recovery-validation',
      launch: stored.launch
    });
    return { schemaVersion: 1, eligible: stored.eligible, launch: decoded.launch };
  } catch {
    return null;
  }
}

function interruptedAttempt(value: unknown): { id: string; parentAttemptId: string | null } | null {
  if (!Array.isArray(value)) return null;
  for (let index = value.length - 1; index >= 0; index -= 1) {
    const attempt = value[index];
    if (!isRecord(attempt) || !isRecord(attempt.metadata)) continue;
    if (attempt.metadata.interruptedByRecovery !== true) continue;
    const id = nonEmpty(attempt.id);
    if (!id) continue;
    return { id, parentAttemptId: nonEmpty(attempt.parentAttemptId) };
  }
  return null;
}

function attemptIds(value: unknown): Set<string> {
  if (!Array.isArray(value)) return new Set();
  return new Set(value.flatMap((attempt) => {
    if (!attempt || typeof attempt !== 'object' || Array.isArray(attempt)) return [];
    const id = nonEmpty((attempt as Record<string, unknown>).id);
    return id ? [id] : [];
  }));
}

function stringArray(value: unknown): string[] {
  return Array.isArray(value)
    ? value.flatMap((entry) => typeof entry === 'string' && entry.trim() ? [entry.trim()] : [])
    : [];
}

function stringRecord(value: unknown): Record<string, string> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
  return Object.fromEntries(Object.entries(value).flatMap(([key, entry]) => (
    key.trim() && typeof entry === 'string' && entry.trim()
      ? [[key, entry.trim()] as const]
      : []
  )));
}

function nonEmpty(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value.trim() : null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function workspaceIdentifierFromInput(value: unknown): string | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  return nonEmpty((value as Record<string, unknown>).workspaceId);
}

function boundedInteger(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, Math.floor(value)));
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
