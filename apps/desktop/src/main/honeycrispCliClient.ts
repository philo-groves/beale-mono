import { spawn, spawnSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { resolveHoneycrispProtocolInvocation } from './honeycrispInvocation';
import { invokeAppServerOperation } from './bealeAppServerClient';
import { honeycrispProtocolSuccess, type HoneycrispProtocolOperation } from 'honeycrisp/protocol';
import type {
  HoneycrispMemorySummary,
  HoneycrispMemoryEdgeSummary,
  HoneycrispMemoryNodeSummary,
  HoneycrispReportDocument,
  HoneycrispReportSummary,
  HoneycrispRunbookDocument,
  MemoryDreamingRunSummary,
  ResearchProfileSnapshot,
  AgentPluginRegistryState,
  RepositoryCloneMode,
  WorkspaceDejunkSummary
} from '@shared/types';
import type { ResolvedResearchProfile } from '../shared/researchProfile';
import {
  decodeHoneycrispProtocolDescriptor,
  decodeHoneycrispProtocolEnvelope,
  type HoneycrispProtocolDescriptor,
  type HoneycrispProtocolEnvelope,
  type HoneycrispProtocolSuccess
} from './honeycrispProtocol';
export {
  HONEYCRISP_PROTOCOL_VERSION,
  HONEYCRISP_PROTOCOL_WEBSOCKET_PATH,
  decodeHoneycrispProtocolEnvelope
} from './honeycrispProtocol';
export type {
  HoneycrispProtocolDescriptor,
  HoneycrispProtocolEnvelope,
  HoneycrispProtocolFailure,
  HoneycrispProtocolSuccess
} from './honeycrispProtocol';

export type HoneycrispSessionStatus = 'active' | 'paused' | 'blocked' | 'completed' | 'failed' | 'stopped';

export interface HoneycrispSessionTokenUsage {
  totalTokens: number;
  totalCostUsd?: number;
  inputTokens?: number;
  outputTokens?: number;
  cacheReadTokens?: number;
  cachePromptTokens?: number;
}

export interface HoneycrispSessionActivityCounts {
  memorySearches: number;
  memoryUpdates: number;
}

export interface HoneycrispSessionEvent {
  id: string;
  kind: string;
  timestamp: string;
  summary: string;
  payload: unknown;
  agentId?: string;
  agentPath?: string;
  parentAgentId?: string;
}

export interface HoneycrispSessionAttempt {
  id: string;
  parentAttemptId: string | null;
  status: HoneycrispSessionStatus;
  summary: string;
  startedAt: string;
  endedAt: string | null;
  capture: Record<string, unknown> | null;
  metadata: Record<string, unknown>;
}

export interface HoneycrispSessionRecord {
  schemaVersion: 1;
  id: string;
  workspaceId: string;
  status: HoneycrispSessionStatus;
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
  attempts: HoneycrispSessionAttempt[];
  events: HoneycrispSessionEvent[];
  createdAt: string;
  startedAt: string;
  endedAt: string | null;
  updatedAt: string;
  revision: number;
  /** Present when materialized from a canonical session summary/update. */
  tokenUsage?: HoneycrispSessionTokenUsage;
  /** Present when materialized from a canonical session summary/update. */
  activityCounts?: HoneycrispSessionActivityCounts;
}

export type HoneycrispSessionSummary = Omit<
  HoneycrispSessionRecord,
  'attempts' | 'events' | 'finalResponse'
> & {
  attempts: Array<Omit<HoneycrispSessionAttempt, 'capture'>>;
  lastMessageAt: string | null;
  tokenUsage?: HoneycrispSessionTokenUsage;
  activityCounts?: HoneycrispSessionActivityCounts;
};

export interface HoneycrispSessionUpdate {
  session: HoneycrispSessionSummary;
  finalResponse: string | null;
  events: HoneycrispSessionEvent[];
  eventOffset: number;
  nextAfterEventId: string | null;
  hasEarlier: boolean;
  hasMore: boolean;
}

export interface HoneycrispSessionEventPage {
  sessionId: string;
  stream: 'all' | 'transcript' | 'trace';
  events: HoneycrispSessionEvent[];
  eventOffset: number;
  nextAfterEventId: string | null;
  hasEarlier: boolean;
  hasMore: boolean;
}

export interface HoneycrispSessionCollaborationState {
  sessionId: string;
  revision: number;
  rooms: HoneycrispSessionEvent[];
  members: HoneycrispSessionEvent[];
  messages: HoneycrispSessionEvent[];
  subagents: HoneycrispSessionEvent[];
}

export interface HoneycrispSessionCaptureSummary {
  attemptId: string;
  capturedAt: string;
  schemaVersion: number;
  sizeBytes: number;
  contentHash: string;
  eventStreams: Record<string, unknown>;
}

export interface HoneycrispSessionMutationReceipt {
  sessionId: string;
  status: HoneycrispSessionStatus;
  revision: number;
  updatedAt: string;
}

export interface HoneycrispSessionStorage {
  databasePath: string;
  artifactDirectoryPath: string;
  profileId?: string;
}

export interface HoneycrispSessionRecoveryReport {
  workspaceId: string;
  recoveredAt: string;
  reason: string;
  interruptedSessions: number;
  interruptedAttempts: number;
  sessionIds: string[];
}

export function resolveHoneycrispStoragePaths(
  profileId: string,
  options: { databasePath?: string; artifactDirectoryPath?: string; registryDirectory?: string } = {}
): HoneycrispSessionStorage {
  const databasePath = options.databasePath
    ? profileId === 'security-research'
      ? resolve(options.databasePath)
      : join(dirname(resolve(options.databasePath)), 'profiles', profileId, 'memory.sqlite')
    : options.registryDirectory
      ? resolve(options.registryDirectory, 'honeycrisp', 'profiles', profileId, 'memory.sqlite')
      : join(homedir(), '.honeycrisp', 'profiles', profileId, 'memory.sqlite');
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

export interface HoneycrispDreamingPreparation {
  instructions: string;
  typeDescriptions: Record<string, string>;
  modelJobDefaults: { size: string; reasoningEffort: string } | null;
  inputTexts: string[];
}

export interface HoneycrispDreamingSessionInput {
  id: string;
  title: string;
  status: string;
  createdAt: string;
  endedAt: string | null;
  prompt: string;
  finalSummary: string;
  transcript: Array<{ role: string; source: string; createdAt: string; content: string }>;
}

export interface HoneycrispResolvedArtifact {
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

export interface HoneycrispAuxiliaryModelRoute {
  provider: 'openai-codex' | 'anthropic' | 'xai' | 'zai' | 'openrouter';
  model: string;
  effort: 'off' | 'minimal' | 'low' | 'medium' | 'high' | 'xhigh' | 'max';
}

export interface HoneycrispProviderSemantics {
  providers: Array<'openai-codex' | 'anthropic' | 'xai' | 'zai' | 'openrouter'>;
  aliases: Record<string, 'openai-codex' | 'anthropic' | 'xai' | 'zai' | 'openrouter'>;
  defaultSmallModels: Record<'openai-codex' | 'anthropic' | 'xai' | 'zai' | 'openrouter', string>;
  auxiliaryEfforts: Array<'off' | 'minimal' | 'low' | 'medium' | 'high' | 'xhigh' | 'max'>;
  sessionTitleEffort: 'medium';
  shellReviewEffort: 'medium';
}

let providerSemanticsCache: HoneycrispProviderSemantics | null = null;
let compatibleProtocolCache: { invocationKey: string; descriptor: HoneycrispProtocolDescriptor } | null = null;
const HONEYCRISP_PROTOCOL_MAX_STDOUT_BYTES = 64 * 1024 * 1024;
const HONEYCRISP_PROTOCOL_MAX_STDERR_CHARS = 2_000_000;

export interface HoneycrispSourceRepositoryCandidate {
  url: string;
  label: string;
  sourceAssetId: string;
  sourceAssetKind: string;
  sensitivity: string;
  clonedDirectory: string | null;
}

export interface HoneycrispMaterializedSourceRepository {
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

export interface HoneycrispWorkspaceRepositoryCandidate {
  path: string;
  repositoryUrl?: string;
  ref?: string;
}

export interface HoneycrispWorkspaceRepositoryRelocation {
  fromPath: string;
  toPath: string;
  repositoryUrl: string | null;
}

export interface HoneycrispMaintenanceRunResult {
  summary: WorkspaceDejunkSummary;
  repositoryRelocations: HoneycrispWorkspaceRepositoryRelocation[];
}

export interface HoneycrispAgentPluginRuntime {
  runtimeDirectory: string;
  skillDirs: string[];
  selectedSkillIds: string[];
  mcpConfigPath: string | null;
  allowedMcpServers: string[];
  args: string[];
  warnings: string[];
}

export interface HoneycrispBuiltinPlugin {
  id: string;
  path: string;
  installedAt: string;
  enabledByDefault?: boolean;
}

export function getHoneycrispProtocolDescriptor(): HoneycrispProtocolDescriptor {
  const envelope = invokeHoneycrispCliProtocol<HoneycrispProtocolDescriptor>(
    'protocol.describe',
    ['protocol', 'describe', '--json']
  );
  return decodeHoneycrispProtocolDescriptor(envelope.result);
}

export function assertHoneycrispProtocolCompatibility(): HoneycrispProtocolDescriptor {
  const invocationKey = honeycrispInvocationKey();
  if (compatibleProtocolCache?.invocationKey === invocationKey) return compatibleProtocolCache.descriptor;
  const descriptor = getHoneycrispProtocolDescriptor();
  compatibleProtocolCache = { invocationKey, descriptor };
  return descriptor;
}

export function honeycrispOwnsSessions(): boolean {
  // The app-server is the canonical Honeycrisp host. Session ownership is no
  // longer conditional on probing a separately installed CLI executable.
  return true;
}

export function createHoneycrispSession(
  input: Record<string, unknown>,
  storage: HoneycrispSessionStorage
): HoneycrispSessionRecord {
  return invokeWithJsonInput<HoneycrispSessionRecord>('session.create', ['session', 'create'], input, storage).result;
}

export function beginHoneycrispSessionAttempt(
  sessionId: string,
  input: Record<string, unknown>,
  storage: HoneycrispSessionStorage
): HoneycrispSessionRecord {
  return invokeWithJsonInput<HoneycrispSessionRecord>(
    'session.begin_attempt',
    ['session', 'begin-attempt', '--session-id', sessionId],
    input,
    storage
  ).result;
}

export function appendHoneycrispSessionEvent(
  sessionId: string,
  input: HoneycrispSessionEvent,
  storage: HoneycrispSessionStorage
): HoneycrispSessionMutationReceipt {
  return invokeWithJsonInput<HoneycrispSessionMutationReceipt>(
    'session.append_event_receipt',
    ['session', 'append-event-receipt', '--session-id', sessionId],
    input,
    storage
  ).result;
}

export async function appendHoneycrispSessionEventAsync(
  sessionId: string,
  input: HoneycrispSessionEvent,
  storage: HoneycrispSessionStorage,
  signal?: AbortSignal
): Promise<HoneycrispSessionMutationReceipt> {
  return (await invokeWithJsonInputAsync<HoneycrispSessionMutationReceipt>(
    'session.append_event_receipt',
    ['session', 'append-event-receipt', '--session-id', sessionId],
    input,
    storage,
    signal,
    30_000
  )).result;
}

export function transitionHoneycrispSession(
  sessionId: string,
  input: Record<string, unknown>,
  storage: HoneycrispSessionStorage
): HoneycrispSessionRecord {
  return invokeWithJsonInput<HoneycrispSessionRecord>(
    'session.transition',
    ['session', 'transition', '--session-id', sessionId],
    input,
    storage
  ).result;
}

export function recoverInterruptedHoneycrispSessions(
  workspaceId: string,
  input: { reason?: string; at?: string },
  storage: HoneycrispSessionStorage
): HoneycrispSessionRecoveryReport {
  return invokeWithJsonInput<HoneycrispSessionRecoveryReport>(
    'session.recover_interrupted',
    ['session', 'recover-interrupted', '--workspace-id', workspaceId],
    input,
    storage
  ).result;
}

export async function recoverInterruptedHoneycrispSessionsAsync(
  workspaceId: string,
  input: { reason?: string; at?: string },
  storage: HoneycrispSessionStorage
): Promise<HoneycrispSessionRecoveryReport> {
  return (await invokeWithJsonInputAsync<HoneycrispSessionRecoveryReport>(
    'session.recover_interrupted',
    ['session', 'recover-interrupted', '--workspace-id', workspaceId],
    input,
    storage,
    undefined,
    30_000
  )).result;
}

export function getHoneycrispSession(sessionId: string, storage: HoneycrispSessionStorage): HoneycrispSessionRecord {
  const summary = invokeHoneycrispCliProtocol<HoneycrispSessionSummary>(
    'session.get',
    ['session', 'get', '--session-id', sessionId, '--json'],
    { env: storageEnvironment(storage) }
  ).result;
  return sessionRecordFromSummary(summary);
}

export async function getHoneycrispSessionAsync(
  sessionId: string,
  storage: HoneycrispSessionStorage,
  signal?: AbortSignal
): Promise<HoneycrispSessionRecord> {
  const summary = (await invokeHoneycrispCliProtocolAsync<HoneycrispSessionSummary>(
    'session.get',
    ['session', 'get', '--session-id', sessionId, '--json'],
    { env: storageEnvironment(storage), timeoutMs: 30_000, ...(signal ? { signal } : {}) }
  )).result;
  return sessionRecordFromSummary(summary);
}

export function getHoneycrispSessionUpdate(
  sessionId: string,
  afterEventId: string | null,
  storage: HoneycrispSessionStorage,
  options: { tail?: boolean; limit?: number; maxBytes?: number } = {}
): HoneycrispSessionUpdate {
  return invokeHoneycrispCliProtocol<HoneycrispSessionUpdate>(
    'session.get_update',
    sessionUpdateArguments(sessionId, afterEventId, options),
    { env: storageEnvironment(storage) }
  ).result;
}

export async function getHoneycrispSessionUpdateAsync(
  sessionId: string,
  afterEventId: string | null,
  storage: HoneycrispSessionStorage,
  signal?: AbortSignal,
  options: { tail?: boolean; limit?: number; maxBytes?: number } = {}
): Promise<HoneycrispSessionUpdate> {
  return (await invokeHoneycrispCliProtocolAsync<HoneycrispSessionUpdate>(
    'session.get_update',
    sessionUpdateArguments(sessionId, afterEventId, options),
    { env: storageEnvironment(storage), timeoutMs: 30_000, ...(signal ? { signal } : {}) }
  )).result;
}

export function getHoneycrispSessionCollaborationState(
  sessionId: string,
  storage: HoneycrispSessionStorage,
  messageLimit = 200
): HoneycrispSessionCollaborationState {
  return invokeHoneycrispCliProtocol<HoneycrispSessionCollaborationState>(
    'session.collaboration',
    ['session', 'collaboration', '--session-id', sessionId, '--message-limit', String(messageLimit), '--json'],
    { env: storageEnvironment(storage) }
  ).result;
}

export function getHoneycrispSessionEventPage(
  sessionId: string,
  storage: HoneycrispSessionStorage,
  options: {
    stream?: 'all' | 'transcript' | 'trace';
    afterEventId?: string;
    tail?: boolean;
    limit?: number;
    maxBytes?: number;
  } = {}
): HoneycrispSessionEventPage {
  return invokeHoneycrispCliProtocol<HoneycrispSessionEventPage>(
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

export async function getHoneycrispSessionEventDetailsAsync(
  sessionId: string,
  eventIds: readonly string[],
  storage: HoneycrispSessionStorage,
  signal?: AbortSignal
): Promise<HoneycrispSessionEvent[]> {
  if (eventIds.length === 0) return [];
  return (await invokeHoneycrispCliProtocolAsync<HoneycrispSessionEvent[]>(
    'session.event_details',
    ['session', 'event-details', '--session-id', sessionId, ...eventIds.flatMap((id) => ['--event-id', id]), '--json'],
    { env: storageEnvironment(storage), timeoutMs: 30_000, ...(signal ? { signal } : {}) }
  )).result;
}

export async function listHoneycrispSessionCaptureSummariesAsync(
  sessionId: string,
  storage: HoneycrispSessionStorage,
  signal?: AbortSignal
): Promise<HoneycrispSessionCaptureSummary[]> {
  return (await invokeHoneycrispCliProtocolAsync<HoneycrispSessionCaptureSummary[]>(
    'session.captures',
    ['session', 'captures', '--session-id', sessionId, '--json'],
    { env: storageEnvironment(storage), timeoutMs: 30_000, ...(signal ? { signal } : {}) }
  )).result;
}

export function listHoneycrispSessions(
  workspaceId: string,
  storage: HoneycrispSessionStorage,
  limit = 100
): HoneycrispSessionRecord[] {
  return invokeHoneycrispCliProtocol<HoneycrispSessionSummary[]>(
    'session.list',
    ['session', 'list', '--workspace-id', workspaceId, '--limit', String(limit), '--json'],
    { env: storageEnvironment(storage) }
  ).result.map(sessionRecordFromSummary);
}

export function listHoneycrispSessionSummaries(
  workspaceId: string,
  storage: HoneycrispSessionStorage,
  limit = 100
): HoneycrispSessionSummary[] {
  return invokeHoneycrispCliProtocol<HoneycrispSessionSummary[]>(
    'session.list_summaries',
    ['session', 'list-summaries', '--workspace-id', workspaceId, '--limit', String(limit), '--json'],
    { env: storageEnvironment(storage) }
  ).result;
}

export async function listHoneycrispSessionSummariesAsync(
  workspaceId: string,
  storage: HoneycrispSessionStorage,
  limit = 200
): Promise<HoneycrispSessionSummary[]> {
  return (await invokeHoneycrispCliProtocolAsync<HoneycrispSessionSummary[]>(
    'session.list_summaries',
    ['session', 'list-summaries', '--workspace-id', workspaceId, '--limit', String(limit), '--json'],
    { env: storageEnvironment(storage), timeoutMs: 30_000 }
  )).result;
}

export async function listHoneycrispSessionSummariesForWorkspacesAsync(
  workspaceIds: readonly string[],
  storage: HoneycrispSessionStorage,
  limitPerWorkspace = 200
): Promise<HoneycrispSessionSummary[]> {
  const normalizedWorkspaceIds = [...new Set(workspaceIds.map((workspaceId) => workspaceId.trim()).filter(Boolean))];
  if (normalizedWorkspaceIds.length === 0) return [];
  return (await invokeHoneycrispCliProtocolAsync<HoneycrispSessionSummary[]>(
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

export function getHoneycrispMemorySummary(
  input: {
    workspaceId: string;
    subjectId: string | null;
    sessionId?: string;
    researchProfile?: ResearchProfileSnapshot | null;
    includeForeignCatalogs?: boolean;
    assetIds?: string[];
  },
  storage: HoneycrispSessionStorage
): HoneycrispMemorySummary {
  return decodeHoneycrispMemorySummary(
    invokeWithJsonInput<unknown>('memory.summary', ['knowledge', 'summary'], input, storage).result
  );
}

export async function getHoneycrispMemorySummaryAsync(
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
  storage: HoneycrispSessionStorage,
  signal?: AbortSignal
): Promise<HoneycrispMemorySummary> {
  return decodeHoneycrispMemorySummary((await invokeWithJsonInputAsync<unknown>(
    'memory.summary',
    ['knowledge', 'summary'],
    input,
    storage,
    signal,
    10_000
  )).result);
}

export async function listHoneycrispReportSummariesAsync(
  input: {
    workspaceId: string;
    workspaceRoot?: string;
    researchProfileId?: string;
  },
  storage: HoneycrispSessionStorage,
  signal?: AbortSignal
): Promise<HoneycrispReportSummary[]> {
  const value = (await invokeWithJsonInputAsync<unknown>(
    'report.list',
    ['knowledge', 'report-list'],
    input,
    storage,
    signal,
    10_000
  )).result;
  if (!Array.isArray(value) || !value.every(validReportSummary)) {
    throw new Error('Honeycrisp returned an invalid report catalog payload.');
  }
  return value as HoneycrispReportSummary[];
}

export function prepareHoneycrispMemoryDreaming(
  typeDescriptions: Record<string, string>,
  profileInput: MemoryDreamingProfileInput,
  nodes: HoneycrispMemoryNodeSummary[],
  edges: HoneycrispMemoryEdgeSummary[],
  sessions: HoneycrispDreamingSessionInput[],
  storage: HoneycrispSessionStorage
): HoneycrispDreamingPreparation {
  return invokeWithJsonInput<HoneycrispDreamingPreparation>(
    'dreaming.prepare',
    ['knowledge', 'dreaming-prepare'],
    { typeDescriptions, profileInput, nodes, edges, sessions },
    storage
  ).result;
}

export function parseHoneycrispMemoryDreamingPlan(
  output: string,
  profileInput: MemoryDreamingProfileInput,
  storage: HoneycrispSessionStorage
): MemoryDreamingPlan {
  return invokeWithJsonInput<MemoryDreamingPlan>(
    'dreaming.parse_plan',
    ['knowledge', 'dreaming-parse-plan'],
    { output, profileInput },
    storage
  ).result;
}

export function applyHoneycrispMemoryDreaming(
  workspaceId: string,
  plan: MemoryDreamingPlan,
  context: MemoryDreamingRunContext,
  profileInput: MemoryDreamingProfileInput,
  storage: HoneycrispSessionStorage
): MemoryDreamingRunSummary {
  return invokeWithJsonInput<MemoryDreamingRunSummary>(
    'dreaming.apply',
    ['knowledge', 'dreaming-apply'],
    { workspaceId, plan, context, profileInput },
    storage
  ).result;
}

export function recordHoneycrispMemoryDreamingFailure(
  workspaceId: string,
  context: MemoryDreamingRunContext,
  errorMessage: string,
  profileInput: MemoryDreamingProfileInput,
  storage: HoneycrispSessionStorage
): MemoryDreamingRunSummary {
  return invokeWithJsonInput<MemoryDreamingRunSummary>(
    'dreaming.record_failure',
    ['knowledge', 'dreaming-record-failure'],
    { workspaceId, context, errorMessage, profileInput },
    storage
  ).result;
}

export function restoreHoneycrispMemoryDreamingChange(
  workspaceId: string,
  changeId: string,
  storage: HoneycrispSessionStorage
): void {
  invokeWithJsonInput<{ restored: true }>(
    'dreaming.restore',
    ['knowledge', 'dreaming-restore'],
    { workspaceId, changeId },
    storage
  );
}

export async function getHoneycrispRunbookDocument(
  workspaceId: string,
  runbookId: string,
  storage: HoneycrispSessionStorage
): Promise<HoneycrispRunbookDocument> {
  return (await invokeWithJsonInputAsync<HoneycrispRunbookDocument>(
    'runbook.get',
    ['knowledge', 'runbook-get'],
    { workspaceId, runbookId },
    storage,
    undefined,
    30_000
  )).result;
}

export function getHoneycrispReportDocument(
  workspaceId: string,
  reportId: string,
  storage: HoneycrispSessionStorage
): HoneycrispReportDocument {
  return invokeWithJsonInput<HoneycrispReportDocument>(
    'report.get',
    ['knowledge', 'report-get'],
    { workspaceId, reportId },
    storage
  ).result;
}

export async function reviseHoneycrispReportContent(
  input: {
    workspaceId: string;
    workspaceName: string;
    reportId: string;
    expectedRevision: number;
    content: string;
  },
  storage: HoneycrispSessionStorage
): Promise<void> {
  await invokeAppServerOperation({
    operation: 'report.revise_content',
    input,
    ...(storage.profileId ? { profileId: storage.profileId } : {})
  });
}

export async function updateHoneycrispReportTriageStatus(
  input: {
    workspaceId: string;
    workspaceName: string;
    reportId: string;
    expectedRevision: number;
    triageStatus: 'editing' | 'submitted' | 'reviewing' | 'rejected' | 'accepted';
  },
  storage: HoneycrispSessionStorage
): Promise<void> {
  await invokeAppServerOperation({
    operation: 'report.update_triage_status',
    input,
    ...(storage.profileId ? { profileId: storage.profileId } : {})
  });
}

export async function replaceHoneycrispReportSubmissionPacket(
  input: {
    workspaceId: string;
    workspaceName: string;
    workspaceRoot: string;
    reportId: string;
    submissionPacketPath: string;
  },
  storage: HoneycrispSessionStorage
): Promise<void> {
  await invokeAppServerOperation({
    operation: 'report.replace_packet',
    input,
    ...(storage.profileId ? { profileId: storage.profileId } : {})
  });
}

export async function replaceHoneycrispReportRecording(
  input: {
    workspaceId: string;
    workspaceName: string;
    workspaceRoot: string;
    reportId: string;
    recordingPath: string;
  },
  storage: HoneycrispSessionStorage
): Promise<void> {
  await invokeAppServerOperation({
    operation: 'report.replace_recording',
    input,
    ...(storage.profileId ? { profileId: storage.profileId } : {})
  });
}

export function resolveHoneycrispArtifact(
  artifactId: string,
  storage: HoneycrispSessionStorage,
  expectedKind?: string
): HoneycrispResolvedArtifact {
  return invokeWithJsonInput<HoneycrispResolvedArtifact>(
    'artifact.resolve',
    ['knowledge', 'artifact-resolve'],
    { artifactId, ...(expectedKind ? { expectedKind } : {}) },
    storage
  ).result;
}

export function resolveHoneycrispAuxiliaryModelRoute(input: Record<string, unknown>): HoneycrispAuxiliaryModelRoute {
  return invokeWithJsonInput<HoneycrispAuxiliaryModelRoute>(
    'model_job.resolve',
    ['harness', 'model-job-resolve'],
    input,
    null
  ).result;
}

export function getHoneycrispProviderSemantics(): HoneycrispProviderSemantics {
  providerSemanticsCache ??= invokeWithJsonInput<HoneycrispProviderSemantics>(
    'provider.describe',
    ['harness', 'provider-describe'],
    {},
    null
  ).result;
  return providerSemanticsCache;
}

export function inspectHoneycrispSources(input: Record<string, unknown>): {
  urls?: string[];
  normalizedUrl?: string | null;
  candidates?: HoneycrispSourceRepositoryCandidate[];
  selection?: { candidate: HoneycrispSourceRepositoryCandidate | null; candidates: HoneycrispSourceRepositoryCandidate[]; reason: 'matched' | 'ambiguous' | 'not_found' };
} {
  return invokeWithJsonInput<{
    urls?: string[];
    normalizedUrl?: string | null;
    candidates?: HoneycrispSourceRepositoryCandidate[];
    selection?: { candidate: HoneycrispSourceRepositoryCandidate | null; candidates: HoneycrispSourceRepositoryCandidate[]; reason: 'matched' | 'ambiguous' | 'not_found' };
  }>('source.inspect', ['harness', 'source-inspect'], input, null).result;
}

export async function materializeHoneycrispSource(
  candidate: HoneycrispSourceRepositoryCandidate,
  ref: string,
  repositoryStoreDirectory: string | undefined,
  signal?: AbortSignal,
  cloneMode: RepositoryCloneMode = 'deep'
): Promise<HoneycrispMaterializedSourceRepository> {
  return (await invokeWithJsonInputAsync<HoneycrispMaterializedSourceRepository>(
    'source.materialize',
    ['harness', 'source-materialize'],
    { candidate, ref, cloneMode, ...(repositoryStoreDirectory ? { repositoryStoreDirectory } : {}) },
    null,
    signal,
    null
  )).result;
}

export function materializeHoneycrispSourceSync(
  candidate: HoneycrispSourceRepositoryCandidate,
  ref: string,
  repositoryStoreDirectory?: string,
  cloneMode: RepositoryCloneMode = 'deep'
): HoneycrispMaterializedSourceRepository {
  return invokeWithJsonInput<HoneycrispMaterializedSourceRepository>(
    'source.materialize',
    ['harness', 'source-materialize'],
    { candidate, ref, cloneMode, ...(repositoryStoreDirectory ? { repositoryStoreDirectory } : {}) },
    null
  ).result;
}

export function listHoneycrispPlugins(input: Record<string, unknown>): AgentPluginRegistryState {
  return invokeWithJsonInput<AgentPluginRegistryState>('plugin.list', ['harness', 'plugin-list'], input, null).result;
}

export function addHoneycrispPluginFromFilesystem(input: Record<string, unknown>): AgentPluginRegistryState {
  return invokeWithJsonInput<AgentPluginRegistryState>('plugin.add_filesystem', ['harness', 'plugin-add-filesystem'], input, null).result;
}

export async function addHoneycrispPluginFromRepository(input: Record<string, unknown>): Promise<AgentPluginRegistryState> {
  return (await invokeWithJsonInputAsync<AgentPluginRegistryState>(
    'plugin.add_repository', ['harness', 'plugin-add-repository'], input, null
  )).result;
}

export function setHoneycrispPluginEnabled(input: Record<string, unknown>): AgentPluginRegistryState {
  return invokeWithJsonInput<AgentPluginRegistryState>('plugin.set_enabled', ['harness', 'plugin-set-enabled'], input, null).result;
}

export function removeHoneycrispPlugin(input: Record<string, unknown>): AgentPluginRegistryState {
  return invokeWithJsonInput<AgentPluginRegistryState>('plugin.remove', ['harness', 'plugin-remove'], input, null).result;
}

export function getHoneycrispPluginRuntime(input: Record<string, unknown>): HoneycrispAgentPluginRuntime {
  return invokeWithJsonInput<HoneycrispAgentPluginRuntime>('plugin.runtime', ['harness', 'plugin-runtime'], input, null).result;
}

export function getHoneycrispMaintenanceSummary(workspacePath: string): WorkspaceDejunkSummary {
  return invokeWithJsonInput<WorkspaceDejunkSummary>(
    'maintenance.summary', ['harness', 'maintenance-summary'], { workspacePath }, null
  ).result;
}

export async function getHoneycrispMaintenanceSummaryAsync(workspacePath: string): Promise<WorkspaceDejunkSummary> {
  return (await invokeWithJsonInputAsync<WorkspaceDejunkSummary>(
    'maintenance.summary', ['harness', 'maintenance-summary'], { workspacePath }, null
  )).result;
}

export function runHoneycrispMaintenance(workspacePath: string): WorkspaceDejunkSummary;
export function runHoneycrispMaintenance(
  workspacePath: string,
  options: {
    repositoryStoreDirectory: string;
    repositories?: HoneycrispWorkspaceRepositoryCandidate[];
  }
): HoneycrispMaintenanceRunResult;
export function runHoneycrispMaintenance(
  workspacePath: string,
  options?: {
    repositoryStoreDirectory: string;
    repositories?: HoneycrispWorkspaceRepositoryCandidate[];
  }
): WorkspaceDejunkSummary | HoneycrispMaintenanceRunResult {
  return invokeWithJsonInput<WorkspaceDejunkSummary | HoneycrispMaintenanceRunResult>(
    'maintenance.run', ['harness', 'maintenance-run'], { workspacePath, ...options }, null
  ).result;
}

export async function runHoneycrispMaintenanceAsync(
  workspacePath: string,
  options: {
    repositoryStoreDirectory: string;
    repositories?: HoneycrispWorkspaceRepositoryCandidate[];
  }
): Promise<HoneycrispMaintenanceRunResult> {
  return (await invokeWithJsonInputAsync<HoneycrispMaintenanceRunResult>(
    'maintenance.run', ['harness', 'maintenance-run'], { workspacePath, ...options }, null, undefined, null
  )).result;
}

export function invokeHoneycrispCliProtocol<T>(
  operation: string,
  args: readonly string[],
  options: { timeoutMs?: number; env?: NodeJS.ProcessEnv } = {}
): HoneycrispProtocolSuccess<T> {
  const invocation = resolveHoneycrispProtocolInvocation();
  const requestId = `beale-${randomUUID()}`;
  const result = spawnSync(invocation.command, [...invocation.prefixArgs, ...args, '--request-id', requestId], {
    cwd: invocation.cwd,
    encoding: 'utf8',
    env: protocolEnvironment(options.env),
    timeout: options.timeoutMs ?? 30_000,
    maxBuffer: HONEYCRISP_PROTOCOL_MAX_STDOUT_BYTES,
    windowsHide: true
  });
  let envelope: HoneycrispProtocolEnvelope<T>;
  try {
    envelope = decodeHoneycrispProtocolEnvelope<T>(String(result.stdout ?? '').trim());
  } catch (error) {
    const detail = protocolProcessDetail(result);
    throw new Error(`Honeycrisp ${operation} returned an invalid protocol envelope${detail ? ` (${detail})` : ''}.`, { cause: error });
  }
  if (envelope.operation !== operation) {
    throw new Error(`Honeycrisp protocol operation mismatch: expected ${operation}, received ${envelope.operation}.`);
  }
  if (envelope.requestId !== requestId) {
    throw new Error(`Honeycrisp protocol request mismatch: expected ${requestId}, received ${envelope.requestId ?? 'none'}.`);
  }
  if (!envelope.ok) {
    throw new Error(`Honeycrisp ${operation} failed (${envelope.error.code}): ${envelope.error.message}`);
  }
  if (result.status !== 0) {
    throw new Error(`Honeycrisp ${operation} returned a success envelope with exit status ${String(result.status)}.`);
  }
  return envelope;
}

function invokeWithJsonInput<T>(
  operation: string,
  args: readonly string[],
  input: unknown,
  storage: HoneycrispSessionStorage | null
): HoneycrispProtocolSuccess<T> {
  const directory = mkdtempSync(join(tmpdir(), 'beale-honeycrisp-protocol-'));
  const inputPath = join(directory, 'input.json');
  try {
    writeFileSync(inputPath, `${JSON.stringify(input)}\n`, { encoding: 'utf8', mode: 0o600 });
    return invokeHoneycrispCliProtocol<T>(operation, [...args, '--input', inputPath, '--json'], {
      ...(storage ? { env: storageEnvironment(storage) } : {})
    });
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

async function invokeWithJsonInputAsync<T>(
  operation: string,
  args: readonly string[],
  input: unknown,
  storage: HoneycrispSessionStorage | null,
  signal?: AbortSignal,
  timeoutMs?: number | null
): Promise<HoneycrispProtocolSuccess<T>> {
  const directory = mkdtempSync(join(tmpdir(), 'beale-honeycrisp-protocol-'));
  const inputPath = join(directory, 'input.json');
  try {
    writeFileSync(inputPath, `${JSON.stringify(input)}\n`, { encoding: 'utf8', mode: 0o600 });
    return await invokeHoneycrispCliProtocolAsync<T>(operation, [...args, '--input', inputPath, '--json'], {
      ...(storage ? { env: storageEnvironment(storage) } : {}),
      ...(signal ? { signal } : {}),
      ...(timeoutMs !== undefined ? { timeoutMs } : {})
    });
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

export function invokeHoneycrispCliProtocolAsync<T>(
  operation: string,
  args: readonly string[],
  options: { timeoutMs?: number | null; env?: NodeJS.ProcessEnv; signal?: AbortSignal; stdin?: string } = {}
): Promise<HoneycrispProtocolSuccess<T>> {
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
    operation: operation as HoneycrispProtocolOperation,
    args,
    ...(input !== undefined ? { input } : {}),
    ...profileSelection(options.env, input),
    ...(signal ? { signal } : {})
  }).then((result) => honeycrispProtocolSuccess(operation as HoneycrispProtocolOperation, result, requestId));
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
  const configuredProfileId = environment?.HONEYCRISP_PROFILE_ID?.trim();
  if (configuredProfileId) return { profileId: configuredProfileId };
  const databasePath = environment?.HONEYCRISP_DATABASE_PATH?.trim();
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

function protocolEnvironment(overrides?: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  return {
    ...process.env,
    NO_COLOR: process.env.NO_COLOR ?? '1',
    NODE_NO_WARNINGS: '1',
    ...overrides
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

function sessionRecordFromSummary(summary: HoneycrispSessionSummary): HoneycrispSessionRecord {
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

function storageEnvironment(storage: HoneycrispSessionStorage): NodeJS.ProcessEnv {
  return {
    HONEYCRISP_DATABASE_PATH: storage.databasePath,
    HONEYCRISP_ARTIFACT_DIRECTORY: storage.artifactDirectoryPath,
    ...(storage.profileId ? { HONEYCRISP_PROFILE_ID: storage.profileId } : {})
  };
}

function honeycrispInvocationKey(): string {
  const invocation = resolveHoneycrispProtocolInvocation();
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

export function decodeHoneycrispMemorySummary(value: unknown): HoneycrispMemorySummary {
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
    throw new Error('Honeycrisp returned an invalid memory summary v9 payload.');
  }
  return value as unknown as HoneycrispMemorySummary;
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
    && Array.isArray(value.evidence)
    && Array.isArray(value.transitions)
    && Array.isArray(value.authors)
    && value.authors.every((author) => isPlainRecord(author)
      && nonEmptyText(author.provider)
      && nonEmptyText(author.model));
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
