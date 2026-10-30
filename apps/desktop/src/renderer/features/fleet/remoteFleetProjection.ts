import type { FleetRemoteCatalog } from '@beale/app-server-runtime/protocol';
import type { ResearchSessionSummary, RunDetail, RunStatus, TraceEventRecord, TranscriptMessageRecord, WorkspaceRegistryState } from '@shared/types';

const EMPTY_DATE = new Date(0).toISOString();

export function remoteWorkspaceRegistry(catalog: FleetRemoteCatalog | null, serverId: string): WorkspaceRegistryState | null {
  if (!catalog) return null;
  return {
    registryPath: '',
    workspaces: catalog.workspaces.map((workspace) => ({
      id: workspace.id,
      workspacePath: `remote:${serverId}:${workspace.workspaceId}`,
      workspaceId: workspace.workspaceId,
      workspaceName: workspace.name,
      researchProfileId: 'security-research',
      researchKitId: 'general',
      scopeOwner: '',
      descriptionMarkdown: '',
      rulesMarkdown: '',
      expiresAt: null,
      createdAt: EMPTY_DATE,
      updatedAt: EMPTY_DATE,
      lastOpenedAt: null,
      runCount: workspace.runCount,
      lastRunAt: catalog.sessions.find((session) => session.workspaceId === workspace.workspaceId)?.updatedAt ?? null
    })),
    researchSessions: catalog.sessions.map((session): ResearchSessionSummary => ({
      id: session.id,
      registryWorkspaceId: catalog.workspaces.find((workspace) => workspace.workspaceId === session.workspaceId)?.id ?? session.workspaceId,
      workspacePath: `remote:${serverId}:${session.workspaceId}`,
      workspaceId: session.workspaceId,
      runId: session.id,
      title: session.title,
      status: remoteRunStatus(session.status),
      runEngine: 'app-server',
      mode: 'open_discovery',
      promptMarkdown: session.prompt,
      summary: '',
      finalDisposition: null,
      model: '',
      reasoningEffort: '',
      sandboxProfile: 'host',
      createdAt: session.updatedAt || EMPTY_DATE,
      startedAt: null,
      endedAt: null,
      updatedAt: session.updatedAt || EMPTY_DATE,
      resultViewedAt: session.updatedAt || EMPTY_DATE
    }))
  };
}

export function remoteRunStatus(status: string): RunStatus {
  switch (status) {
    case 'active':
    case 'queued':
    case 'paused':
    case 'blocked':
    case 'completed':
    case 'failed':
    case 'stopped':
      return status;
    case 'running':
      return 'active';
    default:
      return 'queued';
  }
}

export function remoteRunDetail(value: unknown): RunDetail | null {
  const envelope = asRecord(value);
  const result = asRecord(envelope?.result);
  const session = asRecord(result?.session);
  if (!session || typeof session.id !== 'string') return null;
  const sessionId = session.id;
  const metadata = asRecord(session.metadata);
  const storedRun = asRecord(metadata?.bealeRun);
  const attempts = Array.isArray(session.attempts) ? session.attempts.flatMap((item) => {
    const attempt = asRecord(item);
    return attempt && string(attempt.startedAt) ? [attempt] : [];
  }) : [];
  const projectedAttempts = attempts.flatMap((attempt) => {
    const id = string(attempt.id);
    const startedAt = string(attempt.startedAt);
    return id && startedAt ? [{
      id, runId: sessionId, parentAttemptId: string(attempt.parentAttemptId),
      status: remoteRunStatus(string(attempt.status) ?? 'queued'),
      shortState: string(attempt.summary) ?? '', seed: '', strategyRole: 'research',
      cost: {}, tokenUsage: {}, startedAt, endedAt: string(attempt.endedAt)
    }] : [];
  });
  const rawIntervals = Array.isArray(session.activityIntervals)
    ? session.activityIntervals
    : attempts.map((attempt, index) => ({
        startedAt: attempt.startedAt,
        endedAt: string(attempt.endedAt)
          ?? string(asRecord(attempt.metadata)?.recoveredAt)
          ?? string(attempts[index + 1]?.startedAt)
          ?? (remoteRunStatus(string(session.status) ?? '') === 'active' ? null : string(session.endedAt) ?? string(session.updatedAt))
      }));
  const activityIntervals = rawIntervals.flatMap((item, index) => {
    const interval = asRecord(item);
    const startedAt = string(interval?.startedAt);
    return startedAt ? [{
      id: `activity_${sessionId}_${index}`,
      runId: sessionId,
      attemptId: string([...attempts].reverse().find((attempt) => (string(attempt.startedAt) ?? '') <= startedAt)?.id),
      startedAt,
      endedAt: string(interval?.endedAt)
    }] : [];
  });
  const events = Array.isArray(result?.events) ? result.events.flatMap((item) => {
    const event = asRecord(item);
    return event && typeof event.id === 'string' && typeof event.kind === 'string' ? [event] : [];
  }) : [];
  const traceEvents: TraceEventRecord[] = [];
  const transcriptMessages: TranscriptMessageRecord[] = [];
  events.forEach((event, index) => {
    const payload = asRecord(event.payload);
    const timestamp = string(event.timestamp) ?? string(session.updatedAt) ?? EMPTY_DATE;
    if (event.kind === 'beale.trace' || event.kind === 'beale.trace_batch') {
      const records = event.kind === 'beale.trace_batch' && Array.isArray(payload?.records)
        ? payload.records : [payload?.record];
      records.forEach((candidate, recordIndex) => {
        const record = asRecord(candidate);
        if (record && typeof record.id === 'string') traceEvents.push({
          ...record,
          id: record.id,
          runId: sessionId,
          sequence: typeof record.sequence === 'number' ? record.sequence : index + 1 + recordIndex / 100,
          payload: asRecord(record.payload) ?? {},
          createdAt: string(record.createdAt) ?? timestamp
        } as unknown as TraceEventRecord);
      });
    }
    if (event.kind === 'beale.transcript') {
      const record = asRecord(payload?.record);
      if (record && typeof record.id === 'string' && typeof record.contentMarkdown === 'string') {
        transcriptMessages.push({
          ...record,
          id: record.id,
          runId: sessionId,
          attemptId: string(record.attemptId),
          traceEventId: string(record.traceEventId),
          role: record.role === 'user' || record.role === 'system' ? record.role : 'assistant',
          contentMarkdown: record.contentMarkdown,
          source: string(record.source) ?? 'app-server',
          metadata: asRecord(record.metadata) ?? {},
          createdAt: string(record.createdAt) ?? timestamp
        } as TranscriptMessageRecord);
      }
    }
    if (event.kind === 'model.output' || event.kind === 'model.thought') {
      const text = string(payload?.text);
      if (payload?.phase === 'completed' && text) transcriptMessages.push({
        id: `transcript_${event.id}`,
        runId: sessionId,
        attemptId: string(payload.attemptId),
        traceEventId: event.id as string,
        role: 'assistant',
        ...(event.kind === 'model.output' ? { phase: payload.messagePhase === 'commentary' ? 'commentary' : 'final_answer' } as const : {}),
        contentMarkdown: text,
        source: event.kind === 'model.thought' ? 'openai_reasoning_summary' : payload.messagePhase === 'commentary' ? 'app_server_commentary' : 'app-server',
        metadata: {
          agentPath: string(payload.agentPath) ?? string(event.agentPath) ?? '/root',
          ...(string(payload.provider) ? { provider: payload.provider } : {}),
          ...(string(payload.responseId) ? { responseId: payload.responseId } : {}),
          ...(string(payload.itemId) ? { itemId: payload.itemId } : {})
        },
        createdAt: timestamp
      });
    }
    if (event.kind !== 'beale.trace' && event.kind !== 'beale.trace_batch'
      && event.kind !== 'beale.transcript' && event.kind !== 'model.output' && event.kind !== 'model.thought') {
      traceEvents.push(remoteTraceFromEvent(sessionId, event, index + 1, timestamp));
    }
  });
  const finalResponse = string(result?.finalResponse);
  if (finalResponse && !transcriptMessages.some((message) => message.phase === 'final_answer' && message.contentMarkdown === finalResponse)) {
    transcriptMessages.push({
      id: `transcript_final_${sessionId}`,
      runId: sessionId,
      attemptId: null,
      traceEventId: null,
      role: 'assistant',
      phase: 'final_answer',
      contentMarkdown: finalResponse,
      source: 'app-server',
      metadata: { agentPath: '/root' },
      createdAt: string(session.endedAt) ?? string(session.updatedAt) ?? EMPTY_DATE
    });
  }
  const createdAt = string(session.createdAt) ?? EMPTY_DATE;
  return {
    run: {
      id: sessionId,
      scopeVersionId: string(storedRun?.scopeVersionId) ?? '',
      researchProfileSnapshotId: string(storedRun?.researchProfileSnapshotId),
      shellSafetyMode: metadata?.shellSafetyMode === 'manual_approval' || metadata?.shellSafetyMode === 'danger' ? metadata.shellSafetyMode : 'auto_review',
      mode: string(storedRun?.mode) ?? 'open_discovery',
      status: remoteRunStatus(string(session.status) ?? ''),
      title: string(session.title) ?? string(session.prompt)?.slice(0, 90) ?? sessionId,
      promptMarkdown: string(session.prompt) ?? '',
      model: string(session.model) ?? '',
      reasoningEffort: string(session.reasoningEffort) ?? '',
      attemptStrategy: string(storedRun?.attemptStrategy) ?? 'iterative_research',
      sandboxProfile: string(storedRun?.sandboxProfile) ?? 'host',
      targetAssetId: string(storedRun?.targetAssetId),
      targetPath: string(storedRun?.targetPath),
      budget: asRecord(storedRun?.budget) ?? {},
      summary: string(session.summary) ?? '',
      finalDisposition: null,
      createdAt,
      startedAt: string(session.startedAt),
      endedAt: string(session.endedAt)
    },
    ...(activityIntervals.length ? { activityIntervals } : {}),
    attempts: projectedAttempts,
    traceEvents: traceEvents.map((event) => ({ ...event,
      attemptId: event.attemptId ?? [...projectedAttempts].reverse().find((attempt) =>
        attempt.startedAt <= event.createdAt)?.id ?? null
    })),
    transcriptMessages,
    artifacts: [],
    verifierContracts: [],
    verifierRuns: [],
    modelSessions: [],
    contextCompactions: [],
    policyEvents: [],
    exports: [],
    historyCursor: {
      beforeEventId: typeof events[0]?.id === 'string' ? events[0].id : null,
      hasEarlier: result?.hasEarlier === true
    }
  };
}

export interface RemoteSessionHistoryPage {
  events: unknown[];
  hasEarlier: boolean;
}

export function remoteSessionHistoryPage(value: unknown): RemoteSessionHistoryPage | null {
  const result = asRecord(asRecord(value)?.result);
  if (!result || !Array.isArray(result.events)) return null;
  return { events: result.events, hasEarlier: result.hasEarlier === true };
}

export function mergeRemoteSessionHistory(update: unknown, pages: readonly RemoteSessionHistoryPage[]): unknown {
  const envelope = asRecord(update);
  const result = asRecord(envelope?.result);
  if (!envelope || !result || pages.length === 0) return update;
  const seen = new Set<string>();
  const events = [...pages.flatMap((page) => page.events), ...(Array.isArray(result.events) ? result.events : [])]
    .filter((item) => {
      const id = string(asRecord(item)?.id);
      if (!id || seen.has(id)) return false;
      seen.add(id);
      return true;
    });
  return { ...envelope, result: { ...result, events, hasEarlier: pages[0]?.hasEarlier === true } };
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : null;
}

function remoteTraceFromEvent(runId: string, event: Record<string, unknown>, sequence: number, timestamp: string): TraceEventRecord {
  const payload = asRecord(event.payload);
  const researchEvent = event.kind === 'research.event' ? asRecord(payload?.event) : null;
  const researchKind = string(researchEvent?.kind);
  const researchPayload = asRecord(researchEvent?.payload);
  const tool = (researchKind === 'tool.requested' || researchKind === 'tool.observed') && researchPayload;
  const agentPath = string(researchEvent?.agentPath) ?? string(payload?.agentPath) ?? string(event.agentPath);
  return {
    id: event.id as string,
    runId,
    attemptId: string(payload?.attemptId),
    sequence,
    type: tool ? researchKind === 'tool.requested' ? 'tool_call' : 'tool_result' : 'research_event',
    source: tool ? researchKind === 'tool.requested' ? 'model' : 'tool' : event.kind === 'session.recovery' ? 'system' : 'executor',
    summary: tool ? `app-server ${researchKind}: ${string(researchPayload.toolName) ?? 'tool'}` : string(event.summary) ?? '',
    payload: tool ? {
      appServerEventId: event.id,
      appServerKind: researchKind,
      appServerTimestamp: timestamp,
      toolName: string(researchPayload.toolName) ?? 'tool',
      payload: researchPayload,
      ...(agentPath ? { agentPath } : {})
    } : {
      appServerEventId: event.id,
      appServerKind: event.kind,
      appServerTimestamp: timestamp,
      payload: event.payload,
      ...(agentPath ? { agentPath } : {})
    },
    sensitivity: 'internal',
    modelVisible: event.kind !== 'session.recovery',
    createdAt: string(researchEvent?.timestamp) ?? timestamp,
    artifactId: null,
    toolCallId: tool ? string(researchPayload.toolActionId) : null,
    approvalId: null
  };
}

function string(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value : null;
}
