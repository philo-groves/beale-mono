import { createHash, randomUUID } from 'node:crypto';
import { dirname, join } from 'node:path';
import type {
  ApprovalRecord,
  ArtifactRecord,
  AttemptRecord,
  BreakoutRoomMemberRecord,
  BreakoutRoomMessageRecord,
  BreakoutRoomRecord,
  GeneratedResearchGoalSuggestions,
  ModelSessionRecord,
  NotificationRecord,
  RunDetail,
  RunDetailUpdate,
  RunDetailUpdateCursor,
  RunDetailVersion,
  RunRecord,
  RunRow,
  SessionNextPromptSuggestion,
  SessionTranscriptSearchInput,
  SessionTranscriptSearchResponse,
  TraceEventRecord,
  TranscriptMessageRecord
} from '@shared/types';
import {
  createId,
  normalizeSessionNextStepSuggestions,
  parseSessionNextPromptSuggestions,
  type CreatedRunContext,
  type WorkspaceDatabase
} from './database';
import { resolvedBreakoutRoomStatus } from './breakoutRoomStatus';
import {
  appendAppServerSessionEventAsync,
  beginAppServerSessionAttempt,
  createAppServerSession,
  getAppServerSession,
  getAppServerSessionCollaborationState,
  getAppServerSessionEventPage,
  getAppServerSessionUpdate,
  appServerOwnsSessions,
  listAppServerSessionSummaries,
  listAppServerSessions,
  transitionAppServerSession,
  type AppServerSessionEvent,
  type AppServerSessionCaptureSummary,
  type AppServerSessionCollaborationState,
  type AppServerSessionRecord,
  type AppServerSessionSummary,
  type AppServerSessionUpdate,
  type AppServerSessionStorage
} from './appServerCliClient';
import {
  fetchExistingAppServerCanonicalResult,
  readLiveBealeAppServerDiscovery,
  type BealeAppServerDiscovery
} from './bealeAppServerClient';

const BOUNDARIES = new WeakSet<WorkspaceDatabase>();
interface AppServerSessionBoundaryContext {
  database: WorkspaceDatabase;
  ownedRunIds: ReadonlySet<string>;
  storage: AppServerSessionStorage;
  sessionWrites: AppServerSessionWriteQueue;
}
const BOUNDARY_CONTEXTS = new WeakMap<WorkspaceDatabase, AppServerSessionBoundaryContext>();

const SESSION_WRITE_BATCH_DELAY_MS = 40;
const SESSION_TRACE_BATCH_SIZE = 256;
const SESSION_TRACE_MAX_PENDING = 4_096;
const SESSION_WRITE_RETRY_MAX_MS = 2_000;

type QueuedAppServerSessionWrite =
  | { type: 'trace'; record: TraceEventRecord }
  | { type: 'event'; event: AppServerSessionEvent };

interface InFlightAppServerSessionWrite {
  writes: QueuedAppServerSessionWrite[];
  promise: Promise<void>;
}

class AppServerSessionWriteQueue {
  private readonly pending = new Map<string, QueuedAppServerSessionWrite[]>();
  private readonly timers = new Map<string, ReturnType<typeof setTimeout>>();
  private readonly inFlight = new Map<string, InFlightAppServerSessionWrite>();
  private readonly retryDelay = new Map<string, number>();

  public constructor(private readonly storage: AppServerSessionStorage) {}

  public enqueueTrace(runId: string, record: TraceEventRecord): void {
    this.enqueue(runId, { type: 'trace', record });
  }

  public enqueueEvent(runId: string, event: AppServerSessionEvent): void {
    this.enqueue(runId, { type: 'event', event });
  }

  public overlayEvents(runId: string): AppServerSessionEvent[] {
    const inFlight = this.inFlight.get(runId)?.writes ?? [];
    const pending = this.pending.get(runId) ?? [];
    return [...inFlight, ...pending].flatMap((write) => write.type === 'event' ? [write.event] : []);
  }

  private enqueue(runId: string, write: QueuedAppServerSessionWrite): void {
    const queued = this.pending.get(runId) ?? [];
    queued.push(write);
    trimQueuedTraces(queued);
    this.pending.set(runId, queued);
    if (queued.length >= SESSION_TRACE_BATCH_SIZE) {
      this.schedule(runId, 0);
    } else if (!this.timers.has(runId) && !this.inFlight.has(runId)) {
      this.schedule(runId, SESSION_WRITE_BATCH_DELAY_MS);
    }
  }

  public async flush(runId?: string): Promise<void> {
    const runIds = runId
      ? [runId]
      : [...new Set([...this.pending.keys(), ...this.inFlight.keys()])];
    await Promise.all(runIds.map(async (candidate) => {
      this.clearTimer(candidate);
      this.start(candidate);
      while (this.inFlight.has(candidate)) {
        await this.inFlight.get(candidate)?.promise;
        this.clearTimer(candidate);
        this.start(candidate);
      }
    }));
  }

  private schedule(runId: string, delayMs: number): void {
    this.clearTimer(runId);
    const timer = setTimeout(() => {
      this.timers.delete(runId);
      this.start(runId);
    }, delayMs);
    timer.unref?.();
    this.timers.set(runId, timer);
  }

  private start(runId: string): void {
    if (this.inFlight.has(runId)) return;
    const queued = this.pending.get(runId);
    if (!queued?.length) return;
    const writes = queued[0]?.type === 'event'
      ? queued.splice(0, 1)
      : queued.splice(0, consecutiveTraceWriteCount(queued));
    if (queued.length === 0) this.pending.delete(runId);
    const event = queuedAppServerEvent(writes);
    let operation!: InFlightAppServerSessionWrite;
    const promise = appendAppServerSessionEventAsync(runId, event, this.storage).then(() => {
      this.retryDelay.delete(runId);
    }).catch(() => {
      const current = this.pending.get(runId) ?? [];
      const retried = [...writes, ...current];
      trimQueuedTraces(retried);
      this.pending.set(runId, retried);
      const delay = Math.min(
        (this.retryDelay.get(runId) ?? SESSION_WRITE_BATCH_DELAY_MS) * 2,
        SESSION_WRITE_RETRY_MAX_MS
      );
      this.retryDelay.set(runId, delay);
      this.schedule(runId, delay);
    }).finally(() => {
      if (this.inFlight.get(runId) === operation) this.inFlight.delete(runId);
      if ((this.pending.get(runId)?.length ?? 0) > 0 && !this.timers.has(runId)) {
        this.schedule(runId, this.retryDelay.get(runId) ?? 0);
      }
    });
    operation = { writes, promise };
    this.inFlight.set(runId, operation);
  }

  private clearTimer(runId: string): void {
    const timer = this.timers.get(runId);
    if (!timer) return;
    clearTimeout(timer);
    this.timers.delete(runId);
  }
}

function consecutiveTraceWriteCount(writes: readonly QueuedAppServerSessionWrite[]): number {
  let count = 0;
  while (count < Math.min(writes.length, SESSION_TRACE_BATCH_SIZE) && writes[count]?.type === 'trace') count += 1;
  return Math.max(1, count);
}

function queuedAppServerEvent(writes: readonly QueuedAppServerSessionWrite[]): AppServerSessionEvent {
  const direct = writes[0];
  if (writes.length === 1 && direct?.type === 'event') return direct.event;
  const records = writes.flatMap((write) => write.type === 'trace' ? [write.record] : []);
  return {
    id: createId('trace_batch'),
    kind: 'beale.trace_batch',
    timestamp: records[0]?.createdAt ?? new Date().toISOString(),
    summary: `Beale persisted ${records.length} trace event${records.length === 1 ? '' : 's'}.`,
    payload: { records }
  };
}

function trimQueuedTraces(writes: QueuedAppServerSessionWrite[]): void {
  let traceCount = writes.reduce((count, write) => count + (write.type === 'trace' ? 1 : 0), 0);
  while (traceCount > SESSION_TRACE_MAX_PENDING) {
    const index = writes.findIndex((write) => write.type === 'trace');
    if (index < 0) return;
    writes.splice(index, 1);
    traceCount -= 1;
  }
}

function sessionWithQueuedEvents(
  session: AppServerSessionRecord,
  writes: AppServerSessionWriteQueue
): AppServerSessionRecord {
  const queued = writes.overlayEvents(session.id);
  if (queued.length === 0) return session;
  const eventIds = new Set(session.events.map((event) => event.id));
  const overlay = queued.filter((event) => !eventIds.has(event.id));
  return overlay.length > 0 ? { ...session, events: [...session.events, ...overlay] } : session;
}

function sessionWithDurableSubagentEvents(
  session: AppServerSessionRecord,
  collaboration: Pick<AppServerSessionCollaborationState, 'subagents'>
): { session: AppServerSessionRecord; prependedEventCount: number } {
  if (collaboration.subagents.length === 0) return { session, prependedEventCount: 0 };
  const eventIds = new Set(session.events.map((event) => event.id));
  const missing = collaboration.subagents.filter((event) => !eventIds.has(event.id));
  return {
    session: missing.length > 0 ? { ...session, events: [...missing, ...session.events] } : session,
    prependedEventCount: missing.length
  };
}

function sessionRecordFromUpdate(update: ReturnType<typeof getAppServerSessionUpdate>): AppServerSessionRecord {
  return {
    ...update.session,
    finalResponse: update.finalResponse,
    attempts: update.session.attempts.map((attempt) => ({ ...attempt, capture: null })),
    events: update.events
  };
}

function boundedSessionWithQueuedEvents(
  context: AppServerSessionBoundaryContext,
  runId: string
): AppServerSessionRecord {
  return sessionWithQueuedEvents(sessionRecordFromUpdate(getAppServerSessionUpdate(
    runId,
    null,
    context.storage,
    { tail: true, limit: 1_000, maxBytes: 2 * 1024 * 1024 }
  )), context.sessionWrites);
}

export function createAppServerSessionBoundary(
  database: WorkspaceDatabase,
  ownershipEnabled = usesAppServerSessionOwnership(),
  tracesEnabled: () => boolean = () => true
): WorkspaceDatabase {
  if (!ownershipEnabled) return database;
  const activeProfileId = database.getActiveResearchProfileSnapshot()?.profileId;
  const storage: AppServerSessionStorage = {
    databasePath: database.getDatabasePath(),
    artifactDirectoryPath: join(dirname(database.getDatabasePath()), 'artifacts'),
    ...(activeProfileId ? { profileId: activeProfileId } : {})
  };
  const workspaceId = database.getWorkspaceId();
  const sessionSummaries = listAppServerSessionSummaries(workspaceId, storage);
  let prefetchedSessionSummaries: AppServerSessionSummary[] | null = sessionSummaries;
  const ownedRunIds = new Set(sessionSummaries.map((session) => session.id));
  const nextTraceSequence = new Map(sessionSummaries.map((session) => [session.id, session.revision]));
  const sessionWrites = new AppServerSessionWriteQueue(storage);
  const queuedBreakoutRooms = new Map<string, BreakoutRoomRecord>();
  const queuedBreakoutRoomMembers = new Map<string, BreakoutRoomMemberRecord>();
  let boundary!: WorkspaceDatabase;

  const getSessionMetadata = (runId: string): AppServerSessionRecord | null => {
    return ownedRunIds.has(runId) ? getAppServerSession(runId, storage) : null;
  };

  const getSession = (runId: string): AppServerSessionRecord | null => {
    if (!ownedRunIds.has(runId)) return null;
    return sessionWithQueuedEvents(sessionRecordFromUpdate(getAppServerSessionUpdate(
      runId,
      null,
      storage,
      { tail: true, limit: 1_000, maxBytes: 2 * 1024 * 1024 }
    )), sessionWrites);
  };
  const appendRecordEvent = (runId: string, kind: string, record: Record<string, unknown>): void => {
    sessionWrites.enqueueEvent(runId, {
      // The session log is append-only and deduplicates by event ID. Record IDs
      // identify the entity being revised, so reusing them here would discard
      // every update after the entity's creation.
      id: createId('event'),
      kind,
      timestamp: typeof record.createdAt === 'string' ? record.createdAt : new Date().toISOString(),
      summary: typeof record.summary === 'string' ? record.summary : kind,
      payload: { record }
    });
  };

  const overrides: Partial<Record<keyof WorkspaceDatabase, unknown>> = {
    createRun: ((input: Parameters<WorkspaceDatabase['createRun']>[0]): CreatedRunContext => {
      const engine = typeof input.budget.runEngine === 'string' ? input.budget.runEngine : null;
      if (engine !== 'app-server') return database.createRun(input);
      const createdAt = new Date().toISOString();
      const runId = createId('run');
      const attemptId = createId('attempt');
      const run: RunRecord = {
        id: runId,
        scopeVersionId: input.scopeVersionId,
        researchProfileSnapshotId: input.researchProfileSnapshotId?.trim() || null,
        shellSafetyMode: input.shellSafetyMode,
        mode: input.mode,
        status: 'active',
        title: input.title,
        promptMarkdown: input.promptMarkdown,
        model: input.model,
        reasoningEffort: input.reasoningEffort,
        attemptStrategy: input.attemptStrategy,
        sandboxProfile: input.sandboxProfile,
        targetAssetId: input.targetAssetId ?? null,
        targetPath: input.targetPath ?? null,
        budget: input.budget,
        summary: 'Starting app-server-owned research session.',
        finalDisposition: null,
        createdAt,
        startedAt: createdAt,
        endedAt: null
      };
      const attempt: AttemptRecord = {
        id: attemptId,
        runId,
        parentAttemptId: null,
        status: 'active',
        shortState: 'Initializing app-server research plan.',
        seed: randomUUID(),
        strategyRole: 'initial_portfolio',
        cost: { label: '$0.00' },
        tokenUsage: { promptTokens: 0, completionTokens: 0, source: 'not_reported' },
        startedAt: createdAt,
        endedAt: null
      };
      const researchProfile = run.researchProfileSnapshotId
        ? database.getResearchProfileSnapshot(run.researchProfileSnapshotId)
        : null;
      createAppServerSession({
        id: runId,
        workspaceId,
        attemptId,
        title: run.title,
        prompt: run.promptMarkdown,
        provider: typeof input.budget.modelProvider === 'string' ? input.budget.modelProvider : null,
        model: run.model,
        reasoningEffort: run.reasoningEffort,
        workflowId: typeof input.budget.researchWorkflowId === 'string' ? input.budget.researchWorkflowId : null,
        profile: researchProfile
          ? {
              snapshotId: researchProfile.id,
              id: researchProfile.profileId,
              hash: researchProfile.profileHash
            }
          : null,
        metadata: { bealeRun: run },
        attemptMetadata: { bealeAttempt: attempt },
        createdAt
      }, storage);
      ownedRunIds.add(runId);
      prefetchedSessionSummaries = null;
      nextTraceSequence.set(runId, 1);
      return { run, attempt };
    }) as WorkspaceDatabase['createRun'],

    createAttempt: ((input: Parameters<WorkspaceDatabase['createAttempt']>[0]): AttemptRecord => {
      if (!ownedRunIds.has(input.runId)) return database.createAttempt(input);
      const startedAt = new Date().toISOString();
      const attempt: AttemptRecord = {
        id: createId('attempt'),
        runId: input.runId,
        parentAttemptId: input.parentAttemptId ?? null,
        status: input.status ?? 'active',
        shortState: input.shortState,
        seed: randomUUID(),
        strategyRole: input.strategyRole,
        cost: { label: '$0.00' },
        tokenUsage: { promptTokens: 0, completionTokens: 0, source: 'not_reported' },
        startedAt,
        endedAt: null
      };
      beginAppServerSessionAttempt(input.runId, {
        attemptId: attempt.id,
        parentAttemptId: attempt.parentAttemptId,
        summary: attempt.shortState,
        startedAt,
        metadata: { bealeAttempt: attempt }
      }, storage);
      return attempt;
    }) as WorkspaceDatabase['createAttempt'],

    getRun: ((runId: string): RunRecord | null => {
      const session = getSessionMetadata(runId);
      return session ? sessionRun(session) : database.getRun(runId);
    }) as WorkspaceDatabase['getRun'],

    getRunDetail: ((runId: string): RunDetail => {
      const session = getSession(runId);
      return session ? sessionDetail(session, database) : database.getRunDetail(runId);
    }) as WorkspaceDatabase['getRunDetail'],

    getSessionNextStepSuggestions: ((runId: string): GeneratedResearchGoalSuggestions | null => {
      const session = getSession(runId);
      return session ? sessionNextStepSuggestions(session) : database.getSessionNextStepSuggestions(runId);
    }) as WorkspaceDatabase['getSessionNextStepSuggestions'],

    getCapturedSessionNextPromptSuggestions: ((runId: string): SessionNextPromptSuggestion[] => {
      const session = getSession(runId);
      if (!session) return database.getCapturedSessionNextPromptSuggestions(runId);
      const transcripts = sessionDetail(session, database).transcriptMessages;
      for (let index = transcripts.length - 1; index >= 0; index -= 1) {
        const transcript = transcripts[index];
        if (transcript?.role !== 'assistant') continue;
        const suggestions = parseSessionNextPromptSuggestions(transcript.metadata.nextPromptSuggestions);
        if (suggestions.length > 0) return suggestions;
      }
      return [];
    }) as WorkspaceDatabase['getCapturedSessionNextPromptSuggestions'],

    saveSessionNextStepSuggestions: ((runId: string, value: GeneratedResearchGoalSuggestions): GeneratedResearchGoalSuggestions => {
      if (!ownedRunIds.has(runId)) return database.saveSessionNextStepSuggestions(runId, value);
      const session = getSessionMetadata(runId);
      if (!session || !['blocked', 'completed', 'failed', 'stopped'].includes(sessionStatus(session.status))) {
        throw new Error(`Ended run not found for next-step suggestions: ${runId}`);
      }
      const normalized = normalizeSessionNextStepSuggestions(value);
      if (!normalized) {
        throw new Error('Session next-step suggestions must contain a workflow and exactly three distinct non-empty suggestions.');
      }
      sessionWrites.enqueueEvent(runId, {
        id: createId('next_step_suggestions'),
        kind: 'beale.session_next_step_suggestions',
        timestamp: new Date().toISOString(),
        summary: 'Session next-step suggestions stored.',
        payload: { record: normalized }
      });
      return normalized;
    }) as WorkspaceDatabase['saveSessionNextStepSuggestions'],

    getRunDetailVersion: ((runId: string): RunDetailVersion => {
      const session = getSessionMetadata(runId);
      return session
        ? { runId, version: `beale:${session.revision}`, generatedAt: new Date().toISOString(), databaseMs: 0 }
        : database.getRunDetailVersion(runId);
    }) as WorkspaceDatabase['getRunDetailVersion'],

    getRunDetailUpdate: ((runId: string, _cursor: RunDetailUpdateCursor): RunDetailUpdate => {
      const session = getSession(runId);
      if (!session) return database.getRunDetailUpdate(runId, _cursor);
      const detail = sessionDetail(session, database);
      return {
        ...detail,
        version: { runId, version: `beale:${session.revision}`, generatedAt: new Date().toISOString(), databaseMs: 0 }
      };
    }) as WorkspaceDatabase['getRunDetailUpdate'],

    listRunRows: (() => {
      if (ownedRunIds.size === 0) return database.listRunRows();
      const sessions = prefetchedSessionSummaries ?? listAppServerSessionSummaries(workspaceId, storage);
      prefetchedSessionSummaries = null;
      for (const session of sessions) ownedRunIds.add(session.id);
      const appServerRows: RunRow[] = sessions.map((session) => {
        const recovery = sessionRecovery(session);
        return {
          run: sessionRun(session),
          engine: 'app-server',
          lastMessageAt: latestSessionMessageAt(session.lastMessageAt, sessionWrites.overlayEvents(session.id)),
          tokenUsage: session.tokenUsage ?? { totalTokens: 0 },
          sessionRuns: [{
            id: `session_run_${session.id}`,
            runId: session.id,
            attemptId: session.attempts.at(-1)?.id ?? null,
            status: sessionStatus(session.status),
            activityIntervals: [{
              id: `activity_${session.id}`,
              runId: session.id,
              attemptId: session.attempts.at(-1)?.id ?? null,
              startedAt: session.startedAt,
              endedAt: session.status === 'paused' && recovery ? recovery.recoveredAt : session.endedAt
            }],
            terminationCause: session.status === 'paused' && recovery ? 'workspace_recovery' : null
          }]
        };
      });
      return [...appServerRows, ...database.listRunRows().filter((row) => !ownedRunIds.has(row.run.id))];
    }) as WorkspaceDatabase['listRunRows'],

    appendTraceEvent: ((input: Parameters<WorkspaceDatabase['appendTraceEvent']>[0]): TraceEventRecord => {
      if (!ownedRunIds.has(input.runId)) return database.appendTraceEvent(input);
      const record: TraceEventRecord = {
        id: createId('trace'),
        runId: input.runId,
        attemptId: input.attemptId ?? null,
        sequence: (nextTraceSequence.get(input.runId) ?? 0) + 1,
        type: input.type,
        source: input.source,
        summary: input.summary,
        payload: input.payload ?? {},
        sensitivity: input.sensitivity ?? 'internal',
        modelVisible: input.modelVisible ?? true,
        createdAt: new Date().toISOString(),
        artifactId: input.artifactId ?? null,
        toolCallId: input.toolCallId ?? null,
        approvalId: input.approvalId ?? null
      };
      if (tracesEnabled() || traceRequiredForFunctionalityOrHistory(input)) {
        nextTraceSequence.set(input.runId, record.sequence);
        sessionWrites.enqueueTrace(input.runId, record);
      }
      return record;
    }) as WorkspaceDatabase['appendTraceEvent'],

    createTranscriptMessage: ((input: Parameters<WorkspaceDatabase['createTranscriptMessage']>[0]): TranscriptMessageRecord => {
      if (!ownedRunIds.has(input.runId)) return database.createTranscriptMessage(input);
      const record: TranscriptMessageRecord = {
        id: createId('transcript'),
        runId: input.runId,
        attemptId: input.attemptId ?? null,
        traceEventId: input.traceEventId ?? null,
        role: input.role,
        phase: input.phase ?? null,
        contentMarkdown: input.contentMarkdown,
        source: input.source,
        metadata: input.metadata ?? {},
        createdAt: new Date().toISOString()
      };
      appendRecordEvent(input.runId, 'beale.transcript', record as unknown as Record<string, unknown>);
      return record;
    }) as WorkspaceDatabase['createTranscriptMessage'],

    updateRunStatus: ((runId: string, status: Parameters<WorkspaceDatabase['updateRunStatus']>[1], summary: string, disposition?: Parameters<WorkspaceDatabase['updateRunStatus']>[3]) => {
      if (!ownedRunIds.has(runId)) return database.updateRunStatus(runId, status, summary, disposition);
      const session = transitionAppServerSession(runId, {
        status: sessionStatus(status),
        summary,
        ...(disposition ? { disposition } : {})
      }, storage);
      return sessionRun(session);
    }) as WorkspaceDatabase['updateRunStatus'],

    updateAttemptState: ((attemptId: string, status: Parameters<WorkspaceDatabase['updateAttemptState']>[1], shortState: string): void => {
      const runId = ownedRunIdForAttempt(ownedRunIds, storage, attemptId);
      if (!runId) return database.updateAttemptState(attemptId, status, shortState);
      transitionAppServerSession(runId, { status: sessionStatus(status), summary: shortState, attemptId }, storage);
    }) as WorkspaceDatabase['updateAttemptState'],

    beginSessionRunActivity: ((runId: string, attemptId: string): void => {
      if (!ownedRunIds.has(runId)) database.beginSessionRunActivity(runId, attemptId);
    }) as WorkspaceDatabase['beginSessionRunActivity'],

    updateRunTitle: ((runId: string, title: string): RunRecord => {
      if (!ownedRunIds.has(runId)) return database.updateRunTitle(runId, title);
      sessionWrites.enqueueEvent(runId, {
        id: createId('title'),
        kind: 'session.title',
        timestamp: new Date().toISOString(),
        summary: 'Session title updated.',
        payload: { status: 'generated', title }
      });
      const current = getSessionMetadata(runId);
      if (!current) throw new Error(`Session not found after title update: ${runId}`);
      return { ...sessionRun(current), title };
    }) as WorkspaceDatabase['updateRunTitle'],

    updateRunPrompt: ((runId: string, promptMarkdown: string): RunRecord => {
      if (!ownedRunIds.has(runId)) return database.updateRunPrompt(runId, promptMarkdown);
      const session = getAppServerSession(runId, storage);
      const run = sessionRun(session);
      const normalized = promptMarkdown.trim();
      if (!normalized) throw new Error('Run prompt cannot be empty.');
      const updated = { ...run, promptMarkdown: normalized };
      const next = transitionAppServerSession(runId, {
        status: session.status,
        summary: session.summary,
        configuration: { prompt: normalized },
        metadata: { bealeRun: updated }
      }, storage);
      return sessionRun(next);
    }) as WorkspaceDatabase['updateRunPrompt'],

    updateRunShellSafetyMode: ((runId: string, shellSafetyMode: Parameters<WorkspaceDatabase['updateRunShellSafetyMode']>[1]): RunRecord => {
      if (!ownedRunIds.has(runId)) return database.updateRunShellSafetyMode(runId, shellSafetyMode);
      const current = getAppServerSession(runId, storage);
      const session = transitionAppServerSession(runId, {
        status: current.status,
        summary: current.summary,
        metadata: { shellSafetyMode }
      }, storage);
      return sessionRun(session);
    }) as WorkspaceDatabase['updateRunShellSafetyMode'],

    updateRunModelSelection: ((runId: string, selection: Parameters<WorkspaceDatabase['updateRunModelSelection']>[1]): RunRecord => {
      if (!ownedRunIds.has(runId)) return database.updateRunModelSelection(runId, selection);
      const session = getAppServerSession(runId, storage);
      const run = sessionRun(session);
      const updated = { ...run, model: selection.model, reasoningEffort: selection.reasoningEffort };
      const next = transitionAppServerSession(runId, {
        status: session.status,
        summary: session.summary,
        configuration: {
          provider: selection.provider,
          model: selection.model,
          reasoningEffort: selection.reasoningEffort === 'off' ? '' : selection.reasoningEffort
        },
        metadata: { bealeRun: updated }
      }, storage);
      return sessionRun(next);
    }) as WorkspaceDatabase['updateRunModelSelection'],

    updateRunBudget: ((runId: string, budgetPatch: Parameters<WorkspaceDatabase['updateRunBudget']>[1]): RunRecord => {
      if (!ownedRunIds.has(runId)) return database.updateRunBudget(runId, budgetPatch);
      const session = getAppServerSession(runId, storage);
      const run = sessionRun(session);
      const updated = { ...run, budget: { ...run.budget, ...budgetPatch } };
      const next = transitionAppServerSession(runId, {
        status: session.status,
        summary: session.summary,
        ...(
          budgetPatch.modelProvider !== undefined || budgetPatch.researchWorkflowId !== undefined
            ? {
                configuration: {
                  ...(budgetPatch.modelProvider !== undefined
                    ? { provider: typeof budgetPatch.modelProvider === 'string' ? budgetPatch.modelProvider : null }
                    : {}),
                  ...(budgetPatch.researchWorkflowId !== undefined
                    ? { workflowId: typeof budgetPatch.researchWorkflowId === 'string' ? budgetPatch.researchWorkflowId : null }
                    : {})
                }
              }
            : {}
        ),
        metadata: { bealeRun: updated }
      }, storage);
      return sessionRun(next);
    }) as WorkspaceDatabase['updateRunBudget'],

    getRunResearchProfileSnapshot: ((runId: string) => {
      const session = getSessionMetadata(runId);
      if (!session) return database.getRunResearchProfileSnapshot(runId);
      const run = sessionRun(session);
      return run.researchProfileSnapshotId
        ? database.getResearchProfileSnapshot(run.researchProfileSnapshotId)
        : null;
    }) as WorkspaceDatabase['getRunResearchProfileSnapshot'],

    createModelSession: ((input: Parameters<WorkspaceDatabase['createModelSession']>[0]): ModelSessionRecord => {
      if (!ownedRunIds.has(input.runId)) return database.createModelSession(input);
      const now = new Date().toISOString();
      const record: ModelSessionRecord = {
        id: createId('model_session'),
        runId: input.runId,
        provider: input.provider,
        transport: input.transport,
        previousResponseId: input.previousResponseId ?? null,
        status: input.status,
        metadata: input.metadata ?? {},
        createdAt: now,
        updatedAt: now
      };
      appendRecordEvent(input.runId, 'beale.model_session', record as unknown as Record<string, unknown>);
      return record;
    }) as WorkspaceDatabase['createModelSession'],

    updateModelSessionByRun: ((runId: string, patch: Parameters<WorkspaceDatabase['updateModelSessionByRun']>[1]): void => {
      if (!ownedRunIds.has(runId)) return database.updateModelSessionByRun(runId, patch);
      appendRecordEvent(runId, 'beale.model_session_update', { id: createId('model_session_update'), runId, patch, createdAt: new Date().toISOString() });
    }) as WorkspaceDatabase['updateModelSessionByRun'],

    upsertBreakoutRoom: ((input: Parameters<WorkspaceDatabase['upsertBreakoutRoom']>[0]): BreakoutRoomRecord => {
      if (!ownedRunIds.has(input.runId)) return database.upsertBreakoutRoom(input);
      const record: BreakoutRoomRecord = {
        id: input.id,
        runId: input.runId,
        attemptId: input.attemptId ?? null,
        name: input.name,
        title: input.title,
        purpose: input.purpose ?? '',
        kind: input.kind ?? 'general',
        status: input.status ?? 'active',
        phase: input.phase ?? (input.status === 'completed' ? 'completed' : 'independent'),
        challengeRound: input.challengeRound ?? 0,
        outcomeMarkdown: input.outcomeMarkdown ?? null,
        createdAt: input.createdAt ?? new Date().toISOString(),
        closedAt: input.closedAt ?? null
      };
      queuedBreakoutRooms.set(record.id, record);
      appendRecordEvent(input.runId, 'beale.breakout_room', record as unknown as Record<string, unknown>);
      return record;
    }) as WorkspaceDatabase['upsertBreakoutRoom'],

    upsertBreakoutRoomMember: ((input: Parameters<WorkspaceDatabase['upsertBreakoutRoomMember']>[0]): BreakoutRoomMemberRecord => {
      if (!ownedRunIds.has(input.runId)) return database.upsertBreakoutRoomMember(input);
      const record: BreakoutRoomMemberRecord = {
        id: input.id,
        roomId: input.roomId,
        runId: input.runId,
        attemptId: input.attemptId ?? null,
        agentId: input.agentId,
        agentPath: input.agentPath,
        provider: input.provider,
        model: input.model,
        reasoningEffort: input.reasoningEffort ?? null,
        role: input.role ?? '',
        status: input.status,
        startedAt: input.startedAt ?? null,
        endedAt: input.endedAt ?? null,
        error: input.error ?? null
      };
      queuedBreakoutRoomMembers.set(record.id, record);
      appendRecordEvent(input.runId, 'beale.breakout_member', record as unknown as Record<string, unknown>);
      return record;
    }) as WorkspaceDatabase['upsertBreakoutRoomMember'],

    createBreakoutRoomMessage: ((input: Parameters<WorkspaceDatabase['createBreakoutRoomMessage']>[0]): BreakoutRoomMessageRecord => {
      if (!ownedRunIds.has(input.runId)) return database.createBreakoutRoomMessage(input);
      const record: BreakoutRoomMessageRecord = {
        id: input.id,
        roomId: input.roomId,
        runId: input.runId,
        attemptId: input.attemptId ?? null,
        memberId: input.memberId ?? null,
        senderAgentPath: input.senderAgentPath,
        recipientAgentPath: input.recipientAgentPath ?? null,
        kind: input.kind,
        contentMarkdown: input.contentMarkdown,
        evidenceRefs: input.evidenceRefs ?? [],
        metadata: input.metadata ?? {},
        createdAt: input.createdAt ?? new Date().toISOString()
      };
      appendRecordEvent(input.runId, 'beale.breakout_message', record as unknown as Record<string, unknown>);
      return record;
    }) as WorkspaceDatabase['createBreakoutRoomMessage'],

    findBreakoutRoomMember: ((runId: string, attemptId: string | null, agentPath: string): BreakoutRoomMemberRecord | null => {
      if (!ownedRunIds.has(runId)) return database.findBreakoutRoomMember(runId, attemptId, agentPath);
      return [...queuedBreakoutRoomMembers.values()]
        .filter((member) => member.runId === runId && member.attemptId === attemptId && member.agentPath === agentPath)
        .sort((left, right) => (left.startedAt ?? '').localeCompare(right.startedAt ?? '') || left.id.localeCompare(right.id))
        .at(-1) ?? null;
    }) as WorkspaceDatabase['findBreakoutRoomMember'],

    refreshBreakoutRoomStatus: ((roomId: string): BreakoutRoomRecord | null => {
      const room = queuedBreakoutRooms.get(roomId);
      if (room) {
        const members = [...queuedBreakoutRoomMembers.values()].filter((member) => member.roomId === roomId);
        const status = resolvedBreakoutRoomStatus(room, members);
        const resolved = {
          ...room,
          status,
          closedAt: status === 'active'
            ? null
            : room.closedAt ?? members.flatMap((member) => member.endedAt ? [member.endedAt] : []).sort().at(-1) ?? new Date().toISOString()
        };
        queuedBreakoutRooms.set(roomId, resolved);
        return resolved;
      }
      return database.refreshBreakoutRoomStatus(roomId);
    }) as WorkspaceDatabase['refreshBreakoutRoomStatus'],

    createNotification: ((input: Parameters<WorkspaceDatabase['createNotification']>[0]): NotificationRecord => {
      if (!ownedRunIds.has(input.runId)) return database.createNotification(input);
      const record: NotificationRecord = {
        id: createId('notification'),
        runId: input.runId,
        traceEventId: input.traceEventId ?? null,
        kind: input.kind,
        title: input.title,
        bodyMarkdown: input.bodyMarkdown,
        status: 'unread',
        createdAt: new Date().toISOString(),
        openedAt: null,
        dismissedAt: null
      };
      appendRecordEvent(input.runId, 'beale.notification', record as unknown as Record<string, unknown>);
      return record;
    }) as WorkspaceDatabase['createNotification'],

    createApproval: ((input: Parameters<WorkspaceDatabase['createApproval']>[0]): ApprovalRecord => {
      if (!ownedRunIds.has(input.runId)) return database.createApproval(input);
      const now = new Date().toISOString();
      const record: ApprovalRecord = {
        id: createId('approval'),
        runId: input.runId,
        attemptId: input.attemptId ?? null,
        requestKind: input.requestKind,
        requestedAction: input.requestedAction,
        decision: input.pending ? 'pending' : input.decision,
        reason: input.reason,
        scopeAmendmentId: input.scopeAmendmentId ?? null,
        createdAt: now,
        decidedAt: input.pending ? null : now
      };
      appendRecordEvent(input.runId, 'beale.approval', record as unknown as Record<string, unknown>);
      return record;
    }) as WorkspaceDatabase['createApproval'],

    updateApprovalDecision: ((approvalId: string, runId: string, decision: string, reason: string): ApprovalRecord => {
      if (!ownedRunIds.has(runId)) return database.updateApprovalDecision(approvalId, runId, decision, reason);
      const current = getSession(runId);
      const existing = current ? sessionDetail(current, database).policyEvents.find((approval) => approval.id === approvalId) : null;
      if (!existing) throw new Error(`Approval not found for run ${runId}: ${approvalId}`);
      const record = { ...existing, decision, reason, decidedAt: new Date().toISOString() };
      appendRecordEvent(runId, 'beale.approval', record as unknown as Record<string, unknown>);
      return record;
    }) as WorkspaceDatabase['updateApprovalDecision'],

    createArtifact: ((input: Parameters<WorkspaceDatabase['createArtifact']>[0]): ArtifactRecord => {
      const metadata = input.metadata ?? {};
      const runId = stringValue(metadata.runId);
      if (!runId || !ownedRunIds.has(runId)) return database.createArtifact(input);
      const buffer = typeof input.content === 'string' ? Buffer.from(input.content) : input.content;
      const sha256 = createHash('sha256').update(buffer).digest('hex');
      const record: ArtifactRecord = {
        id: createId('artifact'),
        sha256,
        relativePath: join('.beale', 'artifacts', sha256.slice(0, 2), sha256),
        kind: input.kind,
        sizeBytes: buffer.byteLength,
        mimeType: input.mimeType,
        sensitivity: input.sensitivity,
        modelVisible: input.modelVisible,
        provenanceTraceEventId: null,
        source: input.source,
        metadata,
        createdAt: new Date().toISOString()
      };
      appendRecordEvent(runId, 'beale.artifact', record as unknown as Record<string, unknown>);
      return record;
    }) as WorkspaceDatabase['createArtifact'],

    getFirstAttempt: ((runId: string): AttemptRecord | null => {
      if (!ownedRunIds.has(runId)) return database.getFirstAttempt(runId);
      const session = getSessionMetadata(runId);
      return session ? sessionDetail(session, database).attempts[0] ?? null : null;
    }) as WorkspaceDatabase['getFirstAttempt'],

    getFirstArtifact: ((runId: string): ArtifactRecord | null => {
      if (!ownedRunIds.has(runId)) return database.getFirstArtifact(runId);
      const session = getSession(runId);
      return session ? sessionDetail(session, database).artifacts[0] ?? null : null;
    }) as WorkspaceDatabase['getFirstArtifact'],

    listPendingShellApprovals: (() => {
      const canonical = [...ownedRunIds].flatMap((runId) => {
        const session = getSession(runId);
        return session ? sessionDetail(session, database).policyEvents : [];
      })
        .filter((approval) => ['shell_command', 'computer_use'].includes(approval.requestKind) && approval.decision === 'pending');
      return [...canonical, ...database.listPendingShellApprovals()];
    }) as WorkspaceDatabase['listPendingShellApprovals'],

    listNotifications: ((status: Parameters<WorkspaceDatabase['listNotifications']>[0] = 'unread') => {
      const canonical = [...ownedRunIds].flatMap((runId) => {
        const session = getSession(runId);
        return session ? sessionNotifications(session) : [];
      })
        .filter((notification) => notification.status === status);
      return [...canonical, ...database.listNotifications(status)];
    }) as WorkspaceDatabase['listNotifications'],

    searchTranscriptMessages: ((input: SessionTranscriptSearchInput, context: Parameters<WorkspaceDatabase['searchTranscriptMessages']>[1]): SessionTranscriptSearchResponse => {
      if (ownedRunIds.size === 0) return database.searchTranscriptMessages(input, context);
      return mergeTranscriptSearch(
        input,
        [{ databaseWorkspaceId: workspaceId, ...context }],
        database.searchTranscriptMessages(input, context),
        storage,
        database
      );
    }) as WorkspaceDatabase['searchTranscriptMessages'],

    searchTranscriptMessagesAcrossWorkspaces: ((input: SessionTranscriptSearchInput, contexts: Parameters<WorkspaceDatabase['searchTranscriptMessagesAcrossWorkspaces']>[1]): SessionTranscriptSearchResponse => {
      return mergeTranscriptSearch(
        input,
        contexts,
        database.searchTranscriptMessagesAcrossWorkspaces(input, contexts),
        storage,
        database
      );
    }) as WorkspaceDatabase['searchTranscriptMessagesAcrossWorkspaces'],

    interruptActiveBreakoutRooms: ((runId: string, attemptId?: string | null, endedAt?: string): void => {
      if (!ownedRunIds.has(runId)) {
        database.interruptActiveBreakoutRooms(runId, attemptId, endedAt);
        return;
      }
      queueInterruptedAppServerBreakoutRooms(
        { database, ownedRunIds, storage, sessionWrites },
        runId,
        attemptId,
        endedAt ?? new Date().toISOString()
      );
    }) as WorkspaceDatabase['interruptActiveBreakoutRooms']
  };

  boundary = new Proxy(database, {
    get(target, property, receiver) {
      const override = overrides[property as keyof WorkspaceDatabase];
      if (override !== undefined) return override;
      const value = Reflect.get(target, property, receiver) as unknown;
      return typeof value === 'function' ? (value as (...args: unknown[]) => unknown).bind(target) : value;
    }
  });
  BOUNDARIES.add(boundary);
  BOUNDARY_CONTEXTS.set(boundary, { database, ownedRunIds, storage, sessionWrites });
  return boundary;
}

export function isAppServerSessionBoundary(database: WorkspaceDatabase): boolean {
  return BOUNDARIES.has(database);
}

export function listAppServerPendingApprovalsForRuns(
  database: WorkspaceDatabase,
  runIds: readonly string[]
): ApprovalRecord[] {
  const context = BOUNDARY_CONTEXTS.get(database);
  if (!context) return database.listPendingShellApprovals();
  const canonical = runIds
    .filter((runId) => context.ownedRunIds.has(runId))
    .flatMap((runId) => sessionDetail(
      boundedSessionWithQueuedEvents(context, runId),
      context.database
    ).policyEvents)
    .filter((approval) => ['shell_command', 'computer_use'].includes(approval.requestKind) && approval.decision === 'pending');
  return [...canonical, ...context.database.listPendingShellApprovals()];
}

export function listAppServerNotificationsForRuns(
  database: WorkspaceDatabase,
  runIds: readonly string[],
  status: Parameters<WorkspaceDatabase['listNotifications']>[0] = 'unread'
): NotificationRecord[] {
  const context = BOUNDARY_CONTEXTS.get(database);
  if (!context) return database.listNotifications(status);
  const canonical = runIds
    .filter((runId) => context.ownedRunIds.has(runId))
    .flatMap((runId) => sessionNotifications(
      boundedSessionWithQueuedEvents(context, runId)
    ))
    .filter((notification) => notification.status === status);
  return [...canonical, ...context.database.listNotifications(status)];
}

export function markAppServerSessionInterrupted(
  database: WorkspaceDatabase,
  runId: string,
  attemptId: string,
  reason = 'app_shutdown'
): boolean {
  const context = BOUNDARY_CONTEXTS.get(database);
  if (!context?.ownedRunIds.has(runId)) return false;
  const recoveredAt = new Date().toISOString();
  queueInterruptedAppServerBreakoutRooms(context, runId, attemptId, recoveredAt);
  transitionAppServerSession(runId, {
    status: 'paused',
    summary: 'Paused after Beale closed the active app-server process.',
    attemptId,
    at: recoveredAt,
    metadata: {
      interruptedByRecovery: true,
      recoveryReason: reason,
      recoveredAt,
      previousStatus: 'active',
      recoveredAttemptIds: [attemptId]
    }
  }, context.storage);
  return true;
}

export function traceRequiredForFunctionalityOrHistory(
  input: Parameters<WorkspaceDatabase['appendTraceEvent']>[0]
): boolean {
  if (input.artifactId || input.toolCallId || input.approvalId) return true;
  if (input.source === 'user' || input.source === 'policy' || input.source === 'verifier') return true;
  if (input.type !== 'research_event' && input.type !== 'network_event') return true;
  const payload = input.payload ?? {};
  return payload.interruptedByRecovery === true
    || payload.type === 'subagent.activity'
    || payload.transport === 'app-server';
}

function queueInterruptedAppServerBreakoutRooms(
  context: AppServerSessionBoundaryContext,
  runId: string,
  attemptId: string | null | undefined,
  endedAt: string
): void {
  const summary = getAppServerSession(runId, context.storage);
  const collaboration = getAppServerSessionCollaborationState(runId, context.storage);
  const session = sessionWithQueuedEvents({
    ...summary,
    events: [...collaboration.rooms, ...collaboration.members, ...collaboration.messages]
  }, context.sessionWrites);
  const detail = sessionDetail(session, context.database);
  const interruptedMembers = (detail.breakoutRoomMembers ?? []).filter((member) =>
    (attemptId == null || member.attemptId === attemptId)
    && (member.status === 'pending' || member.status === 'active')
  );
  const roomIds = new Set(interruptedMembers.map((member) => member.roomId));
  for (const member of interruptedMembers) {
    enqueueBoundaryRecordEvent(context.sessionWrites, runId, 'beale.breakout_member', {
      ...member,
      status: 'interrupted',
      endedAt: member.endedAt ?? endedAt
    }, endedAt);
  }
  for (const room of (detail.breakoutRooms ?? []).filter((candidate) => roomIds.has(candidate.id))) {
    enqueueBoundaryRecordEvent(context.sessionWrites, runId, 'beale.breakout_room', {
      ...room,
      status: 'interrupted',
      closedAt: room.closedAt ?? endedAt
    }, endedAt);
  }
}

function enqueueBoundaryRecordEvent(
  writes: AppServerSessionWriteQueue,
  runId: string,
  kind: string,
  record: Record<string, unknown>,
  timestamp: string
): void {
  writes.enqueueEvent(runId, {
    id: createId('event'),
    kind,
    timestamp,
    summary: kind,
    payload: { record }
  });
}

export async function flushAppServerSessionWrites(database: WorkspaceDatabase, runId?: string): Promise<void> {
  await BOUNDARY_CONTEXTS.get(database)?.sessionWrites.flush(runId);
}

export async function getAppServerRunDetailForClient(
  database: WorkspaceDatabase,
  runId: string,
  signal?: AbortSignal
): Promise<RunDetail | null> {
  const context = BOUNDARY_CONTEXTS.get(database);
  if (!context?.ownedRunIds.has(runId)) return null;
  const record = requireExistingSessionAppServer();
  const [update, collaboration, captures] = await Promise.all([
    fetchExistingAppServerCanonicalResult<AppServerSessionUpdate>(record, canonicalSessionPath(
      context,
      runId,
      'update',
      { tail: true, limit: 1_000, maxBytes: 2 * 1024 * 1024 }
    ), { ...(signal ? { signal } : {}) }),
    fetchExistingAppServerCanonicalResult<AppServerSessionCollaborationState>(
      record,
      canonicalSessionPath(context, runId, 'collaboration'),
      { ...(signal ? { signal } : {}) }
    ),
    fetchExistingAppServerCanonicalResult<AppServerSessionCaptureSummary[]>(
      record,
      canonicalSessionPath(context, runId, 'captures'),
      { ...(signal ? { signal } : {}) }
    )
  ]);
  const durableSubagents = sessionWithDurableSubagentEvents(sessionRecordFromUpdate(update), collaboration);
  const session = sessionWithQueuedEvents(
    durableSubagents.session,
    context.sessionWrites
  );
  return sessionDetail(
    session,
    context.database,
    Math.max(0, update.eventOffset - durableSubagents.prependedEventCount),
    captures
  );
}

export async function getAppServerRunDetailVersionForClient(
  database: WorkspaceDatabase,
  runId: string,
  signal?: AbortSignal
): Promise<RunDetailVersion | null> {
  const context = BOUNDARY_CONTEXTS.get(database);
  if (!context?.ownedRunIds.has(runId)) return null;
  const record = requireExistingSessionAppServer();
  const update = await fetchExistingAppServerCanonicalResult<AppServerSessionUpdate>(record, canonicalSessionPath(
    context,
    runId,
    'update',
    { tail: true, limit: 1, maxBytes: 1 }
  ), { ...(signal ? { signal } : {}) });
  return {
    runId,
    version: `beale:${update.session.revision}`,
    generatedAt: new Date().toISOString(),
    databaseMs: 0
  };
}

export async function getAppServerRunDetailUpdateForClient(
  database: WorkspaceDatabase,
  runId: string,
  _cursor: RunDetailUpdateCursor,
  signal?: AbortSignal
): Promise<RunDetailUpdate | null> {
  const context = BOUNDARY_CONTEXTS.get(database);
  if (!context?.ownedRunIds.has(runId)) return null;
  const record = requireExistingSessionAppServer();
  const update = await fetchExistingAppServerCanonicalResult<AppServerSessionUpdate>(record, canonicalSessionPath(
    context,
    runId,
    'update',
    {
      ...(_cursor.afterTraceEventId ? { afterEventId: _cursor.afterTraceEventId } : {}),
      limit: 1_000,
      maxBytes: 2 * 1024 * 1024
    }
  ), { ...(signal ? { signal } : {}) });
  const session = sessionRecordFromUpdate(update);
  if (!_cursor.afterTraceEventId) {
    return runDetailUpdateFromSession(session, context.database, _cursor, update.eventOffset);
  }
  const detail = sessionDetail(session, context.database, update.eventOffset);
  return runDetailUpdateFromDetail(detail, update.session.revision);
}

export async function getAppServerRunTraceEventDetailsForClient(
  database: WorkspaceDatabase,
  runId: string,
  traceEventIds: readonly string[],
  signal?: AbortSignal
): Promise<TraceEventRecord[] | null> {
  const context = BOUNDARY_CONTEXTS.get(database);
  if (!context?.ownedRunIds.has(runId)) return null;
  if (traceEventIds.length === 0) return [];
  const record = requireExistingSessionAppServer();
  const sessionEvents = await fetchExistingAppServerCanonicalResult<AppServerSessionEvent[]>(
    record,
    canonicalSessionPath(context, runId, 'event-details'),
    {
      method: 'POST',
      body: { eventIds: traceEventIds },
      ...(signal ? { signal } : {})
    }
  );
  const requested = new Set(traceEventIds);
  return sessionEvents
    .flatMap((event, index) => traceFromSessionEvent(runId, event, index + 1))
    .filter((event) => requested.has(event.id));
}

function requireExistingSessionAppServer(): BealeAppServerDiscovery {
  const record = readLiveBealeAppServerDiscovery();
  if (!record) {
    throw new Error('The Beale app-server hosting this research session is unavailable. Session reads will retry without launching a replacement.');
  }
  return record;
}

function canonicalSessionPath(
  context: AppServerSessionBoundaryContext,
  runId: string,
  operation: 'update' | 'events' | 'collaboration' | 'captures' | 'event-details',
  query: Record<string, string | number | boolean> = {}
): string {
  const search = new URLSearchParams(Object.entries(query).map(([key, value]) => [key, String(value)]));
  const suffix = search.size > 0 ? `?${search.toString()}` : '';
  return `/v1/workspaces/${encodeURIComponent(context.database.getWorkspaceId())}`
    + `/sessions/${encodeURIComponent(runId)}/${operation}${suffix}`;
}

function runDetailUpdateFromSession(
  session: AppServerSessionRecord,
  database: WorkspaceDatabase,
  cursor: RunDetailUpdateCursor,
  eventSequenceOffset = 0
): RunDetailUpdate {
  const detail = sessionDetail(session, database, eventSequenceOffset);
  const afterTraceSequence = Number.isFinite(cursor.afterTraceSequence)
    ? Math.max(-1, cursor.afterTraceSequence)
    : -1;
  const afterTranscriptCount = Number.isFinite(cursor.afterTranscriptCount)
    ? Math.max(0, Math.floor(cursor.afterTranscriptCount))
    : 0;
  const transcriptMessages = detail.transcriptMessages.slice(afterTranscriptCount);
  const latestAttemptId = session.attempts.at(-1)?.id ?? null;
  const terminalRootResponse = detail.transcriptMessages.findLast((message) =>
    isRootFinalResponse(message, latestAttemptId)
  );
  if (terminalRootResponse && !transcriptMessages.some((message) => message.id === terminalRootResponse.id)) {
    // Synthetic recovery messages can disappear when a paused session resumes,
    // so a count-only cursor may otherwise skip the successful terminal answer.
    transcriptMessages.push(terminalRootResponse);
  }
  return runDetailUpdateFromDetail({
    ...detail,
    traceEvents: detail.traceEvents.filter((event) => event.sequence > afterTraceSequence),
    transcriptMessages
  }, session.revision);
}

function runDetailUpdateFromDetail(detail: RunDetail, revision: number): RunDetailUpdate {
  return {
    ...detail,
    version: {
      runId: detail.run.id,
      version: `beale:${revision}`,
      generatedAt: new Date().toISOString(),
      databaseMs: 0
    }
  };
}

export function usesAppServerSessionOwnership(): boolean {
  return appServerOwnsSessions();
}

function sessionRun(session: AppServerSessionRecord | AppServerSessionSummary): RunRecord {
  const stored = recordValue(session.metadata.bealeRun);
  return {
    id: session.id,
    scopeVersionId: stringValue(stored?.scopeVersionId) ?? '',
    researchProfileSnapshotId: stringValue(stored?.researchProfileSnapshotId),
    shellSafetyMode: session.metadata.shellSafetyMode === 'manual_approval' || session.metadata.shellSafetyMode === 'danger'
      ? session.metadata.shellSafetyMode
      : stored?.shellSafetyMode === 'manual_approval' || stored?.shellSafetyMode === 'danger'
        ? stored.shellSafetyMode
        : 'auto_review',
    mode: stringValue(stored?.mode) ?? 'open_discovery',
    status: sessionStatus(session.status),
    title: session.title,
    promptMarkdown: session.prompt,
    model: session.model,
    reasoningEffort: session.reasoningEffort,
    attemptStrategy: stringValue(stored?.attemptStrategy) ?? 'iterative_research',
    sandboxProfile: stringValue(stored?.sandboxProfile) ?? 'host',
    targetAssetId: stringValue(stored?.targetAssetId),
    targetPath: stringValue(stored?.targetPath),
    budget: recordValue(stored?.budget) ?? {},
    summary: session.summary,
    finalDisposition: session.finalDisposition as RunRecord['finalDisposition'],
    createdAt: session.createdAt,
    startedAt: session.startedAt,
    endedAt: session.endedAt
  };
}

function isRootFinalResponse(message: TranscriptMessageRecord, attemptId: string | null): boolean {
  if (message.role !== 'assistant' || message.phase !== 'final_answer') return false;
  const agentPath = stringValue(message.metadata.agentPath);
  return (!agentPath || agentPath === '/root') && message.attemptId === attemptId;
}

function sessionDetail(
  session: AppServerSessionRecord,
  database: WorkspaceDatabase,
  eventSequenceOffset = 0,
  captureSummaries: readonly AppServerSessionCaptureSummary[] = []
): RunDetail {
  const run = sessionRun(session);
  const events = session.events;
  const traceEvents = events.flatMap((event, index) => traceFromSessionEvent(
    session.id,
    event,
    eventSequenceOffset + index + 1
  ));
  const persistedTranscripts = uniqueCorrelatedTranscripts(latestRecords(events.flatMap((event) => event.kind === 'beale.transcript'
    ? recordArrayValue<TranscriptMessageRecord>(event.payload)
    : [])));
  const persistedTranscriptKeys = new Set(persistedTranscripts.flatMap((message) => {
    const key = transcriptCorrelationKey(message);
    return key ? [key] : [];
  }));
  const transcripts = [
    ...persistedTranscripts,
    ...events.flatMap((event) => {
      const message = transcriptFromCanonicalModelEvent(session, event);
      if (!message) return [];
      const key = transcriptCorrelationKey(message);
      return key && persistedTranscriptKeys.has(key) ? [] : [message];
    })
  ].sort((left, right) => left.createdAt.localeCompare(right.createdAt) || left.id.localeCompare(right.id));
  const recovery = sessionRecovery(session);
  if (
    session.status === 'paused' &&
    recovery &&
    !transcripts.some((message) => message.metadata.interruptedByRecovery === true)
  ) {
    transcripts.push({
      id: `transcript_recovery_${session.id}_${recovery.recoveredAt}`,
      runId: session.id,
      attemptId: recovery.attemptId,
      traceEventId: null,
      role: 'assistant',
      phase: 'final_answer',
      contentMarkdown: 'Unexpected error',
      source: 'app-server',
      metadata: {
        finalResultKind: 'error',
        agentStatus: 'interrupted',
        agentPath: '/root',
        interruptedByRecovery: true,
        recoveredAt: recovery.recoveredAt,
        reason: recovery.reason
      },
      createdAt: recovery.recoveredAt
    });
  }
  if (
    session.status !== 'active' &&
    session.finalResponse &&
    !transcripts.some((message) => isRootFinalResponse(message, session.attempts.at(-1)?.id ?? null))
  ) {
    const finalAttemptId = session.attempts.at(-1)?.id ?? null;
    transcripts.push({
      id: `transcript_final_${session.id}_${finalAttemptId ?? 'session'}`,
      runId: session.id,
      attemptId: finalAttemptId,
      traceEventId: null,
      role: 'assistant',
      phase: 'final_answer',
      contentMarkdown: session.finalResponse,
      source: 'app-server',
      metadata: { agentPath: '/root' },
      createdAt: session.endedAt ?? session.updatedAt
    });
  }
  const modelSessions = materializedModelSessions(events).map((modelSession) => ({
    ...modelSession,
    status: session.status
  }));
  const breakoutRoomMembers = latestRecords(events.flatMap((event) => event.kind === 'beale.breakout_member'
    ? recordArrayValue<BreakoutRoomMemberRecord>(event.payload)
    : []));
  const breakoutRooms = latestRecords(events.flatMap((event) => event.kind === 'beale.breakout_room'
    ? recordArrayValue<BreakoutRoomRecord>(event.payload)
    : [])).map((room) => {
      const roomMembers = breakoutRoomMembers.filter((member) => member.roomId === room.id);
      const status = resolvedBreakoutRoomStatus(
        room,
        roomMembers,
        sessionStatus(session.status)
      );
      const latestMemberEndedAt = roomMembers
        .flatMap((member) => member.endedAt ? [member.endedAt] : [])
        .sort()
        .at(-1) ?? null;
      return {
        ...room,
        status,
        closedAt: status === 'active'
          ? null
          : room.closedAt ?? latestMemberEndedAt ?? session.endedAt ?? session.updatedAt
      };
    });
  const captureArtifacts: ArtifactRecord[] = session.attempts.flatMap((attempt) => {
    const summary = captureSummaries.find((candidate) => candidate.attemptId === attempt.id);
    if (!attempt.capture && !summary) return [];
    const capturedAt = summary?.capturedAt
      ?? stringValue(attempt.capture?.capturedAt)
      ?? session.updatedAt;
    const serialized = attempt.capture ? JSON.stringify(attempt.capture.raw) : null;
    return [{
      id: `capture_${session.id}_${attempt.id}`,
      sha256: summary?.contentHash ?? createHash('sha256').update(serialized ?? '').digest('hex'),
      relativePath: join('.beale', 'app-server-runs', `${session.id}.${attempt.id}.capture.json`),
      kind: 'app_server_flow_capture',
      sizeBytes: summary?.sizeBytes ?? Buffer.byteLength(serialized ?? ''),
      mimeType: 'application/json',
      sensitivity: 'internal',
      modelVisible: false,
      provenanceTraceEventId: null,
      source: 'app-server',
      metadata: {
        capturedAt,
        attemptId: attempt.id,
        detailAvailableOnRequest: true,
        ...(summary ? { eventStreams: summary.eventStreams } : {})
      },
      createdAt: capturedAt
    }];
  });
  return {
    run,
    ...(session.tokenUsage ? { tokenUsage: session.tokenUsage } : {}),
    ...(session.activityCounts ? { activityCounts: session.activityCounts } : {}),
    researchProfile: run.researchProfileSnapshotId
      ? database.getResearchProfileSnapshot(run.researchProfileSnapshotId)
      : null,
    nextStepSuggestions: sessionNextStepSuggestions(session),
    attempts: session.attempts.map((attempt) => {
      const stored = recordValue(attempt.metadata.bealeAttempt);
      return {
        id: attempt.id,
        runId: session.id,
        parentAttemptId: attempt.parentAttemptId,
        status: sessionStatus(attempt.status),
        shortState: attempt.summary,
        seed: stringValue(stored?.seed) ?? attempt.id,
        strategyRole: stringValue(stored?.strategyRole) ?? 'session_continuation',
        cost: recordValue(stored?.cost) ?? { label: '$0.00' },
        tokenUsage: recordValue(stored?.tokenUsage) ?? {},
        startedAt: attempt.startedAt,
        endedAt: attempt.endedAt
      };
    }),
    traceEvents,
    transcriptMessages: transcripts,
    breakoutRooms,
    breakoutRoomMembers,
    breakoutRoomMessages: latestRecords(events.flatMap((event) => event.kind === 'beale.breakout_message' ? recordArrayValue<BreakoutRoomMessageRecord>(event.payload) : [])),
    artifacts: [...captureArtifacts, ...latestRecords(events.flatMap((event) => event.kind === 'beale.artifact' ? recordArrayValue<ArtifactRecord>(event.payload) : []))],
    verifierContracts: [],
    verifierRuns: [],
    modelSessions,
    contextCompactions: [],
    policyEvents: reconciledApprovalRecords(events, traceEvents),
    exports: []
  };
}

function transcriptFromCanonicalModelEvent(
  session: AppServerSessionRecord,
  event: AppServerSessionEvent
): TranscriptMessageRecord | null {
  if (event.kind !== 'model.output' && event.kind !== 'model.thought') return null;
  const payload = recordValue(event.payload);
  if (!payload || stringValue(payload.phase) !== 'completed') return null;
  const contentMarkdown = stringValue(payload.text)?.trim();
  if (!contentMarkdown) return null;
  const thought = event.kind === 'model.thought';
  const messagePhase = stringValue(payload.messagePhase);
  const phase = thought ? undefined : messagePhase === 'commentary' ? 'commentary' : 'final_answer';
  const source = thought
    ? 'openai_reasoning_summary'
    : phase === 'commentary' ? 'app_server_commentary' : 'app-server';
  const agentPath = stringValue(payload.agentPath) ?? event.agentPath ?? '/root';
  return {
    id: `transcript_${event.id}`,
    runId: session.id,
    attemptId: stringValue(payload.attemptId) ?? attemptIdForCanonicalEvent(session, event.timestamp),
    traceEventId: event.id,
    role: 'assistant',
    ...(phase ? { phase } : {}),
    contentMarkdown,
    source,
    metadata: {
      agentPath,
      agentId: stringValue(payload.agentId) ?? event.agentId,
      parentAgentId: stringValue(payload.parentAgentId) ?? event.parentAgentId,
      responseId: stringValue(payload.responseId),
      itemId: stringValue(payload.itemId),
      appServerEventId: event.id,
      turn: typeof payload.turn === 'number' ? payload.turn : undefined,
      provider: stringValue(payload.provider),
      model: stringValue(payload.model),
      ...(thought ? { phase: 'progress' } : { messagePhase: phase })
    },
    createdAt: event.timestamp
  };
}

function attemptIdForCanonicalEvent(session: AppServerSessionRecord, timestamp: string): string | null {
  return [...session.attempts].reverse().find((attempt) => (
    attempt.startedAt <= timestamp && (!attempt.endedAt || timestamp <= attempt.endedAt)
  ))?.id ?? session.attempts.at(-1)?.id ?? null;
}

function transcriptCorrelationKey(message: TranscriptMessageRecord): string | null {
  if (message.source !== 'app_server_commentary'
    && message.source !== 'app-server'
    && message.source !== 'openai_reasoning_summary') return null;
  const responseId = stringValue(message.metadata.responseId);
  const itemId = stringValue(message.metadata.itemId);
  const appServerEventId = stringValue(message.metadata.appServerEventId);
  const turn = typeof message.metadata.turn === 'number' ? String(message.metadata.turn) : '';
  const content = message.contentMarkdown.replace(/\s+/g, ' ').trim();
  if ((!responseId && !itemId && !appServerEventId) || !content) return null;
  return [
    message.source,
    stringValue(message.metadata.agentPath) ?? '/root',
    responseId ?? '',
    itemId ?? '',
    responseId ? '' : appServerEventId ?? turn,
    content
  ].join('\u0000');
}

function uniqueCorrelatedTranscripts(messages: readonly TranscriptMessageRecord[]): TranscriptMessageRecord[] {
  const seen = new Set<string>();
  return messages.filter((message) => {
    const key = transcriptCorrelationKey(message);
    if (!key || !seen.has(key)) {
      if (key) seen.add(key);
      return true;
    }
    return false;
  });
}

function sessionNextStepSuggestions(session: AppServerSessionRecord): GeneratedResearchGoalSuggestions | null {
  for (let index = session.events.length - 1; index >= 0; index -= 1) {
    const event = session.events[index];
    if (event?.kind !== 'beale.session_next_step_suggestions') continue;
    const normalized = normalizeSessionNextStepSuggestions(recordValue(event.payload)?.record);
    if (normalized) return normalized;
  }
  return null;
}

function traceFromSessionEvent(runId: string, event: AppServerSessionEvent, sequence: number): TraceEventRecord[] {
  if (event.kind === 'beale.trace_batch') {
    const records = recordValue(event.payload)?.records;
    if (!Array.isArray(records)) return [];
    const count = records.length;
    return records.flatMap((candidate, index) => {
      const stored = recordValue(candidate);
      return stored ? [storedTraceEvent(stored, event.id, sequence + ((index + 1) / (count + 1)))] : [];
    });
  }
  const stored = event.kind === 'beale.trace' ? recordValue(recordValue(event.payload)?.record) : null;
  if (stored) return [storedTraceEvent(stored, event.id)];
  const eventPayload = recordValue(event.payload);
  const researchEvent = event.kind === 'research.event' ? recordValue(eventPayload?.event) : null;
  const researchKind = stringValue(researchEvent?.kind);
  const researchPayload = recordValue(researchEvent?.payload);
  if (researchEvent && (researchKind === 'tool.requested' || researchKind === 'tool.observed') && researchPayload) {
    const toolName = stringValue(researchPayload.toolName) ?? 'tool';
    const agentId = stringValue(researchEvent.agentId) ?? stringValue(eventPayload?.agentId) ?? event.agentId;
    const agentPath = stringValue(researchEvent.agentPath) ?? stringValue(eventPayload?.agentPath) ?? event.agentPath;
    const parentAgentId = stringValue(researchEvent.parentAgentId)
      ?? stringValue(eventPayload?.parentAgentId)
      ?? event.parentAgentId;
    return [{
      id: event.id,
      runId,
      attemptId: stringValue(eventPayload?.attemptId),
      sequence,
      type: researchKind === 'tool.requested' ? 'tool_call' : 'tool_result',
      source: researchKind === 'tool.requested' ? 'model' : 'tool',
      summary: `app-server ${researchKind}: ${toolName}`,
      payload: {
        appServerEventId: event.id,
        appServerKind: researchKind,
        appServerTimestamp: event.timestamp,
        toolName,
        payload: researchPayload,
        ...(agentId ? { agentId } : {}),
        ...(agentPath ? { agentPath } : {}),
        ...(parentAgentId ? { parentAgentId } : {})
      },
      sensitivity: 'internal',
      modelVisible: true,
      createdAt: stringValue(researchEvent.timestamp) ?? event.timestamp,
      artifactId: null,
      toolCallId: stringValue(researchPayload.toolActionId),
      approvalId: null
    }];
  }
  return [{
    id: event.id,
    runId,
    attemptId: stringValue(eventPayload?.attemptId),
    sequence,
    type: 'research_event',
    source: event.kind === 'session.recovery' ? 'system' : 'executor',
    summary: event.summary,
    payload: {
      appServerEventId: event.id,
      appServerKind: event.kind,
      appServerTimestamp: event.timestamp,
      payload: event.payload,
      ...(event.kind === 'session.recovery' && eventPayload ? eventPayload : {}),
      ...(event.agentId ? { agentId: event.agentId } : {}),
      ...(event.agentPath ? { agentPath: event.agentPath } : {}),
      ...(event.parentAgentId ? { parentAgentId: event.parentAgentId } : {})
    },
    sensitivity: 'internal',
    modelVisible: event.kind !== 'session.recovery',
    createdAt: event.timestamp,
    artifactId: null,
    toolCallId: null,
    approvalId: null
  }];
}

function storedTraceEvent(
  stored: Record<string, unknown>,
  sessionEventId: string,
  fallbackSequence?: number
): TraceEventRecord {
  const trace = stored as unknown as TraceEventRecord;
  return {
    ...trace,
    sequence: fallbackSequence ?? (typeof trace.sequence === 'number' ? trace.sequence : 0),
    payload: {
      ...(recordValue(trace.payload) ?? {}),
      appServerSessionEventId: sessionEventId
    }
  };
}

function sessionRecovery(session: AppServerSessionRecord | AppServerSessionSummary): { recoveredAt: string; reason: string; attemptId: string | null } | null {
  if (session.metadata.interruptedByRecovery !== true) return null;
  const recoveredAt = stringValue(session.metadata.recoveredAt);
  if (!recoveredAt) return null;
  const attemptIds = Array.isArray(session.metadata.recoveredAttemptIds)
    ? session.metadata.recoveredAttemptIds.filter((value): value is string => typeof value === 'string' && value.length > 0)
    : [];
  return {
    recoveredAt,
    reason: stringValue(session.metadata.recoveryReason) ?? 'workspace_open',
    attemptId: attemptIds.at(-1) ?? null
  };
}

function ownedRunIdForAttempt(ownedRunIds: ReadonlySet<string>, storage: AppServerSessionStorage, attemptId: string): string | null {
  for (const runId of ownedRunIds) {
    if (getAppServerSession(runId, storage).attempts.some((attempt) => attempt.id === attemptId)) return runId;
  }
  return null;
}

function recordArrayValue<T>(payload: unknown): T[] {
  const record = recordValue(payload);
  return record?.record && isRecord(record.record) ? [record.record as unknown as T] : [];
}

function latestSessionMessageAt(
  persisted: string | null,
  queuedEvents: readonly AppServerSessionEvent[]
): string | null {
  for (let index = queuedEvents.length - 1; index >= 0; index -= 1) {
    const event = queuedEvents[index];
    if (event?.kind !== 'beale.transcript') continue;
    const message = recordArrayValue<TranscriptMessageRecord>(event.payload)[0];
    return message?.createdAt ?? event.timestamp;
  }
  return persisted;
}

function latestRecords<T extends { id: string }>(records: readonly T[]): T[] {
  const latest = new Map<string, T>();
  for (const record of records) latest.set(record.id, record);
  return [...latest.values()];
}

function materializedModelSessions(events: readonly AppServerSessionEvent[]): ModelSessionRecord[] {
  const sessions = new Map<string, ModelSessionRecord>();
  let latestSessionId: string | null = null;
  for (const event of events) {
    if (event.kind === 'beale.model_session') {
      for (const session of recordArrayValue<ModelSessionRecord>(event.payload)) {
        sessions.set(session.id, session);
        latestSessionId = session.id;
      }
      continue;
    }
    if (event.kind !== 'beale.model_session_update' || !latestSessionId) continue;
    const update = recordArrayValue<{
      id: string;
      patch?: Record<string, unknown>;
      createdAt?: string;
    }>(event.payload)[0];
    const current = sessions.get(latestSessionId);
    const patch = recordValue(update?.patch);
    if (!current || !patch) continue;
    const metadata = recordValue(patch.metadata);
    sessions.set(latestSessionId, {
      ...current,
      ...(Object.prototype.hasOwnProperty.call(patch, 'previousResponseId')
        ? { previousResponseId: stringValue(patch.previousResponseId) }
        : {}),
      ...(stringValue(patch.status) ? { status: stringValue(patch.status)! } : {}),
      ...(metadata ? { metadata: { ...current.metadata, ...metadata } } : {}),
      updatedAt: stringValue(update?.createdAt) ?? event.timestamp
    });
  }
  return [...sessions.values()];
}

function reconciledApprovalRecords(
  events: readonly AppServerSessionEvent[],
  traceEvents: readonly TraceEventRecord[]
): ApprovalRecord[] {
  const approvals = latestRecords(events.flatMap((event) =>
    event.kind === 'beale.approval' ? recordArrayValue<ApprovalRecord>(event.payload) : []
  ));
  const resolutions = new Map<string, { decision: 'approved' | 'denied'; reason: string; decidedAt: string }>();
  for (const trace of traceEvents) {
    if (trace.type !== 'approval_event') continue;
    const approvalRequestId = stringValue(trace.payload.approvalRequestId);
    const decision = stringValue(trace.payload.decision);
    if (!approvalRequestId || (decision !== 'approved' && decision !== 'denied')) continue;
    resolutions.set(approvalRequestId, {
      decision,
      reason: stringValue(trace.payload.reason) ?? trace.summary,
      decidedAt: trace.createdAt
    });
  }
  return approvals.map((approval) => {
    if (approval.decision !== 'pending' || approval.decidedAt !== null) return approval;
    const approvalRequestId = stringValue(approval.requestedAction.approvalRequestId);
    const resolution = approvalRequestId ? resolutions.get(approvalRequestId) : undefined;
    return resolution ? { ...approval, ...resolution } : approval;
  });
}

function sessionNotifications(session: AppServerSessionRecord): NotificationRecord[] {
  return latestRecords(session.events.flatMap((event) => event.kind === 'beale.notification'
    ? recordArrayValue<NotificationRecord>(event.payload)
    : []));
}

function mergeTranscriptSearch(
  input: SessionTranscriptSearchInput,
  contexts: readonly {
    databaseWorkspaceId: string;
    registryWorkspaceId: string;
    workspacePath: string;
    workspaceName: string;
  }[],
  legacy: SessionTranscriptSearchResponse,
  storage: AppServerSessionStorage,
  database: WorkspaceDatabase
): SessionTranscriptSearchResponse {
  const query = input.query.trim().toLowerCase();
  if (!query) return legacy;
  const limit = Math.max(1, Math.floor(input.limit ?? 24));
  const canonicalResults = contexts.flatMap((context) => listAppServerSessions(context.databaseWorkspaceId, storage, 500)
    .flatMap((session) => {
      const page = getAppServerSessionEventPage(session.id, storage, {
        stream: 'transcript',
        tail: true,
        limit: 2_000,
        maxBytes: 8 * 1024 * 1024
      });
      return sessionDetail({ ...session, events: page.events }, database, page.eventOffset).transcriptMessages
      .filter((message) => message.contentMarkdown.toLowerCase().includes(query))
      .map((message) => ({
        registryWorkspaceId: context.registryWorkspaceId,
        workspacePath: context.workspacePath,
        runId: session.id,
        transcriptMessageId: message.id,
        traceEventId: message.traceEventId,
        role: message.role,
        source: message.source,
        sessionTitle: session.title,
        workspaceName: context.workspaceName,
        contentPreview: message.contentMarkdown.slice(0, 500),
        createdAt: message.createdAt
      }));
    }));
  const canonicalCounts = new Map<string, number>();
  for (const result of canonicalResults) {
    canonicalCounts.set(result.registryWorkspaceId, (canonicalCounts.get(result.registryWorkspaceId) ?? 0) + 1);
  }
  const canonicalWorkspaces = contexts.flatMap((context) => {
    const totalTranscriptMatches = canonicalCounts.get(context.registryWorkspaceId) ?? 0;
    return totalTranscriptMatches > 0
      ? [{
          registryWorkspaceId: context.registryWorkspaceId,
          workspacePath: context.workspacePath,
          workspaceName: context.workspaceName,
          totalTranscriptMatches
        }]
      : [];
  });
  const workspaceTotals = new Map<string, SessionTranscriptSearchResponse['workspaces'][number]>();
  for (const workspace of [...legacy.workspaces, ...canonicalWorkspaces]) {
    const existing = workspaceTotals.get(workspace.registryWorkspaceId);
    workspaceTotals.set(workspace.registryWorkspaceId, {
      ...workspace,
      totalTranscriptMatches: workspace.totalTranscriptMatches + (existing?.totalTranscriptMatches ?? 0)
    });
  }
  const results = [...canonicalResults, ...legacy.results]
    .sort((left, right) => Date.parse(right.createdAt) - Date.parse(left.createdAt))
    .slice(0, limit);
  const workspaces = [...workspaceTotals.values()];
  return {
    results,
    totalTranscriptMatches: workspaces.reduce((total, workspace) => total + workspace.totalTranscriptMatches, 0),
    workspaceCount: workspaces.length,
    workspaces
  };
}

function sessionStatus(status: string): RunRecord['status'] {
  return status === 'paused' || status === 'blocked' || status === 'completed' || status === 'failed' || status === 'stopped'
    ? status
    : 'active';
}

function stringValue(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value : null;
}

function recordValue(value: unknown): Record<string, unknown> | null {
  return isRecord(value) ? value : null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}
