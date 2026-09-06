import { randomUUID } from 'node:crypto';
import {
  CampaignTrackStore,
  MemoryGraphStore,
  ResearchClaimStore,
  ResearchResourceCatalog,
  ReportStore,
  RunbookStore,
  createCampaignTrackTools,
  createFindingTools,
  createMemoryGraphTools,
  createResearchResourceTool,
  createReportTools,
  createResearchStorageLayout,
  createResearchToolRegistry,
  createRunbookExecutionTool,
  createRunbookExecutor,
  createRunbookTools,
  createShellAuditCommand,
  createShellTool,
  createWorkspaceHistoryDuplicateTools,
  createWorkspaceHistorySearchTool,
  evaluateShellNetworkAuthorization,
  resolveStoredResearchProfile,
  resolveStoredResearchWorkspaceBinding,
  type ModelAuthor,
  type ResearchExecutableTool,
  type ResearchToolDescriptor,
  type ResearchToolExecutionRecord
} from '@beale/app-server-runtime/runtime-services';
import type { AppServerProtocolStorage } from './appServerProtocolClient.js';

export interface AppServerResearchToolContext {
  workspaceId: string;
  workspaceName: string;
  workspaceRoot: string;
  researchProfileId: string;
  memoryBackend: 'app-server' | 'disabled';
  sessionId?: string;
  investigationId?: string;
  objective?: string;
  modelAuthor?: ModelAuthor;
}

export interface AppServerResearchToolCall extends AppServerResearchToolContext {
  toolName: string;
  toolInput: Record<string, unknown>;
}

export interface AppServerResearchToolCatalog {
  workspaceId: string;
  researchProfileId: string;
  tools: ResearchToolDescriptor[];
}

export async function listAppServerResearchTools(
  input: AppServerResearchToolContext,
  storage: AppServerProtocolStorage
): Promise<AppServerResearchToolCatalog> {
  const runtime = await createResearchToolBridgeRuntime(input, storage);
  try {
    return {
      workspaceId: input.workspaceId,
      researchProfileId: input.researchProfileId,
      tools: runtime.registry.listDescriptors()
    };
  } finally {
    await runtime.close();
  }
}

export async function callAppServerResearchTool(
  input: AppServerResearchToolCall,
  storage: AppServerProtocolStorage,
  expectedEffect: 'read' | 'mutating',
  signal?: AbortSignal
): Promise<ResearchToolExecutionRecord> {
  if (typeof input.toolName !== 'string' || !input.toolName.trim()) {
    throw new Error('A research tool name is required.');
  }
  if (!isRecord(input.toolInput)) throw new Error('Research tool input must be an object.');
  const runtime = await createResearchToolBridgeRuntime(input, storage);
  try {
    const tool = runtime.registry.find(input.toolName);
    if (!tool) throw new Error(`Research tool is unavailable in this workspace: ${input.toolName}`);
    const isRead = tool.descriptor.sideEffects === 'none' || tool.descriptor.sideEffects === 'read';
    if ((expectedEffect === 'read') !== isRead) {
      throw new Error(
        isRead
          ? `${tool.descriptor.name} is read-only; invoke it through the read research-tool operation.`
          : `${tool.descriptor.name} has ${tool.descriptor.sideEffects} side effects; invoke it through the mutating research-tool operation.`
      );
    }
    return await runtime.registry.execute({
      id: `codex_${randomUUID()}`,
      toolName: tool.descriptor.name,
      actionClass: tool.descriptor.actionClasses[0] ?? (isRead ? 'recall' : 'synthesize'),
      input: input.toolInput
    }, {
      ...(signal ? { signal } : {}),
      agentId: 'codex',
      modelAuthor: input.modelAuthor ?? { provider: 'openai-codex', model: 'codex' }
    });
  } finally {
    await runtime.close();
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

async function createResearchToolBridgeRuntime(
  input: AppServerResearchToolContext,
  storage: AppServerProtocolStorage
): Promise<{
  registry: ReturnType<typeof createResearchToolRegistry>;
  close(): Promise<void>;
}> {
  const resolvedProfile = await resolveStoredResearchProfile({
    workspaceRoot: input.workspaceRoot,
    databasePath: storage.databasePath,
    researchProfileId: input.researchProfileId
  });
  if (resolvedProfile.profile.id !== input.researchProfileId) {
    throw new Error(
      `Workspace ${input.workspaceId} uses research profile ${input.researchProfileId}, not ${resolvedProfile.profile.id}.`
    );
  }
  const binding = resolveStoredResearchWorkspaceBinding({
    workspaceRoot: input.workspaceRoot,
    databasePath: storage.databasePath,
    researchProfileId: input.researchProfileId,
    ...(input.sessionId ? { externalSessionId: input.sessionId } : {})
  });
  if (binding.memoryContext.workspaceId !== input.workspaceId) {
    throw new Error('The stored workspace binding did not match the requested workspace.');
  }
  const context = {
    ...binding.memoryContext,
    workspaceId: input.workspaceId,
    workspaceName: input.workspaceName,
    ...(input.sessionId ? { sessionId: input.sessionId } : {})
  };
  const layout = createResearchStorageLayout({
    databasePath: storage.databasePath,
    artifactDirectoryPath: storage.artifactDirectoryPath
  });
  const memoryGraph = new MemoryGraphStore({
    workspaceRoot: input.workspaceRoot,
    databasePath: storage.databasePath,
    context,
    resolvedProfile
  });
  const findingStore = new ResearchClaimStore(memoryGraph);
  const close: Array<() => void | Promise<void>> = [
    () => findingStore.close(),
    () => memoryGraph.close()
  ];
  try {
    const tools: ResearchExecutableTool[] = [];
    const memoryActive = input.memoryBackend !== 'disabled' && resolvedProfile.profile.capabilities.memoryEnabled;

    if (memoryActive) {
      tools.push(...createMemoryGraphTools(memoryGraph));
      tools.push(...createFindingTools(findingStore, {
        classifications: resolvedProfile.profile.claims.classifications.map((classification) => classification.id)
      }));
    }

    let runbookStore: RunbookStore | undefined;
    if (resolvedProfile.profile.capabilities.runbooksEnabled) {
      runbookStore = new RunbookStore(storage.databasePath, layout, context);
      close.unshift(() => runbookStore?.close());
      tools.push(...createRunbookTools(runbookStore));
      const shellTool = createShellTool({
        workspaceRoot: input.workspaceRoot,
        authorize: async (request) => ({
          approvalRequestId: `codex_${randomUUID()}`,
          actionId: request.actionId,
          mode: 'manual_approval',
          decision: 'approved',
          source: 'policy',
          reason: 'The command was admitted through the Codex MCP tool approval boundary.',
          command: createShellAuditCommand(request),
          network: evaluateShellNetworkAuthorization(request)
        })
      });
      tools.push(createRunbookExecutionTool(createRunbookExecutor({ store: runbookStore, shellTool })));
    }

    if (memoryActive || runbookStore) {
      const historyOptions = {
        ...(memoryActive ? { memoryStore: memoryGraph, claimStore: findingStore } : {}),
        ...(runbookStore ? { runbookStore } : {})
      };
      tools.push(createWorkspaceHistorySearchTool(historyOptions));
      tools.push(...createWorkspaceHistoryDuplicateTools(historyOptions));
    }

    if (resolvedProfile.profile.capabilities.reportsEnabled
      && (resolvedProfile.profile.id !== 'security-research' || memoryActive)) {
      const reports = new ReportStore(storage.databasePath, layout, context, {
        packetCandidateRoots: [input.workspaceRoot]
      });
      close.unshift(() => reports.close());
      tools.push(...createReportTools(reports, {
        ...(resolvedProfile.profile.id === 'security-research'
          ? { requireConfirmedChain: true, requireSubmissionPacket: true, claimStore: findingStore }
          : {})
      }));
    }

    if (memoryActive && input.investigationId) {
      const investigations = new CampaignTrackStore({
        databasePath: storage.databasePath,
        context,
        memoryGraph,
        claimStore: findingStore
      });
      if (!investigations.detail(input.investigationId)) {
        investigations.close();
        throw new Error(`Campaign track not found in this workspace: ${input.investigationId}`);
      }
      close.unshift(() => investigations.close());
      tools.push(...createCampaignTrackTools(investigations, input.investigationId));
    }

    const resources = new ResearchResourceCatalog({
      databasePath: storage.databasePath,
      workspaceId: input.workspaceId,
      explicitResources: binding.resources ?? []
    });
    close.unshift(() => resources.close());
    tools.push(createResearchResourceTool({
      catalog: resources,
      ...(binding.researchKitId ? { researchKitId: binding.researchKitId } : {}),
      authorizationRecorded: binding.authorization?.recorded === true,
      ...(input.objective ? { campaignObjective: input.objective } : {}),
      authorizeScopeRelevance: async (request) => request.resource.direction === 'in_scope'
        ? { decision: 'relevant', source: 'policy', reason: 'The resource is explicitly in the recorded authorized scope.' }
        : { decision: 'not_relevant', source: 'policy', reason: 'Ambient resources require review and scope confirmation in Beale before first touch.' }
    }));

    return {
      registry: createResearchToolRegistry(tools),
      async close() {
        for (const cleanup of close) await cleanup();
      }
    };
  } catch (error) {
    for (const cleanup of close) await cleanup();
    throw error;
  }
}
