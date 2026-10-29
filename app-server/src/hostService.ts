import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { mkdir, readFile, stat, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { createHash, randomUUID } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import {
  BEALE_APP_SERVER_CONTROL_VERSION,
  MANAGED_TOOL_PLUGIN_IDS,
  APP_SERVER_SESSION_LAUNCH_VERSION,
  type BealeAppServerCanonicalResult,
  type BealeAppServerWorkspaceDetails,
  type BealeWorkspaceSurfaceMap,
  type BealeWorkspaceResearchTrack,
  type BealeWorkspaceResearchTrackRequest,
  type BealeAppServerQuickChatList,
  type BealeAppServerQuickChatStartInput,
  type BealeAppServerProviderCatalog,
  type BealeProviderLimitId,
  type BealeProviderUsageLimits,
  type BealeMemoryNotificationFeed,
  type BealeWorkspaceMemoryCatalog,
  type BealeWorkspaceClaimSecurityTracking,
  type BealeWorkspaceMemoryNode,
  type BealeAppServerWorkspaceList,
  decodeAppServerSessionLaunchRequest,
  decodeFleetRemoteResearchInput,
  decodeFleetRemotePromptExpansionInput,
  decodeWorkspaceProjectRequest,
  decodeBealeWorkspaceSurfaceMapRequest,
  decodeBealeWorkspaceResearchTrackRequest,
  type AppServerProtocolOperation,
  type AppServerSessionLaunchRequest,
  decodeFleetSshTestInput
} from '@beale/app-server-runtime/protocol';
import { getProviderModelCatalog, refreshOpenRouterModels, readWorkspaceProject, readWorkspaceResearchCacheState, resolveStoredResearchWorkspaceBinding, workspaceResearchIndexNeedsRebuild } from '@beale/app-server-runtime/runtime-services';
import { applyFeaturePromptOverrides, bundledPluginsRequiredByResearchKit, decodeResearchPluginCatalog, researchKitsRequiringBundledPlugin, validateResearchSystemPromptTemplate, verifyBrokerModelAccess, ResearchResourceCatalog, ResearchTrackStore, type ResearchTrackEpochFocus, type ResearchTrackSurfaceState, type ResearchTrackSurfaceProjection } from '@beale/research-agent';
import { runWorkspaceCheckpoint, workspaceOperationKey } from './workspaceCheckpoints.js';
import { projectWorkspaceDetails } from './workspaceDetails.js';
import { readWorkspaceSurfaceMap } from './surfaceMap.js';
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
import { reviewResearchTrackContext, type ResearchTrackContextDecision } from './researchTrackContextReview.js';
import { AppServerWorkerDatabaseCoordinator } from './workerDatabaseBroker.js';
import { FleetIncompleteResponseError, FleetService } from './fleet.js';
import { FleetRemoteController } from './fleetRemote.js';
import { FleetModelBrokerQueue } from './fleetModelBrokerQueue.js';
import type { ModelBrokerRequest } from '@beale/research-agent';
import { FleetWorkspaceStore } from './fleetWorkspace.js';
import { ProviderUsageLimitsService } from './providerUsageLimits.js';
import { defaultDiscoveryPath, readDiscoveryRecord } from './discovery.js';
import { createAppServerPairingQrCode } from './pairing.js';
import { readBoreHostReceipt, saveBoreCheckpoint, saveBoreHostReceipt } from './boreCheckpoint.js';

type ProtocolInvoker = <T>(
  operation: AppServerProtocolOperation,
  options: InvokeAppServerProtocolOptions
) => Promise<T>;

function operationRequiresResearchIndex(operation: AppServerProtocolOperation): boolean {
  return operation.startsWith('research.tools.')
    || operation.startsWith('resource.')
    || operation.startsWith('memory.')
    || operation.startsWith('history.')
    || operation.startsWith('claim.')
    || operation.startsWith('runbook.')
    || operation.startsWith('report.')
    || operation.startsWith('artifact.')
    || operation === 'suggestion.generate'
    || operation === 'suggestion.select'
    || operation === 'prompt.expand';
}

interface StoredRestartLaunchDescriptor {
  schemaVersion: 1;
  eligible: boolean;
  launch: AppServerSessionLaunchRequest['launch'];
}

const ORPHANED_TRACK_EPOCH_DIAGNOSTIC = 'The research worker ended without a terminal session record.';
const ORPHANED_TRACK_PAUSE_MESSAGE = `${ORPHANED_TRACK_EPOCH_DIAGNOSTIC} Review the session before resuming.`;

export type AppServerSessionStopCause = 'user' | 'track_control' | 'track_rotation' | 'track_state';

interface AppServerSessionSummaryProjection {
  id?: unknown;
  workspaceId?: unknown;
  title?: unknown;
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
  lastMessageAt?: unknown;
}

interface AppServerSessionUpdateProjection {
  session?: AppServerSessionSummaryProjection;
  events?: unknown;
}

interface AppServerPluginRuntimeProjection {
  managedPluginIds?: unknown;
  pluginCatalogPath?: unknown;
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
  databaseCoordinator?: AppServerWorkerDatabaseCoordinator;
}

export class AppServerHostService {
  private readonly providerUsage = new ProviderUsageLimitsService();
  private readonly workspaceWriters = new Map<string, string>();
  private readonly workspaceExclusiveOperations = new Set<string>();
  private readonly registry: AppServerHostRegistry;
  private readonly fleet: FleetService;
  private readonly fleetRemote: FleetRemoteController;
  private readonly fleetWorkspace: FleetWorkspaceStore;
  private readonly fleetBroker: FleetModelBrokerQueue;
  private readonly fleetSessionMachines = new Map<string, string[]>();
  private readonly invokeProtocol: ProtocolInvoker;
  private readonly databaseCoordinator: AppServerWorkerDatabaseCoordinator;
  private providerSemanticsPromise: Promise<{
    defaultSmallModels: Record<string, string>;
    sessionTitleEffort: string;
    shellReviewEffort: string;
  }> | null = null;

  public constructor(options: AppServerHostServiceOptions = {}) {
    this.registry = options.registry ?? new AppServerHostRegistry(options);
    this.fleet = new FleetService(join(this.registry.registryDirectory, 'fleet.json'));
    this.fleetWorkspace = new FleetWorkspaceStore(this.registry);
    this.fleetBroker = new FleetModelBrokerQueue(join(this.registry.registryDirectory, 'fleet-model-broker'));
    this.fleetRemote = new FleetRemoteController(this.fleet, this.fleetWorkspace,
      () => this.registry.providerSettings().authenticationPreferences,
      () => this.registry.providerSettings().riskAcknowledgements,
      () => this.registry.providerSettings().daybreakMode ?? 'blue');
    this.databaseCoordinator = options.databaseCoordinator ?? new AppServerWorkerDatabaseCoordinator();
    const invokeProtocol = options.invokeProtocol ?? invokeAppServerProtocol;
    this.invokeProtocol = (operation, invokeOptions) => invokeOptions.storage
        ? this.databaseCoordinator.runWhenAvailable(
            invokeOptions.storage.databasePath,
            () => invokeProtocol(operation, invokeOptions)
          )
        : invokeProtocol(operation, invokeOptions);
  }

  public brokerToken(sessionId: string): string | null { return this.fleetBroker.token(sessionId); }

  public browserHomePage(): string { return this.fleet.browserHomePage(); }

  public async releaseSessionTarget(sessionId: string): Promise<void> {
    const machineIds = this.fleetSessionMachines.get(sessionId);
    if (!machineIds) return;
    this.fleetSessionMachines.delete(sessionId);
    await Promise.all(machineIds.map((machineId) => this.fleet.releaseSessionTarget(machineId, sessionId)));
  }

  public brokerSubmit(sessionId: string, token: string, request: ModelBrokerRequest): void {
    this.fleetBroker.submit(sessionId, token, request);
  }

  public brokerResult(sessionId: string, token: string, requestId: string): unknown {
    return this.fleetBroker.result(sessionId, token, requestId);
  }

  public resumeFleetModelBrokers(): void { this.fleetRemote.resumePendingBrokers(); }
  public stopFleetModelBrokers(): void { this.fleetRemote.stopAllBrokers(); }

  public listWorkspaces(): BealeAppServerWorkspaceList {
    return {
      controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
      workspaces: this.registry.listWorkspaces()
    };
  }

  public async quickChats(): Promise<BealeAppServerQuickChatList> {
    const workspace = this.registry.quickChatWorkspace();
    if (!workspace) return { schemaVersion: 1, chats: [] };
    const catalog = await this.workspaceSessions(workspace.workspaceId, 500);
    const sessions = Array.isArray(catalog.result) ? catalog.result : [];
    return { schemaVersion: 1, chats: sessions.flatMap((value) => {
      if (!isRecord(value) || typeof value.id !== 'string') return [];
      return [{
        sessionId: value.id,
        workspaceId: workspace.workspaceId,
        title: nonEmpty(value.title) ?? 'Quick Chat',
        promptMarkdown: nonEmpty(value.prompt) ?? '',
        status: nonEmpty(value.status) ?? 'queued',
        updatedAt: nonEmpty(value.updatedAt) ?? new Date(0).toISOString()
      }];
    }) };
  }

  public async quickChatLaunchRequest(input: BealeAppServerQuickChatStartInput): Promise<AppServerSessionLaunchRequest> {
    const workspace = this.registry.ensureQuickChatWorkspace();
    const storage = this.registry.storageForProfile(workspace.researchProfileId);
    const state = await this.invokeProtocol<unknown>('workspace.state', {
      args: ['workspace', 'state'],
      input: {
        action: 'initialize', workspacePath: workspace.workspacePath, workspaceId: workspace.workspaceId,
        researchKitId: workspace.researchKitId, artifactRoot: join(workspace.workspacePath, '.beale', 'artifacts')
      }, storage
    });
    const activeScope = isRecord(state) && isRecord(state.activeScope) ? state.activeScope : null;
    if (activeScope?.workspaceName !== 'Quick Chats') {
      await this.invokeProtocol('workspace.state', {
        args: ['workspace', 'state'],
        input: {
          action: 'saveScope', workspacePath: workspace.workspacePath, workspaceId: workspace.workspaceId,
          researchKitId: workspace.researchKitId, artifactRoot: join(workspace.workspacePath, '.beale', 'artifacts'),
          args: [{ workspaceName: 'Quick Chats', scopeOwner: 'Beale',
            descriptionMarkdown: 'Internal workspace for Quick Chat sessions.', rulesMarkdown: '', expiresAt: null, assets: [] },
          { refreshInventory: false }]
        }, storage
      });
    }
    return {
      launchVersion: APP_SERVER_SESSION_LAUNCH_VERSION,
      launch: {
        workspaceId: workspace.workspaceId,
        machineId: 'local',
        mode: 'quick-chat',
        promptMarkdown: input.promptMarkdown.trim(),
        ...(input.provider ? { provider: input.provider } : {}),
        shellSafetyMode: 'auto_review',
        researchProfileId: 'security-research',
        generateTitle: true
      }
    };
  }

  public async providerCatalog(): Promise<BealeAppServerProviderCatalog> {
    const settings = this.registry.providerSettings();
    const connectedProviderIds = new Set([
      ...(settings.defaultProviderId ? [settings.defaultProviderId] : []),
      ...Object.keys(settings.modelDefaults),
      ...Object.keys(settings.authenticationPreferences),
      ...(settings.customProviders ?? []).map((provider) => provider.id)
    ]);
    if (connectedProviderIds.has('openrouter')) await refreshOpenRouterModels();
    return {
      controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
      defaultProviderId: settings.defaultProviderId,
      providers: getProviderModelCatalog(undefined, settings.customProviders ?? [])
        .filter((catalog) => connectedProviderIds.has(catalog.providerId))
        .map((catalog) => {
          const defaults = settings.modelDefaults[catalog.providerId];
          return {
            providerId: catalog.providerId,
            providerName: catalog.providerName,
            riskAcknowledged: settings.riskAcknowledgements.includes(catalog.providerId as (typeof settings.riskAcknowledgements)[number]),
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

  public providerUsageLimits(providerId: BealeProviderLimitId, refresh = false, serverId: string | null = null): Promise<BealeProviderUsageLimits> {
    return serverId ? this.fleet.remoteProviderUsageLimits(serverId, providerId, refresh) : this.providerUsage.read(providerId, refresh);
  }

  public async boreCheckpoint(workspaceId: string, session: unknown): Promise<unknown> {
    const workspace = this.requireWorkspace(workspaceId);
    return saveBoreCheckpoint(workspace.workspacePath, session);
  }

  public async boreHostReceipt(workspaceId: string, receipt: unknown): Promise<unknown> {
    const workspace = this.requireWorkspace(workspaceId);
    return saveBoreHostReceipt(workspace.workspacePath, receipt);
  }

  public async boreHostReceiptStatus(workspaceId: string, sessionId: string, jobId: string): Promise<unknown> {
    const workspace = this.requireWorkspace(workspaceId);
    return readBoreHostReceipt(workspace.workspacePath, sessionId, jobId);
  }

  public async executeOperation(request: {
    operation: AppServerProtocolOperation;
    args?: readonly string[];
    input?: unknown;
    profileId?: string;
    signal?: AbortSignal;
  }): Promise<unknown> {
    if (request.operation === 'provider.complete' && isRecord(request.input)
      && request.input.useHostDefaults === true) {
      const input = request.input;
      const settings = this.registry.providerSettings();
      const provider = settings.defaultProviderId;
      const defaults = provider ? settings.modelDefaults[provider] : undefined;
      if (!provider || !defaults?.leadModel) throw new Error('No Beale Lead provider and model are configured.');
      if (typeof input.systemPrompt !== 'string' || input.systemPrompt.length > 2_000
        || typeof input.prompt !== 'string' || input.prompt.length > 8_000) {
        throw new Error('Invalid hosted completion input.');
      }
      return this.invokeProtocol('provider.complete', {
        args: [],
        input: {
          provider, model: defaults.leadModel,
          systemPrompt: input.systemPrompt, prompt: input.prompt,
          maxTokens: 500,
          ...(defaults.reasoningEffort ? { effort: defaults.reasoningEffort } : {}),
          authenticationPreferences: settings.authenticationPreferences,
          ...(provider === 'openai-codex' ? { codexAuthFile: resolveAppServerCodexAuthFile() } : {})
        },
        ...(request.signal ? { signal: request.signal } : {})
      });
    }
    if (request.operation.startsWith('fleet.')) {
      const input = isRecord(request.input) ? request.input : {};
      if (request.operation === 'fleet.state') return input.workspaceId === undefined
        ? this.fleet.state() : this.fleet.stateForWorkspace(nonEmpty(input.workspaceId) ?? '');
      if (request.operation === 'fleet.browser_home_page') return this.fleet.browserHomePage();
      if (request.operation === 'fleet.remote_machines') return this.fleet.remoteMachines();
      if (request.operation === 'fleet.app_server_statuses') return this.fleet.appServerStatuses();
      if (request.operation === 'fleet.pairing_qr_code') {
        const serverId = nonEmpty(input.serverId) ?? '';
        if (serverId === 'local') {
          const discovery = readDiscoveryRecord(process.env.BEALE_APP_SERVER_STATE_FILE?.trim() || defaultDiscoveryPath());
          if (!discovery || discovery.pid !== process.pid) throw new Error('The local app server is unavailable.');
          return createAppServerPairingQrCode(discovery.url, discovery.operatorToken);
        }
        const server = this.fleet.remoteAppServer(serverId);
        return createAppServerPairingQrCode(server.url, server.operatorToken);
      }
      if (request.operation === 'fleet.remote_catalog') return this.fleet.remoteCatalog(nonEmpty(input.serverId) ?? '');
      if (request.operation === 'fleet.remote_workspace_details') return this.fleet.remoteWorkspaceDetails(nonEmpty(input.serverId) ?? '', nonEmpty(input.workspaceId) ?? '');
      if (request.operation === 'fleet.remote_research_track') {
        const serverId = nonEmpty(input.serverId) ?? '';
        const trackRequest = decodeBealeWorkspaceResearchTrackRequest({ workspaceId: input.workspaceId,
          action: input.action ?? 'status', ...(input.configuration ? { configuration: input.configuration } : {}) });
        try {
          return await this.fleet.callRemote(serverId, 'workspace.research_track', { ...trackRequest });
        } catch (error) {
          if (!(error instanceof FleetIncompleteResponseError) || trackRequest.action === 'status') throw error;
          const current = await this.fleet.callRemote<BealeWorkspaceResearchTrack | null>(serverId,
            'workspace.research_track', { workspaceId: trackRequest.workspaceId, action: 'status' }).catch(() => null);
          if (current && (trackRequest.action === 'start' || trackRequest.action === 'resume'
            ? current.status === 'running' : trackRequest.action === 'pause'
              ? current.status === 'paused' : current.status === 'stopped')) return current;
          throw error;
        }
      }
      if (request.operation === 'fleet.remote_workspace_document') {
        if (input.kind !== 'runbook' && input.kind !== 'report') throw new Error('Invalid remote workspace document kind.');
        return this.fleet.remoteWorkspaceDocument(
          nonEmpty(input.serverId) ?? '', nonEmpty(input.workspaceId) ?? '', input.kind, nonEmpty(input.resourceId) ?? ''
        );
      }
      if (request.operation === 'fleet.remote_quick_chats') return this.fleet.remoteQuickChats(nonEmpty(input.serverId) ?? '');
      if (request.operation === 'fleet.remote_quick_chat_start') return this.fleet.remoteStartQuickChat(nonEmpty(input.serverId) ?? '', input.quickChat);
      if (request.operation === 'fleet.remote_quick_chat_continue') return this.fleet.remoteContinueQuickChat(
        nonEmpty(input.serverId) ?? '', nonEmpty(input.workspaceId) ?? '', nonEmpty(input.sessionId) ?? '', nonEmpty(input.instruction) ?? ''
      );
      if (request.operation === 'fleet.remote_session') return this.fleet.remoteSession(
        nonEmpty(input.serverId) ?? '', nonEmpty(input.workspaceId) ?? '', nonEmpty(input.sessionId) ?? '', nonEmpty(input.beforeEventId)
      );
      if (request.operation === 'fleet.remote_browser_attach') return this.fleet.remoteBrowserAttach(nonEmpty(input.serverId) ?? '', nonEmpty(input.sessionId) ?? '');
      if (request.operation === 'fleet.remote_launch') {
        const workspaceId = nonEmpty(input.workspaceId) ?? '';
        return this.fleet.remoteLaunch(nonEmpty(input.serverId) ?? '', workspaceId, decodeFleetRemoteResearchInput(input.research, workspaceId));
      }
      if (request.operation === 'fleet.remote_prompt_expand') return this.fleet.remoteExpandPrompt(
        nonEmpty(input.serverId) ?? '', nonEmpty(input.workspaceId) ?? '', decodeFleetRemotePromptExpansionInput(input.expansion)
      );
      if (request.operation === 'fleet.remote_goal_suggestions') return this.fleet.remoteGoalSuggestions(
        nonEmpty(input.serverId) ?? '', nonEmpty(input.workspaceId) ?? '', nonEmpty(input.phase) ?? '', input.refresh === true, input.incremental === true
      );
      if (request.operation === 'fleet.remote_control') return this.fleet.remoteControl(nonEmpty(input.serverId) ?? '', nonEmpty(input.sessionId) ?? '', nonEmpty(input.type) ?? '', nonEmpty(input.instruction));
      if (request.operation === 'fleet.restart_app_server') return this.fleet.restartRemoteAppServer(nonEmpty(input.serverId) ?? '');
      if (request.operation === 'fleet.restart_guest_app_server') return this.fleetRemote.restartGuestAppServer(nonEmpty(input.machineId) ?? '');
      if (request.operation === 'fleet.configure') return this.fleet.configure(input);
      if (request.operation === 'fleet.test_ssh') return this.fleet.testSsh(decodeFleetSshTestInput(input));
      if (request.operation === 'fleet.test_app_server') return this.fleet.testAppServer(input);
      if (request.operation === 'fleet.clone') {
        const state = await this.fleet.clone(nonEmpty(input.baseId) ?? '', nonEmpty(input.name) ?? '');
        const owner = fleetOwner(input, this.fleet.machineId());
        const clone = state.machines.find((machine) => machine.name === input.name);
        return owner && clone ? this.fleet.reserve(clone.id, owner) : state;
      }
      if (request.operation === 'fleet.clone_for_session') {
        const owner = fleetOwner(input, null);
        if (!owner) throw new Error('Fleet session clone requires an owning machine and session.');
        const worker = await this.fleet.cloneForSession(nonEmpty(input.baseId) ?? '', { ...owner,
          ...(nonEmpty(input.workspaceId) ? { workspaceId: nonEmpty(input.workspaceId)! } : {}) });
        return { machineId: worker.id };
      }
      if (request.operation === 'fleet.start') {
        const machineId = nonEmpty(input.machineId) ?? '';
        const owner = fleetOwner(input, this.fleet.machineId());
        if (owner) await this.fleet.reserve(machineId, owner);
        return this.fleet.start(machineId, owner ?? undefined);
      }
      if (request.operation === 'fleet.stop') return this.fleet.stop(nonEmpty(input.machineId) ?? '', fleetOwner(input, this.fleet.machineId()) ?? undefined);
      if (request.operation === 'fleet.reserve') {
        const owner = fleetOwner(input, null);
        if (!owner) throw new Error('Fleet reservation requires an owner machine and session.');
        return this.fleet.reserve(nonEmpty(input.machineId) ?? '', { ...owner,
          ...(nonEmpty(input.workspaceId) ? { workspaceId: nonEmpty(input.workspaceId)! } : {}) });
      }
      if (request.operation === 'fleet.release') {
        const owner = fleetOwner(input, null);
        if (!owner) throw new Error('Fleet release requires an owner machine and session.');
        const machineId = nonEmpty(input.machineId) ?? '';
        if (this.fleet.isReservedBy(machineId, owner) && this.fleet.isSessionClone(machineId)) {
          await this.fleet.stop(machineId, owner);
        }
        return this.fleet.release(machineId, owner);
      }
      if (request.operation === 'fleet.prepare') {
        const workspaceId = nonEmpty(input.workspaceId);
        const runId = nonEmpty(input.runId);
        const machineId = nonEmpty(input.machineId);
        if (!workspaceId || !runId) throw new Error('Fleet preparation requires a workspace and run ID.');
        const workspace = this.requireWorkspace(workspaceId);
        if (!machineId || machineId === 'local') { await this.fleetRemote.validateLocal(workspace.workspaceId, nonEmpty(input.mode)); return { machineId: 'local' }; }
        const ownerMachineId = nonEmpty(input.ownerMachineId) ?? this.fleet.machineId();
        if (ownerMachineId !== this.fleet.machineId()) {
          return this.fleetRemote.prepare(workspace, runId, machineId, ownerMachineId);
        }
        const providerId = nonEmpty(input.providerId);
        const modelId = nonEmpty(input.modelId);
        const providerModels = Array.isArray(input.providerModels)
          ? input.providerModels.filter((value): value is { providerId: string; modelId: string } =>
            value !== null && typeof value === 'object'
              && typeof value.providerId === 'string' && typeof value.modelId === 'string'
              && value.providerId.length > 0 && value.modelId.length > 0)
          : providerId && modelId ? [{ providerId, modelId }] : [];
        if (providerModels.length === 0) throw new Error('Select an authenticated model before launching a Fleet session.');
        for (const selection of providerModels) {
          await verifyBrokerModelAccess(selection.providerId, selection.modelId,
            this.registry.providerSettings().authenticationPreferences, resolveAppServerCodexAuthFile());
        }
        const providerIds = providerModels.map((selection) => selection.providerId);
        return this.fleetRemote.prepare(workspace, runId, machineId, ownerMachineId,
          providerIds.length ? { providerIds: [...new Set(providerIds)], fastMode: input.fastMode === true } : undefined);
      }
      if (request.operation === 'fleet.connect') return this.fleetRemote.connect(nonEmpty(input.machineId) ?? '', nonEmpty(input.runId) ?? '', nonEmpty(input.ownerMachineId) ?? this.fleet.machineId());
      if (request.operation === 'fleet.complete') {
        const runId = nonEmpty(input.runId);
        if (!runId) throw new Error('Fleet completion requires a run ID.');
        return this.fleetRemote.complete(runId);
      }
      if (request.operation === 'fleet.broker') {
        const state = await this.fleet.state();
        if (!state.enabled || state.role !== 'guest') throw new Error('Model broker requests require a Fleet guest.');
        const runId = nonEmpty(input.runId);
        if (!runId) throw new Error('Model broker session ID is required.');
        const action = nonEmpty(input.action);
        if (action === 'register') { this.fleetBroker.register(runId); return { registered: true }; }
        if (action === 'poll') return this.fleetBroker.poll(runId);
        const requestId = nonEmpty(input.requestId) ?? '';
        const leaseId = nonEmpty(input.leaseId) ?? '';
        if (action === 'renew') { this.fleetBroker.renew(runId, requestId, leaseId); return { renewed: true }; }
        if (action === 'begin') { this.fleetBroker.beginResult(runId, requestId, leaseId); return { begun: true }; }
        if (action === 'append') {
          this.fleetBroker.appendResult(runId, requestId, leaseId, Number(input.offset), nonEmpty(input.data) ?? '');
          return { appended: true };
        }
        if (action === 'seal') {
          this.fleetBroker.sealResult(runId, requestId, leaseId, nonEmpty(input.sha256) ?? '');
          return { sealed: true };
        }
        throw new Error('Unsupported model broker action.');
      }
      if (request.operation === 'fleet.relay_broker') {
        const state = await this.fleet.state();
        if (!state.enabled || state.role !== 'primary') throw new Error('Model broker relay requires a primary.');
        const machineId = nonEmpty(input.machineId) ?? '';
        const runId = nonEmpty(input.runId) ?? '';
        const ownerMachineId = nonEmpty(input.ownerMachineId) ?? '';
        return this.fleetRemote.brokerOperation(machineId, runId, ownerMachineId, input);
      }
      if (request.operation === 'fleet.stage' || request.operation === 'fleet.export') {
        const state = await this.fleet.state();
        if (!state.enabled || state.role !== 'guest') throw new Error('Fleet workspace transfer is available only in a guest Beale instance.');
        if (request.operation === 'fleet.stage' && input.action === 'begin' && typeof input.primaryName === 'string') {
          await this.fleet.configure({ action: 'set-primary', name: input.primaryName });
        }
        return request.operation === 'fleet.stage' ? this.fleetWorkspace.guestStage(input) : this.fleetWorkspace.guestExport(input);
      }
      if (request.operation === 'fleet.relay_stage' || request.operation === 'fleet.relay_export') {
        const state = await this.fleet.state();
        if (!state.enabled || state.role !== 'primary') throw new Error('Fleet relay transfers require an enabled primary app server.');
        return request.operation === 'fleet.relay_stage' ? this.fleetWorkspace.guestStage(input) : this.fleetWorkspace.guestExport(input);
      }
      throw new Error('Unsupported Fleet operation.');
    }
    const workspaceIdentifier = workspaceIdentifierFromInput(request.input);
    const workspace = workspaceIdentifier
      ? this.registry.resolveWorkspace(workspaceIdentifier)
      : null;
    if (request.operation.startsWith('research.tools.') && !workspace) {
      throw new Error('Codex research-tool operations require a registered Beale workspace.');
    }
    if (workspace && request.profileId && request.profileId !== workspace.researchProfileId) {
      throw new Error(`Workspace ${workspace.workspaceId} uses research profile ${workspace.researchProfileId}, not ${request.profileId}.`);
    }
    if (workspace?.memoryBackend === 'disabled' && request.operation.startsWith('claim.')) {
      throw new Error(`Workspace ${workspace.workspaceId} has memory disabled.`);
    }
    if (request.operation === 'workspace.surface_map') {
      if (!workspace) throw new Error('A registered workspace is required.');
      const surfaceRequest = decodeBealeWorkspaceSurfaceMapRequest(request.input);
      if (surfaceRequest.workspaceId !== workspace.workspaceId) throw new Error('The surface map workspace does not match the registered workspace.');
      return this.workspaceSurfaceMap(workspace.workspaceId, surfaceRequest.sessionId);
    }
    if (request.operation === 'workspace.research_track') {
      if (!workspace) throw new Error('A registered workspace is required.');
      const input = decodeBealeWorkspaceResearchTrackRequest(request.input);
      return this.workspaceResearchTrack(workspace.workspaceId, input.action, input.configuration);
    }
    const storageProfileId = workspace?.researchProfileId ?? request.profileId;
    if (request.operation === 'workspace.project') {
      const input = decodeWorkspaceProjectRequest(request.input);
      if (!workspace) throw new Error('A registered workspace is required.');
      const project = readWorkspaceProject(workspace.workspacePath);
      if (input.action === 'status') {
        return { project, checkpoint: null,
          researchIndex: readWorkspaceResearchCacheState(workspace.workspacePath) };
      }
      if (!project) throw new Error('This reference workspace does not use the research project layout. Create a new workspace.');
      const key = workspaceOperationKey(workspace.workspacePath);
      if (this.workspaceExclusiveOperations.has(key)) throw new Error('Another workspace operation is in progress.');
      const exclusive = input.action === 'import' || input.action === 'export' || input.action === 'sync' || input.action === 'rebuild-index' || input.action === 'release-index';
      if (exclusive && [...this.workspaceWriters.values()].some((root) => workspaceOperationKey(root) === key)) throw new Error(`Stop workspace research before ${input.action.replace(/-/gu, ' ')}.`);
      const storage = this.registry.storageForProfile(workspace.researchProfileId || 'security-research');
      if (exclusive) this.workspaceExclusiveOperations.add(key);
      try { return await runWorkspaceCheckpoint({ workspaceRoot: workspace.workspacePath, workspaceId: workspace.workspaceId, ...storage },
        input.action === 'import' ? 'Import research file edit' : input.action === 'export' ? 'Export compatibility research snapshot' : input.action === 'sync' ? 'Synchronize file-authority research' : input.action === 'rebuild-index' ? 'Rebuild derived research index' : input.action === 'release-index' ? 'Release derived research index' : 'Synchronize workspace research',
        input.action === 'import' ? input : undefined, undefined, this.databaseCoordinator,
        (input.action === 'export' || input.action === 'sync') ? { exportResearch: true }
          : input.action === 'rebuild-index' ? { researchIndexAction: 'rebuild' }
          : input.action === 'release-index' ? { researchIndexAction: 'release' }
          : undefined);
      } finally { if (exclusive) this.workspaceExclusiveOperations.delete(key); }
    }
    const storage = request.operation === 'workspace.state'
      ? persistenceStorageFromInput(request.input)
      : storageProfileId ? this.registry.storageForProfile(storageProfileId) : null;
    if (workspace && storage && operationRequiresResearchIndex(request.operation)
      && workspaceResearchIndexNeedsRebuild(workspace.workspacePath)) {
      const key = workspaceOperationKey(workspace.workspacePath);
      if (this.workspaceExclusiveOperations.has(key)) throw new Error('The workspace research index is unavailable during another workspace operation.');
      this.workspaceExclusiveOperations.add(key);
      try {
        const rebuilt = await runWorkspaceCheckpoint({ workspaceRoot: workspace.workspacePath, workspaceId: workspace.workspaceId, ...storage },
          'Rehydrate derived research index', undefined, undefined, this.databaseCoordinator, { researchIndexAction: 'rebuild' });
        if (rebuilt.status === 'failed' || rebuilt.researchIndex?.state !== 'ready') throw new Error(rebuilt.error ?? 'Derived research index rehydration failed.');
      } finally { this.workspaceExclusiveOperations.delete(key); }
    }
    const researchToolWorkspaceReferences = workspace && storage && request.operation.startsWith('research.tools.')
      ? (await this.sameSubjectWorkspaceReferences(workspace, storage)).map(({ workspaceId, workspaceName }) => ({
          workspaceId,
          workspaceName
        }))
      : undefined;
    const operationInput = workspace && storage
      ? request.operation.startsWith('research.tools.')
        ? this.hostedResearchToolInput(request.input, workspace, researchToolWorkspaceReferences)
        : request.operation.startsWith('suggestion.')
        ? this.hostedSuggestionInput(request.operation, request.input, workspace, storage)
        : request.operation === 'prompt.expand'
          ? await this.hostedPromptExpansionInput(request.input, workspace, storage)
          : request.input
      : request.input;
    const result = await this.invokeProtocol(request.operation, {
      args: request.args ?? [],
      ...(operationInput !== undefined ? { input: operationInput } : {}),
      ...(storage ? { storage } : {}),
      ...(request.signal ? { signal: request.signal } : {})
    });
    const researchMutation = request.operation === 'research.tools.mutate'
      || ['claim.mark_duplicate', 'claim.undo_duplicate', 'claim.board_transition', 'history.mark_duplicate', 'history.undo_duplicate', 'report.revise_content', 'report.update_triage_status', 'report.replace_packet', 'report.replace_recording'].includes(request.operation)
      || request.operation === 'workspace.state' && isRecord(request.input) && ['saveScope', 'setResearchSubject', 'addWorkspaceRules', 'addWorkspaceRule'].includes(String(request.input.action));
    if (workspace && storage && researchMutation && readWorkspaceProject(workspace.workspacePath)) {
      // Canonical persistence has already succeeded. A checkpoint failure must
      // remain visible without making a retryable mutation appear rolled back.
      const mutation = isRecord(operationInput) ? operationInput : {};
      await runWorkspaceCheckpoint({ workspaceRoot: workspace.workspacePath, workspaceId: workspace.workspaceId, ...storage,
        ...(nonEmpty(mutation.sessionId) ? { sessionId: nonEmpty(mutation.sessionId)! } : {}),
      }, 'Canonical research updated', undefined, undefined, this.databaseCoordinator);
    }
    if (workspace?.memoryBackend !== 'disabled') return result;
    if (request.operation === 'memory.summary') return withoutWorkspaceMemory(result);
    if (request.operation === 'memory.notification_feed'
      && result && typeof result === 'object' && !Array.isArray(result)) {
      return { ...(result as Record<string, unknown>), nodes: [] };
    }
    return result;
  }

  private hostedResearchToolInput(
    input: unknown,
    workspace: AppServerHostWorkspace,
    workspaceReferences?: readonly { workspaceId: string; workspaceName: string }[]
  ): Record<string, unknown> {
    const requested = isRecord(input) ? input : {};
    const toolInput = isRecord(requested.toolInput) ? requested.toolInput : {};
    const modelAuthor = isRecord(requested.modelAuthor)
      && nonEmpty(requested.modelAuthor.provider)
      && nonEmpty(requested.modelAuthor.model)
      ? {
          provider: nonEmpty(requested.modelAuthor.provider)!,
          model: nonEmpty(requested.modelAuthor.model)!
        }
      : undefined;
    return {
      workspaceId: workspace.workspaceId,
      workspaceName: workspace.name,
      workspaceRoot: workspace.workspacePath,
      researchProfileId: workspace.researchProfileId,
      memoryBackend: workspace.memoryBackend,
      ...(workspaceReferences?.length ? { workspaceReferences } : {}),
      ...(nonEmpty(requested.sessionId) ? { sessionId: nonEmpty(requested.sessionId)! } : {}),
      ...(nonEmpty(requested.objective) ? { objective: nonEmpty(requested.objective)! } : {}),
      ...(nonEmpty(requested.toolName) ? { toolName: nonEmpty(requested.toolName)! } : {}),
      ...(isRecord(requested.toolInput) ? { toolInput } : {}),
      ...(modelAuthor ? { modelAuthor } : {})
    };
  }

  private hostedSuggestionInput(
    operation: AppServerProtocolOperation,
    input: unknown,
    workspace: AppServerHostWorkspace,
    storage: AppServerHostStorage
  ): Record<string, unknown> {
    if (!isRecord(input)) throw new Error('Suggestion input is required.');
    if (operation === 'suggestion.select') {
      return {
        ...input,
        workspaceId: workspace.workspaceId,
        databasePath: storage.databasePath
      };
    }
    if (operation !== 'suggestion.generate' && operation !== 'suggestion.steering') return { ...input };
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
      ...(operation === 'suggestion.generate'
        ? { researchProfileId: workspace.researchProfileId, memoryEnabled: workspace.memoryBackend !== 'disabled' }
        : {}),
      provider: {
        id: providerId,
        ...(defaults?.smallModel ? { smallModel: defaults.smallModel } : {}),
        ...(defaults?.reasoningEffort ? { reasoningEffort: defaults.reasoningEffort } : {}),
        ...(codexAuthFile ? { codexAuthFile } : {}),
        authenticationPreferences: settings.authenticationPreferences
      }
    };
  }

  private async hostedPromptExpansionInput(
    input: unknown,
    workspace: AppServerHostWorkspace,
    storage: AppServerHostStorage
  ): Promise<Record<string, unknown>> {
    if (!isRecord(input)) throw new Error('Research prompt expansion input is required.');
    const settings = this.registry.providerSettings();
    const requestedProvider = isRecord(input.provider) ? nonEmpty(input.provider.id) : undefined;
    const providerId = requestedProvider ?? settings.defaultProviderId;
    if (!providerId) throw new Error('No Lead provider is configured for this Beale host.');
    if (providerId === 'openrouter') await refreshOpenRouterModels();

    const connectedProviderIds = new Set([
      ...(settings.defaultProviderId ? [settings.defaultProviderId] : []),
      ...Object.keys(settings.modelDefaults),
      ...Object.keys(settings.authenticationPreferences),
      ...(settings.customProviders ?? []).map((provider) => provider.id)
    ]);
    if (!connectedProviderIds.has(providerId)) {
      throw new Error(`Provider ${providerId} is not connected to this Beale host.`);
    }
    const catalog = getProviderModelCatalog(undefined, settings.customProviders ?? []).find((candidate) => candidate.providerId === providerId);
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
    const targetId = request.launch.machineId ?? 'local';
    const selectedTargetIds = request.launch.targetIds ?? [targetId];
    if (request.launch.mode !== 'quick-chat' && !request.launch.continuation) {
      const state = await this.fleet.stateForWorkspace(workspace.workspaceId);
      const allowed = state.workspaceTargets[workspace.workspaceId] ?? ['local', 'browser:in-agent'];
      for (const selectedId of selectedTargetIds) {
        if (!allowed.includes(selectedId) && !(selectedId === 'local' && allowed.includes('browser:in-agent'))) {
          throw new Error('This target is not enabled for the workspace.');
        }
        if (selectedId !== 'local' && !state.enabled) throw new Error('Fleet is disabled on this Beale instance.');
        if (selectedId.startsWith('browser:') && !state.browsers.some((browser) => browser.enabled && `browser:${browser.id}` === selectedId)) {
          throw new Error('The selected browser is unavailable in Fleet.');
        }
        if (selectedId !== 'local' && !selectedId.startsWith('browser:') && !selectedId.startsWith('vm:')) {
          throw new Error('Select a configured workspace target.');
        }
      }
    }
    const attemptId = request.launch.attemptId ?? `attempt-${randomUUID()}`;
    const requestedProfileId = request.launch.researchProfileId?.trim();
    if (requestedProfileId && requestedProfileId !== workspace.researchProfileId) {
      throw new Error(
        `Workspace ${workspace.workspaceId} uses research profile ${workspace.researchProfileId}, not ${requestedProfileId}.`
      );
    }
    const profileId = workspace.researchProfileId || 'security-research';
    if (profileId !== 'security-research') {
      throw new Error('Beale starts new research sessions only in Security workspaces. Existing non-security workspaces remain available for historical inspection.');
    }
    const storage = this.registry.storageForProfile(profileId);
    const providerSettings = this.registry.providerSettings();
    if (providerSettings.customProviders?.length) {
      process.env.APP_SERVER_CUSTOM_PROVIDERS = JSON.stringify(providerSettings.customProviders);
    } else {
      delete process.env.APP_SERVER_CUSTOM_PROVIDERS;
    }
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
    const daybreakMode = providerSettings.daybreakMode ?? 'blue';
    const daybreakBlue = providerId === 'openai-codex' && daybreakMode === 'blue';
    if (fastMode && providerId !== 'openai-codex') {
      throw new Error('Fast mode is available only when OpenAI is the Lead provider.');
    }
    if (request.launch.provider?.daybreakBlue !== undefined && providerId !== 'openai-codex') {
      throw new Error('Daybreak Blue is available only when OpenAI is the Lead provider.');
    }
    const shellReviewModels = {
      ...providerSemantics.defaultSmallModels,
      ...Object.fromEntries(Object.entries(providerSettings.modelDefaults).flatMap(([id, defaults]) => (
        defaults.smallModel ? [[id, defaults.smallModel] as const] : []
      )))
    };
    const runDirectory = readWorkspaceProject(workspace.workspacePath)
      ? join(workspace.workspacePath, 'traces', sessionId, 'outputs')
      : join(workspace.workspacePath, '.beale', 'app-server-runs');
    const continuation = request.launch.continuation;
    const fileStem = continuation ? `${sessionId}.${attemptId}` : sessionId;
    const capturePath = join(runDirectory, `${fileStem}.capture.json`);
    await mkdir(runDirectory, { recursive: true });
    const promptTemplate = this.registry.promptTemplateOverride(profileId);
    if (promptTemplate !== null) validateResearchSystemPromptTemplate(promptTemplate);
    const promptTemplatePath = promptTemplate !== null
      ? join(this.registry.registryDirectory, 'prompt-templates', `${createHash('sha256').update(promptTemplate).digest('hex')}.txt`)
      : undefined;
    if (promptTemplatePath && promptTemplate !== null) {
      await mkdir(join(this.registry.registryDirectory, 'prompt-templates'), { recursive: true, mode: 0o700 });
      try {
        await writeFile(promptTemplatePath, promptTemplate, { encoding: 'utf8', mode: 0o600, flag: 'wx' });
      } catch (error) {
        if (!(error instanceof Error && 'code' in error && error.code === 'EEXIST')) throw error;
        if (readFileSync(promptTemplatePath, 'utf8') !== promptTemplate) {
          throw new Error('Stored prompt template snapshot does not match its content hash.');
        }
      }
    }
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
    const requiredPluginIds = bundledPluginsRequiredByResearchKit(workspace.researchKitId);
    if (requiredPluginIds.length > 0) {
      const pluginInput = {
        registryDirectory: this.registry.registryDirectory,
        builtinPlugins: defaultBuiltinPlugins()
      };
      const pluginState = await this.invokeProtocol<{ plugins: Array<{ id: string; installed: boolean }> }>('plugin.list', {
        args: ['harness', 'plugin-list'], input: pluginInput
      });
      for (const pluginId of requiredPluginIds) {
        if (pluginState.plugins.find((plugin) => plugin.id === pluginId)?.installed === false) {
          await this.invokeProtocol('plugin.set_enabled', {
            args: ['harness', 'plugin-set-enabled'],
            input: { ...pluginInput, pluginId, enabled: true }
          });
        }
      }
    }
    const [loadedPluginRuntime, workspaceReferences] = await Promise.all([
      introspection?.runtimeMode === 'isolated'
        ? this.loadIntrospectionPluginRuntime()
        : this.resolvePluginRuntime(workspace),
      this.sameSubjectWorkspaceReferences(workspace, storage)
    ]);
    const metaSkillDirectory = resolve(builtinPluginPath('meta-skills', 'managed'), 'skills');
    const sourcePluginCatalogPath = loadedPluginRuntime?.pluginCatalogPath;
    const featurePromptOverrides = this.registry.featurePromptOverrides();
    const pluginCatalogPath = sourcePluginCatalogPath && (workspace.researchKitId !== 'meta-bug-bounty' || !introspection
      || Object.keys(featurePromptOverrides).length > 0)
      ? join(runDirectory, `${fileStem}.plugin-catalog.json`)
      : sourcePluginCatalogPath;
    if (pluginCatalogPath && sourcePluginCatalogPath && pluginCatalogPath !== sourcePluginCatalogPath) {
      const catalog = decodeResearchPluginCatalog(JSON.parse(await readFile(sourcePluginCatalogPath, 'utf8')) as unknown);
      await writePrivateJson(pluginCatalogPath, applyFeaturePromptOverrides(catalog.filter((plugin) =>
        (workspace.researchKitId === 'meta-bug-bounty' || plugin.id !== 'meta-skills-builtin')
        && (introspection || plugin.id !== 'beale-introspection-builtin')), featurePromptOverrides));
    }
    const pluginRuntime = loadedPluginRuntime
      ? {
          ...loadedPluginRuntime,
          ...(pluginCatalogPath ? { pluginCatalogPath } : {}),
          ...(workspace.researchKitId !== 'meta-bug-bounty'
            ? {
                skillDirectories: (loadedPluginRuntime.skillDirectories ?? []).filter(
                  (directory) => resolve(directory) !== metaSkillDirectory
                ),
                selectedSkillIds: (loadedPluginRuntime.selectedSkillIds ?? []).filter(
                  (id) => id !== 'meta-bug-bounty-tools'
                )
              }
            : {}),
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
      ...(daybreakBlue ? { daybreakBlue: true } : {}),
      profileId
    });
    const trackAssignment = request.launch.mode === 'research-track'
      ? this.researchTrackSessionAssignment(storage.databasePath, workspace.workspaceId, sessionId)
      : null;
    if (request.launch.mode === 'research-track' && !trackAssignment) {
      throw new Error('The research track has no active surface assigned to this session.');
    }
    const remoteShells: Record<string, Awaited<ReturnType<FleetService['prepareSessionTarget']>>> = {};
    try {
      for (const selectedId of selectedTargetIds) {
        if (selectedId.startsWith('vm:')) {
          remoteShells[selectedId] = await this.fleet.prepareSessionTarget(selectedId, workspace.workspaceId, sessionId, !request.launch.continuation);
        }
      }
    } catch (error) {
      await Promise.allSettled(Object.values(remoteShells).map((shell) => this.fleet.releaseSessionTarget(shell.machineId, sessionId)));
      throw error;
    }
    const remoteShell = selectedTargetIds.length === 1 ? remoteShells[selectedTargetIds[0]!] ?? null : null;
    try {
      await this.ensureCanonicalSession({
      sessionId,
      attemptId,
      workspace,
      storage,
      prompt: request.launch.promptMarkdown,
      ...(trackAssignment ? { initialTitle: initialResearchTrackTitle(trackAssignment) } : {}),
      providerId,
      ...(model ? { model } : {}),
      ...(reasoningEffort ? { reasoningEffort } : {}),
      ...(request.launch.workflowId ? { workflowId: request.launch.workflowId } : {}),
      ...(request.launch.repeatSchedule && request.launch.repeatSchedule.type !== 'none'
        ? { repeatSchedule: request.launch.repeatSchedule,
            machineId: request.launch.machineId ?? 'local', targetIds: [...selectedTargetIds],
            goal: request.launch.goal, fastMode, daybreakBlue,
            shellSafetyMode: request.launch.shellSafetyMode?.trim() || 'auto_review' }
        : {}),
      ...(request.launch.collaboration ? { collaboration: request.launch.collaboration } : {}),
      profileId,
      ...(request.launch.researchProfileHash
        ? { profileHash: request.launch.researchProfileHash }
        : {}),
      continuation: Boolean(continuation),
      restartLaunch,
      ...(continuation?.resumeAttemptId ? { parentAttemptId: continuation.resumeAttemptId } : {})
      });
    } catch (error) {
      await Promise.allSettled(Object.values(remoteShells).map((shell) => this.fleet.releaseSessionTarget(shell.machineId, sessionId)));
      throw error;
    }
    if (Object.keys(remoteShells).length > 0) this.fleetSessionMachines.set(sessionId, Object.values(remoteShells).map((shell) => shell.machineId));
    return {
      sessionId,
      attemptId,
      launch: {
        workspaceRoot: workspace.workspacePath,
        workspaceDirectories: [workspace.workspacePath],
        ...(workspaceReferences.length > 0 ? { workspaceReferences } : {}),
        capturePath,
        attemptId,
        promptMarkdown: request.launch.promptMarkdown,
        ...(request.launch.goal ? { goal: request.launch.goal } : {}),
        ...(request.launch.devHarnessFeedback ? { devHarnessFeedback: true } : {}),
        provider: {
          id: providerId,
          ...(model ? { model } : {}),
          ...(reasoningEffort ? { reasoningEffort } : {}),
          ...(fastMode ? { fastMode: true } : {}),
          ...(daybreakBlue ? { daybreakBlue: true } : {}),
          ...(providerId === 'openai-codex' ? { daybreakMode } : {}),
          contextSize: providerSettings.contextSizes?.['openai-codex'] ?? 'default',
          riskAcknowledgements: this.fleetBroker.token(sessionId) && request.launch.brokerRiskAcknowledgements
            ? request.launch.brokerRiskAcknowledgements : providerSettings.riskAcknowledgements,
          authenticationPreferences: this.fleetBroker.token(sessionId) && request.launch.brokerAuthenticationPreferences
            ? request.launch.brokerAuthenticationPreferences
            : providerSettings.authenticationPreferences,
          ...(request.launch.generateTitle || (request.launch.mode === 'research-track' && !request.launch.continuation?.resumeAttemptId)
            ? {
                title: {
                  ...(providerDefaults?.smallModel || providerSemantics.defaultSmallModels[providerId]
                    ? { model: providerDefaults?.smallModel ?? providerSemantics.defaultSmallModels[providerId]! }
                    : {}),
                  effort: providerId.startsWith('custom-') ? 'off' : providerSemantics.sessionTitleEffort,
                  ...(request.launch.mode === 'research-track' ? { style: 'research-track' as const } : {})
                }
              }
            : {}),
          shellReview: {
            models: shellReviewModels,
            effort: providerId.startsWith('custom-') ? 'off' : providerSemantics.shellReviewEffort
          }
        },
        shellSafetyMode: request.launch.shellSafetyMode?.trim() || 'auto_review',
        guardianEnabled: request.launch.guardianEnabled !== false,
        ...(remoteShell ? { remoteShell: { sshArgs: remoteShell.sshArgs, guestPlatform: remoteShell.guestPlatform, guestCwd: remoteShell.guestCwd } } : {}),
        ...(selectedTargetIds.length > 1 ? { selectedTargets: { ids: selectedTargetIds,
          remoteShells: Object.fromEntries(Object.entries(remoteShells).map(([id, shell]) => [id,
            { sshArgs: shell.sshArgs, guestPlatform: shell.guestPlatform, guestCwd: shell.guestCwd }])) } } : {}),
        ...(existsSync(this.registry.shellOptionsPath) ? { shellOptionsPath: this.registry.shellOptionsPath } : {}),
        ...(collaborationConfigPath ? { collaborationConfigPath } : {}),
        ...(promptTemplatePath ? { promptTemplatePath } : {}),
        ...(resumeCapturePath ? { resumeCapturePath } : {}),
        ...(resumeFallbackPromptPath ? { resumeFallbackPromptPath } : {}),
        ...(request.launch.mode === 'research-track' ? { trackEpoch: true } : {}),
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

  private async sameSubjectWorkspaceReferences(
    current: AppServerHostWorkspace,
    storage: AppServerHostStorage
  ): Promise<Array<{ workspaceId: string; workspaceName: string; workspaceRoot: string; subjectId: string }>> {
    return this.databaseCoordinator.runWhenAvailable(storage.databasePath, () => {
      let currentBinding: ReturnType<typeof resolveStoredResearchWorkspaceBinding>;
      try {
        currentBinding = resolveStoredResearchWorkspaceBinding({
          workspaceRoot: current.workspacePath,
          databasePath: storage.databasePath,
          researchProfileId: current.researchProfileId
        });
      } catch {
        return [];
      }
      if (currentBinding.memoryContext.workspaceId !== current.workspaceId) return [];
      const subjectId = currentBinding.memoryContext.subjectId;
      return this.registry.listHostWorkspaces().flatMap((candidate) => {
        if (candidate.workspaceId === current.workspaceId
          || candidate.researchProfileId !== current.researchProfileId) return [];
        try {
          const binding = resolveStoredResearchWorkspaceBinding({
            workspaceRoot: candidate.workspacePath,
            databasePath: storage.databasePath,
            researchProfileId: candidate.researchProfileId
          });
          if (binding.memoryContext.workspaceId !== candidate.workspaceId
            || binding.memoryContext.subjectId !== subjectId) return [];
          return [{
            workspaceId: candidate.workspaceId,
            workspaceName: candidate.name,
            workspaceRoot: candidate.workspacePath,
            subjectId
          }];
        } catch {
          return [];
        }
      });
    });
  }

  public setWorkspaceSessionActive(workspaceIdentifier: string, sessionId: string, active: boolean): void {
    if (active) {
      const root = this.requireWorkspace(workspaceIdentifier).workspacePath;
      if (this.workspaceExclusiveOperations.has(workspaceOperationKey(root))) throw new Error('Wait for workspace import or housekeeping to finish before starting research.');
      this.workspaceWriters.set(sessionId, root);
    }
    else this.workspaceWriters.delete(sessionId);
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

  public async createSessionContinuationRequest(
    workspaceId: string,
    sessionId: string,
    instruction: string
  ): Promise<AppServerSessionLaunchRequest> {
    const normalizedInstruction = instruction.trim();
    if (!normalizedInstruction) throw new Error('A session continuation instruction is required.');
    const workspace = this.requireWorkspace(workspaceId);
    const storage = this.registry.storageForProfile(workspace.researchProfileId || 'security-research');
    const update = await this.invokeProtocol<AppServerSessionUpdateProjection>('session.get_update', {
      args: [
        'session', 'get-update', '--session-id', sessionId,
        '--tail', '--limit', '500', '--max-bytes', String(2 * 1024 * 1024)
      ],
      storage
    });
    const session = update.session;
    if (!session || nonEmpty(session.id) !== sessionId || nonEmpty(session.workspaceId) !== workspace.workspaceId) {
      throw new Error(`Session ${sessionId} does not belong to workspace ${workspace.workspaceId}.`);
    }
    const status = nonEmpty(session.status);
    if (!status || !['blocked', 'completed', 'failed', 'stopped'].includes(status)) {
      throw new Error(`Session ${sessionId} is not inactive and cannot be continued.`);
    }
    const restart = decodeRestartLaunchDescriptor(session.metadata);
    if (!restart) {
      throw new Error(`Session ${sessionId} does not contain a compatible continuation launch descriptor.`);
    }
    const previousAttempt = latestAttempt(session.attempts);
    if (!previousAttempt) throw new Error(`Session ${sessionId} does not contain a resumable attempt.`);
    return {
      launchVersion: APP_SERVER_SESSION_LAUNCH_VERSION,
      sessionId,
      launch: {
        ...restart.launch,
        attemptId: `attempt-${randomUUID()}`,
        promptMarkdown: normalizedInstruction,
        generateTitle: false,
        continuation: {
          resumeAttemptId: previousAttempt.id,
          resumeFromInitialAttempt: previousAttempt.parentAttemptId === null,
          fallbackPrompt: manualContinuationFallbackPrompt(session, update.events, normalizedInstruction)
        }
      }
    };
  }

  public async recordSessionContinuationInstruction(
    request: AppServerSessionLaunchRequest
  ): Promise<void> {
    const sessionId = request.sessionId;
    const attemptId = request.launch.attemptId;
    if (!sessionId || !attemptId || !request.launch.continuation) return;
    const workspace = this.requireWorkspace(request.launch.workspaceId);
    const storage = this.registry.storageForProfile(workspace.researchProfileId || 'security-research');
    const eventId = `session_continuation_${attemptId}`;
    const timestamp = new Date().toISOString();
    await this.invokeProtocol('session.append_event', {
      args: ['session', 'append-event', '--session-id', sessionId],
      storage,
      input: {
        id: eventId,
        kind: 'beale.transcript',
        timestamp,
        summary: 'beale.transcript',
        payload: {
          record: {
            id: `transcript_${eventId}`,
            runId: sessionId,
            attemptId,
            traceEventId: eventId,
            role: 'user',
            phase: null,
            contentMarkdown: request.launch.promptMarkdown,
            source: 'user_steering',
            metadata: { continuation: true },
            createdAt: timestamp
          }
        },
        agentPath: '/root'
      }
    });
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
          if (restart?.launch.mode === 'research-track' && previousAttempt) {
            const trackStore = new ResearchTrackStore({ databasePath: storage.databasePath, workspaceId: workspace.workspaceId });
            try {
              const track = trackStore.get();
              const assigned = track?.surfaces.find((work) => work.surfaceId === track.activeSurfaceId);
              if (track?.status === 'running' && track.activeSessionId === sessionId
                && assigned && assigned.status !== 'active') {
                await this.invokeProtocol('session.transition', {
                  args: ['session', 'transition', '--session-id', sessionId], storage,
                  input: { status: 'completed', attemptId: previousAttempt.id,
                    summary: 'The research track epoch checkpoint was recovered after app-server restart.' }
                });
                trackStore.finishEpoch(sessionId, true);
                skippedSessions += 1;
                continue;
              }
            } finally { trackStore.close(); }
          }
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
      if (workspace.researchProfileId !== 'security-research') continue;
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

  public async workspaceResearchTrack(workspaceIdentifier: string, action: string,
    configuration?: BealeWorkspaceResearchTrackRequest['configuration']): Promise<BealeWorkspaceResearchTrack | null> {
    const workspace = this.requireWorkspace(workspaceIdentifier);
    if (workspace.researchProfileId !== 'security-research') throw new Error('A Security workspace is required for a research track.');
    const storage = this.registry.storageForProfile(workspace.researchProfileId);
    const store = new ResearchTrackStore({ databasePath: storage.databasePath, workspaceId: workspace.workspaceId });
    try {
      if (action === 'status') {
        const track = store.get();
        if (!track) return null;
        if (workspace.memoryBackend === 'disabled' && track.status === 'running') {
          return store.pauseWithError('Workspace memory was disabled. Re-enable memory before resuming the research track.');
        }
        if (workspace.memoryBackend === 'disabled') return track;
        const context = await this.workspaceResearchTrackContext(workspace);
        const current = store.syncSurfaces(context.surfaceMap.surfaces)!;
        if (current.status === 'running' && (!context.scope || context.scope.id !== current.scopeId
          || context.scope.expiresAt && Date.parse(context.scope.expiresAt) <= Date.now())) {
          return store.pauseWithError('The authorized scope changed. Review the workspace before resuming.');
        }
        if (current.status === 'running' && current.activeSessionId && current.activeSurfaceId
          && !context.surfaceMap.surfaces.some((surface) => surface.id === current.activeSurfaceId)) {
          return store.finishEpoch(current.activeSessionId, true, undefined, { countNoProgress: false });
        }
        return current;
      }
      if (action === 'pause' || action === 'stop') return store.control(action);
      if (workspace.memoryBackend === 'disabled') throw new Error('Enable workspace memory before starting or resuming a research track.');
      if (action === 'resume') {
        const track = store.get();
        if (!track) throw new Error('No research track exists in this workspace.');
        const details = await this.workspaceResearchTrackContext(workspace);
        if (!details.scope?.id || details.scope.expiresAt && Date.parse(details.scope.expiresAt) <= Date.now()) {
          throw new Error('Renew the recorded authorization before resuming continuous research.');
        }
        if (details.scope.id !== track.scopeId) {
          const readyTargets = new Set(details.targets.filter((target) => target.ready)
            .map((target) => target.id === 'browser:in-agent' ? 'local' : target.id));
          const targetIds = track.targetIds;
          if (targetIds.some((targetId) => !readyTargets.has(targetId))) {
            throw new Error('A selected research target is unavailable. Restore it before resuming the track.');
          }
          if (!targetIds.includes('local')) throw new Error('Enable Local as a workspace target to run the research track on its owning app-server.');
          return store.start({ scopeId: details.scope.id, provider: track.provider,
            guardianEnabled: track.guardianEnabled,
            ...(track.collaboration ? { collaboration: track.collaboration } : {}),
            targetIds, surfaces: details.surfaceMap.surfaces, replaceUnfinished: true });
        }
        return store.control('resume');
      }
      if (action !== 'start') throw new Error('Unsupported research track action.');
      const details = await this.workspaceResearchTrackContext(workspace);
      if (!details.scope?.id) throw new Error('Record an authorized workspace scope before starting continuous research.');
      if (details.scope.expiresAt && Date.parse(details.scope.expiresAt) <= Date.now()) {
        throw new Error('The recorded authorization has expired. Renew the workspace scope before starting.');
      }
      const readyTargets = new Set(details.targets.filter((target) => target.ready)
        .map((target) => target.id === 'browser:in-agent' ? 'local' : target.id));
      const targetIds = [...new Set(['local', ...(configuration?.targetIds ?? [...readyTargets])])];
      if (targetIds.length === 0 || targetIds.length > 16) throw new Error('Enable 1 to 16 ready workspace targets for continuous research.');
      if (targetIds.some((targetId) => !readyTargets.has(targetId))) throw new Error('A selected research target is unavailable in this workspace.');
      if (!targetIds.includes('local')) throw new Error('Enable Local as a workspace target to run the research track on its owning app-server.');
      const settings = this.registry.providerSettings();
      const providerId = configuration?.provider.id ?? settings.defaultProviderId;
      if (!providerId) throw new Error('Select a default research provider before starting continuous research.');
      const defaults = settings.modelDefaults[providerId] ?? {};
      return store.start({ scopeId: details.scope.id,
        provider: { id: providerId, ...(configuration?.provider.model || defaults.leadModel
          ? { model: configuration?.provider.model ?? defaults.leadModel } : {}),
          ...(configuration?.provider.reasoningEffort || defaults.reasoningEffort
            ? { reasoningEffort: configuration?.provider.reasoningEffort ?? defaults.reasoningEffort } : {}) },
        guardianEnabled: configuration?.guardianEnabled !== false,
        ...(configuration?.collaboration ? { collaboration: configuration.collaboration } : {}),
        targetIds, surfaces: details.surfaceMap.surfaces });
    } finally { store.close(); }
  }

  public async dueResearchTracks(at = new Date(), activeSessionIds: ReadonlySet<string> = new Set(),
    busyWorkspaceIds: ReadonlySet<string> = new Set()): Promise<DueAppServerAutomation[]> {
    const due: DueAppServerAutomation[] = [];
    for (const summary of this.registry.listWorkspaces()) {
      const workspace = this.registry.resolveWorkspace(summary.workspaceId);
      if (!workspace) continue;
      const storage = this.registry.storageForProfile(workspace.researchProfileId);
      const store = new ResearchTrackStore({ databasePath: storage.databasePath, workspaceId: workspace.workspaceId });
      try {
        let track = store.get();
        const recoveringPriorOrphan = track?.status === 'paused' && track.lastError === ORPHANED_TRACK_PAUSE_MESSAGE
          && track.activeSessionId === null;
        if (!track || (track.status !== 'running' && !recoveringPriorOrphan)
          || busyWorkspaceIds.has(workspace.workspaceId)) continue;
        if (workspace.memoryBackend === 'disabled') {
          store.pauseWithError('Workspace memory was disabled. Re-enable memory before resuming the research track.');
          continue;
        }
        const details = await this.workspaceResearchTrackContext(workspace);
        const activeSurfaceId = track.activeSurfaceId;
        if (!details.scope || details.scope.id !== track.scopeId
          || details.scope.expiresAt && Date.parse(details.scope.expiresAt) <= at.getTime()) {
          store.pauseWithError('The recorded authorization changed. Review the workspace before resuming.');
          continue;
        }
        if (track.activeSessionId && activeSurfaceId
          && !details.surfaceMap.surfaces.some((surface) => surface.id === activeSurfaceId)) {
          store.syncSurfaces(details.surfaceMap.surfaces);
          store.finishEpoch(track.activeSessionId, true, undefined, { countNoProgress: false });
          continue;
        }
        const ready = new Set(details.targets.filter((target) => target.ready).map((target) => target.id));
        if (track.targetIds.some((targetId) => !ready.has(targetId))) {
          store.pauseWithError('A selected workspace target is unavailable. Restore it or start a new pass with updated targets.');
          continue;
        }
        if (recoveringPriorOrphan) {
          try {
            const session = await this.invokeProtocol<AppServerSessionSummaryProjection>('session.get', {
              args: ['session', 'get', '--session-id', track.id], storage });
            if (!track.guardianEnabled || !guardianEnabledForTrackSession(session)) continue;
            await this.markOrphanedTrackSessionForReview(track.id, session, storage);
            track = store.control('resume');
          } catch { continue; }
        }
        // The canonical session can finish before its worker exits. Keep the assignment
        // until that worker and its terminal persistence have both settled.
        if (track.activeSessionId && activeSessionIds.has(track.activeSessionId)) continue;
        if (track.activeSessionId) {
          let guardianRecoveryPending = false;
          try {
            const session = await this.invokeProtocol<AppServerSessionSummaryProjection>('session.get', {
              args: ['session', 'get', '--session-id', track.activeSessionId], storage });
            if (session.status === 'completed' || session.status === 'failed' || session.status === 'stopped' || session.status === 'blocked') {
              const failed = session.status === 'failed';
              store.finishEpoch(track.activeSessionId, session.status === 'completed' || session.status === 'blocked',
                failed ? 'The research worker failed. Open the failed session for details.' : undefined);
              if (failed) store.pauseWithError('The research worker failed. Open the failed session for details.');
            } else if (!hasRecentTrackSessionActivity(track.updatedAt, session, at)) {
              if (track.guardianEnabled && guardianEnabledForTrackSession(session)) {
                guardianRecoveryPending = true;
                await this.markOrphanedTrackSessionForReview(track.activeSessionId, session, storage);
                store.finishEpoch(track.activeSessionId, false, ORPHANED_TRACK_EPOCH_DIAGNOSTIC,
                  { countNoProgress: false });
              } else {
                store.finishEpoch(track.activeSessionId, false, ORPHANED_TRACK_EPOCH_DIAGNOSTIC);
                store.pauseWithError(ORPHANED_TRACK_PAUSE_MESSAGE);
              }
            }
          } catch {
            if (guardianRecoveryPending) continue;
            if (at.getTime() - Date.parse(track.updatedAt) > 120_000) {
              store.finishEpoch(track.activeSessionId, false, 'The reserved research session was not found after recovery.');
              store.pauseWithError('The reserved research session was not found after recovery. Review the workspace before resuming.');
            }
          }
          track = store.get();
          if (track?.activeSessionId || track?.status !== 'running') continue;
        }
        const sessionId = track.id;
        const assignment = store.reserveNext(sessionId, details.surfaceMap.surfaces);
        if (!assignment) continue;
        const work = assignment.track.surfaces.find((item) => item.surfaceId === assignment.surface.id)!;
        const continuing = assignment.track.epochCount > 1;
        const contextReview = continuing
          ? await this.reviewResearchTrackEpochContext(workspace, storage, sessionId)
          : null;
        const promptMarkdown = researchTrackEpochPrompt(assignment.track.id, assignment.track.pass,
          assignment.track.epochCount, assignment.surface, work, assignment.focus,
          details.surfaceMap.surfaces.length, assignment.track.targetIds, contextReview?.decision,
          assignment.track.collaboration?.mode !== undefined && assignment.track.collaboration.mode !== 'solo',
          workspace.researchKitId, recoveringPriorOrphan || assignment.track.lastError === ORPHANED_TRACK_EPOCH_DIAGNOSTIC);
        due.push({ dueAt: at.toISOString(), request: { launchVersion: APP_SERVER_SESSION_LAUNCH_VERSION,
          sessionId, launch: { workspaceId: workspace.workspaceId,
            mode: 'research-track',
            machineId: 'local',
            targetIds: track.targetIds, promptMarkdown,
            provider: track.provider, ...(track.collaboration ? { collaboration: track.collaboration } : {}),
            shellSafetyMode: 'auto_review', guardianEnabled: track.guardianEnabled,
            ...(continuing ? { continuation: {
              fallbackPrompt: promptMarkdown,
              ...(contextReview?.resumeAttemptId ? {
                resumeAttemptId: contextReview.resumeAttemptId,
                resumeFromInitialAttempt: contextReview.resumeFromInitialAttempt,
              } : {}),
            } } : {}),
            generateTitle: true } } });
      } catch (error) {
        const track = store.get();
        if (track?.activeSessionId) store.finishEpoch(track.activeSessionId, false, errorMessage(error));
        store.pauseWithError(`Research track could not prepare the next surface: ${errorMessage(error)}`);
      } finally { store.close(); }
    }
    return due;
  }

  private async reviewResearchTrackEpochContext(
    workspace: AppServerHostWorkspace,
    storage: AppServerHostStorage,
    sessionId: string,
  ): Promise<{ decision: ResearchTrackContextDecision; resumeAttemptId?: string; resumeFromInitialAttempt?: boolean }> {
    let session: AppServerSessionSummaryProjection;
    try {
      session = await this.invokeProtocol<AppServerSessionSummaryProjection>('session.get', {
        args: ['session', 'get', '--session-id', sessionId], storage,
      });
    } catch {
      return { decision: { mode: 'reset', reason: 'capture_unavailable' } };
    }
    if (nonEmpty(session.workspaceId) !== workspace.workspaceId) {
      return { decision: { mode: 'reset', reason: 'capture_unavailable' } };
    }
    const attempt = latestAttempt(session.attempts);
    if (!attempt || !/^[A-Za-z0-9_-]{1,128}$/u.test(attempt.id)) {
      return { decision: { mode: 'reset', reason: 'capture_unavailable' } };
    }
    const firstAttempt = Array.isArray(session.attempts) ? latestAttempt(session.attempts.slice(0, 1)) : null;
    const fromInitialAttempt = firstAttempt?.id === attempt.id;
    const workspacePath = nonEmpty(workspace.workspacePath);
    if (!workspacePath) return { decision: { mode: 'reset', reason: 'capture_unavailable' } };
    const runDirectory = readWorkspaceProject(workspacePath)
      ? join(workspacePath, 'traces', sessionId, 'outputs')
      : join(workspacePath, '.beale', 'app-server-runs');
    const capturePath = join(runDirectory,
      `${fromInitialAttempt ? sessionId : `${sessionId}.${attempt.id}`}.capture.json`);
    let captureAvailable = false;
    try {
      const capture = await stat(capturePath);
      if (capture.size > 256 * 1024 * 1024) {
        return { decision: { mode: 'reset', reason: 'capture_oversized' } };
      }
      captureAvailable = capture.isFile() && capture.size > 0;
    } catch { /* A missing capture starts a fresh model context. */ }
    let events: unknown = [];
    if (captureAvailable) {
      try {
        const page = await this.invokeProtocol<{ events?: unknown }>('session.events', {
          args: ['session', 'events', '--session-id', sessionId, '--stream', 'trace',
            '--tail', '--limit', '2000', '--max-bytes', '4000000'], storage,
        });
        events = page.events;
      } catch { /* The capture can still be resumed without context metrics. */ }
    }
    const decision = reviewResearchTrackContext({ captureAvailable,
      ...(attempt.startedAt ? { attemptStartedAt: attempt.startedAt } : {}),
      events: Array.isArray(events) ? events : [] });
    return decision.mode === 'resume'
      ? { decision, resumeAttemptId: attempt.id, resumeFromInitialAttempt: fromInitialAttempt }
      : { decision };
  }

  private async markOrphanedTrackSessionForReview(
    sessionId: string, session: AppServerSessionSummaryProjection, storage: AppServerHostStorage
  ): Promise<void> {
    if (session.status === 'blocked' || session.status === 'completed'
      || session.status === 'failed' || session.status === 'stopped') return;
    const attempt = latestAttempt(session.attempts);
    await this.invokeProtocol('session.transition', {
      args: ['session', 'transition', '--session-id', sessionId], storage,
      input: { status: 'failed', ...(attempt ? { attemptId: attempt.id } : {}),
        summary: 'The research worker ended without a terminal record; the next track epoch will review the interruption.' }
    });
  }

  private researchTrackSessionAssignment(databasePath: string, workspaceId: string, sessionId: string):
    { surfaceName: string; theory: string; nextAction: string } | null {
    const store = new ResearchTrackStore({ databasePath, workspaceId });
    try {
      const track = store.get();
      if (track?.status !== 'running' || track.activeSessionId !== sessionId) return null;
      const surface = track.surfaces.find((item) => item.surfaceId === track.activeSurfaceId);
      if (!surface) return null;
      return { surfaceName: surface.name, theory: surface.theory, nextAction: surface.nextAction };
    } finally { store.close(); }
  }

  public recordResearchTrackEpochLaunchFailure(workspaceId: string, sessionId: string, reason: string): void {
    const workspace = this.requireWorkspace(workspaceId);
    const storage = this.registry.storageForProfile(workspace.researchProfileId);
    const store = new ResearchTrackStore({ databasePath: storage.databasePath, workspaceId });
    try {
      if (store.get()?.activeSessionId === sessionId) {
        store.finishEpoch(sessionId, false, reason);
        store.pauseWithError(reason);
      }
    } finally { store.close(); }
  }

  public async recordSessionControlState(input: {
    request: AppServerSessionLaunchRequest;
    sessionId: string;
    attemptId: string;
    state: 'active' | 'paused' | 'stopped';
    stopCause?: AppServerSessionStopCause;
  }): Promise<void> {
    const workspace = this.requireWorkspace(input.request.launch.workspaceId);
    const storage = this.registry.storageForProfile(workspace.researchProfileId || 'security-research');
    const summary = input.state === 'paused'
      ? 'Paused by the user.'
      : input.state === 'stopped'
        ? input.stopCause === 'track_rotation' ? 'Rotated to the next research epoch.'
          : input.stopCause === 'track_state' ? 'Stopped because the research track assignment changed.'
            : input.stopCause === 'track_control' ? 'Stopped with the research track controls.'
              : 'Stopped by the user.'
        : 'Resumed by the user.';
    await this.invokeProtocol('session.transition', {
      args: ['session', 'transition', '--session-id', input.sessionId],
      storage,
      input: {
        status: input.state,
        summary,
        attemptId: input.attemptId,
        metadata: input.state === 'stopped' && input.stopCause && input.stopCause !== 'user'
          ? { appServerStopCause: input.stopCause, appServerStopAt: new Date().toISOString() }
          : { manualControlState: input.state, manualControlAt: new Date().toISOString() }
      }
    });
  }

  public async recordSessionLaunchFailure(input: {
    request: AppServerSessionLaunchRequest;
    sessionId: string;
    attemptId: string;
    diagnostic: string;
  }): Promise<void> {
    const workspace = this.requireWorkspace(input.request.launch.workspaceId);
    const storage = this.registry.storageForProfile(workspace.researchProfileId || 'security-research');
    await this.invokeProtocol('session.transition', {
      args: ['session', 'transition', '--session-id', input.sessionId],
      storage,
      input: {
        status: 'failed',
        summary: 'The app-server session failed before its research worker started.',
        attemptId: input.attemptId,
        metadata: {
          appServerLaunchFailure: true,
          diagnostic: input.diagnostic.slice(-1_000)
        }
      }
    });
  }

  public async recordSessionTerminalState(input: {
    request: AppServerSessionLaunchRequest;
    sessionId: string;
    attemptId: string;
    state: 'completed' | 'failed' | 'stopped';
    trackRotation?: boolean;
    diagnostic?: string;
  }): Promise<void> {
    await this.releaseSessionTarget(input.sessionId);
    const workspace = this.requireWorkspace(input.request.launch.workspaceId);
    const storage = this.registry.storageForProfile(workspace.researchProfileId || 'security-research');
    const current = await this.invokeProtocol<AppServerSessionSummaryProjection>('session.get', {
      args: ['session', 'get', '--session-id', input.sessionId], storage
    });
    if (current.status !== 'blocked' && current.status !== 'completed'
      && current.status !== 'failed' && current.status !== 'stopped') {
      await this.invokeProtocol('session.transition', {
        args: ['session', 'transition', '--session-id', input.sessionId], storage,
        input: {
          status: input.state,
          summary: input.state === 'completed' ? 'The app-server research session completed.'
            : input.state === 'stopped' ? 'Stopped by the user.'
              : 'The app-server research worker ended before the session completed.',
          attemptId: input.attemptId
        }
      });
    }
    const trackStore = new ResearchTrackStore({ databasePath: storage.databasePath, workspaceId: workspace.workspaceId });
    const recordTrackEpochMarker = input.request.launch.mode === 'research-track'
      && input.request.sessionId === input.sessionId;
    try {
      const track = trackStore.get();
      const assigned = input.request.launch.mode === 'research-track'
        && track?.activeSessionId === input.sessionId;
      if (assigned) {
        const checkpointed = track.surfaces.some((work) => work.surfaceId === track.activeSurfaceId
          && work.status !== 'active');
        const workerFailure = input.state === 'failed' ? input.diagnostic?.trim() || 'The research worker failed.' : null;
        const diagnostic = workerFailure
          ?? (input.state === 'stopped' && input.trackRotation && !checkpointed
            ? 'The research epoch reached its time limit without a track checkpoint.' : undefined);
        const expectedStop = input.state === 'stopped' && (!input.trackRotation || checkpointed);
        trackStore.finishEpoch(input.sessionId,
          input.state === 'completed' || current.status === 'blocked' || expectedStop, diagnostic);
        if (workerFailure) trackStore.pauseWithError(workerFailure);
      }
    }
    finally { trackStore.close(); }
    if (recordTrackEpochMarker) {
      const eventId = `track_epoch_resolving_${input.attemptId}`;
      const timestamp = new Date().toISOString();
      try {
        await this.invokeProtocol('session.append_event', {
          args: ['session', 'append-event', '--session-id', input.sessionId],
          storage,
          input: {
            id: eventId,
            kind: 'agent.event',
            timestamp,
            summary: 'Research track epoch resolved.',
            payload: { type: 'track_epoch_resolving', attemptId: input.attemptId, agentPath: '/root' },
            agentPath: '/root'
          }
        });
      } catch (error) {
        console.warn('Could not record research track epoch marker:', error);
      }
    }
  }

  public async workspaceMemory(
    workspaceIdentifier: string
  ): Promise<BealeAppServerCanonicalResult<BealeWorkspaceMemoryCatalog>> {
    const workspace = this.requireWorkspace(workspaceIdentifier);
    const result = await this.workspaceMemorySummary(workspace);
    return canonicalResult(workspace, pathFreeWorkspaceMemoryCatalog(workspace.workspaceId, result));
  }

  private async workspaceMemorySummary(workspace: AppServerHostWorkspace): Promise<unknown> {
    const result = await this.invokeProtocol<unknown>('memory.summary', {
      args: ['knowledge', 'summary'],
      input: {
        workspaceId: workspace.workspaceId,
        workspaceRoot: workspace.workspacePath,
        researchProfileId: workspace.researchProfileId
      },
      storage: this.registry.storageForProfile(workspace.researchProfileId)
    });
    return workspace.memoryBackend === 'disabled' ? withoutWorkspaceMemory(result) : result;
  }

  private async workspaceResearchTrackContext(workspace: AppServerHostWorkspace): Promise<{
    scope: { id: string; expiresAt: string | null } | null;
    targets: BealeAppServerWorkspaceDetails['targets'];
    surfaceMap: BealeWorkspaceSurfaceMap;
  }> {
    const state = await this.invokeProtocol<unknown>('workspace.state', {
      args: ['workspace', 'state'],
      input: {
        action: 'initialize',
        workspacePath: workspace.workspacePath,
        workspaceId: workspace.workspaceId,
        researchKitId: workspace.researchKitId,
        artifactRoot: join(workspace.workspacePath, '.beale', 'artifacts')
      },
      storage: this.registry.storageForProfile(workspace.researchProfileId)
    });
    const activeScope = isRecord(state) && isRecord(state.activeScope) ? state.activeScope : null;
    const scopeId = nonEmpty(activeScope?.id);
    const fleet = await this.fleet.state();
    const allowed = new Set(fleet.workspaceTargets[workspace.workspaceId] ?? ['local', 'browser:in-agent']);
    const recoveryBases = fleet.machines.filter((machine) => machine.canClone && machine.state === 'stopped'
      && machine.sshConfigured && !machine.owner && allowed.has(`vm:clone:${machine.id}`));
    const targets: BealeAppServerWorkspaceDetails['targets'] = [
      { id: 'local', name: 'Local', ready: true },
      ...fleet.browsers.filter((browser) => browser.enabled).map((browser) => ({
        id: `browser:${browser.id}`, name: browser.name, ready: fleet.enabled
      })),
      ...fleet.machines.flatMap((machine) => [
        ...(machine.canRun ? [{ id: `vm:run:${machine.id}`, name: `${machine.name} · Run`,
          ready: fleet.enabled && !machine.owner && (machine.sshConfigured && machine.state !== 'unknown'
            || recoveryBases.some((base) => base.guestPlatform === machine.guestPlatform)) }] : []),
        ...(machine.canClone ? [{ id: `vm:clone:${machine.id}`, name: `${machine.name} · Clone`, ready: fleet.enabled && machine.sshConfigured && machine.state === 'stopped' && !machine.owner }] : [])
      ])
    ].filter((target) => allowed.has(target.id));
    return {
      scope: scopeId
        ? { id: scopeId, expiresAt: nonEmpty(activeScope?.expiresAt) }
        : null,
      targets,
      surfaceMap: await this.workspaceSurfaceMap(workspace.workspaceId)
    };
  }

  public async workspaceDetails(workspaceIdentifier: string): Promise<BealeAppServerWorkspaceDetails> {
    const workspace = this.requireWorkspace(workspaceIdentifier);
    const storage = this.registry.storageForProfile(workspace.researchProfileId);
    const state = await this.invokeProtocol<unknown>('workspace.state', {
      args: ['workspace', 'state'],
      input: {
        action: 'initialize',
        workspacePath: workspace.workspacePath,
        workspaceId: workspace.workspaceId,
        researchKitId: workspace.researchKitId,
        artifactRoot: join(workspace.workspacePath, '.beale', 'artifacts')
      },
      storage
    });
    const memorySummary = await this.workspaceMemorySummary(workspace);
    const memory = pathFreeWorkspaceMemoryCatalog(workspace.workspaceId, memorySummary);
    const fleet = await this.fleet.state();
    const allowed = new Set(fleet.workspaceTargets[workspace.workspaceId] ?? ['local', 'browser:in-agent']);
    const availableTargets: BealeAppServerWorkspaceDetails['targets'] = [
      { id: 'local', name: 'Local', ready: true },
      ...fleet.browsers.filter((browser) => browser.enabled).map((browser) => ({
        id: `browser:${browser.id}`, name: browser.name, ready: fleet.enabled
      })),
      ...fleet.machines.flatMap((machine) => [
        ...(machine.canRun ? [{ id: `vm:run:${machine.id}`, name: `${machine.name} · Run`, ready: fleet.enabled && machine.sshConfigured && machine.state !== 'unknown' && !machine.owner }] : []),
        ...(machine.canClone ? [{ id: `vm:clone:${machine.id}`, name: `${machine.name} · Clone`, ready: fleet.enabled && machine.sshConfigured && machine.state === 'stopped' && !machine.owner }] : [])
      ])
    ];
    const { workspacePath: _workspacePath, workspaceDirectories: _workspaceDirectories, memoryBackend: _memoryBackend, ...summary } = workspace;
    return projectWorkspaceDetails(summary, state, memory, memorySummary,
      availableTargets.filter((target) => allowed.has(target.id)), await this.workspaceSurfaceMap(workspace.workspaceId));
  }

  public async workspaceSurfaceMap(workspaceIdentifier: string, sessionId?: string): Promise<BealeWorkspaceSurfaceMap> {
    const workspace = this.requireWorkspace(workspaceIdentifier);
    const storage = this.registry.storageForProfile(workspace.researchProfileId);
    return this.databaseCoordinator.runWhenAvailable(storage.databasePath, () => {
      const binding = resolveStoredResearchWorkspaceBinding({
        workspaceRoot: workspace.workspacePath,
        databasePath: storage.databasePath,
        researchProfileId: workspace.researchProfileId
      });
      if (binding.source === 'beale') {
        if (binding.memoryContext.workspaceId !== workspace.workspaceId) {
          throw new Error('The recorded workspace scope does not match this workspace.');
        }
        const resources = new ResearchResourceCatalog({
          databasePath: storage.databasePath,
          workspaceId: workspace.workspaceId,
          explicitResources: binding.resources,
          ...(binding.authorization ? { authorization: binding.authorization } : {})
        });
        resources.close();
      }
      return readWorkspaceSurfaceMap(storage.databasePath, workspace.workspaceId, sessionId);
    });
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
    options: { stream?: string; afterEventId?: string; beforeEventId?: string; tail?: boolean; limit?: number; maxBytes?: number }
  ): Promise<BealeAppServerCanonicalResult> {
    const stream = options.stream === 'transcript' || options.stream === 'trace' || options.stream === 'commentary'
      ? options.stream
      : 'all';
    return this.sessionQuery(workspaceIdentifier, sessionId, 'session.events', [
      'session', 'events', '--session-id', sessionId,
      '--stream', stream,
      ...(options.afterEventId ? ['--after-event-id', options.afterEventId] : []),
      ...(options.beforeEventId ? ['--before-event-id', options.beforeEventId] : []),
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

  private async resolvePluginRuntime(workspace?: AppServerHostWorkspace): Promise<ResolvedAppServerSessionLaunch['pluginRuntime'] | null> {
    return this.loadPluginRuntime(workspace);
  }

  private async loadPluginRuntime(workspace?: AppServerHostWorkspace): Promise<ResolvedAppServerSessionLaunch['pluginRuntime'] | null> {
    try {
      const runtime = await this.invokeProtocol<AppServerPluginRuntimeProjection>('plugin.runtime', {
        args: ['harness', 'plugin-runtime'],
        input: {
          registryDirectory: this.registry.registryDirectory,
          builtinPlugins: defaultBuiltinPlugins(),
          ...(workspace ? { workspaceId: workspace.workspaceId, researchKitId: workspace.researchKitId } : {})
        }
      });
      return {
        ...(Array.isArray(runtime.managedPluginIds) ? { managedPluginIds: stringArray(runtime.managedPluginIds) } : {}),
        ...(nonEmpty(runtime.pluginCatalogPath) ? { pluginCatalogPath: nonEmpty(runtime.pluginCatalogPath)! } : {}),
        skillDirectories: stringArray(runtime.skillDirs),
        selectedSkillIds: stringArray(runtime.selectedSkillIds),
        ...(nonEmpty(runtime.mcpConfigPath) ? { mcpConfigPath: nonEmpty(runtime.mcpConfigPath)! } : {}),
        allowedMcpServers: stringArray(runtime.allowedMcpServers)
      };
    } catch (error) {
      throw new Error('Managed plugin settings could not be loaded for this session.', { cause: error });
    }
  }

  private async loadIntrospectionPluginRuntime(): Promise<ResolvedAppServerSessionLaunch['pluginRuntime'] | null> {
    const plugin = builtinPlugin('beale-introspection-builtin', 'beale-introspection', false, 'feature');
    if (!existsSync(plugin.path)) {
      throw new Error('Beale Introspection is unavailable for Quick Chat.');
    }
    const standardRuntime = await this.resolvePluginRuntime();
    if (!standardRuntime?.allowedMcpServers?.includes('beale-introspection.beale')) {
      throw new Error('Enable Introspection in Agent Settings > Optional Features to use Quick Chat.');
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
      throw new Error('Beale Introspection did not provide the Quick Chat tool runtime.');
    }
    const managedPluginIds = standardRuntime.managedPluginIds;
    return {
      ...(managedPluginIds !== undefined ? { managedPluginIds } : {}),
      ...(nonEmpty(runtime.pluginCatalogPath) ? { pluginCatalogPath: nonEmpty(runtime.pluginCatalogPath)! } : {}),
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
      shellReviewEffort: nonEmpty(descriptor.shellReviewEffort) ?? 'low'
    };
  }

  private async ensureCanonicalSession(input: {
    sessionId: string;
    attemptId: string;
    workspace: AppServerHostWorkspace;
    storage: AppServerHostStorage;
    prompt: string;
    initialTitle?: string;
    providerId: string;
    model?: string;
    reasoningEffort?: string;
    workflowId?: string;
    repeatSchedule?: import('@beale/app-server-runtime/protocol').AppServerRepeatSchedule;
    machineId?: string;
    targetIds?: string[];
    goal?: { objective?: string };
    fastMode?: boolean;
    daybreakBlue?: boolean;
    collaboration?: Record<string, unknown>;
    shellSafetyMode?: string;
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
          title: input.initialTitle ?? generatedTitle(input.prompt),
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
            appServerRestartLaunch: input.restartLaunch,
            ...(input.repeatSchedule ? { shellSafetyMode: input.shellSafetyMode } : {}),
            ...(input.repeatSchedule || input.collaboration ? {
              bealeRun: { budget: {
                ...(input.repeatSchedule ? {
                  repeatSchedule: input.repeatSchedule,
                  machineId: input.machineId ?? 'local', targetIds: input.targetIds ?? ['local'],
                  goalEnabled: Boolean(input.goal), goalObjective: input.goal?.objective ?? null,
                  fastMode: input.fastMode === true, daybreakBlue: input.daybreakBlue === true
                } : {}),
                ...(input.collaboration ? { collaboration: input.collaboration } : {})
              } }
            } : {})
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
          ...(!input.continuation ? { prompt: input.prompt } : {}),
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
      momentum: { state: 'empty', reason: 'Workspace memory is disabled.', supportingNodeIds: [] },
      counts: { leads: 0, findings: 0, verifiedFindings: 0, disclosedFindings: 0, coverageGaps: 0, contradictions: 0 }
    },
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
    ...(nonEmpty(claim.surfaceId) ? { surfaceId: nonEmpty(claim.surfaceId) } : {}),
    sessionIds: stringArray([claim.originSessionId, ...evidence.flatMap((entry) => (
      entry && typeof entry === 'object' && !Array.isArray(entry) ? [(entry as Record<string, unknown>).sessionId] : []
    ))]),
    projection,
    maturity: nonEmpty(claim.maturity) ?? 'lead',
    status: nonEmpty(claim.status) ?? 'hypothesis',
    staleFromStatus: nonEmpty(claim.staleFromStatus),
    rating: researchClaimRating(claim.rating),
    classification,
    duplicateClaims: Array.isArray(claim.duplicateClaims)
      ? claim.duplicateClaims.flatMap(pathFreeWorkspaceClaimDuplicate)
      : [],
    title,
    summary: typeof claim.summary === 'string' ? claim.summary : '',
    impact: typeof claim.impact === 'string' ? claim.impact : '',
    stepsToReproduce: typeof claim.stepsToReproduce === 'string' ? claim.stepsToReproduce : '',
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
    maturity: nonEmpty(claim.maturity) ?? 'lead',
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
    cvssAssessments,
    affectedAssetIds: stringArray(tracking.affectedAssetIds),
    surfaceIds: stringArray(tracking.surfaceIds),
    progressBlocker: tracking.progressBlocker && typeof tracking.progressBlocker === 'object' && !Array.isArray(tracking.progressBlocker)
      ? { reason: nonEmpty((tracking.progressBlocker as Record<string, unknown>).reason) ?? '',
          setAt: nonEmpty((tracking.progressBlocker as Record<string, unknown>).setAt) ?? new Date(0).toISOString() }
      : null,
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
    ...(nonEmpty(node.surfaceId) ? { surfaceId: nonEmpty(node.surfaceId) } : {}),
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
      : 1,
    duplicateMemories: Array.isArray(node.duplicateMemories)
      ? node.duplicateMemories.flatMap(pathFreeWorkspaceMemoryDuplicate)
      : []
  }];
}

function pathFreeWorkspaceMemoryDuplicate(value: unknown): BealeWorkspaceMemoryNode['duplicateMemories'] {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return [];
  const duplicate = value as Record<string, unknown>;
  const id = nonEmpty(duplicate.id);
  const type = nonEmpty(duplicate.type);
  const title = nonEmpty(duplicate.title);
  const status = nonEmpty(duplicate.status);
  const markedAt = nonEmpty(duplicate.markedAt);
  if (!id || !type || !title || !status || !markedAt) return [];
  return [{
    id,
    type,
    title,
    status,
    revision: typeof duplicate.revision === 'number' && Number.isInteger(duplicate.revision) && duplicate.revision > 0
      ? duplicate.revision
      : 1,
    markedAt
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
    ...MANAGED_TOOL_PLUGIN_IDS.map((id) => builtinPlugin(`${id}-builtin`, id, false, 'feature')),
    builtinPlugin('beale-goals-builtin', 'beale-goals', false, 'feature'),
    builtinPlugin('beale-introspection-builtin', 'beale-introspection', false, 'feature'),
    ...bundledManagedPlugins()
  ].flatMap((plugin) => existsSync(plugin.path) ? [plugin] : []);
}

function bundledManagedPlugins(): ReturnType<typeof builtinPlugin>[] {
  const root = builtinPluginPath('', 'managed');
  if (!existsSync(root)) return [];
  return readdirSync(root, { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && existsSync(join(root, entry.name, 'plugin.json')))
    .map((entry) => ({
      ...builtinPlugin(`${entry.name}-builtin`, entry.name, true, 'managed'),
      installable: true,
      requiredByResearchKits: researchKitsRequiringBundledPlugin(entry.name)
    }));
}

function builtinPlugin(
  id: string,
  directory: string,
  disabledByDefault: boolean,
  source: 'managed' | 'feature'
): { id: string; path: string; installedAt: string; enabledByDefault?: boolean } {
  return {
    id,
    path: builtinPluginPath(directory, source),
    installedAt: '2026-08-21T00:00:00.000Z',
    ...(disabledByDefault ? { enabledByDefault: false } : {})
  };
}

function builtinPluginPath(directory: string, source: 'managed' | 'feature'): string {
  const resourcesPath = (process as NodeJS.Process & { resourcesPath?: string }).resourcesPath;
  const candidates = source === 'managed'
    ? [
        ...(resourcesPath
          ? [
              resolve(resourcesPath, 'managed-plugins', directory),
              resolve(resourcesPath, 'app-server', 'resources', 'managed-plugins', directory)
            ]
          : []),
        fileURLToPath(new URL(`../../managed-plugins/${directory}`, import.meta.url)),
        resolve(process.cwd(), '..', 'managed-plugins', directory),
        resolve(process.cwd(), 'managed-plugins', directory)
      ]
    : [
        ...(resourcesPath
          ? [
              resolve(resourcesPath, 'harness-features', directory),
              resolve(resourcesPath, 'resources', 'harness-features', directory)
            ]
          : []),
        fileURLToPath(new URL(`../resources/harness-features/${directory}`, import.meta.url)),
        resolve(process.cwd(), 'resources', 'harness-features', directory)
      ];
  return candidates.find((candidate) => existsSync(candidate)) ?? candidates.at(-1)!;
}

function generatedTitle(prompt: string): string {
  const normalized = prompt.trim().replace(/\s+/gu, ' ');
  return normalized.slice(0, 120) || 'Beale research session';
}

function initialResearchTrackTitle(assignment: { nextAction: string; theory: string; surfaceName: string }): string {
  const text = assignment.nextAction || assignment.theory || assignment.surfaceName;
  return text.trim().replace(/\s+/gu, ' ').split(' ').slice(0, 6).join(' ').slice(0, 80) || 'Research Session';
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
    daybreakBlue?: boolean;
    profileId: string;
  }
): StoredRestartLaunchDescriptor {
  return {
    schemaVersion: 1,
    eligible: request.launch.introspection === undefined
      || request.launch.introspection.runtimeMode === 'standard',
    launch: {
      workspaceId: request.launch.workspaceId,
      ...(request.launch.machineId ? { machineId: request.launch.machineId } : {}),
      ...(request.launch.targetIds ? { targetIds: request.launch.targetIds } : {}),
      promptMarkdown: request.launch.promptMarkdown,
      ...(request.launch.goal ? { goal: request.launch.goal } : {}),
      provider: {
        id: resolved.providerId,
        ...(resolved.model ? { model: resolved.model } : {}),
        ...(resolved.reasoningEffort ? { reasoningEffort: resolved.reasoningEffort } : {}),
        ...(resolved.fastMode ? { fastMode: true } : {}),
        ...(resolved.daybreakBlue ? { daybreakBlue: true } : {})
      },
      shellSafetyMode: request.launch.shellSafetyMode?.trim() || 'auto_review',
      guardianEnabled: request.launch.guardianEnabled !== false,
      ...(request.launch.devHarnessFeedback ? { devHarnessFeedback: true } : {}),
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
  const targetId = typeof budget.machineId === 'string' ? budget.machineId : 'local';
  const targetIds = Array.isArray(budget.targetIds) && budget.targetIds.length > 0
    && budget.targetIds.every((id: unknown) => typeof id === 'string')
    ? budget.targetIds as string[] : [targetId];
  if (targetIds.some((id) => id !== 'local' && !id.startsWith('browser:'))) return null;
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
  const daybreakBlue = providerId === 'openai-codex';
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
        machineId: targetIds.length > 1 ? 'local' : targetId,
        targetIds,
        ...(targetIds.some((id) => id.startsWith('browser:')) ? { browserRelay: true } : {}),
        promptMarkdown,
        ...(goalEnabled ? { goal: { objective: goalObjective } } : {}),
        ...(providerId || model || reasoningEffort || fastMode || daybreakBlue ? {
          provider: {
            ...(providerId ? { id: providerId } : {}),
            ...(model ? { model } : {}),
            ...(reasoningEffort ? { reasoningEffort } : {}),
            ...(fastMode ? { fastMode: true } : {}),
            ...(daybreakBlue ? { daybreakBlue: true } : {})
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

function guardianEnabledForTrackSession(session: AppServerSessionSummaryProjection): boolean {
  const restart = decodeRestartLaunchDescriptor(session.metadata);
  return restart?.launch.mode === 'research-track' && restart.launch.guardianEnabled === true;
}

function latestAttempt(value: unknown): { id: string; parentAttemptId: string | null; startedAt: string | null } | null {
  if (!Array.isArray(value)) return null;
  for (let index = value.length - 1; index >= 0; index -= 1) {
    const attempt = value[index];
    if (!isRecord(attempt)) continue;
    const id = nonEmpty(attempt.id);
    if (id) return { id, parentAttemptId: nonEmpty(attempt.parentAttemptId), startedAt: nonEmpty(attempt.startedAt) };
  }
  return null;
}

function hasRecentTrackSessionActivity(
  trackUpdatedAt: string,
  session: AppServerSessionSummaryProjection,
  at: Date,
): boolean {
  const recentAfter = at.getTime() - 120_000;
  return [trackUpdatedAt, nonEmpty(session.updatedAt), nonEmpty(session.lastMessageAt),
    latestAttempt(session.attempts)?.startedAt].some((value) => {
    if (!value) return false;
    const timestamp = Date.parse(value);
    return Number.isFinite(timestamp) && timestamp > recentAfter;
  });
}

function manualContinuationFallbackPrompt(
  session: AppServerSessionSummaryProjection,
  value: unknown,
  instruction: string
): string {
  const originalRequest = nonEmpty(session.prompt) ?? '';
  const turns = Array.isArray(value)
    ? value.flatMap((event) => {
        if (!isRecord(event) || !isRecord(event.payload) || !isRecord(event.payload.record)) return [];
        const record = event.payload.record;
        const content = nonEmpty(record.contentMarkdown);
        if (!content) return [];
        const role = nonEmpty(record.role) ?? 'unknown';
        const metadata = isRecord(record.metadata) ? record.metadata : null;
        const agentPath = nonEmpty(metadata?.agentPath);
        const label = role === 'assistant'
          ? `Agent${agentPath && agentPath !== '/root' ? ` ${agentPath}` : ''}`
          : role === 'user' ? 'User' : 'System';
        return [`${label}:\n${content}`];
      })
    : [];
  const contextLimit = 32_000;
  const retained: string[] = [];
  let retainedLength = Math.min(originalRequest.length, contextLimit / 2);
  for (let index = turns.length - 1; index >= 0; index -= 1) {
    const turn = turns[index];
    if (!turn) continue;
    const remaining = contextLimit - retainedLength;
    if (remaining <= 0) break;
    retained.unshift(turn.length > remaining ? `[Earlier content omitted]\n${turn.slice(-remaining)}` : turn);
    retainedLength += Math.min(turn.length, remaining);
  }
  return [
    '# Continue the existing Beale research session',
    '',
    'Continue from the prior durable session state. Preserve established facts, decisions, explored paths, and tool-backed observations. Do not restart the investigation or treat this as turn 1.',
    '',
    '## New steering instruction',
    instruction,
    '',
    '## Existing session context',
    `Original request:\n${originalRequest.slice(0, contextLimit / 2)}`,
    ...(retained.length > 0 ? ['', ...retained] : [])
  ].join('\n');
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

function researchTrackEpochPrompt(
  trackId: string,
  pass: number,
  epoch: number,
  surface: ResearchTrackSurfaceProjection,
  work: ResearchTrackSurfaceState,
  focus: ResearchTrackEpochFocus,
  surfaceCount: number,
  targetIds: readonly string[],
  contextReview: ResearchTrackContextDecision | undefined,
  collaborationEnabled: boolean,
  researchKitId?: string,
  reviewInterruptedEpoch = false
): string {
  const linkedClaims = (ids: readonly string[] | undefined): string => !ids?.length ? 'none'
    : `${ids.slice(0, 6).join(', ')}${ids.length > 6 ? `, and ${ids.length - 6} more` : ''}`;
  const ranking = surface.foundation?.ranking;
  return [
    `# Continuous research track epoch ${epoch}`,
    `Track ${trackId}, pass ${pass}. The workspace currently contains ${surfaceCount} eligible security surfaces.`,
    `Assigned surface: ${surface.name} (${surface.id}).`,
    `Epoch focus: ${focus}.`,
    ...(contextReview ? [contextReview.mode === 'resume'
      ? 'Context transition: use retained model history when available. This epoch assignment supersedes earlier surface and work-direction instructions; use current canonical records when prior notes conflict.'
      : `Context transition: start with fresh model context after ${contextReview.reason.replaceAll('_', ' ')}. Durable research state remains available; follow this assignment without repeating broad orientation.`] : []),
    `Work direction: ${focus === 'depth'
      ? 'advance a specific attacker path, lead, or finding toward a decisive result; use this component in its parent context when relevant'
      : 'explore an underexamined resource or component for distinct attacker-controlled paths and trust boundaries'}.`,
    `Linked open leads: ${linkedClaims(surface.coverageState.openLeadIds)}. Findings needing resolution: ${linkedClaims(surface.coverageState.unblockedFindingIds)}.`,
    `Prior impact estimate: ${work.impact}. Prior theory: ${work.theory || 'Unexamined.'}`,
    `Current actionability: ${work.actionabilityScore}/100.`,
    `Current surface factors: ${ranking
      ? (['impact', 'reachability', 'relevance', 'exposure', 'precedence'] as const)
        .map((factor) => `${factor} ${ranking[factor].score}/10`).join(', ')
      : 'unscored'}.`,
    'When the surface is understood well enough, record all five 1–10 ranking factors (impact, reachability, relevance, exposure, precedence) with a concrete basis in surface.update foundation.ranking. Leave it unscored while important inputs are unknown; revise scores when evidence changes.',
    `Prior validation: ${work.validation ? `${work.validation.environment}; target-dependent=${work.validation.targetDependent}; ${work.validation.reason}` : 'No target-validation result recorded.'}`,
    `Next action: ${work.nextAction}`,
    ...(work.angles.length ? [
      'Prior attacker paths:',
      ...work.angles.map((angle) => `- ${angle.name} [${angle.status}]: ${angle.vector
        ? `Control: ${angle.vector.attackerControl}; boundary: ${angle.vector.trustBoundary}; possible failure: ${angle.vector.failureMode}; effect: ${angle.vector.unauthorizedEffect}.`
        : 'Causal vector not yet recorded; map it before relying on this path.'} Proof: ${angle.proofQuestion} Next: ${angle.nextAction} Evidence: ${angle.evidenceRefs.join(', ') || 'none'}.`)
    ] : ['No attacker paths have been recorded yet.']),
    `Selected execution targets: ${targetIds.join(', ')}.`,
    ...(reviewInterruptedEpoch ? [
      'Guardian recovery review: The prior worker ended without a terminal session record. Inspect the available prior attempt and durable workspace state before continuing. Identify work that was recorded, work that remains unverified, and the next safe proof step. Do not infer progress from the missing terminal record or repeat completed work merely to reconstruct context. Record the recovery assessment in the next durable checkpoint.'
    ] : []),
    '',
    'Conduct one bounded research epoch on the assigned component. Inspect its existing notes and relevant evidence, then pursue the most promising authorized question. Map concrete subcomponents with stable noun names when that structure helps future work. Keep attack theories, experiments, and unanswered questions in notes or research records rather than adding them as surfaces. Use surface.focus to associate work with the component when useful. Compare alternative explanations and capture the observation that would distinguish them. Check nearby resources when they materially affect the current theory.',
    ...(focus === 'depth' ? [
      'Depth epoch: inspect the linked leads and findings, using surface.views for the full Recon and Analysis lists when the IDs above are truncated; use claim.get for candidate evidence and history. Pick the highest-value unresolved proof obligation. Establish attacker control, target reachability, and the unauthorized effect with a discriminating experiment where possible. Move a supported claim through the evidence and review gates; narrow, reject, or record a concrete blocker for a candidate when evidence warrants it. If this component has no linked claim, deepen one specific attacker path from the existing notes and checkpoint instead of repeating a general map. Finish with the exact remaining proof question.'
    ] : [
      'Breadth epoch: look for a distinct, in-scope path that the current board does not cover. Record a concrete lead when the attacker control, boundary, and possible effect form a testable theory. Add a surface only for a real component, not for a hypothesis. Avoid extending the inventory after the best new path is clear; leave its proof question ready for a depth epoch.'
    ]),
    ...(collaborationEnabled ? [
      `Track collaboration: keep one complementary ${focus === 'breadth' ? 'depth' : 'breadth'} subagent active while useful work and capacity permit. Check active agents first and continue a suitable assignment rather than duplicating it. ${focus === 'breadth'
        ? 'Give a distinct collaborator one existing lead, finding, or recorded attacker path to advance with a bounded proof or review question; in advanced mode, use a Prover for reproduction or a Reviewer only when independent verification is due.'
        : 'Give a breadth scout a distinct underexamined in-scope surface to search for new attacker paths and leads; use the Discoverer role in advanced mode.'} Include the surface ID and canonical claim IDs when relevant. The lead alone records the track checkpoint for its assigned surface; collaborators record durable research and report results to the lead. Continue the primary epoch while they run, reconcile their durable results, and resolve active delegates before the final response. If no concrete complementary task or capacity exists, proceed with the primary work.`
    ] : []),
    'The listed resources are an inventory, not the whole authorization boundary. When source research reveals a distinct covered daemon, service, executable, API, or SDK, record it with resource.catalog discover and request Auto-Reviewed touch before substantive research. Do not leave a separate in-scope target only as a note beneath the current repository surface: an approved discovered resource becomes eligible for a later track epoch. Record the scope link; explicit exclusions still control. Official vendor APIs and SDKs may be covered even when their source was not attached. Third-party applications, customers, and accounts are not authorized just because they integrate with an in-scope vendor. Treat sample implementations as context unless independently included; prioritize the vendor-owned API or SDK behavior and impacts.',
    'For a web-facing theory, use available browser tools to inspect the actual application behavior and test discriminating requests within the recorded authorization. Browser tools are exposed when the Browser feature is enabled; if no usable browser target exists, record that limit in the surface checkpoint. Do not substitute a broad crawl for threat-model reasoning.',
    'Favor concrete source-to-sink reasoning and discriminating experiments over inventory scans. Follow privilege changes, object ownership, state transitions, and cross-component handoffs beyond obvious entry points. For the chosen path, state the observation that would support the suspected failure, an alternative harmless explanation, and a control or comparison that separates them. A missing prerequisite or unreachable target leaves the path open or blocked; it does not refute the mechanism. Search existing findings before creating a new one. Advance a candidate through the existing evidence, reproduction, independent-review, and report gates when authorized work can do so. A negative result must name its bounds and be recorded in durable memory before it supports ruling out an attacker path.',
    'For target-dependent theories, prioritize a discriminating test on the authorized target or faithful device/VM. Localhost is useful for building harnesses and source-level checks, but a localhost run alone does not validate target behavior. Check target availability once, then use the target where possible. If unavailable, complete bounded source-level work that can still change the theory, record the exact target-validation gap in surface.update and the checkpoint next action, and defer to another in-scope surface when no distinct accessible proof step remains. Do not repeatedly run localhost proxies for the same blocked target question.',
    ...(researchKitId === 'apple-security-bounty' ? [
      'For iOS-specific behavior, inspect the Apple security device capability and use an authorized physical iPhone when one is available. Listing or describing a device is availability checking, not testing: when the theory can be exercised through an operator-built app, use the physical-iPhone install and launch tools and capture the observed result. The iOS Simulator is not a supported substitute. If device testing cannot be performed, state why in surface.update and the checkpoint next action; do not infer a negative on-device result from host tests.'
    ] : []),
    'Checkpoint at a useful stopping point, including when a proof reaches a result or needs an external prerequisite. Record a short assessment and a useful next action with track.checkpoint. For evidenceRefs, cite a parent workspace record as finding:<claim ID>, memory:<memory ID>, runbook:<runbook ID>, or report:<report ID>; do not use an evidence entry ID, trace ID, artifact ID, or file path. Use an empty list when no canonical record exists. Attacker-path evidenceRefs are included in the checkpoint automatically. Add structured attacker-path updates or validation details when they clarify the work; they are not required for every checkpoint. A bounded review can complete with open leads or unfinished child components. Use blocked only for a concrete external dependency; otherwise defer with a useful next step. After a successful checkpoint, end the epoch with only this neutral final sentence: "Epoch checkpoint recorded." Do not recap findings, evidence, IDs, or next steps in that final response; the durable records carry those details. If the checkpoint cannot be recorded, state the failure briefly instead.',
  ].join('\n');
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

function fleetOwner(input: Record<string, unknown>, defaultMachineId: string | null): { machineId: string; sessionId: string } | null {
  const sessionId = nonEmpty(input.sessionId);
  if (!sessionId) return null;
  const machineId = nonEmpty(input.ownerMachineId) ?? defaultMachineId;
  if (!machineId) throw new Error('Fleet owner machine ID is required.');
  return { machineId, sessionId };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function workspaceIdentifierFromInput(value: unknown): string | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  return nonEmpty((value as Record<string, unknown>).workspaceId);
}

function persistenceStorageFromInput(value: unknown): AppServerHostStorage {
  if (!isRecord(value)) throw new Error('Workspace persistence input is required.');
  const databasePath = nonEmpty(value.databasePath);
  const artifactDirectoryPath = nonEmpty(value.artifactDirectoryPath);
  if (!databasePath || !artifactDirectoryPath) {
    throw new Error('Workspace persistence storage paths are required.');
  }
  return { databasePath: resolve(databasePath), artifactDirectoryPath: resolve(artifactDirectoryPath) };
}

function boundedInteger(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, Math.floor(value)));
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
