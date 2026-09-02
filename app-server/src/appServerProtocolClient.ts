import {
  AppServerSessionStore,
  ResearchChannelStore,
  AgentPluginRegistry,
  BUNDLED_RESEARCH_PROFILE_IDS,
  CampaignTrackStore,
  MemoryGraphStore,
  ResearchClaimStore,
  RunbookStore,
  attentionHeatForClaim,
  attentionHeatForMemoryNode,
  RESEARCH_PROFILE_SCHEMA_VERSION,
  ReportStore,
  completeAuxiliaryText,
  createResearchStorageLayout,
  extractSourceRepositoryUrls,
  getAppServerMemorySummary,
  listAppServerReportSummaries,
  getAuthStatus,
  getProviderModelCatalog,
  getKnowledgeReport,
  getKnowledgeRunbook,
  expandStoredResearchPrompt,
  generateStoredResearchGoalSuggestions,
  getWorkspaceDejunkSummary,
  materializeGitRepositoryAsync,
  migrateWorkspaceResearchClaims,
  normalizeSourceRepositoryUrl,
  listAuthProviders,
  parseMemoryDreamingPlanOutput,
  prepareMemoryDreamingRequest,
  providerSemanticsDescriptor,
  recordFailedMemoryDreaming,
  resolveAuxiliaryModelRoute,
  resolveResearchProfile,
  resolveKnowledgeArtifact,
  resolveStoredResearchProfile,
  resolveStoredResearchWorkspaceBinding,
  restoreMemoryDreamingChange,
  logoutAuthProvider,
  verifyProviderAuth,
  runMemoryDreaming,
  runWorkspaceDejunk,
  runWorkspaceDejunkAndConsolidateRepositories,
  selectSourceRepository,
  selectStoredResearchGoalSuggestion,
  sourceRepositoryCandidates,
  type AgentPluginRecord,
  type BeginAppServerSessionAttemptInput,
  type BuiltinAgentPluginDefinition,
  type CreateAppServerSessionInput,
  type AppServerSessionEvent,
  type AppServerSessionTransitionInput,
  type MemoryDreamingPlan,
  type MemoryDreamingProfileInput,
  type MemoryDreamingRunContext,
  type ResearchProfileSnapshot,
  type SourceRepositoryCandidate
} from '@beale/app-server-runtime/runtime-services';
import {
  appServerProtocolDescriptor,
  type BealeMemoryNotificationFeed,
  type AppServerProtocolOperation
} from '@beale/app-server-runtime/protocol';
import { WorkspaceDatabase } from './workspaceDatabase.js';
import { WorkspaceRegistry } from './workspaceRegistryStore.js';

export interface AppServerProtocolStorage {
  databasePath: string;
  artifactDirectoryPath: string;
}

export interface InvokeAppServerProtocolOptions {
  args: readonly string[];
  storage?: AppServerProtocolStorage;
  input?: unknown;
  timeoutMs?: number;
  signal?: AbortSignal;
}

/** Execute an allowlisted app-server operation inside the app-server process. */
export async function invokeAppServerProtocol<T>(
  operation: AppServerProtocolOperation,
  options: InvokeAppServerProtocolOptions
): Promise<T> {
  return await invokeOperation(operation, options) as T;
}

async function invokeOperation(operation: AppServerProtocolOperation, options: InvokeAppServerProtocolOptions): Promise<unknown> {
  if (operation === 'protocol.describe') return appServerProtocolDescriptor();
  if (operation === 'profile.resolve') return profileOperation(options);
  if (operation.startsWith('auth.') || operation === 'model.list') return authOperation(operation, options);
  if (operation.startsWith('tools.') || operation.startsWith('config.')) {
    const { executeHostedUtilityOperation } = await import('@beale/app-server-runtime/runtime');
    return executeHostedUtilityOperation(
      operation as 'tools.list' | 'tools.config' | 'config.show' | 'config.set',
      options.args
    );
  }
  if (operation.startsWith('session.')) return sessionOperation(operation, options);
  if (operation.startsWith('channel.')) return channelOperation(operation, options);
  if (operation === 'provider.complete') return completeAuxiliaryText(requiredRecord(options.input, 'completion input') as never);
  if (operation === 'provider.describe') return providerSemanticsDescriptor();
  if (operation === 'model_job.resolve') return resolveAuxiliaryModelRoute(requiredRecord(options.input, 'model job input') as never);
  if (operation === 'prompt.expand') {
    return expandStoredResearchPrompt(
      requiredRecord(options.input, 'research prompt expansion input') as never,
      options.signal ? { signal: options.signal } : {}
    );
  }
  if (operation === 'suggestion.generate') {
    return generateStoredResearchGoalSuggestions(
      requiredRecord(options.input, 'research goal suggestion input') as never,
      options.signal ? { signal: options.signal } : {}
    );
  }
  if (operation === 'suggestion.select') {
    selectStoredResearchGoalSuggestion(requiredRecord(options.input, 'research goal suggestion selection') as never);
    return { selected: true };
  }
  if (operation === 'workspace.state') return workspaceStateOperation(options);
  if (operation === 'registry.state') return registryStateOperation(options);
  if (operation.startsWith('memory.') || operation.startsWith('dreaming.')
    || operation.startsWith('history.')
    || operation.startsWith('claim.')
    || operation.startsWith('investigation.')
    || operation === 'runbook.get' || operation.startsWith('report.')
    || operation === 'artifact.resolve') {
    return knowledgeOperation(operation, options);
  }
  return harnessOperation(operation, options);
}

function channelOperation(operation: AppServerProtocolOperation, options: InvokeAppServerProtocolOptions): unknown {
  const storage = requiredStorage(options.storage);
  const store = new ResearchChannelStore({ databasePath: storage.databasePath });
  try {
    const input = isRecord(options.input) ? options.input : {};
    const workspaceId = requiredText(input.workspaceId ?? option(options.args, '--workspace-id'), 'workspaceId');
    const channel = optionalText(input.channel ?? option(options.args, '--channel'));
    switch (operation) {
      case 'channel.list': return store.list(
        workspaceId,
        integerOption(options.args, '--limit') ?? 200,
        input.archived === true || option(options.args, '--archived') === 'true'
      );
      case 'channel.get': {
        const detail = store.get(workspaceId, requiredText(channel, 'channel'), integerOption(options.args, '--message-limit') ?? 500);
        if (!detail) throw new Error(`Channel not found in workspace: ${String(channel)}`);
        return detail;
      }
      case 'channel.create': return store.create({
        workspaceId,
        name: requiredText(input.name, 'name'),
        ...(optionalText(input.title) ? { title: optionalText(input.title)! } : {}),
        topic: requiredText(input.topic, 'topic'),
        createdBySessionId: optionalText(input.sessionId),
        createdByAgentPath: optionalText(input.agentPath) ?? '/human'
      });
      case 'channel.join': return store.join({
        workspaceId,
        channel: requiredText(channel, 'channel'),
        sessionId: optionalText(input.sessionId),
        agentId: optionalText(input.agentId),
        agentPath: requiredText(input.agentPath, 'agentPath'),
        provider: optionalText(input.provider),
        model: optionalText(input.model),
        role: optionalText(input.role) ?? 'researcher'
      });
      case 'channel.post': return store.append({
        workspaceId,
        channel: requiredText(channel, 'channel'),
        sessionId: optionalText(input.sessionId),
        attemptId: optionalText(input.attemptId),
        agentId: optionalText(input.agentId),
        agentPath: optionalText(input.agentPath) ?? '/human',
        provider: optionalText(input.provider),
        model: optionalText(input.model),
        role: optionalText(input.role) ?? (optionalText(input.agentPath) ? 'researcher' : 'human'),
        kind: input.kind as never,
        contentMarkdown: requiredText(input.contentMarkdown, 'contentMarkdown'),
        evidenceRefs: Array.isArray(input.evidenceRefs) ? input.evidenceRefs as string[] : [],
        metadata: isRecord(input.metadata) ? input.metadata : {}
      });
      case 'channel.share': return store.share({
        workspaceId,
        channel: requiredText(channel, 'channel'),
        sessionId: optionalText(input.sessionId),
        attemptId: optionalText(input.attemptId),
        agentId: optionalText(input.agentId),
        agentPath: optionalText(input.agentPath) ?? '/human',
        provider: optionalText(input.provider),
        model: optionalText(input.model),
        role: optionalText(input.role) ?? (optionalText(input.agentPath) ? 'researcher' : 'human'),
        kind: input.kind as never,
        resourceId: requiredText(input.resourceId, 'resourceId'),
        title: requiredText(input.title, 'title'),
        ...(optionalText(input.note) ? { note: optionalText(input.note)! } : {})
      });
      case 'channel.archive': return store.archive(workspaceId, requiredText(channel, 'channel'));
      case 'channel.restore': return store.restore(workspaceId, requiredText(channel, 'channel'));
      case 'channel.delete': return store.delete(workspaceId, requiredText(channel, 'channel'));
      default: throw new Error(`Unsupported app-server channel operation: ${operation}`);
    }
  } finally {
    store.close();
  }
}

async function profileOperation(options: InvokeAppServerProtocolOptions): Promise<unknown> {
  const workspaceRoot = option(options.args, '--workspace-root') ?? process.cwd();
  const profilePath = option(options.args, '--profile');
  const profileId = option(options.args, '--profile-id');
  if (profilePath && profileId) throw new Error('--profile and --profile-id cannot be used together.');
  if (profileId && !BUNDLED_RESEARCH_PROFILE_IDS.includes(profileId as never)) {
    throw new Error(`--profile-id must be one of: ${BUNDLED_RESEARCH_PROFILE_IDS.join(', ')}.`);
  }
  const resolved = await resolveResearchProfile({
    workspaceRoot,
    ...(profilePath ? { profilePath } : {}),
    ...(profileId ? { bundledProfileId: profileId as (typeof BUNDLED_RESEARCH_PROFILE_IDS)[number] } : {})
  });
  return {
    catalogProtocolVersion: 1,
    supportedResearchProfileSchemaVersions: [RESEARCH_PROFILE_SCHEMA_VERSION],
    profile: resolved.profile,
    hash: resolved.hash,
    source: resolved.source,
    ...(resolved.path ? { path: resolved.path } : {})
  };
}

async function authOperation(operation: AppServerProtocolOperation, options: InvokeAppServerProtocolOptions): Promise<unknown> {
  const positionals = options.args.filter((value) => !value.startsWith('--'));
  const providerId = positionals.at(-1);
  switch (operation) {
    case 'auth.list': return listAuthProviders();
    case 'auth.status': return getAuthStatus(providerId);
    case 'auth.verify': {
      const commandIndex = options.args.indexOf('verify');
      return verifyProviderAuth(options.args[commandIndex + 1] ?? '', options.args[commandIndex + 2]);
    }
    case 'auth.logout': {
      if (!providerId) throw new Error('An auth provider is required.');
      await logoutAuthProvider(providerId);
      return { providerId, removed: true };
    }
    case 'model.list': return { providers: getProviderModelCatalog(providerId === 'list' ? undefined : providerId) };
    default: throw new Error(`Unsupported app-server auth operation: ${operation}`);
  }
}

function sessionOperation(operation: AppServerProtocolOperation, options: InvokeAppServerProtocolOptions): unknown {
  const store = new AppServerSessionStore({
    ...(options.storage ? { databasePath: options.storage.databasePath } : {}),
    readOnly: new Set<AppServerProtocolOperation>([
      'session.get', 'session.get_update', 'session.events', 'session.event_details',
      'session.collaboration', 'session.captures', 'session.capture', 'session.list', 'session.list_summaries'
    ]).has(operation)
  });
  try {
    const sessionId = option(options.args, '--session-id');
    switch (operation) {
      case 'session.create': return store.create(options.input as CreateAppServerSessionInput);
      case 'session.begin_attempt': return store.beginAttempt(required(sessionId, '--session-id'), options.input as BeginAppServerSessionAttemptInput);
      case 'session.append_event':
      case 'session.append_event_receipt': return store.appendEventReceipt(required(sessionId, '--session-id'), options.input as AppServerSessionEvent);
      case 'session.transition': return store.transition(required(sessionId, '--session-id'), options.input as AppServerSessionTransitionInput);
      case 'session.recover_interrupted': return store.recoverInterrupted(required(option(options.args, '--workspace-id'), '--workspace-id'), options.input as never);
      case 'session.import_capture': {
        const input = requiredRecord(options.input, 'capture input');
        return store.importCapture(required(sessionId, '--session-id'), {
          attemptId: requiredText(input.attemptId, 'attemptId'), capture: input.capture as never
        });
      }
      case 'session.get': {
        const result = store.getSummary(required(sessionId, '--session-id'));
        if (!result) throw new Error(`Session not found: ${sessionId}`);
        return result;
      }
      case 'session.get_update': {
        const result = store.getUpdate(required(sessionId, '--session-id'), option(options.args, '--after-event-id'), {
          ...pageOptions(options.args), tail: options.args.includes('--tail')
        });
        if (!result) throw new Error(`Session not found: ${sessionId}`);
        return result;
      }
      case 'session.events': return store.getEventPage(required(sessionId, '--session-id'), {
        ...(option(options.args, '--after-event-id') ? { afterEventId: option(options.args, '--after-event-id')! } : {}),
        stream: eventStream(option(options.args, '--stream')), ...pageOptions(options.args), tail: options.args.includes('--tail')
      });
      case 'session.event_details': return store.getEventDetails(required(sessionId, '--session-id'), requiredMany(options.args, '--event-id'));
      case 'session.collaboration': return store.getCollaborationState(required(sessionId, '--session-id'), integerOption(options.args, '--message-limit') ?? 200);
      case 'session.captures': return store.listCaptureSummaries(required(sessionId, '--session-id'));
      case 'session.capture': {
        const result = store.getCapture(required(sessionId, '--session-id'), required(option(options.args, '--attempt-id'), '--attempt-id'));
        if (!result) throw new Error(`Capture not found for session ${sessionId}.`);
        return result;
      }
      case 'session.list':
      case 'session.list_summaries': return store.listSummariesForWorkspaces(requiredMany(options.args, '--workspace-id'), integerOption(options.args, '--limit') ?? 100);
      default: throw new Error(`Unsupported app-server session operation: ${operation}`);
    }
  } finally {
    store.close();
  }
}

async function knowledgeOperation(operation: AppServerProtocolOperation, options: InvokeAppServerProtocolOptions): Promise<unknown> {
  const storage = requiredStorage(options.storage);
  const layout = createResearchStorageLayout({ databasePath: storage.databasePath, artifactDirectoryPath: storage.artifactDirectoryPath });
  const input = requiredRecord(options.input, `${operation} input`);
  switch (operation) {
    case 'report.list': return listAppServerReportSummaries(
      layout.databasePath,
      requiredText(input.workspaceId, 'workspaceId')
    );
    case 'memory.summary': {
      const workspaceId = requiredText(input.workspaceId, 'workspaceId');
      migrateWorkspaceResearchClaims(layout.databasePath, workspaceId);
      const context = await resolveCanonicalMemoryContext(layout, input, workspaceId);
      const campaignTracks = new CampaignTrackStore({
        databasePath: layout.databasePath,
        context: {
          workspaceId,
          workspaceName: optionalText(input.workspaceName) ?? workspaceId,
          subjectId: context.subjectId ?? `subject_workspace:${workspaceId}`,
          subjectName: optionalText(input.subjectName) ?? optionalText(input.workspaceName) ?? workspaceId,
          ...(typeof input.sessionId === 'string' ? { sessionId: input.sessionId } : {})
        }
      });
      try {
        campaignTracks.repairPlaceholderTracks({ skipActiveSessions: true });
      } finally {
        campaignTracks.close();
      }
      return getAppServerMemorySummary({
        databasePath: layout.databasePath, artifactDirectoryPath: layout.artifactDirectoryPath,
        workspaceId, subjectId: context.subjectId,
        ...(typeof input.sessionId === 'string' ? { sessionId: input.sessionId } : {}),
        ...(context.researchProfile !== undefined ? { researchProfile: context.researchProfile } : {}),
        ...(input.includeForeignCatalogs === true ? { includeForeignCatalogs: true } : {}),
        ...(Array.isArray(input.assetIds) ? { assetIds: input.assetIds as string[] } : {})
      });
    }
    case 'memory.notification_feed': return memoryNotificationFeed(layout, input);
    case 'history.mark_duplicate':
    case 'history.undo_duplicate':
    case 'claim.mark_duplicate':
    case 'claim.undo_duplicate': {
      const workspaceId = requiredText(input.workspaceId, 'workspaceId');
      const workspaceName = optionalText(input.workspaceName) ?? workspaceId;
      const graph = new MemoryGraphStore({
        databasePath: layout.databasePath,
        context: {
          workspaceId,
          workspaceName,
          subjectId: optionalText(input.subjectId) ?? `subject_workspace:${workspaceId}`,
          subjectName: optionalText(input.subjectName) ?? workspaceName,
          ...(optionalText(input.sessionId) ? { sessionId: optionalText(input.sessionId)! } : {})
        }
      });
      const recordType = operation.startsWith('claim.') ? 'claim' : requiredHistoryRecordType(input.type);
      const claims = recordType === 'claim' ? new ResearchClaimStore(graph) : null;
      const runbooks = recordType === 'runbook' ? new RunbookStore(layout.databasePath, layout, graph.getContext()) : null;
      try {
        const id = operation.startsWith('claim.') ? requiredText(input.claimId, 'claimId') : requiredText(input.id, 'id');
        const expectedRevision = input.expectedRevision;
        if (!Number.isSafeInteger(expectedRevision) || (expectedRevision as number) < 1) {
          throw new Error('expectedRevision must be a positive integer.');
        }
        const undo = operation === 'history.undo_duplicate' || operation === 'claim.undo_duplicate';
        const reason = optionalText(input.reason) ?? (undo
          ? 'Duplicate marking undone by the Beale user.'
          : 'Marked as a duplicate by the Beale user.');
        const parentId = undo ? null : operation === 'claim.mark_duplicate'
          ? requiredText(input.parentClaimId, 'parentClaimId')
          : requiredText(input.parentId, 'parentId');
        if (recordType === 'claim') {
          return undo
            ? claims!.undoDuplicate(id, { expectedRevision: expectedRevision as number, reason }, undefined, optionalText(input.actorId) ?? 'human')
            : claims!.markDuplicate(id, { expectedRevision: expectedRevision as number, parentClaimId: parentId!, reason }, undefined, optionalText(input.actorId) ?? 'human');
        }
        if (recordType === 'memory') {
          return undo
            ? graph.undoDuplicate(id, { expectedRevision: expectedRevision as number, reason })
            : graph.markDuplicate(id, { expectedRevision: expectedRevision as number, parentMemoryId: parentId!, reason });
        }
        return undo
          ? runbooks!.undoDuplicate(id, { expectedRevision: expectedRevision as number, reason })
          : runbooks!.markDuplicate(id, { expectedRevision: expectedRevision as number, parentRunbookId: parentId!, reason });
      } finally {
        claims?.close();
        runbooks?.close();
        graph.close();
      }
    }
    case 'investigation.list':
    case 'investigation.get':
    case 'investigation.replay': {
      const workspaceId = requiredText(input.workspaceId, 'workspaceId');
      const store = new CampaignTrackStore({
        databasePath: layout.databasePath,
        context: {
          workspaceId,
          workspaceName: optionalText(input.workspaceName) ?? workspaceId,
          subjectId: optionalText(input.subjectId) ?? `subject_workspace:${workspaceId}`,
          subjectName: optionalText(input.subjectName) ?? optionalText(input.workspaceName) ?? workspaceId,
          ...(typeof input.sessionId === 'string' ? { sessionId: input.sessionId } : {})
        }
      });
      try {
        store.repairPlaceholderTracks({ skipActiveSessions: true });
        if (operation === 'investigation.list') return store.list({ includeArchived: input.includeArchived === true });
        if (operation === 'investigation.replay') return store.replayWorkspace({ persist: input.persist === true });
        const detail = store.detail(requiredText(input.investigationId, 'investigationId'));
        if (!detail) throw new Error(`Campaign track not found: ${String(input.investigationId)}`);
        return detail;
      } finally {
        store.close();
      }
    }
    case 'dreaming.prepare': return prepareMemoryDreamingRequest(input as never);
    case 'dreaming.parse_plan': return parseMemoryDreamingPlanOutput(requiredText(input.output, 'output'), input.profileInput as MemoryDreamingProfileInput);
    case 'dreaming.apply': return runMemoryDreaming(layout.databasePath, requiredText(input.workspaceId, 'workspaceId'), input.plan as MemoryDreamingPlan, input.context as MemoryDreamingRunContext, input.profileInput as MemoryDreamingProfileInput);
    case 'dreaming.record_failure': return recordFailedMemoryDreaming(layout.databasePath, requiredText(input.workspaceId, 'workspaceId'), input.context as MemoryDreamingRunContext, requiredText(input.errorMessage, 'errorMessage'), input.profileInput as MemoryDreamingProfileInput);
    case 'dreaming.restore':
      restoreMemoryDreamingChange(layout.databasePath, requiredText(input.workspaceId, 'workspaceId'), requiredText(input.changeId, 'changeId'));
      return { restored: true };
    case 'runbook.get': return getKnowledgeRunbook(layout.databasePath, layout.artifactDirectoryPath, requiredText(input.workspaceId, 'workspaceId'), requiredText(input.runbookId, 'runbookId'));
    case 'report.get': return getKnowledgeReport(layout.databasePath, layout.artifactDirectoryPath, requiredText(input.workspaceId, 'workspaceId'), requiredText(input.reportId, 'reportId'));
    case 'report.revise_content': {
      const workspaceId = requiredText(input.workspaceId, 'workspaceId');
      const workspaceName = optionalText(input.workspaceName) ?? workspaceId;
      const expectedRevision = input.expectedRevision;
      if (!Number.isSafeInteger(expectedRevision) || (expectedRevision as number) < 1) {
        throw new Error('expectedRevision must be a positive integer.');
      }
      const store = new ReportStore(layout.databasePath, layout, {
        workspaceId,
        workspaceName,
        subjectId: optionalText(input.subjectId) ?? `subject_workspace:${workspaceId}`,
        subjectName: optionalText(input.subjectName) ?? workspaceName
      });
      try {
        const reportId = requiredText(input.reportId, 'reportId');
        const current = store.get(reportId);
        if (!current) throw new Error(`Report not found in this workspace: ${reportId}`);
        return store.revise({
          id: reportId,
          expectedRevision: expectedRevision as number,
          content: requiredText(input.content, 'content'),
          summary: current.summary,
          status: current.status
        }).report;
      } finally {
        store.close();
      }
    }
    case 'report.update_triage_status': {
      const workspaceId = requiredText(input.workspaceId, 'workspaceId');
      const workspaceName = optionalText(input.workspaceName) ?? workspaceId;
      const expectedRevision = input.expectedRevision;
      if (!Number.isSafeInteger(expectedRevision) || (expectedRevision as number) < 1) {
        throw new Error('expectedRevision must be a positive integer.');
      }
      const store = new ReportStore(layout.databasePath, layout, {
        workspaceId,
        workspaceName,
        subjectId: optionalText(input.subjectId) ?? `subject_workspace:${workspaceId}`,
        subjectName: optionalText(input.subjectName) ?? workspaceName
      });
      try {
        return store.updateTriageStatus({
          id: requiredText(input.reportId, 'reportId'),
          expectedRevision: expectedRevision as number,
          triageStatus: requiredText(input.triageStatus, 'triageStatus') as 'editing' | 'submitted' | 'reviewing' | 'rejected' | 'accepted'
        });
      } finally {
        store.close();
      }
    }
    case 'report.replace_packet': {
      const workspaceId = requiredText(input.workspaceId, 'workspaceId');
      const workspaceName = optionalText(input.workspaceName) ?? workspaceId;
      const store = new ReportStore(layout.databasePath, layout, {
        workspaceId,
        workspaceName,
        subjectId: optionalText(input.subjectId) ?? `subject_workspace:${workspaceId}`,
        subjectName: optionalText(input.subjectName) ?? workspaceName
      }, { packetCandidateRoots: [requiredText(input.workspaceRoot, 'workspaceRoot')] });
      try {
        const reportId = requiredText(input.reportId, 'reportId');
        const current = store.get(reportId);
        if (!current) throw new Error(`Report not found in this workspace: ${reportId}`);
        return store.revise({
          id: reportId,
          expectedRevision: current.revision,
          content: current.content,
          summary: current.summary,
          status: current.status,
          submissionPacketPath: requiredText(input.submissionPacketPath, 'submissionPacketPath')
        }).report;
      } finally {
        store.close();
      }
    }
    case 'report.replace_recording': {
      const workspaceId = requiredText(input.workspaceId, 'workspaceId');
      const workspaceName = optionalText(input.workspaceName) ?? workspaceId;
      const store = new ReportStore(layout.databasePath, layout, {
        workspaceId,
        workspaceName,
        subjectId: optionalText(input.subjectId) ?? `subject_workspace:${workspaceId}`,
        subjectName: optionalText(input.subjectName) ?? workspaceName
      }, { packetCandidateRoots: [requiredText(input.workspaceRoot, 'workspaceRoot')] });
      try {
        const reportId = requiredText(input.reportId, 'reportId');
        const current = store.get(reportId);
        if (!current) throw new Error(`Report not found in this workspace: ${reportId}`);
        return store.revise({
          id: reportId,
          expectedRevision: current.revision,
          content: current.content,
          summary: current.summary,
          status: current.status,
          recordingPath: requiredText(input.recordingPath, 'recordingPath')
        }).report;
      } finally {
        store.close();
      }
    }
    case 'artifact.resolve': return resolveKnowledgeArtifact(requiredText(input.artifactId, 'artifactId'), {
      databasePath: layout.databasePath, artifactDirectoryPath: layout.artifactDirectoryPath,
      ...(typeof input.expectedKind === 'string' ? { expectedKind: input.expectedKind } : {})
    });
    default: throw new Error(`Unsupported app-server knowledge operation: ${operation}`);
  }
}

const WORKSPACE_STATE_ACTIONS = new Set([
  'checkpoint', 'getLastWorkspaceBackup', 'recordWorkspaceBackup', 'recoverInterruptedState',
  'getActiveScope', 'getScopeVersion', 'activateResearchProfileSnapshot', 'getActiveResearchProfileSnapshot',
  'getResearchProfileSnapshot', 'getResearchProfileSnapshotForWorkspace', 'getRunResearchProfileSnapshot',
  'getResearchSubject', 'setResearchSubject', 'saveScope', 'rewriteRepositoryPathReferences',
  'listWorkspaceRules', 'addWorkspaceRules', 'addWorkspaceRule', 'createRun', 'createModelSession',
  'createContextCompaction', 'setContextCompactionTrace', 'createAttempt', 'updateModelSessionByRun',
  'appendTraceEvent', 'createTranscriptMessage', 'upsertBreakoutRoom', 'upsertBreakoutRoomMember',
  'createBreakoutRoomMessage', 'interruptActiveBreakoutRooms', 'findBreakoutRoomMember',
  'refreshBreakoutRoomStatus', 'listBreakoutRoomSummaries', 'createNotification', 'listNotifications',
  'markNotificationOpened', 'dismissNotification', 'createToolCall', 'linkToolCallTrace', 'finishToolCall',
  'updateRunStatus', 'beginSessionRunActivity', 'getSessionNextStepSuggestions',
  'getCapturedSessionNextPromptSuggestions', 'saveSessionNextStepSuggestions',
  'getResearchGoalSuggestionContextRevision', 'getResearchGoalSuggestionCache',
  'saveResearchGoalSuggestionCache', 'listResearchGoalSuggestionHistory', 'selectResearchGoalSuggestion',
  'updateRunTitle', 'updateRunPrompt', 'updateRunModelSelection', 'updateRunShellSafetyMode', 'updateRunBudget',
  'updateAttemptState', 'createArtifact', 'setArtifactProvenance', 'markArtifactSensitive',
  'createVerifierContract', 'updateVerifierContract', 'createVerifierRun', 'countCodeBrowserReadsForPath',
  'countCodeBrowserReadsForPathAndHash', 'countCodeBrowserReadsForPathHashAndRange', 'countBroadSearchesForRun',
  'markPostSourceIndexingDeferred', 'getProjectIndexingDeferredState', 'recordRunSetupState',
  'getRunSetupState', 'listRunFixtureSetups', 'createExportRecord', 'updateExportReview', 'createApproval',
  'updateApprovalDecision', 'listPendingShellApprovals', 'listRunRows', 'getRunRow',
  'listResearchRecommendationRuns', 'getRunDetail', 'searchTranscriptMessages',
  'searchTranscriptMessagesAcrossWorkspaces', 'getProjectInventorySummary', 'getProjectRetrievalFeedbackSummary',
  'findProjectInventoryItemByPath', 'getProjectStructureSummary', 'getProjectStructureCoverageRecords',
  'listProjectSourceReviewObservations', 'findProjectStructureEntity', 'findProjectStructureEntityContainingLine',
  'listProjectStructureEntitiesInRange', 'listProjectStructureRelationsForEntity',
  'listProjectStructureReferencesForTarget', 'ensureProjectInventory', 'refreshProjectInventory',
  'rebuildProjectSearchIndex', 'searchProjectDocumentsForRun', 'getRunDetailUpdate', 'getRunDetailVersion',
  'getRun', 'getFirstAttempt', 'getFirstArtifact', 'getFirstVerifierContract',
]);

function workspaceStateOperation(options: InvokeAppServerProtocolOptions): unknown {
  const storage = requiredStorage(options.storage);
  const input = requiredRecord(options.input, 'workspace state input');
  const workspacePath = requiredText(input.workspacePath, 'workspacePath');
  const workspaceId = optionalText(input.workspaceId) ?? undefined;
  const researchKitId = optionalText(input.researchKitId) ?? undefined;
  const artifactRoot = requiredText(input.artifactRoot, 'artifactRoot');
  const database = new WorkspaceDatabase(storage.databasePath, artifactRoot, {
    workspacePath,
    ...(workspaceId ? { workspaceId } : {}),
    ...(researchKitId ? { researchKitId } : {}),
  });
  try {
    database.initialize();
    const action = requiredText(input.action, 'action');
    if (action === 'initialize') {
      return {
        workspaceId: database.getWorkspaceId(),
        researchKitId: database.getResearchKitId(),
        activeScope: database.getActiveScope(),
        activeResearchProfile: database.getActiveResearchProfileSnapshot(),
        researchSubject: database.getResearchSubject(),
        workspaceRules: database.listWorkspaceRules(),
        lastWorkspaceBackup: database.getLastWorkspaceBackup(),
      };
    }
    if (!WORKSPACE_STATE_ACTIONS.has(action)) throw new Error(`Unsupported workspace state action: ${action}`);
    const method = Reflect.get(database, action) as unknown;
    if (typeof method !== 'function') throw new Error(`Workspace state action is unavailable: ${action}`);
    const args = Array.isArray(input.args) ? input.args : [];
    if (action === 'createArtifact') restoreArtifactBuffer(args);
    return Reflect.apply(method, database, args) ?? null;
  } finally {
    database.close();
  }
}

function restoreArtifactBuffer(args: unknown[]): void {
  const artifact = args[0];
  if (!artifact || typeof artifact !== 'object' || Array.isArray(artifact)) return;
  const record = artifact as Record<string, unknown>;
  const content = record.content;
  if (!content || typeof content !== 'object' || Array.isArray(content)) return;
  const serialized = content as Record<string, unknown>;
  if (serialized.type !== 'Buffer' || !Array.isArray(serialized.data)
    || serialized.data.some((value) => !Number.isInteger(value) || Number(value) < 0 || Number(value) > 255)) return;
  record.content = Buffer.from(serialized.data as number[]);
}

const REGISTRY_STATE_ACTIONS = new Set([
  'getState', 'markResearchSessionViewed', 'archiveResearchSession', 'restoreResearchSession',
  'listArchivedQuickChats', 'getProfilingEnabled', 'setProfilingEnabled', 'getDeveloperSettings',
  'getDeveloperModeEnabled', 'setDeveloperModeEnabled', 'getProviderSettings', 'setDefaultProviderId',
  'setProviderModelDefaults', 'setProviderOptionalModelEnabled', 'setProviderCyberPolicyRiskAcknowledged',
  'setProviderPreferredAuthenticationMethod', 'getMemorySettings', 'setMemoryTypeDescriptions',
  'setWorkspaceMemoryBackend', 'getShellOptions', 'setShellOptions', 'getShellOptionsPath',
  'inspectDirectory', 'getWorkspace', 'getWorkspaceByPath', 'getDebuggingSettings', 'setTracesEnabled',
  'getComputerUseSettings', 'setComputerUsePermissionMode', 'getWorkspaceByDirectory',
  'setWorkspaceDirectories', 'getLastKnownWorkspace', 'rememberWorkspaceOpened',
  'removeRegisteredWorkspace', 'syncWorkspace', 'syncWorkspaceFromStorage', 'syncResearchSession', 'touchResearchSessionActivity',
  'reconcileAppServerSessions', 'markAppServerSessionsInterrupted',
]);

function registryStateOperation(options: InvokeAppServerProtocolOptions): unknown {
  const input = requiredRecord(options.input, 'registry state input');
  const registryDirectory = requiredText(input.registryDirectory, 'registryDirectory');
  const action = requiredText(input.action, 'action');
  const registry = new WorkspaceRegistry(registryDirectory);
  try {
    if (action === 'initialize') {
      return {
        state: registry.getState(),
        profilingEnabled: registry.getProfilingEnabled(),
        developerSettings: registry.getDeveloperSettings(),
        providerSettings: registry.getProviderSettings(),
        memorySettings: registry.getMemorySettings(),
        shellOptions: registry.getShellOptions(),
        debuggingSettings: registry.getDebuggingSettings(),
        computerUseSettings: registry.getComputerUseSettings(),
        lastKnownWorkspace: registry.getLastKnownWorkspace(),
      };
    }
    if (!REGISTRY_STATE_ACTIONS.has(action)) throw new Error(`Unsupported registry state action: ${action}`);
    if (action === 'syncWorkspaceFromStorage') {
      return syncRegistryWorkspaceFromStorage(registry, input);
    }
    const method = Reflect.get(registry, action) as unknown;
    if (typeof method !== 'function') throw new Error(`Registry state action is unavailable: ${action}`);
    return Reflect.apply(method, registry, Array.isArray(input.args) ? input.args : []) ?? null;
  } finally {
    registry.close();
  }
}

function syncRegistryWorkspaceFromStorage(registry: WorkspaceRegistry, input: Record<string, unknown>): {
  state: unknown;
  lastKnownWorkspace: unknown;
} {
  const args = Array.isArray(input.args) ? input.args : [];
  const sync = requiredRecord(args[0], 'workspace registry storage sync');
  const workspacePath = requiredText(sync.workspacePath, 'workspacePath');
  const databasePath = requiredText(sync.databasePath, 'databasePath');
  const artifactRoot = requiredText(sync.artifactRoot, 'artifactRoot');
  const researchProfileId = requiredText(sync.researchProfileId, 'researchProfileId');
  const workspaceDirectories = Array.isArray(sync.workspaceDirectories)
    ? sync.workspaceDirectories.filter((value): value is string => typeof value === 'string' && Boolean(value.trim()))
    : [workspacePath];
  const database = new WorkspaceDatabase(databasePath, artifactRoot, { workspacePath });
  try {
    database.initialize();
    const workspaceId = database.getWorkspaceId();
    registry.syncWorkspace({
      workspace: {
        workspaceId,
        workspacePath,
        workspaceDirectories,
        researchKitId: database.getResearchKitId(),
      },
      activeScope: database.getActiveScope(),
      researchProfile: { profileId: researchProfileId },
      runs: database.listRunRows(),
    } as never, {
      rememberLast: sync.rememberLast !== false,
      researchProfileId,
    } as never);
    const sessions = new AppServerSessionStore({ databasePath, readOnly: true });
    try {
      registry.reconcileAppServerSessions(
        researchProfileId as never,
        workspaceId,
        sessions.listSummaries(workspaceId, 200) as never,
      );
    } finally {
      sessions.close();
    }
    return {
      state: registry.getState(),
      lastKnownWorkspace: registry.getLastKnownWorkspace(),
    };
  } finally {
    database.close();
  }
}

function requiredHistoryRecordType(value: unknown): 'claim' | 'memory' | 'runbook' {
  if (value === 'claim' || value === 'memory' || value === 'runbook') return value;
  throw new Error('type must be claim, memory, or runbook.');
}

async function memoryNotificationFeed(layout: ReturnType<typeof createResearchStorageLayout>, input: Record<string, unknown>): Promise<BealeMemoryNotificationFeed> {
  const workspaceId = requiredText(input.workspaceId, 'workspaceId');
  migrateWorkspaceResearchClaims(layout.databasePath, workspaceId);
  const context = await resolveCanonicalMemoryContext(layout, input, workspaceId);
  if (!context.researchProfile) throw new Error('The memory notification profile could not be resolved.');
  const researchProfile = context.researchProfile;
  const summary = getAppServerMemorySummary({
    databasePath: layout.databasePath, artifactDirectoryPath: layout.artifactDirectoryPath,
    workspaceId, subjectId: context.subjectId,
    ...(typeof input.sessionId === 'string' ? { sessionId: input.sessionId } : {}), researchProfile
  });
  if (summary.status === 'error') throw new Error('The memory notification feed could not be read.');
  const memoryNotifications = summary.nodes.flatMap((node) => {
    const definition = researchProfile.profile.memory.types.find(
      (candidate) => candidate.id === node.type || candidate.aliases?.includes(node.type)
    );
    const heat = attentionHeatForMemoryNode(node);
    return !definition || heat === 'none' ? [] : [{
      id: node.id, kind: 'memory' as const, sessionIds: node.sessionIds, type: definition.id, typeName: definition.name,
      title: node.title, summary: node.summary, status: node.status, heat, rating: null,
      createdAt: node.createdAt, updatedAt: node.updatedAt, revision: node.revision
    }];
  });
  const claimNotifications = [...summary.leads, ...summary.findings].flatMap((claim) => {
    const heat = attentionHeatForClaim(claim);
    if (heat === 'none') return [];
    const definition = researchProfile.profile.claims.classifications.find((candidate) => candidate.id === claim.classification);
    return [{
      id: claim.id,
      kind: 'claim' as const,
      sessionIds: [...new Set([claim.originSessionId, ...claim.evidence.map((evidence) => evidence.sessionId)].filter((id): id is string => Boolean(id)))],
      type: claim.classification,
      typeName: definition?.name ?? (claim.projection === 'lead' ? 'Lead' : 'Finding'),
      title: claim.title,
      summary: claim.summary,
      status: claim.status,
      heat,
      rating: claim.rating,
      createdAt: claim.createdAt,
      updatedAt: claim.updatedAt,
      revision: claim.revision,
    }];
  });
  return {
    schemaVersion: 3, workspaceId,
    profile: { id: researchProfile.profile.id, version: researchProfile.profile.version, hash: researchProfile.profileHash },
    nodes: [...memoryNotifications, ...claimNotifications]
      .sort((left, right) => right.updatedAt.localeCompare(left.updatedAt)).slice(0, 500)
  };
}

async function resolveCanonicalMemoryContext(
  layout: ReturnType<typeof createResearchStorageLayout>,
  input: Record<string, unknown>,
  workspaceId: string
): Promise<{ subjectId: string | null; researchProfile?: ResearchProfileSnapshot | null }> {
  const workspaceRoot = typeof input.workspaceRoot === 'string' && input.workspaceRoot.trim()
    ? input.workspaceRoot.trim()
    : null;
  if (!workspaceRoot) {
    return {
      subjectId: typeof input.subjectId === 'string' ? input.subjectId : null,
      ...(input.researchProfile !== undefined
        ? { researchProfile: input.researchProfile as ResearchProfileSnapshot | null }
        : {})
    };
  }

  const researchProfileId = requiredText(input.researchProfileId, 'researchProfileId');
  const binding = resolveStoredResearchWorkspaceBinding({
    workspaceRoot,
    databasePath: layout.databasePath,
    researchProfileId
  });
  if (binding.memoryContext.workspaceId !== workspaceId) {
    throw new Error('The stored workspace binding did not match the requested workspace.');
  }
  if (input.researchProfile !== undefined) {
    return {
      subjectId: binding.memoryContext.subjectId,
      researchProfile: input.researchProfile as ResearchProfileSnapshot | null
    };
  }
  const resolvedProfile = await resolveStoredResearchProfile({
    workspaceRoot,
    databasePath: layout.databasePath,
    researchProfileId
  });
  return {
    subjectId: binding.memoryContext.subjectId,
    researchProfile: {
      id: `canonical-read-${resolvedProfile.hash}`,
      workspaceId,
      profileId: resolvedProfile.profile.id,
      profileVersion: resolvedProfile.profile.version,
      profileHash: resolvedProfile.hash,
      source: resolvedProfile.source,
      sourcePath: null,
      profile: resolvedProfile.profile,
      active: true,
      createdAt: new Date(0).toISOString()
    }
  };
}

async function harnessOperation(operation: AppServerProtocolOperation, options: InvokeAppServerProtocolOptions): Promise<unknown> {
  const input = requiredRecord(options.input, `${operation} input`);
  switch (operation) {
    case 'source.inspect': {
      const scope = input.scope as Parameters<typeof sourceRepositoryCandidates>[0] | undefined;
      const text = optionalText(input.text); const value = optionalText(input.value); const requested = optionalText(input.requested);
      return {
        ...(text ? { urls: extractSourceRepositoryUrls(text) } : {}),
        ...(value ? { normalizedUrl: normalizeSourceRepositoryUrl(value) } : {}),
        ...(scope ? { candidates: sourceRepositoryCandidates(scope) } : {}),
        ...(scope && requested ? { selection: selectSourceRepository(scope, requested) } : {})
      };
    }
    case 'source.materialize': return materializeGitRepositoryAsync(input.candidate as SourceRepositoryCandidate, optionalText(input.ref) ?? '', {
      cloneMode: repositoryCloneMode(input.cloneMode),
      ...(optionalText(input.repositoryStoreDirectory) ? { repositoryStoreDirectory: optionalText(input.repositoryStoreDirectory)! } : {})
    });
    case 'plugin.list': return pluginRegistry(input).getState();
    case 'plugin.add_filesystem': return pluginRegistry(input).addFromFilesystem(requiredText(input.pluginRoot, 'pluginRoot'));
    case 'plugin.add_repository': return pluginRegistry(input).addFromRepository(requiredText(input.repositoryUrl, 'repositoryUrl'));
    case 'plugin.set_enabled': return pluginRegistry(input).setEnabled(requiredText(input.pluginId, 'pluginId'), input.enabled === true);
    case 'plugin.remove': return pluginRegistry(input).remove(requiredText(input.pluginId, 'pluginId'));
    case 'plugin.runtime': return pluginRegistry(input).getAppServerRuntime();
    case 'maintenance.summary': return getWorkspaceDejunkSummary(requiredText(input.workspacePath, 'workspacePath'));
    case 'maintenance.run': {
      const workspacePath = requiredText(input.workspacePath, 'workspacePath');
      const repositoryStoreDirectory = optionalText(input.repositoryStoreDirectory);
      if (!repositoryStoreDirectory) return runWorkspaceDejunk(workspacePath);
      return runWorkspaceDejunkAndConsolidateRepositories(workspacePath, {
        repositoryStoreDirectory,
        repositories: Array.isArray(input.repositories)
          ? input.repositories.flatMap((candidate) => {
              if (!isRecord(candidate) || !optionalText(candidate.path)) return [];
              return [{
                path: optionalText(candidate.path)!,
                ...(optionalText(candidate.repositoryUrl) ? { repositoryUrl: optionalText(candidate.repositoryUrl)! } : {}),
                ...(optionalText(candidate.ref) ? { ref: optionalText(candidate.ref)! } : {})
              }];
            })
          : []
      });
    }
    default: throw new Error(`Unsupported app-server operation: ${operation}`);
  }
}

function pluginRegistry(input: Record<string, unknown>): AgentPluginRegistry {
  const builtinPlugins = Array.isArray(input.builtinPlugins) ? input.builtinPlugins as BuiltinAgentPluginDefinition[] : [];
  const runtimeEnvironment = isRecord(input.runtimeEnvironment) ? input.runtimeEnvironment as Record<string, Record<string, string>> : {};
  return new AgentPluginRegistry(requiredText(input.registryDirectory, 'registryDirectory'), {
    builtinPlugins, runtimeEnvironment: (plugin: AgentPluginRecord) => runtimeEnvironment[plugin.id] ?? {}
  });
}

function requiredStorage(storage: AppServerProtocolStorage | undefined): AppServerProtocolStorage { if (!storage) throw new Error('app-server storage is required for this app-server operation.'); return storage; }
function option(args: readonly string[], name: string): string | undefined { const index = args.indexOf(name); return index >= 0 ? args[index + 1] : undefined; }
function options(args: readonly string[], name: string): string[] { return args.flatMap((value, index) => value === name && args[index + 1] ? [args[index + 1]!] : []); }
function required(value: string | undefined, name: string): string { if (!value?.trim()) throw new Error(`Missing required option ${name}.`); return value.trim(); }
function requiredMany(args: readonly string[], name: string): string[] { const values = options(args, name); if (!values.length) throw new Error(`Missing required option ${name}.`); return values; }
function integerOption(args: readonly string[], name: string): number | undefined { const value = option(args, name); if (!value) return undefined; const parsed = Number(value); if (!Number.isSafeInteger(parsed) || parsed <= 0) throw new Error(`${name} must be a positive integer.`); return parsed; }
function pageOptions(args: readonly string[]): { limit?: number; maxBytes?: number } { const limit = integerOption(args, '--limit'); const maxBytes = integerOption(args, '--max-bytes'); return { ...(limit ? { limit } : {}), ...(maxBytes ? { maxBytes } : {}) }; }
function eventStream(value: string | undefined): 'all' | 'transcript' | 'trace' | 'commentary' {
  if (!value) return 'all';
  if (value === 'all' || value === 'transcript' || value === 'trace' || value === 'commentary') return value;
  throw new Error('Invalid session event stream.');
}
function requiredRecord(value: unknown, name: string): Record<string, unknown> { if (!isRecord(value)) throw new Error(`${name} must be an object.`); return value; }
function requiredText(value: unknown, name: string): string { if (typeof value !== 'string' || !value.trim()) throw new Error(`${name} must be a non-empty string.`); return value.trim(); }
function optionalText(value: unknown): string | null { return typeof value === 'string' && value.trim() ? value.trim() : null; }
function repositoryCloneMode(value: unknown): 'deep' | 'shallow' {
  if (value === undefined || value === null || value === 'deep') return 'deep';
  if (value === 'shallow') return 'shallow';
  throw new Error(`cloneMode must be deep or shallow.`);
}
function isRecord(value: unknown): value is Record<string, unknown> { return Boolean(value) && typeof value === 'object' && !Array.isArray(value); }
