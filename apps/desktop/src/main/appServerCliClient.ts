import { spawn, spawnSync } from 'node:child_process';
import { Worker } from 'node:worker_threads';
import { randomUUID } from 'node:crypto';
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { resolveAppServerProtocolInvocation } from './appServerInvocation';
import { invokeAppServerOperation } from './bealeAppServerClient';
import { appServerProtocolSuccess, type AppServerProtocolOperation } from '@beale/app-server-runtime/protocol';
import {
  compatibleExistingPath,
  PRE_BEALE_DATA_DIRECTORY_NAME,
  preBealeRuntimeId
} from '@beale/research-agent/legacy-compatibility';
import type {
  AppServerMemorySummary,
  AppServerMemoryEdgeSummary,
  AppServerMemoryNodeSummary,
  AppServerReportDocument,
  AppServerReportSummary,
  AppServerRunbookDocument,
  MemoryDreamingRunSummary,
  ResearchProfileSnapshot,
  AgentPluginRegistryState,
  RepositoryCloneMode,
  WorkspaceDejunkSummary
} from '@shared/types';
import type { ResolvedResearchProfile } from '../shared/researchProfile';
import {
  decodeAppServerProtocolDescriptor,
  decodeAppServerProtocolEnvelope,
  type AppServerProtocolDescriptor,
  type AppServerProtocolEnvelope,
  type AppServerProtocolSuccess
} from './appServerProtocol';
export {
  APP_SERVER_PROTOCOL_VERSION,
  APP_SERVER_PROTOCOL_WEBSOCKET_PATH,
  decodeAppServerProtocolEnvelope
} from './appServerProtocol';
export type {
  AppServerProtocolDescriptor,
  AppServerProtocolEnvelope,
  AppServerProtocolFailure,
  AppServerProtocolSuccess
} from './appServerProtocol';

export type AppServerSessionStatus = 'active' | 'paused' | 'blocked' | 'completed' | 'failed' | 'stopped';

export interface AppServerSessionTokenUsage {
  totalTokens: number;
  totalCostUsd?: number;
  inputTokens?: number;
  outputTokens?: number;
  cacheReadTokens?: number;
  cachePromptTokens?: number;
}

export interface AppServerSessionActivityCounts {
  memorySearches: number;
  memoryUpdates: number;
}

export interface AppServerSessionEvent {
  id: string;
  kind: string;
  timestamp: string;
  summary: string;
  payload: unknown;
  agentId?: string;
  agentPath?: string;
  parentAgentId?: string;
}

export interface AppServerSessionAttempt {
  id: string;
  parentAttemptId: string | null;
  status: AppServerSessionStatus;
  summary: string;
  startedAt: string;
  endedAt: string | null;
  capture: Record<string, unknown> | null;
  metadata: Record<string, unknown>;
}

export interface AppServerSessionRecord {
  schemaVersion: 1;
  id: string;
  workspaceId: string;
  status: AppServerSessionStatus;
  title: string;
  prompt: string;
  summary: string;
  provider: string | null;
  model: string;
  reasoningEffort: string;
  workflowId: string | null;
  profile: Record<string, unknown> | null;
  metadata: Record<string, unknown>;
  finalDisposition: Record<string, unknown> | null;
  finalResponse: string | null;
  attempts: AppServerSessionAttempt[];
  events: AppServerSessionEvent[];
  createdAt: string;
  startedAt: string;
  endedAt: string | null;
  updatedAt: string;
  revision: number;
  /** Present when materialized from a canonical session summary/update. */
  tokenUsage?: AppServerSessionTokenUsage;
  /** Present when materialized from a canonical session summary/update. */
  activityCounts?: AppServerSessionActivityCounts;
}

export type AppServerSessionSummary = Omit<
  AppServerSessionRecord,
  'attempts' | 'events' | 'finalResponse'
> & {
  attempts: Array<Omit<AppServerSessionAttempt, 'capture'>>;
  lastMessageAt: string | null;
  tokenUsage?: AppServerSessionTokenUsage;
  activityCounts?: AppServerSessionActivityCounts;
};

export interface AppServerSessionUpdate {
  session: AppServerSessionSummary;
  finalResponse: string | null;
  events: AppServerSessionEvent[];
  eventOffset: number;
  nextAfterEventId: string | null;
  hasEarlier: boolean;
  hasMore: boolean;
}

export interface AppServerSessionEventPage {
  sessionId: string;
  stream: 'all' | 'transcript' | 'trace';
  events: AppServerSessionEvent[];
  eventOffset: number;
  nextAfterEventId: string | null;
  hasEarlier: boolean;
  hasMore: boolean;
}

export interface AppServerSessionCollaborationState {
  sessionId: string;
  revision: number;
  rooms: AppServerSessionEvent[];
  members: AppServerSessionEvent[];
  messages: AppServerSessionEvent[];
  subagents: AppServerSessionEvent[];
}

export interface AppServerSessionCaptureSummary {
  attemptId: string;
  capturedAt: string;
  schemaVersion: number;
  sizeBytes: number;
  contentHash: string;
  eventStreams: Record<string, unknown>;
}

export interface AppServerSessionMutationReceipt {
  sessionId: string;
  status: AppServerSessionStatus;
  revision: number;
  updatedAt: string;
}

export interface AppServerSessionStorage {
  databasePath: string;
  artifactDirectoryPath: string;
  profileId?: string;
}

export interface AppServerSessionRecoveryReport {
  workspaceId: string;
  recoveredAt: string;
  reason: string;
  interruptedSessions: number;
  interruptedAttempts: number;
  sessionIds: string[];
}

export function resolveAppServerStoragePaths(
  profileId: string,
  options: { databasePath?: string; artifactDirectoryPath?: string; registryDirectory?: string } = {}
): AppServerSessionStorage {
  const canonicalDatabasePath = options.databasePath
    ? profileId === 'security-research'
      ? resolve(options.databasePath)
      : join(dirname(resolve(options.databasePath)), 'profiles', profileId, 'memory.sqlite')
    : options.registryDirectory
      ? resolve(options.registryDirectory, 'app-server', 'profiles', profileId, 'memory.sqlite')
      : join(homedir(), '.beale', 'profiles', profileId, 'memory.sqlite');
  const previousDatabasePath = options.databasePath
    ? canonicalDatabasePath
    : options.registryDirectory
      ? resolve(options.registryDirectory, preBealeRuntimeId(), 'profiles', profileId, 'memory.sqlite')
      : join(homedir(), PRE_BEALE_DATA_DIRECTORY_NAME, 'profiles', profileId, 'memory.sqlite');
  const databasePath = compatibleExistingPath(canonicalDatabasePath, previousDatabasePath);
  const artifactDirectoryPath = options.artifactDirectoryPath
    ? profileId === 'security-research'
      ? resolve(options.artifactDirectoryPath)
      : join(dirname(resolve(options.artifactDirectoryPath)), profileId, 'artifacts')
    : join(dirname(databasePath), 'artifacts');
  return { databasePath, artifactDirectoryPath, profileId };
}

export type MemoryDreamingProfileInput =
  | { profileSnapshot: ResearchProfileSnapshot; resolvedProfile?: never }
  | { resolvedProfile: ResolvedResearchProfile; profileSnapshot?: never };

export interface MemoryDreamingPlan {
  prune: Array<{ nodeId: string; reason: string }>;
  merge: Array<{
    survivorNodeId: string;
    duplicateNodeIds: string[];
    summary: string | null;
    body: string | null;
    attributes?: Record<string, string | number | boolean>;
    reason: string;
  }>;
  revise: Array<{
    nodeId: string;
    summary: string | null;
    body: string | null;
    attributes?: Record<string, string | number | boolean>;
    reason: string;
  }>;
  reclassify: Array<{
    nodeId: string;
    type: string;
    attributes?: Record<string, string | number | boolean>;
    reason: string;
  }>;
}

export interface MemoryDreamingRunContext {
  provider: string;
  model: string;
  reasoningEffort: string;
  inputNodeCount: number;
  inputSessionCount: number;
}

export interface AppServerDreamingPreparation {
  instructions: string;
  typeDescriptions: Record<string, string>;
  modelJobDefaults: { size: string; reasoningEffort: string } | null;
  inputTexts: string[];
}

export interface AppServerDreamingSessionInput {
  id: string;
  title: string;
  status: string;
  createdAt: string;
  endedAt: string | null;
  prompt: string;
  finalSummary: string;
  transcript: Array<{ role: string; source: string; createdAt: string; content: string }>;
}

export interface AppServerResolvedArtifact {
  id: string;
  kind: string;
  purpose: string;
  path: string;
  relativePath: string;
  uri: string;
  sizeBytes: number;
  contentHash: string;
  createdAt: string;
  updatedAt: string;
}

export interface AppServerAuxiliaryModelRoute {
  provider: 'openai-codex' | 'anthropic' | 'xai' | 'zai' | 'openrouter';
  model: string;
  effort: 'off' | 'minimal' | 'low' | 'medium' | 'high' | 'xhigh' | 'max';
}

export interface AppServerProviderSemantics {
  providers: Array<'openai-codex' | 'anthropic' | 'xai' | 'zai' | 'openrouter'>;
  aliases: Record<string, 'openai-codex' | 'anthropic' | 'xai' | 'zai' | 'openrouter'>;
  defaultSmallModels: Record<'openai-codex' | 'anthropic' | 'xai' | 'zai' | 'openrouter', string>;
  auxiliaryEfforts: Array<'off' | 'minimal' | 'low' | 'medium' | 'high' | 'xhigh' | 'max'>;
  sessionTitleEffort: 'medium';
  shellReviewEffort: 'medium';
}

let providerSemanticsCache: AppServerProviderSemantics | null = null;
let compatibleProtocolCache: { invocationKey: string; descriptor: AppServerProtocolDescriptor } | null = null;
const APP_SERVER_PROTOCOL_MAX_STDOUT_BYTES = 64 * 1024 * 1024;
const APP_SERVER_PROTOCOL_MAX_STDERR_CHARS = 2_000_000;

export interface AppServerSourceRepositoryCandidate {
  url: string;
  label: string;
  sourceAssetId: string;
  sourceAssetKind: string;
  sensitivity: string;
  clonedDirectory: string | null;
}

export interface AppServerMaterializedSourceRepository {
  repositoryUrl: string;
  localPath: string;
  cloned: boolean;
  ref: string | null;
  head: string | null;
  headRefName: string | null;
  headDescribe: string | null;
  requestedRefHead: string | null;
  requestedRefMatchesHead: boolean | null;
  cloneMode: RepositoryCloneMode;
}

export interface AppServerWorkspaceRepositoryCandidate {
  path: string;
  repositoryUrl?: string;
  ref?: string;
}

export interface AppServerWorkspaceRepositoryRelocation {
  fromPath: string;
  toPath: string;
  repositoryUrl: string | null;
}

export interface AppServerMaintenanceRunResult {
  summary: WorkspaceDejunkSummary;
  repositoryRelocations: AppServerWorkspaceRepositoryRelocation[];
}

export interface AppServerAgentPluginRuntime {
  runtimeDirectory: string;
  skillDirs: string[];
  selectedSkillIds: string[];
  mcpConfigPath: string | null;
  allowedMcpServers: string[];
  args: string[];
  warnings: string[];
}

export interface AppServerBuiltinPlugin {
  id: string;
  path: string;
  installedAt: string;
  enabledByDefault?: boolean;
}

export function getAppServerProtocolDescriptor(): AppServerProtocolDescriptor {
  const envelope = invokeAppServerCliProtocol<AppServerProtocolDescriptor>(
    'protocol.describe',
    ['protocol', 'describe', '--json']
  );
  return decodeAppServerProtocolDescriptor(envelope.result);
}

export function assertAppServerProtocolCompatibility(): AppServerProtocolDescriptor {
  const invocationKey = appServerInvocationKey();
  if (compatibleProtocolCache?.invocationKey === invocationKey) return compatibleProtocolCache.descriptor;
  const descriptor = getAppServerProtocolDescriptor();
  compatibleProtocolCache = { invocationKey, descriptor };
  return descriptor;
}

export function appServerOwnsSessions(): boolean {
  // The app-server is the canonical app-server host. Session ownership is no
  // longer conditional on probing a separately installed CLI executable.
  return true;
}

export function createAppServerSession(
  input: Record<string, unknown>,
  storage: AppServerSessionStorage
): AppServerSessionRecord {
  return invokeWithJsonInput<AppServerSessionRecord>('session.create', ['session', 'create'], input, storage).result;
}

export function beginAppServerSessionAttempt(
  sessionId: string,
  input: Record<string, unknown>,
  storage: AppServerSessionStorage
): AppServerSessionRecord {
  return invokeWithJsonInput<AppServerSessionRecord>(
    'session.begin_attempt',
    ['session', 'begin-attempt', '--session-id', sessionId],
    input,
    storage
  ).result;
}

export function appendAppServerSessionEvent(
  sessionId: string,
  input: AppServerSessionEvent,
  storage: AppServerSessionStorage
): AppServerSessionMutationReceipt {
  return invokeWithJsonInput<AppServerSessionMutationReceipt>(
    'session.append_event_receipt',
    ['session', 'append-event-receipt', '--session-id', sessionId],
    input,
    storage
  ).result;
}

export async function appendAppServerSessionEventAsync(
  sessionId: string,
  input: AppServerSessionEvent,
  storage: AppServerSessionStorage,
  signal?: AbortSignal
): Promise<AppServerSessionMutationReceipt> {
  return (await invokeWithJsonInputAsync<AppServerSessionMutationReceipt>(
    'session.append_event_receipt',
    ['session', 'append-event-receipt', '--session-id', sessionId],
    input,
    storage,
    signal,
    30_000
  )).result;
}

export function transitionAppServerSession(
  sessionId: string,
  input: Record<string, unknown>,
  storage: AppServerSessionStorage
): AppServerSessionRecord {
  return invokeWithJsonInput<AppServerSessionRecord>(
    'session.transition',
    ['session', 'transition', '--session-id', sessionId],
    input,
    storage
  ).result;
}

export function recoverInterruptedAppServerSessions(
  workspaceId: string,
  input: { reason?: string; at?: string },
  storage: AppServerSessionStorage
): AppServerSessionRecoveryReport {
  return invokeWithJsonInput<AppServerSessionRecoveryReport>(
    'session.recover_interrupted',
    ['session', 'recover-interrupted', '--workspace-id', workspaceId],
    input,
    storage
  ).result;
}

export async function recoverInterruptedAppServerSessionsAsync(
  workspaceId: string,
  input: { reason?: string; at?: string },
  storage: AppServerSessionStorage
): Promise<AppServerSessionRecoveryReport> {
  return (await invokeWithJsonInputAsync<AppServerSessionRecoveryReport>(
    'session.recover_interrupted',
    ['session', 'recover-interrupted', '--workspace-id', workspaceId],
    input,
    storage,
    undefined,
    30_000
  )).result;
}

export function getAppServerSession(sessionId: string, storage: AppServerSessionStorage): AppServerSessionRecord {
  const summary = invokeAppServerCliProtocol<AppServerSessionSummary>(
    'session.get',
    ['session', 'get', '--session-id', sessionId, '--json'],
    { env: storageEnvironment(storage) }
  ).result;
  return sessionRecordFromSummary(summary);
}

export async function getAppServerSessionAsync(
  sessionId: string,
  storage: AppServerSessionStorage,
  signal?: AbortSignal
): Promise<AppServerSessionRecord> {
  const summary = (await invokeAppServerCliProtocolAsync<AppServerSessionSummary>(
    'session.get',
    ['session', 'get', '--session-id', sessionId, '--json'],
    { env: storageEnvironment(storage), timeoutMs: 30_000, ...(signal ? { signal } : {}) }
  )).result;
  return sessionRecordFromSummary(summary);
}

export function getAppServerSessionUpdate(
  sessionId: string,
  afterEventId: string | null,
  storage: AppServerSessionStorage,
  options: { tail?: boolean; limit?: number; maxBytes?: number } = {}
): AppServerSessionUpdate {
  return invokeAppServerCliProtocol<AppServerSessionUpdate>(
    'session.get_update',
    sessionUpdateArguments(sessionId, afterEventId, options),
    { env: storageEnvironment(storage) }
  ).result;
}

export async function getAppServerSessionUpdateAsync(
  sessionId: string,
  afterEventId: string | null,
  storage: AppServerSessionStorage,
  signal?: AbortSignal,
  options: { tail?: boolean; limit?: number; maxBytes?: number } = {}
): Promise<AppServerSessionUpdate> {
  return (await invokeAppServerCliProtocolAsync<AppServerSessionUpdate>(
    'session.get_update',
    sessionUpdateArguments(sessionId, afterEventId, options),
    { env: storageEnvironment(storage), timeoutMs: 30_000, ...(signal ? { signal } : {}) }
  )).result;
}

export function getAppServerSessionCollaborationState(
  sessionId: string,
  storage: AppServerSessionStorage,
  messageLimit = 200
): AppServerSessionCollaborationState {
  return invokeAppServerCliProtocol<AppServerSessionCollaborationState>(
    'session.collaboration',
    ['session', 'collaboration', '--session-id', sessionId, '--message-limit', String(messageLimit), '--json'],
    { env: storageEnvironment(storage) }
  ).result;
}

export function getAppServerSessionEventPage(
  sessionId: string,
  storage: AppServerSessionStorage,
  options: {
    stream?: 'all' | 'transcript' | 'trace';
    afterEventId?: string;
    tail?: boolean;
    limit?: number;
    maxBytes?: number;
  } = {}
): AppServerSessionEventPage {
  return invokeAppServerCliProtocol<AppServerSessionEventPage>(
    'session.events',
    [
      'session', 'events', '--session-id', sessionId,
      ...(options.stream ? ['--stream', options.stream] : []),
      ...(options.afterEventId ? ['--after-event-id', options.afterEventId] : []),
      ...(options.tail ? ['--tail'] : []),
      ...(options.limit ? ['--limit', String(options.limit)] : []),
      ...(options.maxBytes ? ['--max-bytes', String(options.maxBytes)] : []),
      '--json'
    ],
    { env: storageEnvironment(storage) }
  ).result;
}

export async function getAppServerSessionEventDetailsAsync(
  sessionId: string,
  eventIds: readonly string[],
  storage: AppServerSessionStorage,
  signal?: AbortSignal
): Promise<AppServerSessionEvent[]> {
  if (eventIds.length === 0) return [];
  return (await invokeAppServerCliProtocolAsync<AppServerSessionEvent[]>(
    'session.event_details',
    ['session', 'event-details', '--session-id', sessionId, ...eventIds.flatMap((id) => ['--event-id', id]), '--json'],
    { env: storageEnvironment(storage), timeoutMs: 30_000, ...(signal ? { signal } : {}) }
  )).result;
}

export async function listAppServerSessionCaptureSummariesAsync(
  sessionId: string,
  storage: AppServerSessionStorage,
  signal?: AbortSignal
): Promise<AppServerSessionCaptureSummary[]> {
  return (await invokeAppServerCliProtocolAsync<AppServerSessionCaptureSummary[]>(
    'session.captures',
    ['session', 'captures', '--session-id', sessionId, '--json'],
    { env: storageEnvironment(storage), timeoutMs: 30_000, ...(signal ? { signal } : {}) }
  )).result;
}

export function listAppServerSessions(
  workspaceId: string,
  storage: AppServerSessionStorage,
  limit = 100
): AppServerSessionRecord[] {
  return invokeAppServerCliProtocol<AppServerSessionSummary[]>(
    'session.list',
    ['session', 'list', '--workspace-id', workspaceId, '--limit', String(limit), '--json'],
    { env: storageEnvironment(storage) }
  ).result.map(sessionRecordFromSummary);
}

export function listAppServerSessionSummaries(
  workspaceId: string,
  storage: AppServerSessionStorage,
  limit = 100
): AppServerSessionSummary[] {
  return invokeAppServerCliProtocol<AppServerSessionSummary[]>(
    'session.list_summaries',
    ['session', 'list-summaries', '--workspace-id', workspaceId, '--limit', String(limit), '--json'],
    { env: storageEnvironment(storage) }
  ).result;
}

export async function listAppServerSessionSummariesAsync(
  workspaceId: string,
  storage: AppServerSessionStorage,
  limit = 200
): Promise<AppServerSessionSummary[]> {
  return (await invokeAppServerCliProtocolAsync<AppServerSessionSummary[]>(
    'session.list_summaries',
    ['session', 'list-summaries', '--workspace-id', workspaceId, '--limit', String(limit), '--json'],
    { env: storageEnvironment(storage), timeoutMs: 30_000 }
  )).result;
}

export async function listAppServerSessionSummariesForWorkspacesAsync(
  workspaceIds: readonly string[],
  storage: AppServerSessionStorage,
  limitPerWorkspace = 200
): Promise<AppServerSessionSummary[]> {
  const normalizedWorkspaceIds = [...new Set(workspaceIds.map((workspaceId) => workspaceId.trim()).filter(Boolean))];
  if (normalizedWorkspaceIds.length === 0) return [];
  return (await invokeAppServerCliProtocolAsync<AppServerSessionSummary[]>(
    'session.list_summaries',
    [
      'session',
      'list-summaries',
      ...normalizedWorkspaceIds.flatMap((workspaceId) => ['--workspace-id', workspaceId]),
      '--limit',
      String(limitPerWorkspace),
      '--json'
    ],
    { env: storageEnvironment(storage), timeoutMs: 30_000 }
  )).result;
}

export function getAppServerMemorySummary(
  input: {
    workspaceId: string;
    subjectId: string | null;
    sessionId?: string;
    researchProfile?: ResearchProfileSnapshot | null;
    includeForeignCatalogs?: boolean;
    assetIds?: string[];
  },
  storage: AppServerSessionStorage
): AppServerMemorySummary {
  return decodeAppServerMemorySummary(
    invokeWithJsonInput<unknown>('memory.summary', ['knowledge', 'summary'], input, storage).result
  );
}

export async function getAppServerMemorySummaryAsync(
  input: {
    workspaceId: string;
    workspaceRoot?: string;
    researchProfileId?: string;
    subjectId: string | null;
    sessionId?: string;
    researchProfile?: ResearchProfileSnapshot | null;
    includeForeignCatalogs?: boolean;
    assetIds?: string[];
  },
  storage: AppServerSessionStorage,
  signal?: AbortSignal
): Promise<AppServerMemorySummary> {
  return decodeAppServerMemorySummary((await invokeWithJsonInputAsync<unknown>(
    'memory.summary',
    ['knowledge', 'summary'],
    input,
    storage,
    signal,
    10_000
  )).result);
}

export async function listAppServerReportSummariesAsync(
  input: {
    workspaceId: string;
    workspaceRoot?: string;
    researchProfileId?: string;
  },
  storage: AppServerSessionStorage,
  signal?: AbortSignal
): Promise<AppServerReportSummary[]> {
  const value = (await invokeWithJsonInputAsync<unknown>(
    'report.list',
    ['knowledge', 'report-list'],
    input,
    storage,
    signal,
    10_000
  )).result;
  if (!Array.isArray(value) || !value.every(validReportSummary)) {
    throw new Error('app-server returned an invalid report catalog payload.');
  }
  return value as AppServerReportSummary[];
}

export function prepareAppServerMemoryDreaming(
  typeDescriptions: Record<string, string>,
  profileInput: MemoryDreamingProfileInput,
  nodes: AppServerMemoryNodeSummary[],
  edges: AppServerMemoryEdgeSummary[],
  sessions: AppServerDreamingSessionInput[],
  storage: AppServerSessionStorage
): AppServerDreamingPreparation {
  return invokeWithJsonInput<AppServerDreamingPreparation>(
    'dreaming.prepare',
    ['knowledge', 'dreaming-prepare'],
    { typeDescriptions, profileInput, nodes, edges, sessions },
    storage
  ).result;
}

export function parseAppServerMemoryDreamingPlan(
  output: string,
  profileInput: MemoryDreamingProfileInput,
  storage: AppServerSessionStorage
): MemoryDreamingPlan {
  return invokeWithJsonInput<MemoryDreamingPlan>(
    'dreaming.parse_plan',
    ['knowledge', 'dreaming-parse-plan'],
    { output, profileInput },
    storage
  ).result;
}

export function applyAppServerMemoryDreaming(
  workspaceId: string,
  plan: MemoryDreamingPlan,
  context: MemoryDreamingRunContext,
  profileInput: MemoryDreamingProfileInput,
  storage: AppServerSessionStorage
): MemoryDreamingRunSummary {
  return invokeWithJsonInput<MemoryDreamingRunSummary>(
    'dreaming.apply',
    ['knowledge', 'dreaming-apply'],
    { workspaceId, plan, context, profileInput },
    storage
  ).result;
}

export function recordAppServerMemoryDreamingFailure(
  workspaceId: string,
  context: MemoryDreamingRunContext,
  errorMessage: string,
  profileInput: MemoryDreamingProfileInput,
  storage: AppServerSessionStorage
): MemoryDreamingRunSummary {
  return invokeWithJsonInput<MemoryDreamingRunSummary>(
    'dreaming.record_failure',
    ['knowledge', 'dreaming-record-failure'],
    { workspaceId, context, errorMessage, profileInput },
    storage
  ).result;
}

export function restoreAppServerMemoryDreamingChange(
  workspaceId: string,
  changeId: string,
  storage: AppServerSessionStorage
): void {
  invokeWithJsonInput<{ restored: true }>(
    'dreaming.restore',
    ['knowledge', 'dreaming-restore'],
    { workspaceId, changeId },
    storage
  );
}

export interface AppServerWorkspaceStateInput {
  workspacePath: string;
  workspaceId?: string;
  researchKitId?: string;
  artifactRoot: string;
  databasePath: string;
  artifactDirectoryPath: string;
  action: string;
  args?: unknown[];
}

export function invokeAppServerWorkspaceState<T>(
  input: AppServerWorkspaceStateInput,
  storage: AppServerSessionStorage,
): T {
  return invokeWithJsonInput<T>('workspace.state', ['workspace', 'state'], input, storage).result;
}

export function invokeAppServerRegistryState<T>(
  registryDirectory: string,
  action: string,
  args: readonly unknown[] = [],
): T {
  return invokeWithJsonInput<T>(
    'registry.state',
    ['registry', 'state'],
    { registryDirectory, action, args: [...args] },
    null,
  ).result;
}

export async function markAppServerHistoryDuplicate(
  input: {
    workspaceId: string;
    workspaceName: string;
    subjectId: string;
    subjectName: string;
    type: 'claim' | 'memory' | 'runbook';
    id: string;
    parentId: string;
    expectedRevision: number;
  },
  storage: AppServerSessionStorage
): Promise<void> {
  await invokeAppServerOperation({
    operation: 'history.mark_duplicate',
    input,
    ...(storage.profileId ? { profileId: storage.profileId } : {})
  });
}

export async function undoAppServerHistoryDuplicate(
  input: {
    workspaceId: string;
    workspaceName: string;
    subjectId: string;
    subjectName: string;
    type: 'claim' | 'memory' | 'runbook';
    id: string;
    expectedRevision: number;
  },
  storage: AppServerSessionStorage
): Promise<void> {
  await invokeAppServerOperation({
    operation: 'history.undo_duplicate',
    input,
    ...(storage.profileId ? { profileId: storage.profileId } : {})
  });
}

export async function getAppServerRunbookDocument(
  workspaceId: string,
  runbookId: string,
  storage: AppServerSessionStorage
): Promise<AppServerRunbookDocument> {
  return (await invokeWithJsonInputAsync<AppServerRunbookDocument>(
    'runbook.get',
    ['knowledge', 'runbook-get'],
    { workspaceId, runbookId },
    storage,
    undefined,
    30_000
  )).result;
}

export function getAppServerReportDocument(
  workspaceId: string,
  reportId: string,
  storage: AppServerSessionStorage
): AppServerReportDocument {
  return invokeWithJsonInput<AppServerReportDocument>(
    'report.get',
    ['knowledge', 'report-get'],
    { workspaceId, reportId },
    storage
  ).result;
}

export async function reviseAppServerReportContent(
  input: {
    workspaceId: string;
    workspaceName: string;
    reportId: string;
    expectedRevision: number;
    content: string;
  },
  storage: AppServerSessionStorage
): Promise<void> {
  await invokeAppServerOperation({
    operation: 'report.revise_content',
    input,
    ...(storage.profileId ? { profileId: storage.profileId } : {})
  });
}

export async function updateAppServerReportTriageStatus(
  input: {
    workspaceId: string;
    workspaceName: string;
    reportId: string;
    expectedRevision: number;
    triageStatus: 'editing' | 'submitted' | 'reviewing' | 'rejected' | 'accepted';
  },
  storage: AppServerSessionStorage
): Promise<void> {
  await invokeAppServerOperation({
    operation: 'report.update_triage_status',
    input,
    ...(storage.profileId ? { profileId: storage.profileId } : {})
  });
}

export async function replaceAppServerReportSubmissionPacket(
  input: {
    workspaceId: string;
    workspaceName: string;
    workspaceRoot: string;
    reportId: string;
    submissionPacketPath: string;
  },
  storage: AppServerSessionStorage
): Promise<void> {
  await invokeAppServerOperation({
    operation: 'report.replace_packet',
    input,
    ...(storage.profileId ? { profileId: storage.profileId } : {})
  });
}

export async function replaceAppServerReportRecording(
  input: {
    workspaceId: string;
    workspaceName: string;
    workspaceRoot: string;
    reportId: string;
    recordingPath: string;
  },
  storage: AppServerSessionStorage
): Promise<void> {
  await invokeAppServerOperation({
    operation: 'report.replace_recording',
    input,
    ...(storage.profileId ? { profileId: storage.profileId } : {})
  });
}

export function resolveAppServerArtifact(
  artifactId: string,
  storage: AppServerSessionStorage,
  expectedKind?: string
): AppServerResolvedArtifact {
  return invokeWithJsonInput<AppServerResolvedArtifact>(
    'artifact.resolve',
    ['knowledge', 'artifact-resolve'],
    { artifactId, ...(expectedKind ? { expectedKind } : {}) },
    storage
  ).result;
}

export function resolveAppServerAuxiliaryModelRoute(input: Record<string, unknown>): AppServerAuxiliaryModelRoute {
  return invokeWithJsonInput<AppServerAuxiliaryModelRoute>(
    'model_job.resolve',
    ['harness', 'model-job-resolve'],
    input,
    null
  ).result;
}

export function getAppServerProviderSemantics(): AppServerProviderSemantics {
  providerSemanticsCache ??= invokeWithJsonInput<AppServerProviderSemantics>(
    'provider.describe',
    ['harness', 'provider-describe'],
    {},
    null
  ).result;
  return providerSemanticsCache;
}

export function inspectAppServerSources(input: Record<string, unknown>): {
  urls?: string[];
  normalizedUrl?: string | null;
  candidates?: AppServerSourceRepositoryCandidate[];
  selection?: { candidate: AppServerSourceRepositoryCandidate | null; candidates: AppServerSourceRepositoryCandidate[]; reason: 'matched' | 'ambiguous' | 'not_found' };
} {
  return invokeWithJsonInput<{
    urls?: string[];
    normalizedUrl?: string | null;
    candidates?: AppServerSourceRepositoryCandidate[];
    selection?: { candidate: AppServerSourceRepositoryCandidate | null; candidates: AppServerSourceRepositoryCandidate[]; reason: 'matched' | 'ambiguous' | 'not_found' };
  }>('source.inspect', ['harness', 'source-inspect'], input, null).result;
}

export async function materializeAppServerSource(
  candidate: AppServerSourceRepositoryCandidate,
  ref: string,
  repositoryStoreDirectory: string | undefined,
  signal?: AbortSignal,
  cloneMode: RepositoryCloneMode = 'deep'
): Promise<AppServerMaterializedSourceRepository> {
  return (await invokeWithJsonInputAsync<AppServerMaterializedSourceRepository>(
    'source.materialize',
    ['harness', 'source-materialize'],
    { candidate, ref, cloneMode, ...(repositoryStoreDirectory ? { repositoryStoreDirectory } : {}) },
    null,
    signal,
    null
  )).result;
}

export function materializeAppServerSourceSync(
  candidate: AppServerSourceRepositoryCandidate,
  ref: string,
  repositoryStoreDirectory?: string,
  cloneMode: RepositoryCloneMode = 'deep'
): AppServerMaterializedSourceRepository {
  return invokeWithJsonInput<AppServerMaterializedSourceRepository>(
    'source.materialize',
    ['harness', 'source-materialize'],
    { candidate, ref, cloneMode, ...(repositoryStoreDirectory ? { repositoryStoreDirectory } : {}) },
    null
  ).result;
}

export function listAppServerPlugins(input: Record<string, unknown>): AgentPluginRegistryState {
  return invokeWithJsonInput<AgentPluginRegistryState>('plugin.list', ['harness', 'plugin-list'], input, null).result;
}

export function addAppServerPluginFromFilesystem(input: Record<string, unknown>): AgentPluginRegistryState {
  return invokeWithJsonInput<AgentPluginRegistryState>('plugin.add_filesystem', ['harness', 'plugin-add-filesystem'], input, null).result;
}

export async function addAppServerPluginFromRepository(input: Record<string, unknown>): Promise<AgentPluginRegistryState> {
  return (await invokeWithJsonInputAsync<AgentPluginRegistryState>(
    'plugin.add_repository', ['harness', 'plugin-add-repository'], input, null
  )).result;
}

export function setAppServerPluginEnabled(input: Record<string, unknown>): AgentPluginRegistryState {
  return invokeWithJsonInput<AgentPluginRegistryState>('plugin.set_enabled', ['harness', 'plugin-set-enabled'], input, null).result;
}

export function removeAppServerPlugin(input: Record<string, unknown>): AgentPluginRegistryState {
  return invokeWithJsonInput<AgentPluginRegistryState>('plugin.remove', ['harness', 'plugin-remove'], input, null).result;
}

export function getAppServerPluginRuntime(input: Record<string, unknown>): AppServerAgentPluginRuntime {
  return invokeWithJsonInput<AppServerAgentPluginRuntime>('plugin.runtime', ['harness', 'plugin-runtime'], input, null).result;
}

export function getAppServerMaintenanceSummary(workspacePath: string): WorkspaceDejunkSummary {
  return invokeWithJsonInput<WorkspaceDejunkSummary>(
    'maintenance.summary', ['harness', 'maintenance-summary'], { workspacePath }, null
  ).result;
}

export async function getAppServerMaintenanceSummaryAsync(workspacePath: string): Promise<WorkspaceDejunkSummary> {
  return (await invokeWithJsonInputAsync<WorkspaceDejunkSummary>(
    'maintenance.summary', ['harness', 'maintenance-summary'], { workspacePath }, null
  )).result;
}

export function runAppServerMaintenance(workspacePath: string): WorkspaceDejunkSummary;
export function runAppServerMaintenance(
  workspacePath: string,
  options: {
    repositoryStoreDirectory: string;
    repositories?: AppServerWorkspaceRepositoryCandidate[];
  }
): AppServerMaintenanceRunResult;
export function runAppServerMaintenance(
  workspacePath: string,
  options?: {
    repositoryStoreDirectory: string;
    repositories?: AppServerWorkspaceRepositoryCandidate[];
  }
): WorkspaceDejunkSummary | AppServerMaintenanceRunResult {
  return invokeWithJsonInput<WorkspaceDejunkSummary | AppServerMaintenanceRunResult>(
    'maintenance.run', ['harness', 'maintenance-run'], { workspacePath, ...options }, null
  ).result;
}

export async function runAppServerMaintenanceAsync(
  workspacePath: string,
  options: {
    repositoryStoreDirectory: string;
    repositories?: AppServerWorkspaceRepositoryCandidate[];
  }
): Promise<AppServerMaintenanceRunResult> {
  return (await invokeWithJsonInputAsync<AppServerMaintenanceRunResult>(
    'maintenance.run', ['harness', 'maintenance-run'], { workspacePath, ...options }, null, undefined, null
  )).result;
}

export function invokeAppServerCliProtocol<T>(
  operation: string,
  args: readonly string[],
  options: { timeoutMs?: number; env?: NodeJS.ProcessEnv } = {}
): AppServerProtocolSuccess<T> {
  const invocation = resolveAppServerProtocolInvocation();
  const requestId = `beale-${randomUUID()}`;
  const result = spawnSync(invocation.command, [...invocation.prefixArgs, ...args, '--request-id', requestId], {
    cwd: invocation.cwd,
    encoding: 'utf8',
    env: protocolEnvironment(options.env, invocation.usesNodeRuntime),
    timeout: options.timeoutMs ?? 30_000,
    maxBuffer: APP_SERVER_PROTOCOL_MAX_STDOUT_BYTES,
    windowsHide: true
  });
  let envelope: AppServerProtocolEnvelope<T>;
  try {
    envelope = decodeAppServerProtocolEnvelope<T>(String(result.stdout ?? '').trim());
  } catch (error) {
    const detail = protocolProcessDetail(result);
    throw new Error(`app-server ${operation} returned an invalid protocol envelope${detail ? ` (${detail})` : ''}.`, { cause: error });
  }
  if (envelope.operation !== operation) {
    throw new Error(`app-server protocol operation mismatch: expected ${operation}, received ${envelope.operation}.`);
  }
  if (envelope.requestId !== requestId) {
    throw new Error(`app-server protocol request mismatch: expected ${requestId}, received ${envelope.requestId ?? 'none'}.`);
  }
  if (!envelope.ok) {
    throw new Error(`app-server ${operation} failed (${envelope.error.code}): ${envelope.error.message}`);
  }
  if (result.status !== 0) {
    throw new Error(`app-server ${operation} returned a success envelope with exit status ${String(result.status)}.`);
  }
  return envelope;
}

function invokeWithJsonInput<T>(
  operation: string,
  args: readonly string[],
  input: unknown,
  storage: AppServerSessionStorage | null
): AppServerProtocolSuccess<T> {
  const directory = mkdtempSync(join(tmpdir(), 'beale-app-server-protocol-'));
  const inputPath = join(directory, 'input.json');
  try {
    writeFileSync(inputPath, `${JSON.stringify(input)}\n`, { encoding: 'utf8', mode: 0o600 });
    const operationArgs = [...args, '--input', inputPath, '--json'];
    if ((operation === 'workspace.state' || operation === 'registry.state') && existsSync(stateDiscoveryPath())) {
      try {
        return invokeStateOperationThroughHost<T>(operation, operationArgs, input, directory);
      } catch {
        // Let the normal client bootstrap or replace an unavailable host.
      }
    }
    const result = invokeAppServerCliProtocol<T>(operation, operationArgs, {
      ...(storage ? { env: storageEnvironment(storage) } : {})
    });
    return result;
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

async function invokeWithJsonInputAsync<T>(
  operation: string,
  args: readonly string[],
  input: unknown,
  storage: AppServerSessionStorage | null,
  signal?: AbortSignal,
  timeoutMs?: number | null
): Promise<AppServerProtocolSuccess<T>> {
  const directory = mkdtempSync(join(tmpdir(), 'beale-app-server-protocol-'));
  const inputPath = join(directory, 'input.json');
  try {
    writeFileSync(inputPath, `${JSON.stringify(input)}\n`, { encoding: 'utf8', mode: 0o600 });
    return await invokeAppServerCliProtocolAsync<T>(operation, [...args, '--input', inputPath, '--json'], {
      ...(storage ? { env: storageEnvironment(storage) } : {}),
      ...(signal ? { signal } : {}),
      ...(timeoutMs !== undefined ? { timeoutMs } : {})
    });
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

export function invokeAppServerCliProtocolAsync<T>(
  operation: string,
  args: readonly string[],
  options: { timeoutMs?: number | null; env?: NodeJS.ProcessEnv; signal?: AbortSignal; stdin?: string } = {}
): Promise<AppServerProtocolSuccess<T>> {
  const requestId = `beale-${randomUUID()}`;
  const inputPathIndex = args.indexOf('--input');
  const input = options.stdin?.trim()
    ? JSON.parse(options.stdin) as unknown
    : inputPathIndex >= 0 && args[inputPathIndex + 1]
      ? JSON.parse(readFileSync(args[inputPathIndex + 1]!, 'utf8')) as unknown
      : undefined;
  const timeoutSignal = options.timeoutMs === null ? null : AbortSignal.timeout(options.timeoutMs ?? 5 * 60_000);
  const signal = options.signal && timeoutSignal
    ? AbortSignal.any([options.signal, timeoutSignal])
    : options.signal ?? timeoutSignal ?? undefined;
  return invokeAppServerOperation<T>({
    operation: operation as AppServerProtocolOperation,
    args,
    ...(input !== undefined ? { input } : {}),
    ...profileSelection(options.env, input),
    ...(signal ? { signal } : {})
  }).then((result) => appServerProtocolSuccess(operation as AppServerProtocolOperation, result, requestId));
}

function profileSelection(environment: NodeJS.ProcessEnv | undefined, input?: unknown): { profileId?: string } {
  // A profile carried by the typed input lets the host serve a workspace's
  // first operation before that workspace has been written to the shared
  // registry. Once registered, the app-server validates this hint against
  // its authoritative workspace record.
  const inputProfileId = profileIdFromInput(input);
  if (inputProfileId) return { profileId: inputProfileId };
  // Do not infer a competing profile for an already-scoped workspace.
  if (workspaceIdFromInput(input)) return {};
  const configuredProfileId = environment?.APP_SERVER_PROFILE_ID?.trim();
  if (configuredProfileId) return { profileId: configuredProfileId };
  const databasePath = environment?.APP_SERVER_DATABASE_PATH?.trim();
  if (!databasePath) return {};
  const directory = dirname(databasePath);
  const parent = directory.split(/[\\/]/u).at(-1);
  const grandparent = dirname(directory).split(/[\\/]/u).at(-1);
  return { profileId: grandparent === 'profiles' && parent ? parent : 'security-research' };
}

function workspaceIdFromInput(input: unknown): string | null {
  if (!input || typeof input !== 'object' || Array.isArray(input)) return null;
  const workspaceId = (input as Record<string, unknown>).workspaceId;
  return typeof workspaceId === 'string' && workspaceId.trim() ? workspaceId.trim() : null;
}

function profileIdFromInput(input: unknown): string | null {
  if (!input || typeof input !== 'object' || Array.isArray(input)) return null;
  const record = input as Record<string, unknown>;
  if (typeof record.researchProfileId === 'string' && record.researchProfileId.trim()) return record.researchProfileId.trim();
  const researchProfile = record.researchProfile;
  if (researchProfile && typeof researchProfile === 'object' && !Array.isArray(researchProfile)) {
    const id = (researchProfile as Record<string, unknown>).profileId;
    if (typeof id === 'string' && id.trim()) return id.trim();
  }
  const profileInput = record.profileInput;
  if (profileInput && typeof profileInput === 'object' && !Array.isArray(profileInput)) {
    const snapshot = (profileInput as Record<string, unknown>).profileSnapshot;
    if (snapshot && typeof snapshot === 'object' && !Array.isArray(snapshot)) {
      const id = (snapshot as Record<string, unknown>).profileId;
      if (typeof id === 'string' && id.trim()) return id.trim();
    }
  }
  return null;
}

function safeProtocolDetail(value: unknown): string {
  return String(value ?? '')
    .replace(/\u001b\[[0-9;]*m/gu, '')
    .replace(/(?:sk|xai)-[A-Za-z0-9_-]+/gu, '...redacted')
    .replace(/Bearer\s+[A-Za-z0-9._~+/=-]+/giu, 'Bearer ...redacted')
    .trim()
    .slice(-2_000);
}

function protocolEnvironment(overrides?: NodeJS.ProcessEnv, usesNodeRuntime = false): NodeJS.ProcessEnv {
  return {
    ...process.env,
    NO_COLOR: process.env.NO_COLOR ?? '1',
    NODE_NO_WARNINGS: '1',
    ...overrides,
    ...(usesNodeRuntime ? { ELECTRON_RUN_AS_NODE: '1' } : {})
  };
}

function protocolProcessDetail(result: ReturnType<typeof spawnSync>): string {
  const details: string[] = [];
  if (result.error) details.push(`process error: ${result.error.message}`);
  if (result.signal) details.push(`signal: ${result.signal}`);
  if (result.status !== null && result.status !== 0) details.push(`exit status: ${result.status}`);
  const stderr = safeProtocolDetail(result.stderr);
  if (stderr) details.push(`stderr: ${stderr}`);
  const stdoutBytes = Buffer.byteLength(String(result.stdout ?? ''), 'utf8');
  if (stdoutBytes > 0) details.push(`invalid stdout bytes: ${stdoutBytes}`);
  return details.join('; ');
}

function asyncProtocolProcessDetail(code: number | null, stdout: string, stderr: string): string {
  const details: string[] = [];
  if (code !== null && code !== 0) details.push(`exit status: ${code}`);
  const safeStderr = safeProtocolDetail(stderr);
  if (safeStderr) details.push(`stderr: ${safeStderr}`);
  const stdoutBytes = Buffer.byteLength(stdout, 'utf8');
  if (stdoutBytes > 0) details.push(`invalid stdout bytes: ${stdoutBytes}`);
  return details.length > 0 ? `(${details.join('; ')})` : '';
}

function sessionRecordFromSummary(summary: AppServerSessionSummary): AppServerSessionRecord {
  return {
    ...summary,
    attempts: summary.attempts.map((attempt) => ({ ...attempt, capture: null })),
    finalResponse: null,
    events: []
  };
}

function sessionUpdateArguments(
  sessionId: string,
  afterEventId: string | null,
  options: { tail?: boolean; limit?: number; maxBytes?: number }
): string[] {
  return [
    'session',
    'get-update',
    '--session-id',
    sessionId,
    ...(afterEventId ? ['--after-event-id', afterEventId] : []),
    ...(options.tail ? ['--tail'] : []),
    ...(options.limit ? ['--limit', String(options.limit)] : []),
    ...(options.maxBytes ? ['--max-bytes', String(options.maxBytes)] : []),
    '--json'
  ];
}

function storageEnvironment(storage: AppServerSessionStorage): NodeJS.ProcessEnv {
  return {
    APP_SERVER_DATABASE_PATH: storage.databasePath,
    APP_SERVER_ARTIFACT_DIRECTORY: storage.artifactDirectoryPath,
    ...(storage.profileId ? { APP_SERVER_PROFILE_ID: storage.profileId } : {})
  };
}

function appServerInvocationKey(): string {
  const invocation = resolveAppServerProtocolInvocation();
  const executablePath = invocation.prefixArgs.find((argument) => /(?:^|[\\/])cli\.js$/u.test(argument));
  let executableFingerprint = '';
  if (executablePath) {
    try {
      const stats = statSync(executablePath);
      executableFingerprint = `${stats.size}:${stats.mtimeMs}`;
    } catch {
      executableFingerprint = 'missing';
    }
  }
  return [invocation.command, invocation.cwd, ...invocation.prefixArgs, executableFingerprint].join('\0');
}

export function decodeAppServerMemorySummary(value: unknown): AppServerMemorySummary {
  if (!isPlainRecord(value)
    || !nonNegativeNumber(value.nodeCount)
    || !nonNegativeNumber(value.edgeCount)
    || !Array.isArray(value.nodes)
    || !Array.isArray(value.edges)
    || !Array.isArray(value.runbooks)
    || !value.runbooks.every(validRunbookSummary)
    || !Array.isArray(value.leads)
    || !value.leads.every(validFindingSummary)
    || !Array.isArray(value.findings)
    || !value.findings.every(validFindingSummary)
    || !validCampaignGraph(value.campaign)) {
    throw new Error('app-server returned an invalid memory summary v12 payload.');
  }
  return value as unknown as AppServerMemorySummary;
}

function validRunbookSummary(value: unknown): boolean {
  if (!isPlainRecord(value)
    || !nonEmptyText(value.id)
    || !nonEmptyText(value.workspaceId)
    || !nonEmptyText(value.title)
    || !nonNegativeNumber(value.revision)
    || !nonNegativeNumber(value.contentRevision)
    || !isPlainRecord(value.execution)
    || !nonNegativeNumber(value.execution.runCount)
    || !nonNegativeNumber(value.execution.completedRunCount)
    || !nonNegativeNumber(value.execution.executedCellCount)
    || !(value.execution.latestSuccessfulRunId === null || nonEmptyText(value.execution.latestSuccessfulRunId))) return false;
  const latest = value.execution.latest;
  return latest === null || (isPlainRecord(latest)
    && nonEmptyText(latest.runId)
    && (latest.status === 'running' || latest.status === 'succeeded'
      || latest.status === 'failed' || latest.status === 'blocked')
    && nonEmptyText(latest.startedAt));
}

function validReportSummary(value: unknown): boolean {
  return isPlainRecord(value)
    && nonEmptyText(value.id)
    && nonEmptyText(value.workspaceId)
    && nonEmptyText(value.workspaceName)
    && (value.subjectId === null || typeof value.subjectId === 'string')
    && (value.subjectName === null || typeof value.subjectName === 'string')
    && (value.sessionId === null || typeof value.sessionId === 'string')
    && nonEmptyText(value.title)
    && typeof value.summary === 'string'
    && (value.status === 'complete' || value.status === 'stale')
    && ['editing', 'submitted', 'reviewing', 'rejected', 'accepted'].includes(String(value.triageStatus))
    && nonEmptyText(value.artifactId)
    && Number.isInteger(value.revision) && (value.revision as number) > 0
    && validReportAttachment(value.submissionPacket)
    && validReportAttachment(value.recording)
    && Array.isArray(value.revisions)
    && value.revisions.every((revision) => isPlainRecord(revision)
      && Number.isInteger(revision.revision) && (revision.revision as number) > 0
      && (revision.sessionId === null || typeof revision.sessionId === 'string')
      && typeof revision.createdAt === 'string')
    && (value.authors === undefined || (Array.isArray(value.authors)
      && value.authors.every((author) => isPlainRecord(author)
        && nonEmptyText(author.provider) && nonEmptyText(author.model))))
    && typeof value.createdAt === 'string'
    && typeof value.updatedAt === 'string';
}

function validReportAttachment(value: unknown): boolean {
  return value === null || (isPlainRecord(value)
    && nonEmptyText(value.artifactId)
    && nonEmptyText(value.filename)
    && nonNegativeNumber(value.sizeBytes)
    && nonEmptyText(value.contentHash));
}

function validFindingSummary(value: unknown): boolean {
  return isPlainRecord(value)
    && nonEmptyText(value.id)
    && nonEmptyText(value.workspaceId)
    && (value.memoryNodeId === null || nonEmptyText(value.memoryNodeId))
    && (value.projection === 'lead' || value.projection === 'finding')
    && ['informational', 'low', 'medium', 'high', 'critical'].includes(String(value.rating))
    && ['proposed', 'observed', 'reproduced', 'verified', 'refuted'].includes(String(value.maturity))
    && (value.freshness === 'current' || value.freshness === 'stale')
    && ['open', 'active', 'reporting', 'published', 'closed'].includes(String(value.workflow))
    && nonEmptyText(value.classification)
    && Array.isArray(value.componentClaimIds)
    && nonEmptyText(value.title)
    && (value.securityTracking === null || validFindingSecurityTracking(value.securityTracking))
    && nonEmptyText(value.status)
    && nonNegativeNumber(value.revision)
    && (value.duplicateOfClaimId === null || nonEmptyText(value.duplicateOfClaimId))
    && (value.duplicateMarkedAt === null || nonEmptyText(value.duplicateMarkedAt))
    && Array.isArray(value.duplicateClaims)
    && value.duplicateClaims.every(validResearchClaimDuplicateSummary)
    && Array.isArray(value.evidence)
    && Array.isArray(value.transitions)
    && Array.isArray(value.authors)
    && value.authors.every((author) => isPlainRecord(author)
      && nonEmptyText(author.provider)
      && nonEmptyText(author.model));
}

let stateTransportWorker: Worker | null = null;
let stateTransportRequestId = 0;

function invokeStateOperationThroughHost<T>(
  operation: string,
  args: readonly string[],
  input: unknown,
  directory: string,
): AppServerProtocolSuccess<T> {
  const worker = stateTransportWorker ??= createStateTransportWorker();
  const requestId = `beale-state-${++stateTransportRequestId}`;
  const responsePath = join(directory, 'response.json');
  const signalBuffer = new SharedArrayBuffer(Int32Array.BYTES_PER_ELEMENT);
  const signal = new Int32Array(signalBuffer);
  worker.postMessage({
    requestId,
    operation,
    args,
    inputJson: JSON.stringify(input),
    responsePath,
    signalBuffer,
    discoveryPath: stateDiscoveryPath(),
  });
  const waitResult = Atomics.wait(signal, 0, 0, 30_000);
  if (waitResult === 'timed-out') throw new Error(`app-server ${operation} timed out.`);
  const response = JSON.parse(readFileSync(responsePath, 'utf8')) as {
    requestId?: unknown;
    status?: unknown;
    payload?: unknown;
    error?: unknown;
  };
  if (response.requestId !== requestId) throw new Error(`app-server ${operation} response correlation failed.`);
  if (typeof response.error === 'string') throw new Error(response.error);
  if (typeof response.status !== 'number' || !response.payload || typeof response.payload !== 'object') {
    throw new Error(`app-server ${operation} returned an invalid hosted response.`);
  }
  const payload = response.payload as { result?: unknown; error?: string | { code?: unknown; message?: unknown } };
  if (response.status < 200 || response.status >= 300) {
    const structured = payload.error && typeof payload.error === 'object' ? payload.error : null;
    const message = structured && typeof structured.message === 'string'
      ? structured.message
      : typeof payload.error === 'string' ? payload.error : `HTTP ${response.status}`;
    throw new Error(`app-server ${operation} failed: ${message}`);
  }
  return appServerProtocolSuccess(operation as AppServerProtocolOperation, payload.result as T, requestId);
}

function stateDiscoveryPath(): string {
  return process.env.BEALE_APP_SERVER_STATE_FILE?.trim() || join(homedir(), '.beale', 'app-server.json');
}

function createStateTransportWorker(): Worker {
  const worker = new Worker(`
    const { parentPort } = require('node:worker_threads');
    const { readFileSync, writeFileSync } = require('node:fs');
    parentPort.on('message', async (message) => {
      const signal = new Int32Array(message.signalBuffer);
      let response;
      try {
        const discovery = JSON.parse(readFileSync(message.discoveryPath, 'utf8'));
        const baseUrl = String(discovery.localUrl || discovery.url || '').trim();
        const request = await fetch(baseUrl + '/v1/operations', {
          method: 'POST',
          headers: {
            authorization: 'Bearer ' + discovery.operatorToken,
            'content-type': 'application/json',
          },
          body: JSON.stringify({ operation: message.operation, args: message.args, input: JSON.parse(message.inputJson) }),
        });
        response = { requestId: message.requestId, status: request.status, payload: await request.json() };
      } catch (error) {
        response = { requestId: message.requestId, error: error instanceof Error ? error.message : String(error) };
      }
      try {
        writeFileSync(message.responsePath, JSON.stringify(response), { encoding: 'utf8', mode: 0o600 });
      } finally {
        Atomics.store(signal, 0, 1);
        Atomics.notify(signal, 0);
      }
    });
  `, { eval: true });
  worker.unref();
  return worker;
}

function validResearchClaimDuplicateSummary(value: unknown): boolean {
  return isPlainRecord(value)
    && nonEmptyText(value.id)
    && (value.projection === 'lead' || value.projection === 'finding')
    && ['proposed', 'observed', 'reproduced', 'verified', 'refuted'].includes(String(value.maturity))
    && ['informational', 'low', 'medium', 'high', 'critical'].includes(String(value.rating))
    && nonEmptyText(value.classification)
    && nonEmptyText(value.title)
    && nonEmptyText(value.status)
    && nonNegativeNumber(value.revision)
    && nonEmptyText(value.markedAt);
}

function validFindingSecurityTracking(value: unknown): boolean {
  if (!isPlainRecord(value) || !isPlainRecord(value.reachability)) return false;
  return ['not_assessed', 'unreachable', 'conditional', 'reachable'].includes(String(value.reachability.state))
    && typeof value.reachability.conditions === 'string'
    && Array.isArray(value.reachability.evidenceIds)
    && ['unreviewed', 'remediate', 'mitigated', 'accepted', 'transferred'].includes(String(value.riskTreatment))
    && Array.isArray(value.riskDecisions)
    && Array.isArray(value.cvssAssessments)
    && Array.isArray(value.affectedAssetIds)
    && Array.isArray(value.affectedVersions)
    && Array.isArray(value.externalReferences);
}

function validCampaignGraph(value: unknown): boolean {
  if (!isPlainRecord(value)) return false;
  return Array.isArray(value.nodes)
    && Array.isArray(value.edges)
    && Array.isArray(value.coverageGaps)
    && Array.isArray(value.contradictions)
    && Array.isArray(value.nextActions)
    && isPlainRecord(value.momentum)
    && nonEmptyText(value.momentum.state)
    && nonEmptyText(value.momentum.reason)
    && Array.isArray(value.momentum.supportingNodeIds)
    && isPlainRecord(value.counts)
    && nonNegativeNumber(value.counts.leads)
    && nonNegativeNumber(value.counts.findings)
    && nonNegativeNumber(value.counts.coverageGaps)
    && (value.tracks === undefined || (Array.isArray(value.tracks) && value.tracks.every(validCampaignTrack)))
    && (value.activeTrackId === undefined || value.activeTrackId === null || nonEmptyText(value.activeTrackId))
    && (value.replayMetrics === undefined || validCampaignReplayMetrics(value.replayMetrics));
}

function validCampaignTrack(value: unknown): boolean {
  if (!isPlainRecord(value) || !isPlainRecord(value.counts)) return false;
  const counts = value.counts;
  return nonEmptyText(value.id)
    && nonEmptyText(value.title)
    && typeof value.objective === 'string'
    && ['active', 'blocked', 'complete', 'archived'].includes(String(value.status))
    && ['orienting', 'exploring', 'testing', 'reproducing', 'verifying', 'reporting', 'complete', 'blocked'].includes(String(value.stage))
    && ['runtime', 'shadow', 'replay', 'manual'].includes(String(value.source))
    && Array.isArray(value.sessionIds) && value.sessionIds.every(nonEmptyText)
    && nonEmptyText(value.updatedAt)
    && nonNegativeNumber(value.revision)
    && Array.isArray(value.questions) && value.questions.every(validCampaignQuestion)
    && Array.isArray(value.experiments) && value.experiments.every(validCampaignExperiment)
    && Array.isArray(value.observations) && value.observations.every(validCampaignObservation)
    && ['questions', 'openQuestions', 'experiments', 'observations', 'openNextActions', 'memoryNodes', 'evidenceRefs', 'findings', 'runbooks', 'reports']
      .every((field) => nonNegativeNumber(counts[field]));
}

function validCampaignQuestion(value: unknown): boolean {
  return isPlainRecord(value)
    && nonEmptyText(value.id)
    && nonEmptyText(value.investigationId)
    && nonEmptyText(value.text)
    && ['open', 'answered', 'blocked', 'superseded'].includes(String(value.status))
    && ['critical', 'high', 'medium', 'low'].includes(String(value.priority))
    && typeof value.answer === 'string'
    && nonEmptyText(value.updatedAt)
    && nonNegativeNumber(value.revision);
}

function validCampaignExperiment(value: unknown): boolean {
  return isPlainRecord(value)
    && nonEmptyText(value.id)
    && nonEmptyText(value.investigationId)
    && (value.questionId === null || nonEmptyText(value.questionId))
    && (value.runbookId === null || nonEmptyText(value.runbookId))
    && nonEmptyText(value.title)
    && ['planned', 'running', 'succeeded', 'failed', 'inconclusive', 'blocked'].includes(String(value.status))
    && typeof value.resultSummary === 'string'
    && (value.startedAt === null || nonEmptyText(value.startedAt))
    && (value.completedAt === null || nonEmptyText(value.completedAt))
    && nonEmptyText(value.updatedAt)
    && nonNegativeNumber(value.revision);
}

function validCampaignObservation(value: unknown): boolean {
  return isPlainRecord(value)
    && nonEmptyText(value.id)
    && nonEmptyText(value.investigationId)
    && (value.experimentId === null || nonEmptyText(value.experimentId))
    && ['source', 'runtime', 'artifact', 'verifier', 'human', 'historical'].includes(String(value.kind))
    && ['supports', 'refutes', 'narrows', 'neutral'].includes(String(value.outcome))
    && nonEmptyText(value.summary)
    && nonEmptyText(value.createdAt);
}

function validCampaignReplayMetrics(value: unknown): boolean {
  return isPlainRecord(value)
    && value.schemaVersion === 1
    && ['historical', 'shadow', 'active'].includes(String(value.mode))
    && nonEmptyText(value.workspaceId)
    && ['sessionCount', 'generatedTrackCount', 'linkedMemoryNodeCount', 'repeatedMemoryCandidateCount', 'rejectedHypothesisResurrectionCount']
      .every((field) => nonNegativeNumber(value[field]))
    && unitInterval(value.environmentTaggedNodeRate)
    && unitInterval(value.crossSessionReuseRate)
    && (value.medianMinutesToFirstEvidence === null || nonNegativeNumber(value.medianMinutesToFirstEvidence));
}

function isPlainRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function nonEmptyText(value: unknown): value is string {
  return typeof value === 'string' && value.trim().length > 0;
}

function nonNegativeNumber(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0;
}

function unitInterval(value: unknown): value is number {
  return nonNegativeNumber(value) && value <= 1;
}
