import { randomBytes } from 'node:crypto';
import { dirname, join } from 'node:path';
import type {
  ApprovalRecord,
  ArtifactRecord,
  AttemptRecord,
  AttemptStatus,
  BreakoutRoomMemberRecord,
  BreakoutRoomMessageRecord,
  BreakoutRoomRecord,
  BreakoutRoomSummary,
  ContextCompactionRecord,
  ExportRecord,
  ExportReviewDecision,
  GeneratedResearchGoalSuggestions,
  ModelSessionRecord,
  NotificationRecord,
  NotificationStatus,
  ProjectInventoryRefreshReport,
  ProjectInventorySummary,
  ProjectSearchResult,
  ProjectStructureSummary,
  ResearchModelSelection,
  ResearchKitId,
  ResearchProfileSnapshot,
  ResearchSubject,
  ResearchSubjectInput,
  ResolvedResearchProfile,
  RunDetail,
  RunDetailUpdate,
  RunDetailUpdateCursor,
  RunDetailVersion,
  RunRecord,
  RunRow,
  RunStatus,
  SessionNextPromptSuggestion,
  SessionTranscriptSearchInput,
  SessionTranscriptSearchResponse,
  ShellSafetyMode,
  StartRunInput,
  TraceEventRecord,
  TranscriptMessageRecord,
  VerifierContractEditInput,
  VerifierContractRecord,
  VerifierRunRecord,
  WorkspaceExportResult,
  WorkspaceRecoveryReport,
  WorkspaceRule,
  WorkspaceScopeDraft,
  WorkspaceScopeVersion,
} from '@shared/types';
import { invokeAppServerWorkspaceState, type AppServerSessionStorage } from './appServerCliClient';

type WorkspaceDatabaseConstructor = new (
  databasePath: string,
  artifactRoot: string,
  workspaceIdentity?: { workspacePath: string; workspaceId?: string; researchKitId?: ResearchKitId },
) => WorkspaceDatabase;

export type CreatedRunContext = import('../../../../app-server/src/workspaceDatabase').CreatedRunContext;
export type StartRunRecordInput = import('../../../../app-server/src/workspaceDatabase').StartRunRecordInput;
export type ProjectSourceCoveragePathRecord = import('../../../../app-server/src/workspaceDatabase').ProjectSourceCoveragePathRecord;
export type ProjectSourceReviewObservation = import('../../../../app-server/src/workspaceDatabase').ProjectSourceReviewObservation;
export type ProjectStructureEntityRecord = import('../../../../app-server/src/workspaceDatabase').ProjectStructureEntityRecord;
export type ProjectStructureRelationRecord = import('../../../../app-server/src/workspaceDatabase').ProjectStructureRelationRecord;
export type ResearchRecommendationRunContext = import('../../../../app-server/src/workspaceDatabase').ResearchRecommendationRunContext;
type AppendTraceInput = import('../../../../app-server/src/workspaceDatabase').AppendTraceInput;
type CreateApprovalInput = import('../../../../app-server/src/workspaceDatabase').CreateApprovalInput;
type CreateArtifactInput = import('../../../../app-server/src/workspaceDatabase').CreateArtifactInput;
type CreateAttemptInput = import('../../../../app-server/src/workspaceDatabase').CreateAttemptInput;
type CreateBreakoutRoomMessageInput = import('../../../../app-server/src/workspaceDatabase').CreateBreakoutRoomMessageInput;
type CreateContextCompactionInput = import('../../../../app-server/src/workspaceDatabase').CreateContextCompactionInput;
type CreateExportInput = import('../../../../app-server/src/workspaceDatabase').CreateExportInput;
type CreateModelSessionInput = import('../../../../app-server/src/workspaceDatabase').CreateModelSessionInput;
type CreateNotificationInput = import('../../../../app-server/src/workspaceDatabase').CreateNotificationInput;
type CreateToolCallInput = import('../../../../app-server/src/workspaceDatabase').CreateToolCallInput;
type CreateTranscriptMessageInput = import('../../../../app-server/src/workspaceDatabase').CreateTranscriptMessageInput;
type CreateVerifierContractInput = import('../../../../app-server/src/workspaceDatabase').CreateVerifierContractInput;
type CreateVerifierRunInput = import('../../../../app-server/src/workspaceDatabase').CreateVerifierRunInput;
type RepositoryPathRelocation = import('../../../../app-server/src/workspaceDatabase').RepositoryPathRelocation;
type ResearchGoalSuggestionCacheRecord = import('../../../../app-server/src/workspaceDatabase').ResearchGoalSuggestionCacheRecord;
type ResearchGoalSuggestionHistoryRecord = import('../../../../app-server/src/workspaceDatabase').ResearchGoalSuggestionHistoryRecord;
type UpsertBreakoutRoomInput = import('../../../../app-server/src/workspaceDatabase').UpsertBreakoutRoomInput;
type UpsertBreakoutRoomMemberInput = import('../../../../app-server/src/workspaceDatabase').UpsertBreakoutRoomMemberInput;

export interface WorkspaceDatabase {
  initialize(): void;
  checkpoint(): void;
  close(): void;
  getWorkspaceId(): string;
  getResearchKitId(): ResearchKitId;
  getDatabasePath(): string;
  getWorkspacePath(): string;
  getArtifactRoot(): string;
  getLastWorkspaceBackup(): WorkspaceExportResult | null;
  recordWorkspaceBackup(result: WorkspaceExportResult): void;
  recoverInterruptedState(reason?: string): WorkspaceRecoveryReport;
  getActiveScope(): WorkspaceScopeVersion;
  getScopeVersion(scopeVersionId: string): WorkspaceScopeVersion;
  activateResearchProfileSnapshot(resolvedProfile: ResolvedResearchProfile): ResearchProfileSnapshot;
  getActiveResearchProfileSnapshot(): ResearchProfileSnapshot | null;
  getResearchProfileSnapshot(snapshotId: string): ResearchProfileSnapshot | null;
  getResearchProfileSnapshotForWorkspace(workspaceId: string, snapshotId: string): ResearchProfileSnapshot | null;
  getRunResearchProfileSnapshot(runId: string): ResearchProfileSnapshot | null;
  getResearchSubject(): ResearchSubject;
  setResearchSubject(input: ResearchSubjectInput): ResearchSubject;
  saveScope(draft: WorkspaceScopeDraft, options?: { refreshInventory?: boolean }): WorkspaceScopeVersion;
  rewriteRepositoryPathReferences(relocations: readonly RepositoryPathRelocation[]): void;
  listWorkspaceRules(): WorkspaceRule[];
  addWorkspaceRules(values: readonly string[], createdBy?: string): WorkspaceRule[];
  addWorkspaceRule(value: string, createdBy?: string): WorkspaceRule;
  createRun(input: StartRunRecordInput): CreatedRunContext;
  createModelSession(input: CreateModelSessionInput): ModelSessionRecord;
  createContextCompaction(input: CreateContextCompactionInput): ContextCompactionRecord;
  setContextCompactionTrace(compactionId: string, traceEventId: string): void;
  createAttempt(input: CreateAttemptInput): AttemptRecord;
  updateModelSessionByRun(runId: string, patch: { previousResponseId?: string | null; status?: string; metadata?: Record<string, unknown> }): void;
  appendTraceEvent(input: AppendTraceInput): TraceEventRecord;
  createTranscriptMessage(input: CreateTranscriptMessageInput): TranscriptMessageRecord;
  upsertBreakoutRoom(input: UpsertBreakoutRoomInput): BreakoutRoomRecord;
  upsertBreakoutRoomMember(input: UpsertBreakoutRoomMemberInput): BreakoutRoomMemberRecord;
  createBreakoutRoomMessage(input: CreateBreakoutRoomMessageInput): BreakoutRoomMessageRecord;
  interruptActiveBreakoutRooms(runId: string, attemptId?: string | null, endedAt?: string): void;
  findBreakoutRoomMember(runId: string, attemptId: string | null, agentPath: string): BreakoutRoomMemberRecord | null;
  refreshBreakoutRoomStatus(roomId: string): BreakoutRoomRecord | null;
  listBreakoutRoomSummaries(runId: string): BreakoutRoomSummary[];
  createNotification(input: CreateNotificationInput): NotificationRecord;
  listNotifications(status?: NotificationStatus): NotificationRecord[];
  markNotificationOpened(notificationId: string): NotificationRecord | null;
  dismissNotification(notificationId: string): NotificationRecord | null;
  createToolCall(input: CreateToolCallInput): string;
  linkToolCallTrace(toolCallId: string, traceEventId: string): void;
  finishToolCall(toolCallId: string, status: string, resultSummary: string, result: Record<string, unknown>): void;
  updateRunStatus(runId: string, status: RunStatus, summary: string, dispositionInput?: Record<string, unknown>): void;
  beginSessionRunActivity(runId: string, attemptId: string): void;
  getSessionNextStepSuggestions(runId: string): GeneratedResearchGoalSuggestions | null;
  getCapturedSessionNextPromptSuggestions(runId: string): SessionNextPromptSuggestion[];
  saveSessionNextStepSuggestions(runId: string, value: GeneratedResearchGoalSuggestions): GeneratedResearchGoalSuggestions;
  getResearchGoalSuggestionContextRevision(scopeId: string): string;
  getResearchGoalSuggestionCache(scopeId: string, profileHash: string, phase: string): ResearchGoalSuggestionCacheRecord | null;
  saveResearchGoalSuggestionCache(input: { scopeId: string; profileHash: string; phase: string; contextRevision: string; suggestions: readonly string[]; generatedAt?: string }): ResearchGoalSuggestionCacheRecord;
  listResearchGoalSuggestionHistory(scopeId: string, profileHash: string, phase: string, limit?: number): ResearchGoalSuggestionHistoryRecord[];
  selectResearchGoalSuggestion(input: { scopeId: string; profileHash: string; phase: string; suggestion: string; selectedAt?: string }): void;
  updateRunTitle(runId: string, title: string): RunRecord;
  updateRunPrompt(runId: string, promptMarkdown: string): RunRecord;
  updateRunModelSelection(runId: string, selection: ResearchModelSelection): RunRecord;
  updateRunShellSafetyMode(runId: string, shellSafetyMode: ShellSafetyMode): RunRecord;
  updateRunBudget(runId: string, budgetPatch: Partial<StartRunInput['budget']>): RunRecord;
  updateAttemptState(attemptId: string, status: AttemptStatus, shortState: string): void;
  createArtifact(input: CreateArtifactInput): ArtifactRecord;
  setArtifactProvenance(artifactId: string, traceEventId: string): void;
  markArtifactSensitive(artifactId: string): void;
  createVerifierContract(input: CreateVerifierContractInput): VerifierContractRecord;
  updateVerifierContract(contractId: string, patch: VerifierContractEditInput & { status?: string }): VerifierContractRecord;
  createVerifierRun(input: CreateVerifierRunInput): VerifierRunRecord;
  createExportRecord(input: CreateExportInput): string;
  updateExportReview(exportId: string, decision: ExportReviewDecision, note: string): ExportRecord;
  createApproval(input: CreateApprovalInput): ApprovalRecord;
  updateApprovalDecision(approvalId: string, runId: string, decision: string, reason: string): ApprovalRecord;
  listPendingShellApprovals(): ApprovalRecord[];
  listRunRows(): RunRow[];
  getRunRow(runId: string): RunRow | null;
  listResearchRecommendationRuns(limit?: number, prioritizeRunId?: string | null): ResearchRecommendationRunContext[];
  getRunDetail(runId: string): RunDetail;
  searchTranscriptMessages(input: SessionTranscriptSearchInput, context: { registryWorkspaceId: string; workspacePath: string; workspaceName: string }): SessionTranscriptSearchResponse;
  searchTranscriptMessagesAcrossWorkspaces(input: SessionTranscriptSearchInput, contexts: readonly { databaseWorkspaceId: string; registryWorkspaceId: string; workspacePath: string; workspaceName: string }[]): SessionTranscriptSearchResponse;
  getProjectInventorySummary(scopeVersionId?: string): ProjectInventorySummary;
  getProjectStructureSummary(scopeVersionId?: string): ProjectStructureSummary;
  getProjectStructureCoverageRecords(scopeVersionId?: string, limits?: { entities?: number; relations?: number; refreshIndex?: boolean }): { index: { rootCount: number; skippedCount: number; truncated: boolean }; paths: ProjectSourceCoveragePathRecord[]; entities: ProjectStructureEntityRecord[]; relations: ProjectStructureRelationRecord[] };
  listProjectSourceReviewObservations(scopeVersionId?: string, maxTraceRows?: number): ProjectSourceReviewObservation[];
  ensureProjectInventory(scopeVersionId?: string): ProjectInventorySummary;
  refreshProjectInventory(scopeVersionId?: string): ProjectInventoryRefreshReport;
  rebuildProjectSearchIndex(options?: { includeInventory?: boolean }): void;
  searchProjectDocumentsForRun(runId: string, query: string, limit?: number, options?: { refreshInventory?: boolean; scopeVersionId?: string }): ProjectSearchResult[];
  getRunDetailUpdate(runId: string, cursor: RunDetailUpdateCursor): RunDetailUpdate;
  getRunDetailVersion(runId: string): RunDetailVersion;
  getRun(runId: string): RunRecord | null;
  getFirstAttempt(runId: string): AttemptRecord | null;
  getFirstArtifact(runId: string): ArtifactRecord | null;
  getFirstVerifierContract(runId: string): VerifierContractRecord | null;
}

interface WorkspaceStateSnapshot {
  workspaceId: string;
  researchKitId: ResearchKitId;
  activeScope: WorkspaceScopeVersion;
  activeResearchProfile: ResearchProfileSnapshot | null;
  researchSubject: ResearchSubject;
  workspaceRules: WorkspaceRule[];
  lastWorkspaceBackup: WorkspaceExportResult | null;
}

class WorkspaceDatabaseClient {
  private state: WorkspaceStateSnapshot | null = null;
  private readonly storage: AppServerSessionStorage;
  private readonly workspaceIdentity: { workspacePath: string; workspaceId?: string; researchKitId?: ResearchKitId };

  public constructor(
    private readonly databasePath: string,
    private readonly artifactRoot: string,
    workspaceIdentity: { workspacePath: string; workspaceId?: string; researchKitId?: ResearchKitId } = {
      workspacePath: dirname(databasePath),
    },
  ) {
    this.workspaceIdentity = workspaceIdentity;
    this.storage = { databasePath, artifactDirectoryPath: join(dirname(databasePath), 'artifacts') };
    return new Proxy(this, {
      get: (target, property, receiver) => {
        if (Reflect.has(target, property)) return Reflect.get(target, property, receiver) as unknown;
        if (typeof property !== 'string') return undefined;
        return (...args: unknown[]) => target.invoke(property, args);
      },
    });
  }

  public initialize(): void {
    this.state = this.invoke<WorkspaceStateSnapshot>('initialize', []);
  }

  public checkpoint(): void {
    this.invoke('checkpoint', []);
  }

  public close(): void {
    // The app-server owns short-lived SQLite handles for each state operation.
  }

  public getWorkspaceId(): string {
    return this.requireState().workspaceId;
  }

  public getResearchKitId(): ResearchKitId {
    return this.requireState().researchKitId;
  }

  public getDatabasePath(): string {
    return this.databasePath;
  }

  public getWorkspacePath(): string {
    return this.workspaceIdentity.workspacePath;
  }

  public getArtifactRoot(): string {
    return this.artifactRoot;
  }

  public getLastWorkspaceBackup(): WorkspaceExportResult | null {
    return this.requireState().lastWorkspaceBackup;
  }

  public recordWorkspaceBackup(result: WorkspaceExportResult): void {
    this.invoke('recordWorkspaceBackup', [result]);
    this.requireState().lastWorkspaceBackup = result;
  }

  public getActiveScope(): WorkspaceScopeVersion {
    return this.requireState().activeScope;
  }

  public activateResearchProfileSnapshot(resolvedProfile: ResolvedResearchProfile): ResearchProfileSnapshot {
    const result = this.invoke<ResearchProfileSnapshot>('activateResearchProfileSnapshot', [resolvedProfile]);
    this.requireState().activeResearchProfile = result;
    return result;
  }

  public getActiveResearchProfileSnapshot(): ResearchProfileSnapshot | null {
    return this.requireState().activeResearchProfile;
  }

  public getResearchSubject(): ResearchSubject {
    return this.requireState().researchSubject;
  }

  public setResearchSubject(input: ResearchSubjectInput): ResearchSubject {
    const result = this.invoke<ResearchSubject>('setResearchSubject', [input]);
    this.requireState().researchSubject = result;
    return result;
  }

  public saveScope(draft: WorkspaceScopeDraft, options: { refreshInventory?: boolean } = {}): WorkspaceScopeVersion {
    const result = this.invoke<WorkspaceScopeVersion>('saveScope', [draft, options]);
    this.requireState().activeScope = result;
    return result;
  }

  public rewriteRepositoryPathReferences(relocations: readonly RepositoryPathRelocation[]): void {
    this.invoke('rewriteRepositoryPathReferences', [relocations]);
    this.requireState().activeScope = this.invoke<WorkspaceScopeVersion>('getActiveScope', []);
  }

  public listWorkspaceRules(): WorkspaceRule[] {
    return [...this.requireState().workspaceRules];
  }

  public addWorkspaceRules(values: readonly string[], createdBy = 'local_user'): WorkspaceRule[] {
    const result = this.invoke<WorkspaceRule[]>('addWorkspaceRules', [values, createdBy]);
    this.requireState().workspaceRules = mergeRules(this.requireState().workspaceRules, result);
    return result;
  }

  public addWorkspaceRule(value: string, createdBy = 'local_user'): WorkspaceRule {
    const result = this.invoke<WorkspaceRule>('addWorkspaceRule', [value, createdBy]);
    this.requireState().workspaceRules = mergeRules(this.requireState().workspaceRules, [result]);
    return result;
  }

  private invoke<T = unknown>(action: string, args: unknown[]): T {
    return invokeAppServerWorkspaceState<T>({
      ...this.workspaceIdentity,
      artifactRoot: this.artifactRoot,
      databasePath: this.storage.databasePath,
      artifactDirectoryPath: this.storage.artifactDirectoryPath,
      action,
      args,
    }, this.storage);
  }

  private requireState(): WorkspaceStateSnapshot {
    if (!this.state) throw new Error('Workspace state has not been initialized through the app-server.');
    return this.state;
  }
}

export const WorkspaceDatabase = WorkspaceDatabaseClient as unknown as WorkspaceDatabaseConstructor;

export function createId(prefix: string): string {
  return `${prefix}_${Date.now().toString(36)}_${randomBytes(6).toString('hex')}`;
}

export function nowIso(): string {
  return new Date().toISOString();
}

export function normalizeSessionNextStepSuggestions(value: unknown): GeneratedResearchGoalSuggestions | null {
  const parsed = typeof value === 'string' ? parseObject(value) : isRecord(value) ? value : {};
  const phase = typeof parsed.phase === 'string' ? parsed.phase.trim() : '';
  const suggestions = Array.isArray(parsed.suggestions)
    ? parsed.suggestions.map((suggestion) => typeof suggestion === 'string' ? suggestion.replace(/\s+/g, ' ').trim() : '')
    : [];
  if (!phase || suggestions.length !== 3 || suggestions.some((suggestion) => !suggestion)
    || new Set(suggestions.map((suggestion) => suggestion.toLocaleLowerCase())).size !== 3) return null;
  const promptSuggestions = parseSessionNextPromptSuggestions(parsed.promptSuggestions);
  return { phase, suggestions, ...(promptSuggestions.length === 3 ? { promptSuggestions } : {}) };
}

export function parseSessionNextPromptSuggestions(value: unknown): SessionNextPromptSuggestion[] {
  if (!Array.isArray(value)) return [];
  const suggestions: SessionNextPromptSuggestion[] = [];
  const identities = new Set<string>();
  for (const candidate of value) {
    if (!isRecord(candidate)) continue;
    const title = typeof candidate.title === 'string' ? candidate.title.replace(/\s+/g, ' ').trim() : '';
    const promptMarkdown = typeof candidate.promptMarkdown === 'string' ? candidate.promptMarkdown.trim() : '';
    const rationale = typeof candidate.rationale === 'string' ? candidate.rationale.replace(/\s+/g, ' ').trim() : '';
    const identity = title.toLocaleLowerCase();
    if (!title || !promptMarkdown || identities.has(identity)) continue;
    identities.add(identity);
    suggestions.push({ title, promptMarkdown, ...(rationale ? { rationale } : {}) });
    if (suggestions.length === 3) break;
  }
  return suggestions;
}

function mergeRules(current: WorkspaceRule[], next: WorkspaceRule[]): WorkspaceRule[] {
  return [...new Map([...current, ...next].map((rule) => [rule.id, rule])).values()];
}

function parseObject(value: string): Record<string, unknown> {
  try {
    const parsed = JSON.parse(value) as unknown;
    return isRecord(parsed) ? parsed : {};
  } catch {
    return {};
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}
