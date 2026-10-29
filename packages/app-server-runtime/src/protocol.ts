import { createHash } from "node:crypto";
export { MANAGED_TOOL_PLUGINS, MANAGED_TOOL_PLUGIN_IDS } from "@beale/research-agent/managed-tool-plugins";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import type {
  GeneratedResearchGoalSuggestions,
  ResearchClaimRating,
  ResearchResourceKind,
  ResearchGoalSuggestionInput,
  ResearchGoalSuggestionSelectionInput,
  KnowledgeReportDocument,
  KnowledgeRunbookDocument,
} from "@beale/research-agent";

export type {
  ResourcePriorArtPage,
  ResourcePriorArtDetail,
  ResourcePriorArtSummary,
  ResearchClaimRating,
  GeneratedResearchGoalSuggestions,
  ResearchGoalSuggestionInput,
  ResearchGoalSuggestionSelectionInput,
} from "@beale/research-agent";

export interface SteeringSuggestionResult {
  suggestion: string | null;
}

export function decodeGeneratedResearchGoalSuggestions(value: unknown, expectedPhase?: string): GeneratedResearchGoalSuggestions {
  if (!isRecord(value) || typeof value.phase !== "string" || !value.phase.trim()
    || (expectedPhase !== undefined && value.phase !== expectedPhase)
    || !Array.isArray(value.suggestions)
    || !value.suggestions.every((suggestion) => typeof suggestion === "string")) {
    throw new Error("The app-server returned an invalid research suggestion response.");
  }
  return value as unknown as GeneratedResearchGoalSuggestions;
}

export interface ResourcePriorArtListInput {
  workspaceId: string;
  resources: Array<{ kind: ResearchResourceKind; locator: string }>;
  before?: number;
}
export interface ResourcePriorArtGetInput extends Omit<ResourcePriorArtListInput, "before"> {
  id: string;
  offset?: number;
}

export const APP_SERVER_PROTOCOL_NAME = "app-server" as const;
export const APP_SERVER_PROTOCOL_VERSION = 1 as const;
export const APP_SERVER_CONTRACT_VERSION = 64 as const;
export const APP_SERVER_RUNTIME_VERSION = "0.9.5" as const;
export const APP_SERVER_PROTOCOL_WEBSOCKET_PATH = "/v1/session" as const;
export const APP_SERVER_PROTOCOL_BOOTSTRAP_PREFIX = "APP_SERVER_TRANSPORT " as const;
/**
 * Bump this UTC timestamp whenever the Desktop/app-server control contract
 * changes. Both binaries compile the same value and compare it directionally.
 */
export const BEALE_APP_SERVER_CONTRACT_TIMESTAMP = "2026-10-26T20:35:24.000Z" as const;
export const BEALE_APP_SERVER_CONTROL_VERSION = 1 as const;
export const BEALE_APP_SERVER_CAPABILITIES = [
  "fleet.inventory.v1",
  "fleet.lifecycle.v1",
  "fleet.session.v1",
  "fleet.multi-target-session.v1",
  "fleet.session-clone.v1",
  "fleet.remote-servers.v1",
  "fleet.session-ownership.v1",
  "fleet.model-broker.v1",
  "fleet.browser-relay.v1",
  "fleet.remote-browser-relay.v1",
  "workspace.research-project.v3",
  "workspace.details.v1",
  "workspace.surface-map.v1",
  "workspace.research-track.v1",
  "quick-chat.v1",
  "session.typed-launch.v3",
  "session.openai-fast-mode.v1",
  "session.openai-daybreak-blue.v1",
  "session.introspection-runtime.v1",
  "session.exit-diagnostics",
  "session.transport-path.v1",
  "session.reconnect.v1",
  "session.event-identity.v1",
  "session.continuation.v1",
  "session.startup-recovery.v1",
  "session.multi-client.v1",
  "session.http-control.v1",
  "host.control.v1",
  "host.descriptor.v1",
  "host.provider-catalog.v1",
  "canonical.reads.v1",
  "session.commentary.v1",
  "memory.notifications.v3",
  "host.operations.v1",
  "research.tools.v1",
  "source.clone-modes.v1",
  "host.shutdown-guard.v1",
  "workspace.memory-backend.v1",
  "workspace.research-subject-mutation.v1",
  "knowledge.claims.v2",
  "knowledge.claim-evidence-validation.v1",
  "knowledge.claim-sql-pagination.v1",
  "runbook.execution-snapshots.v1",
  "knowledge.claim-deduplication.v1",
  "knowledge.claim-board-transition.v1",
  "knowledge.history-deduplication.v1",
  "workspace.state.v1",
  "registry.state.v1",
  "registry.workspace-sync.v2",
  "knowledge.claim-security-tracking.v1",
  "workspace.goal-suggestions.v1",
  "workspace.prompt-expansion.v1",
  "knowledge.report-content-revise.v1",
  "knowledge.report-triage-status.v1",
  "knowledge.report-packet-replace.v1",
  "knowledge.report-recording-replace.v1",
  "knowledge.report-list.v1",
  "workspace.resource-prior-art.v1",
  "runbook.delegated-cell-runtime.v1",
  "runbook.host-cell-runtime.v1",
  "tart.guest-helper-repair.v1",
] as const;
export const BEALE_APP_SERVER_SERVER_PATH = "/v1/server" as const;
export const BEALE_APP_SERVER_SESSIONS_PATH = "/v1/sessions" as const;
export const BEALE_APP_SERVER_WORKSPACES_PATH = "/v1/workspaces" as const;
export const BEALE_APP_SERVER_QUICK_CHATS_PATH = "/v1/quick-chats" as const;
export function bealeAppServerWorkspaceDetailsPath(workspaceId: string): string {
  return `${BEALE_APP_SERVER_WORKSPACES_PATH}/${encodeURIComponent(workspaceId)}/details`;
}
export const BEALE_APP_SERVER_PROVIDERS_PATH = "/v1/providers" as const;
export const BEALE_APP_SERVER_PROVIDER_LIMITS_PATH = "/v1/providers/limits" as const;
export const BEALE_APP_SERVER_OPERATIONS_PATH = "/v1/operations" as const;
export const BEALE_APP_SERVER_BORE_CHECKPOINTS_PATH = "/v1/bore/checkpoints" as const;
export const BEALE_APP_SERVER_BORE_RECEIPTS_PATH = "/v1/bore/receipts" as const;
export interface BealeBoreContract {
  schemaVersion: 1;
  checkpointVersion: 1;
  hostReceiptVersion: 1;
}
export interface BealeBoreCheckpointResult { schemaVersion: 1; sessionId: string; contentHash: string; }
export interface BealeBoreHostReceiptResult { schemaVersion: 1; jobId: string; contentHash: string; }
export const BEALE_APP_SERVER_SHUTDOWN_PATH = "/v1/server/shutdown" as const;
export const BEALE_APP_SERVER_SUPERVISOR_LOCAL_PORT = 47_175 as const;
export const BEALE_APP_SERVER_SUPERVISOR_HTTPS_PORT = 47_176 as const;
export const BEALE_APP_SERVER_SUPERVISOR_RESTART_PATH = "/v1/supervisor/restart" as const;
export const BEALE_APP_SERVER_SUPERVISOR_SHUTDOWN_PATH = "/v1/supervisor/shutdown" as const;
export const BEALE_APP_SERVER_MAX_REPLAY_BYTES = 4_194_304 as const;
export const BEALE_APP_SERVER_MAX_REPLAY_FRAMES = 256 as const;

export interface BealeAppServerHealth {
  ok: true;
  controlVersion: typeof BEALE_APP_SERVER_CONTROL_VERSION;
  contractTimestamp: typeof BEALE_APP_SERVER_CONTRACT_TIMESTAMP;
  capabilities: typeof BEALE_APP_SERVER_CAPABILITIES;
}

export interface BealeAppServerDescriptor extends BealeAppServerHealth {
  sessionLaunchVersion: typeof APP_SERVER_SESSION_LAUNCH_VERSION;
  appServerProtocolVersion: typeof APP_SERVER_PROTOCOL_VERSION;
  endpoints: {
    sessions: typeof BEALE_APP_SERVER_SESSIONS_PATH;
    workspaces: typeof BEALE_APP_SERVER_WORKSPACES_PATH;
    providers: typeof BEALE_APP_SERVER_PROVIDERS_PATH;
    operations: typeof BEALE_APP_SERVER_OPERATIONS_PATH;
    shutdown: typeof BEALE_APP_SERVER_SHUTDOWN_PATH;
  };
  limits: {
    requestBodyBytes: number;
    frameBytes: number;
    replayBytes: typeof BEALE_APP_SERVER_MAX_REPLAY_BYTES;
    replayFrames: typeof BEALE_APP_SERVER_MAX_REPLAY_FRAMES;
  };
}

export interface BealeAppServerProviderModel {
  id: string;
  name: string;
  reasoning: boolean;
  effortLevels: string[];
}

export interface BealeAppServerProviderCatalogEntry {
  providerId: string;
  providerName: string;
  riskAcknowledged?: boolean;
  defaultLeadModel: string | null;
  defaultSubagentModel: string | null;
  defaultReasoningEffort: string | null;
  models: BealeAppServerProviderModel[];
}

export interface BealeAppServerProviderCatalog {
  controlVersion: typeof BEALE_APP_SERVER_CONTROL_VERSION;
  defaultProviderId: string | null;
  providers: BealeAppServerProviderCatalogEntry[];
}

export type BealeProviderLimitId = "openai-codex" | "xai" | "anthropic" | "zai" | "openrouter";

export interface BealeProviderUsageWindow {
  id: string;
  label: string;
  usedPercent: number;
  resetsAt: string | null;
  durationMinutes: number | null;
}

export interface BealeProviderUsageLimits {
  providerId: BealeProviderLimitId;
  checkedAt: string;
  status: "available" | "unsupported" | "not_configured" | "unavailable";
  detail: string | null;
  windows: BealeProviderUsageWindow[];
}

export type BealeAppServerSessionState = "starting" | "running" | "completed" | "failed" | "stopped";

export interface BealeAppServerSessionCatalogEntry {
  sessionId: string;
  state: BealeAppServerSessionState;
  startedAt: string;
  endedAt: string | null;
  exitCode: number | null;
  clientConnected: boolean;
  diagnostic: string | null;
  replay: {
    bufferedFrames: number;
    bufferedBytes: number;
    droppedFrames: number;
  };
}

export interface BealeAppServerSessionCatalog {
  controlVersion: typeof BEALE_APP_SERVER_CONTROL_VERSION;
  sessions: BealeAppServerSessionCatalogEntry[];
}

export interface BealeAppServerSessionResult {
  controlVersion: typeof BEALE_APP_SERVER_CONTROL_VERSION;
  session: BealeAppServerSessionCatalogEntry;
}

export interface BealeAppServerSessionTransport {
  path: string;
  protocolVersion: typeof APP_SERVER_PROTOCOL_VERSION;
  authentication: "bearer";
  token: string;
  reconnect: "replay";
}

export interface BealeAppServerSessionStartResult {
  controlVersion: typeof BEALE_APP_SERVER_CONTROL_VERSION;
  session: BealeAppServerSessionCatalogEntry;
  attemptId: string;
  transport: BealeAppServerSessionTransport;
}

export interface BealeAppServerSessionContinuationRequest {
  workspaceId: string;
  instruction: string;
}

export type BealeAppServerSessionContinuationResult = BealeAppServerSessionStartResult;

export interface BealeAppServerSessionAttachResult {
  controlVersion: typeof BEALE_APP_SERVER_CONTROL_VERSION;
  session: BealeAppServerSessionCatalogEntry;
  transport: BealeAppServerSessionTransport;
}

export interface BealeAppServerSessionStopResult {
  controlVersion: typeof BEALE_APP_SERVER_CONTROL_VERSION;
  stopped: boolean;
  sessionId: string;
}

export type BealeAppServerSessionControlRequest =
  | { type: "steer"; instruction: string }
  | { type: "pause" }
  | { type: "resume" }
  | { type: "stop" };

export interface BealeAppServerSessionControlResult {
  controlVersion: typeof BEALE_APP_SERVER_CONTROL_VERSION;
  accepted: true;
  sessionId: string;
  requestId: string;
  type: BealeAppServerSessionControlRequest["type"];
}

export function decodeBealeAppServerSessionControlRequest(
  value: unknown,
): BealeAppServerSessionControlRequest {
  if (!isRecord(value)) throw new Error("Session control body must be a JSON object.");
  if (value.type === "steer") {
    requiredBoundedString(value, "instruction", 131_072);
    return { type: "steer", instruction: value.instruction as string };
  }
  if (value.type === "pause" || value.type === "resume" || value.type === "stop") {
    return { type: value.type };
  }
  throw new Error("Session control type must be steer, pause, resume, or stop.");
}

export interface BealeAppServerShutdownResult {
  controlVersion: typeof BEALE_APP_SERVER_CONTROL_VERSION;
  shuttingDown: true;
}

export interface BealeAppServerErrorResponse {
  controlVersion: typeof BEALE_APP_SERVER_CONTROL_VERSION;
  error: AppServerProtocolErrorDetail;
}

export interface BealeAppServerWorkspaceSummary {
  id: string;
  workspaceId: string;
  name: string;
  researchProfileId: string;
  researchKitId: string;
  runCount: number;
  lastRunAt: string | null;
  updatedAt: string;
}

export interface BealeAppServerWorkspaceList {
  controlVersion: typeof BEALE_APP_SERVER_CONTROL_VERSION;
  workspaces: BealeAppServerWorkspaceSummary[];
}

/** Quick Chats belong to the app server's internal workspace, not a client workspace. */
export interface BealeAppServerQuickChatSummary {
  sessionId: string;
  workspaceId: string;
  title: string;
  promptMarkdown: string;
  status: string;
  updatedAt: string;
}

export interface BealeAppServerQuickChatList {
  schemaVersion: 1;
  chats: BealeAppServerQuickChatSummary[];
}

export interface BealeAppServerQuickChatStartInput {
  promptMarkdown: string;
  provider?: AppServerSessionLaunchProvider;
}

export interface BealeAppServerQuickChatStartResult {
  schemaVersion: 1;
  sessionId: string;
  workspaceId: string;
}

export function decodeBealeAppServerQuickChatStartInput(value: unknown): BealeAppServerQuickChatStartInput {
  if (!isRecord(value)) throw new Error("Quick Chat request must be an object.");
  requiredBoundedString(value, "promptMarkdown", 131_072);
  if (value.provider !== undefined) {
    const provider = requiredRecord(value, "provider");
    optionalBoundedString(provider, "id", 128);
    optionalBoundedString(provider, "model", 512);
    optionalBoundedString(provider, "reasoningEffort", 64);
    if (provider.fastMode !== undefined && typeof provider.fastMode !== "boolean") throw new Error("Invalid Quick Chat fast mode.");
    if (provider.daybreakBlue !== undefined && typeof provider.daybreakBlue !== "boolean") throw new Error("Invalid Quick Chat Daybreak Blue mode.");
  }
  return value as unknown as BealeAppServerQuickChatStartInput;
}

export function decodeBealeAppServerQuickChatList(value: unknown): BealeAppServerQuickChatList {
  if (!isRecord(value) || value.schemaVersion !== 1 || !Array.isArray(value.chats)
    || !value.chats.every((chat) => isRecord(chat) && typeof chat.sessionId === "string"
      && typeof chat.workspaceId === "string" && typeof chat.title === "string"
      && typeof chat.promptMarkdown === "string" && typeof chat.status === "string"
      && typeof chat.updatedAt === "string")) throw new Error("The app-server returned an invalid Quick Chat list.");
  return value as unknown as BealeAppServerQuickChatList;
}

export function decodeBealeAppServerQuickChatStartResult(value: unknown): BealeAppServerQuickChatStartResult {
  if (!isRecord(value) || value.schemaVersion !== 1 || typeof value.sessionId !== "string"
    || typeof value.workspaceId !== "string") throw new Error("The app-server returned an invalid Quick Chat start result.");
  return value as unknown as BealeAppServerQuickChatStartResult;
}

/** Read-only workspace context shared by Desktop and mobile clients. Host paths and credentials are excluded. */
export interface BealeAppServerWorkspaceDetails {
  schemaVersion: 1;
  workspace: BealeAppServerWorkspaceSummary;
  scope: {
    id: string;
    name: string;
    owner: string;
    descriptionMarkdown: string;
    expiresAt: string | null;
    resources: Array<{
      id: string;
      kind: string;
      direction: "in_scope" | "out_of_scope";
      value: string | null;
      sensitivity: string;
    }>;
  } | null;
  researchSubject: string;
  rules: Array<{ id: string; text: string }>;
  memory: BealeWorkspaceMemoryCatalog;
  runbooks: Array<{ id: string; title: string; purpose: string; updatedAt: string }>;
  reports: Array<{ id: string; title: string; summary: string; status: "complete" | "stale"; updatedAt: string }>;
  targets: Array<{ id: string; name: string; ready: boolean }>;
  surfaceMap: BealeWorkspaceSurfaceMap;
}

export interface BealeWorkspaceSurfaceMap {
  schemaVersion: 1;
  workspaceId: string;
  /** Unique surfaces created, focused, or updated by the requested session. */
  sessionSurfaceIds?: string[];
  surfaces: Array<{
    id: string; resourceId: string; resourceKind?: string; resourceSource?: string;
    resourceLocator?: string; resourceDirection?: string; scopeAssetId?: string | null;
    parentId: string | null; name: string; description: string;
    firstTouchedAt: string | null; firstTouchedBy?: string | null; firstTouchedModel?: string | null; firstTouchedProvider?: string | null;
    updatedAt?: string; foundation: {
      discovery?: string;
      ranking?: Record<'impact' | 'reachability' | 'relevance' | 'exposure' | 'precedence', { score: number; basis: string }> | null;
      threatModelReviewed?: boolean;
      techStack?: Array<{ technology: string; role: string; version: string; environment: string; evidence: string; confidence: 'observed' | 'inferred' | 'unknown' }>;
      assets?: Array<{ name: string; status: 'untouched' | 'in_progress' | 'complete' }>;
      bugHistory?: { entries?: Array<{ identifier: string; impact: string; affectedVersions: string; reference: string; notes: string }>; summary: string; limits: string };
      threatModel?: {
        diagram?: string; securityObjectives?: string[] | string; risks?: string[]; assumptions?: string[] | string;
        /** Legacy freeform notes retained for compatibility. */
        riskImpact?: string;
        invariants?: string; sinks?: string; mitigations?: string;
        attackerControl?: string; trustBoundaries?: string; assetsAtRisk?: string; plausibleImpacts?: string;
      };
      coverage: { summary?: string; gaps?: string; opportunities?: string; mappingComplete: boolean; sinksInvestigated: boolean; openQuestions: string[]; unexploredConnections: string[] };
    } | null;
    progressBlocker: { reason: string; actorId: string; setAt: string } | null;
    coverageState: { covered: boolean; unmetRequirements: string[]; openLeadIds?: string[]; unblockedFindingIds: string[] };
    history?: Array<{
      kind: 'created' | 'first_touch' | 'focused' | 'updated' | 'blocked' | 'unblocked' | 'snapshot_update';
      at: string; actorId: string | null; detail?: string | null;
    }>;
  }>;
  agents: Array<{ sessionId: string; sessionTitle: string; agentId: string; surfaceId: string }>;
}

export interface BealeWorkspaceSurfaceMapRequest { workspaceId: string; sessionId?: string }

export function decodeBealeWorkspaceSurfaceMapRequest(value: unknown): BealeWorkspaceSurfaceMapRequest {
  if (!isRecord(value) || typeof value.workspaceId !== 'string' || !value.workspaceId.trim()
    || (value.sessionId !== undefined && (typeof value.sessionId !== 'string' || !value.sessionId.trim() || value.sessionId.length > 200))) {
    throw new Error('Invalid workspace surface map request.');
  }
  return value as unknown as BealeWorkspaceSurfaceMapRequest;
}

export interface BealeWorkspaceResearchTrack {
  id: string;
  workspaceId: string;
  status: "running" | "paused" | "waiting" | "complete" | "stopped";
  pass: number;
  scopeId: string;
  provider: { id: string; model?: string; reasoningEffort?: string };
  guardianEnabled?: boolean;
  targetIds: string[];
  collaboration?: Record<string, unknown>;
  activeSessionId: string | null;
  activeSessionPhase?: "selecting" | "preparing" | "researching" | "switching" | null;
  activeSurfaceId: string | null;
  epochCount: number;
  lastError: string | null;
  createdAt: string;
  updatedAt: string;
  surfaces: Array<{
    surfaceId: string;
    name: string;
    status: "unexamined" | "active" | "deferred" | "blocked" | "complete" | "excluded";
    impact: "unknown" | "low" | "medium" | "high" | "critical";
    theory: string;
    nextAction: string;
    evidenceRefs: string[];
    angles: Array<{
      name: string;
      status: "open" | "supported" | "ruled_out" | "blocked";
      vector?: {
        attackerControl: string;
        trustBoundary: string;
        failureMode: string;
        unauthorizedEffect: string;
      };
      proofQuestion: string;
      evidenceRefs: string[];
      nextAction: string;
    }>;
    actionabilityScore: number;
    /** Deterministic base scheduler score; resource breadth and rotation are applied during selection. */
    priority?: {
      baseScore: number;
      actionability: number;
      impactAdjustment: number;
      surfaceRankingAdjustment?: number;
      testableAngleBonus: number;
      targetValidationPenalty: number;
      noProgressPenalty: number;
    };
    validation: { targetDependent: boolean; environment: "device" | "vm" | "web" | "localhost" | "none"; reason: string } | null;
    epochs: number;
    noProgressEpochs: number;
    revisits: number;
    revisitAfter: string | null;
    lastWorkedAt: string | null;
    updatedAt: string;
  }>;
}

export interface BealeWorkspaceResearchTrackRequest {
  workspaceId: string;
  action: "status" | "start" | "pause" | "resume" | "stop";
  configuration?: {
    provider: { id: string; model: string; reasoningEffort: string };
    guardianEnabled?: boolean;
    targetIds: string[];
    collaboration?: Record<string, unknown>;
  };
}

export function decodeBealeWorkspaceResearchTrackRequest(value: unknown): BealeWorkspaceResearchTrackRequest {
  if (!isRecord(value) || typeof value.workspaceId !== "string" || !value.workspaceId.trim()
    || value.workspaceId.length > 200 || value.action !== "status" && value.action !== "start"
    && value.action !== "pause" && value.action !== "resume" && value.action !== "stop") {
    throw new Error("Invalid workspace research track request.");
  }
  const configuration = value.configuration;
  if (configuration !== undefined) {
    if (value.action !== "start" || !isRecord(configuration)) {
      throw new Error("Invalid workspace research track configuration: only start accepts settings.");
    }
    const provider = configuration.provider;
    if (!isRecord(provider) || !nonEmptyString(provider.id) || provider.id.length > 100
      || !nonEmptyString(provider.model) || provider.model.length > 200) {
      throw new Error("Invalid workspace research track configuration: choose a lead provider and model.");
    }
    if (!nonEmptyString(provider.reasoningEffort) || provider.reasoningEffort.length > 40) {
      throw new Error("Invalid workspace research track configuration: choose a reasoning effort.");
    }
    const targetIds = configuration.targetIds;
    if (!Array.isArray(targetIds) || targetIds.length < 1 || targetIds.length > 16
      || !targetIds.every((id: unknown) => typeof id === "string" && id.length > 0 && id.length <= 256)
      || new Set(targetIds).size !== targetIds.length) {
      throw new Error("Invalid workspace research track configuration: choose 1 to 16 distinct targets.");
    }
    if (configuration.collaboration !== undefined && (!isRecord(configuration.collaboration)
      || JSON.stringify(configuration.collaboration).length > 32_000)) {
      throw new Error("Invalid workspace research track configuration: collaborator settings are malformed or too large.");
    }
    if (configuration.guardianEnabled !== undefined && typeof configuration.guardianEnabled !== "boolean") {
      throw new Error("Invalid workspace research track configuration: Guardian must be enabled or disabled.");
    }
  }
  return { workspaceId: value.workspaceId.trim(), action: value.action,
    ...(configuration === undefined ? {} : { configuration: configuration as NonNullable<BealeWorkspaceResearchTrackRequest["configuration"]> }) };
}

export function decodeBealeWorkspaceResearchTrack(value: unknown): BealeWorkspaceResearchTrack | null {
  if (value === null) return null;
  const statuses = ["running", "paused", "waiting", "complete", "stopped"];
  const workStatuses = ["unexamined", "active", "deferred", "blocked", "complete", "excluded"];
  const impacts = ["unknown", "low", "medium", "high", "critical"];
  if (!isRecord(value) || typeof value.id !== "string" || typeof value.workspaceId !== "string"
    || !statuses.includes(String(value.status)) || !Number.isSafeInteger(value.pass)
    || typeof value.scopeId !== "string" || !isRecord(value.provider)
    || typeof value.provider.id !== "string" || !Array.isArray(value.targetIds)
    || (value.guardianEnabled !== undefined && typeof value.guardianEnabled !== "boolean")
    || (value.collaboration !== undefined && !isRecord(value.collaboration))
    || !value.targetIds.every((id: unknown) => typeof id === "string")
    || (value.activeSessionId !== null && typeof value.activeSessionId !== "string")
    || (value.activeSessionPhase !== undefined && value.activeSessionPhase !== null
      && !["selecting", "preparing", "researching", "switching"].includes(String(value.activeSessionPhase)))
    || (value.activeSurfaceId !== null && typeof value.activeSurfaceId !== "string")
    || !Number.isSafeInteger(value.epochCount)
    || (value.lastError !== null && typeof value.lastError !== "string")
    || typeof value.createdAt !== "string" || typeof value.updatedAt !== "string"
    || !Array.isArray(value.surfaces) || !value.surfaces.every((work) => isRecord(work)
      && typeof work.surfaceId === "string" && typeof work.name === "string" && workStatuses.includes(String(work.status))
      && impacts.includes(String(work.impact)) && typeof work.theory === "string"
      && typeof work.nextAction === "string" && Array.isArray(work.evidenceRefs)
      && work.evidenceRefs.every((ref: unknown) => typeof ref === "string")
      && Array.isArray(work.angles) && work.angles.length <= 24
      && work.angles.every((angle: unknown) => isRecord(angle)
        && typeof angle.name === "string" && angle.name.trim().length > 0 && angle.name.length <= 160
        && ["open", "supported", "ruled_out", "blocked"].includes(String(angle.status))
        && (angle.vector === undefined || (isRecord(angle.vector)
          && typeof angle.vector.attackerControl === "string" && angle.vector.attackerControl.trim().length > 0 && angle.vector.attackerControl.length <= 500
          && typeof angle.vector.trustBoundary === "string" && angle.vector.trustBoundary.trim().length > 0 && angle.vector.trustBoundary.length <= 500
          && typeof angle.vector.failureMode === "string" && angle.vector.failureMode.trim().length > 0 && angle.vector.failureMode.length <= 500
          && typeof angle.vector.unauthorizedEffect === "string" && angle.vector.unauthorizedEffect.trim().length > 0 && angle.vector.unauthorizedEffect.length <= 500))
        && typeof angle.proofQuestion === "string" && angle.proofQuestion.trim().length > 0 && angle.proofQuestion.length <= 500
        && Array.isArray(angle.evidenceRefs) && angle.evidenceRefs.length <= 8
        && angle.evidenceRefs.every((ref: unknown) => typeof ref === "string" && ref.length <= 168)
        && (!["supported", "ruled_out"].includes(String(angle.status)) || angle.evidenceRefs.length > 0)
        && typeof angle.nextAction === "string" && angle.nextAction.trim().length > 0 && angle.nextAction.length <= 500)
      && Number.isSafeInteger(work.actionabilityScore) && Number(work.actionabilityScore) >= 0
      && Number(work.actionabilityScore) <= 100
      && (work.priority === undefined || validResearchTrackPriority(work.priority, Number(work.actionabilityScore)))
      && (work.validation === null || isRecord(work.validation)
        && typeof work.validation.targetDependent === "boolean"
        && ["device", "vm", "web", "localhost", "none"].includes(String(work.validation.environment))
        && typeof work.validation.reason === "string" && work.validation.reason.trim().length > 0
        && work.validation.reason.length <= 1_000)
      && Number.isSafeInteger(work.epochs) && Number.isSafeInteger(work.noProgressEpochs)
      && Number.isSafeInteger(work.revisits)
      && (work.revisitAfter === null || typeof work.revisitAfter === "string")
      && (work.lastWorkedAt === null || typeof work.lastWorkedAt === "string")
      && typeof work.updatedAt === "string")) {
    throw new Error("The app-server returned an invalid workspace research track.");
  }
  return value as unknown as BealeWorkspaceResearchTrack;
}

function validResearchTrackPriority(value: unknown, actionability: number): boolean {
  if (!isRecord(value)) return false;
  const keys = ["baseScore", "actionability", "impactAdjustment", "testableAngleBonus",
    "targetValidationPenalty", "noProgressPenalty"] as const;
  if (!keys.every((key) => typeof value[key] === "number" && Number.isSafeInteger(value[key]))) return false;
  if (value.surfaceRankingAdjustment !== undefined && !Number.isSafeInteger(value.surfaceRankingAdjustment)) return false;
  return value.actionability === actionability
    && Number(value.targetValidationPenalty) >= 0 && Number(value.noProgressPenalty) >= 0
    && value.baseScore === Number(value.actionability) + Number(value.impactAdjustment)
      + Number(value.surfaceRankingAdjustment ?? 0) + Number(value.testableAngleBonus)
      - Number(value.targetValidationPenalty) - Number(value.noProgressPenalty);
}

export function decodeBealeWorkspaceSurfaceMap(value: unknown): BealeWorkspaceSurfaceMap {
  const strings = (items: unknown): items is string[] => Array.isArray(items) && items.every((item) => typeof item === 'string');
  const validFoundation = (foundation: unknown): boolean => {
    if (foundation === null) return true;
    if (!isRecord(foundation) || !isRecord(foundation.coverage)) return false;
    const coverage = foundation.coverage;
    const threatModel = foundation.threatModel;
    const techStack = foundation.techStack;
    const assets = foundation.assets;
    const bugHistory = foundation.bugHistory;
    const ranking = foundation.ranking;
    const rankingFactors = ['impact', 'reachability', 'relevance', 'exposure', 'precedence'];
    return typeof coverage.mappingComplete === 'boolean'
      && typeof coverage.sinksInvestigated === 'boolean'
      && strings(coverage.openQuestions) && strings(coverage.unexploredConnections)
      && ['summary', 'gaps', 'opportunities'].every((key) => coverage[key] === undefined || typeof coverage[key] === 'string')
      && (foundation.discovery === undefined || typeof foundation.discovery === 'string')
      && (ranking === undefined || ranking === null || isRecord(ranking)
        && Object.keys(ranking).length === rankingFactors.length
        && rankingFactors.every((factor) => isRecord(ranking[factor])
          && Number.isSafeInteger(ranking[factor].score) && Number(ranking[factor].score) >= 1
          && Number(ranking[factor].score) <= 10
          && typeof ranking[factor].basis === 'string'
          && String(ranking[factor].basis).trim().length > 0 && String(ranking[factor].basis).length <= 500))
      && (foundation.threatModelReviewed === undefined || typeof foundation.threatModelReviewed === 'boolean')
      && (techStack === undefined || Array.isArray(techStack) && techStack.every((entry) => isRecord(entry)
        && ['technology', 'role', 'version', 'environment', 'evidence'].every((key) => typeof entry[key] === 'string')
        && ['observed', 'inferred', 'unknown'].includes(String(entry.confidence))))
      && (assets === undefined || Array.isArray(assets) && assets.every((entry) => isRecord(entry)
        && typeof entry.name === 'string' && ['untouched', 'in_progress', 'complete'].includes(String(entry.status))))
      && (bugHistory === undefined || isRecord(bugHistory)
        && typeof bugHistory.summary === 'string' && typeof bugHistory.limits === 'string'
        && (bugHistory.entries === undefined || Array.isArray(bugHistory.entries) && bugHistory.entries.every((entry) => isRecord(entry)
          && ['identifier', 'impact', 'affectedVersions', 'reference', 'notes'].every((key) => typeof entry[key] === 'string'))))
      && (threatModel === undefined || isRecord(threatModel)
        && (threatModel.securityObjectives === undefined || typeof threatModel.securityObjectives === 'string' || strings(threatModel.securityObjectives))
        && (threatModel.risks === undefined || strings(threatModel.risks))
        && (threatModel.assumptions === undefined || typeof threatModel.assumptions === 'string' || strings(threatModel.assumptions))
        && ['diagram', 'riskImpact', 'invariants', 'sinks', 'mitigations',
          'attackerControl', 'trustBoundaries', 'assetsAtRisk', 'plausibleImpacts']
          .every((key) => threatModel[key] === undefined || typeof threatModel[key] === 'string'));
  };
  if (!isRecord(value) || value.schemaVersion !== 1 || typeof value.workspaceId !== 'string'
    || !Array.isArray(value.surfaces) || !value.surfaces.every((surface) => isRecord(surface)
      && typeof surface.id === 'string' && typeof surface.resourceId === 'string'
      && (surface.resourceKind === undefined || typeof surface.resourceKind === 'string')
      && (surface.resourceSource === undefined || typeof surface.resourceSource === 'string')
      && (surface.resourceLocator === undefined || typeof surface.resourceLocator === 'string')
      && (surface.resourceDirection === undefined || typeof surface.resourceDirection === 'string')
      && (surface.scopeAssetId === undefined || surface.scopeAssetId === null || typeof surface.scopeAssetId === 'string')
      && (surface.parentId === null || typeof surface.parentId === 'string')
      && typeof surface.name === 'string' && typeof surface.description === 'string'
      && (surface.firstTouchedAt === null || typeof surface.firstTouchedAt === 'string')
      && (surface.firstTouchedBy === undefined || surface.firstTouchedBy === null || typeof surface.firstTouchedBy === 'string')
      && (surface.firstTouchedModel === undefined || surface.firstTouchedModel === null || typeof surface.firstTouchedModel === 'string')
      && (surface.firstTouchedProvider === undefined || surface.firstTouchedProvider === null || typeof surface.firstTouchedProvider === 'string')
      && (surface.updatedAt === undefined || typeof surface.updatedAt === 'string')
      && validFoundation(surface.foundation)
      && (surface.progressBlocker === null || isRecord(surface.progressBlocker)
        && typeof surface.progressBlocker.reason === 'string' && typeof surface.progressBlocker.actorId === 'string'
        && typeof surface.progressBlocker.setAt === 'string')
      && isRecord(surface.coverageState) && typeof surface.coverageState.covered === 'boolean'
      && strings(surface.coverageState.unmetRequirements)
      && (surface.coverageState.openLeadIds === undefined || strings(surface.coverageState.openLeadIds))
      && strings(surface.coverageState.unblockedFindingIds)
      && (surface.history === undefined || Array.isArray(surface.history) && surface.history.every((event) =>
        isRecord(event) && ['created', 'first_touch', 'focused', 'updated', 'blocked', 'unblocked', 'snapshot_update'].includes(String(event.kind))
        && typeof event.at === 'string' && (event.actorId === null || typeof event.actorId === 'string')
        && (event.detail === undefined || event.detail === null || typeof event.detail === 'string'))))
    || (value.sessionSurfaceIds !== undefined && !strings(value.sessionSurfaceIds))
    || !Array.isArray(value.agents) || !value.agents.every((event) => isRecord(event)
      && typeof event.sessionId === 'string' && typeof event.sessionTitle === 'string'
      && typeof event.agentId === 'string' && typeof event.surfaceId === 'string')) {
    throw new Error('The app-server returned an invalid workspace surface map.');
  }
  return value as unknown as BealeWorkspaceSurfaceMap;
}

export function decodeBealeAppServerWorkspaceDetails(value: unknown): BealeAppServerWorkspaceDetails {
  if (!isRecord(value) || value.schemaVersion !== 1 || !isRecord(value.workspace)
    || typeof value.workspace.id !== "string" || typeof value.workspace.workspaceId !== "string"
    || typeof value.workspace.name !== "string" || typeof value.workspace.researchProfileId !== "string"
    || typeof value.workspace.researchKitId !== "string" || typeof value.workspace.runCount !== "number"
    || !(value.scope === null || isRecord(value.scope) && typeof value.scope.id === "string"
      && typeof value.scope.name === "string" && typeof value.scope.owner === "string"
      && typeof value.scope.descriptionMarkdown === "string" && Array.isArray(value.scope.resources)
      && value.scope.resources.every((asset) => isRecord(asset) && typeof asset.id === "string"
        && typeof asset.kind === "string" && (asset.direction === "in_scope" || asset.direction === "out_of_scope")
        && (asset.value === null || typeof asset.value === "string") && typeof asset.sensitivity === "string"))
    || typeof value.researchSubject !== "string" || !Array.isArray(value.rules)
    || !value.rules.every((rule) => isRecord(rule) && typeof rule.id === "string" && typeof rule.text === "string")
    || !isRecord(value.memory) || value.memory.schemaVersion !== 4
    || value.memory.workspaceId !== value.workspace.workspaceId
    || !Array.isArray(value.memory.nodes) || !Array.isArray(value.memory.leads) || !Array.isArray(value.memory.findings)
    || !Array.isArray(value.runbooks)
    || !value.runbooks.every((runbook) => isRecord(runbook) && typeof runbook.id === "string"
      && typeof runbook.title === "string" && typeof runbook.purpose === "string" && typeof runbook.updatedAt === "string")
    || !Array.isArray(value.reports)
    || !value.reports.every((report) => isRecord(report) && typeof report.id === "string"
      && typeof report.title === "string" && typeof report.summary === "string"
      && (report.status === "complete" || report.status === "stale") && typeof report.updatedAt === "string")
    || !isRecord(value.surfaceMap) || decodeBealeWorkspaceSurfaceMap(value.surfaceMap).workspaceId !== value.workspace.workspaceId
    || !Array.isArray(value.targets)
    || !value.targets.every((target) => isRecord(target) && typeof target.id === "string"
      && typeof target.name === "string" && typeof target.ready === "boolean")) {
    throw new Error("The app-server returned invalid workspace details.");
  }
  return value as unknown as BealeAppServerWorkspaceDetails;
}

export type BealeAppServerWorkspaceDocument =
  | { schemaVersion: 1; kind: "runbook"; resourceId: string; document: KnowledgeRunbookDocument }
  | { schemaVersion: 1; kind: "report"; resourceId: string; document: KnowledgeReportDocument };

function validWorkspaceRunbookCell(value: unknown): boolean {
  if (!isRecord(value) || typeof value.id !== "string"
    || !["markdown", "code", "raw"].includes(String(value.type))
    || typeof value.source !== "string" || !Array.isArray(value.features)
    || !value.features.every((feature) => typeof feature === "string")
    || typeof value.active !== "boolean"
    || !(value.language === null || typeof value.language === "string")
    || !(value.executionCount === null || typeof value.executionCount === "number")
    || !(value.latestRun === null || isRecord(value.latestRun))
    || !Array.isArray(value.outputs)
    || !value.outputs.every((output) => isRecord(output)
      && ["stream", "display", "error"].includes(String(output.kind))
      && typeof output.text === "string"
      && (output.streamName === null || output.streamName === "stdout" || output.streamName === "stderr")
      && (output.mimeType === null || typeof output.mimeType === "string"))
    || !isRecord(value.executor)) return false;
  const executor = value.executor;
  return executor.kind === "host" && typeof executor.timeoutSeconds === "number"
    || executor.kind === "tart-vm" && executor.workspacePath === null
      && typeof executor.vmName === "string" && typeof executor.timeoutSeconds === "number"
      && Array.isArray(executor.argv) && executor.argv.every((argument) => typeof argument === "string");
}

export function decodeBealeAppServerWorkspaceDocument(value: unknown): BealeAppServerWorkspaceDocument {
  if (!isRecord(value) || value.schemaVersion !== 1 || typeof value.resourceId !== "string"
    || !isRecord(value.document)) throw new Error("The app-server returned an invalid workspace document.");
  if (value.kind === "report" && value.document.reportId === value.resourceId
    && typeof value.document.content === "string") return value as BealeAppServerWorkspaceDocument;
  if (value.kind === "runbook" && value.document.runbookId === value.resourceId
    && value.document.nbformat === 4 && Array.isArray(value.document.cells)
    && typeof value.document.nbformatMinor === "number"
    && (value.document.language === null || typeof value.document.language === "string")
    && (value.document.revision === null || typeof value.document.revision === "number")
    && Array.isArray(value.document.enabledFeatures)
    && value.document.enabledFeatures.every((feature) => typeof feature === "string")
    && (value.document.latestRun === null || isRecord(value.document.latestRun))
    && value.document.cells.every(validWorkspaceRunbookCell)) {
    return value as BealeAppServerWorkspaceDocument;
  }
  throw new Error("The app-server returned an invalid workspace document.");
}

export interface BealeAppServerCanonicalResult<T = unknown> {
  controlVersion: typeof BEALE_APP_SERVER_CONTROL_VERSION;
  workspace: BealeAppServerWorkspaceSummary;
  result: T;
}

export type BealeMemorySessionHeat = "low" | "medium" | "high" | "critical";

export interface BealeMemoryNotificationNode {
  id: string;
  kind: "memory" | "claim";
  sessionIds: string[];
  type: string;
  typeName: string;
  title: string;
  summary: string;
  status: string;
  heat: BealeMemorySessionHeat;
  rating: ResearchClaimRating | null;
  createdAt: string;
  updatedAt: string;
  revision: number;
}

export interface BealeMemoryNotificationFeed {
  schemaVersion: 3;
  workspaceId: string;
  profile: {
    id: string;
    version: string;
    hash: string;
  };
  nodes: BealeMemoryNotificationNode[];
}

export interface BealeWorkspaceMemoryNode {
  id: string;
  surfaceId?: string | null;
  sessionIds: string[];
  type: string;
  title: string;
  summary: string;
  status: string;
  confidence: number;
  tags: string[];
  createdAt: string;
  updatedAt: string;
  revision: number;
  duplicateMemories: BealeWorkspaceMemoryDuplicate[];
}

export interface BealeWorkspaceMemoryDuplicate {
  id: string;
  type: string;
  title: string;
  status: string;
  revision: number;
  markedAt: string;
}

export interface BealeWorkspaceMemoryCatalog {
  schemaVersion: 4;
  workspaceId: string;
  status: string;
  nodeCount: number;
  nodeTypeCounts: Record<string, number>;
  nodes: BealeWorkspaceMemoryNode[];
  leads: BealeWorkspaceResearchClaim[];
  findings: BealeWorkspaceResearchClaim[];
}

export interface BealeWorkspaceResearchClaim {
  id: string;
  surfaceId?: string | null;
  sessionIds: string[];
  projection: "lead" | "finding";
  maturity: string;
  status: string;
  staleFromStatus: string | null;
  rating: ResearchClaimRating;
  classification: string;
  duplicateClaims: BealeWorkspaceResearchClaimDuplicate[];
  title: string;
  summary: string;
  impact: string;
  stepsToReproduce: string;
  securityTracking: BealeWorkspaceClaimSecurityTracking | null;
  confidence: number;
  evidenceCount: number;
  createdAt: string;
  updatedAt: string;
  revision: number;
}

export interface BealeWorkspaceClaimSecurityTracking {
  reachability: {
    state: "not_assessed" | "unreachable" | "conditional" | "reachable";
    conditions: string;
    assessedAt: string | null;
  };
  cvssAssessments: Array<{
    version: "4.0" | "3.1";
    vector: string;
    score: number;
    nomenclature: "CVSS-B" | "CVSS-BT" | "CVSS-BE" | "CVSS-BTE" | "CVSS:3.1";
    assessedAt: string;
  }>;
  affectedAssetIds: string[];
  surfaceIds?: string[];
  progressBlocker?: { reason: string; setAt: string } | null;
  externalReferences: Array<{ kind: string; identifier: string; url: string | null }>;
}

export const APP_SERVER_PROTOCOL_WEBSOCKET_CAPABILITIES = [
  "session.events",
  "session.controls",
  "session.event-identity.v1",
] as const;
export const APP_SERVER_PROTOCOL_MAX_REQUEST_ID_LENGTH = 200 as const;
export const APP_SERVER_PROTOCOL_CAPABILITIES = [
  "knowledge.findings",
  "knowledge.claims.v2",
  "knowledge.claim_deduplication",
  "knowledge.claim_board_transition",
  "knowledge.history_deduplication",
  "workspace.state",
  "registry.state",
  "registry.workspace_sync.v2",
  "knowledge.claim_security_tracking",
  "knowledge.finding_staleness",
  "knowledge.campaign_graph",
  "knowledge.evidence_gates",
  "session.append_only",
  "session.controls",
  "session.event_identity",
  "session.bounded_reads",
  "session.targeted_details",
  "workspace.goal-suggestions.v1",
  "workspace.prompt-expansion.v1",
  "knowledge.report-content-revise.v1",
  "knowledge.report-triage-status.v1",
  "knowledge.report-packet-replace.v1",
  "knowledge.report-recording-replace.v1",
  "knowledge.report-list.v1",
] as const;

/** @deprecated Import the protocol-named constants from `@beale/app-server-runtime/protocol`. */
export const APP_SERVER_TRANSPORT_PROTOCOL_VERSION = APP_SERVER_PROTOCOL_VERSION;
/** @deprecated Import the protocol-named constants from `@beale/app-server-runtime/protocol`. */
export const APP_SERVER_TRANSPORT_PREFIX = APP_SERVER_PROTOCOL_BOOTSTRAP_PREFIX;
/** @deprecated Import the protocol-named constants from `@beale/app-server-runtime/protocol`. */
export const APP_SERVER_TRANSPORT_PATH = APP_SERVER_PROTOCOL_WEBSOCKET_PATH;

export const APP_SERVER_PROTOCOL_OPERATIONS = [
  "fleet.state", "fleet.browser_home_page", "fleet.configure", "fleet.test_ssh", "fleet.test_app_server", "fleet.clone", "fleet.clone_for_session", "fleet.start", "fleet.stop",
  "fleet.prepare", "fleet.connect", "fleet.complete", "fleet.stage", "fleet.export", "fleet.reserve", "fleet.release",
  "fleet.broker", "fleet.relay_broker",
  "fleet.remote_machines", "fleet.app_server_statuses", "fleet.pairing_qr_code", "fleet.remote_catalog", "fleet.remote_workspace_details", "fleet.remote_research_track", "fleet.remote_workspace_document", "fleet.remote_quick_chats", "fleet.remote_quick_chat_start", "fleet.remote_quick_chat_continue", "fleet.remote_session", "fleet.remote_launch", "fleet.remote_prompt_expand", "fleet.remote_goal_suggestions", "fleet.remote_control", "fleet.remote_browser_attach", "fleet.relay_stage", "fleet.relay_export",
  "fleet.restart_app_server", "fleet.restart_guest_app_server",
  "resource.prior_art.list", "resource.prior_art.get",
  "protocol.describe", "session.create", "session.begin_attempt", "session.append_event", "session.append_event_receipt",
  "session.transition", "session.recover_interrupted", "session.import_capture", "session.get", "session.get_update", "session.events", "session.event_details",
  "session.collaboration", "session.captures", "session.capture", "session.list", "session.list_summaries",
  "memory.summary", "memory.notification_feed", "history.mark_duplicate", "history.undo_duplicate", "claim.mark_duplicate", "claim.undo_duplicate", "claim.board_transition", "workspace.state", "workspace.surface_map", "workspace.research_track", "registry.state",
  "runbook.get", "report.list", "report.get", "report.revise_content", "report.update_triage_status", "report.replace_packet", "report.replace_recording",
  "artifact.resolve", "provider.complete", "provider.describe", "model_job.resolve",
  "suggestion.generate", "suggestion.select", "suggestion.steering", "prompt.expand",
  "profile.resolve", "auth.list", "auth.status", "auth.verify", "auth.logout", "model.list",
  "tools.list", "tools.config", "config.show", "config.set",
  "research.tools.list", "research.tools.read", "research.tools.mutate",
  "source.inspect", "source.materialize", "plugin.list", "plugin.add_filesystem",
  "plugin.add_repository", "plugin.set_enabled", "plugin.remove", "plugin.runtime",
  "workspace.project",
] as const;

export type AppServerProtocolOperation = (typeof APP_SERVER_PROTOCOL_OPERATIONS)[number];

export type FleetBackend = "tart" | "hyper-v";
export type FleetRole = "primary" | "guest";
export type FleetMachineState = "running" | "stopped" | "unknown";

export interface FleetMachine {
  id: string;
  name: string;
  backend: FleetBackend;
  state: FleetMachineState;
  base: boolean;
  canRun: boolean;
  canClone: boolean;
  guestPlatform: 'win32' | 'darwin' | 'linux';
  privilege: "standard" | "elevated";
  sshConfigured: boolean;
  sshIdentityConfigured: boolean;
  sshKnownHostsConfigured: boolean;
  sshHost: string | null;
  sshUser: string | null;
  owner: { machineId: string; sessionId: string } | null;
}

export interface FleetAppServer {
  id: string;
  name: string;
  url: string;
}

export interface FleetAppServerStatus {
  id: string;
  state: 'online' | 'offline' | 'issue';
  message: string;
}

export interface FleetPairingQrCode {
  dataUrl: string;
}

export function decodeFleetPairingQrCode(value: unknown): FleetPairingQrCode {
  if (!isRecord(value) || typeof value.dataUrl !== 'string'
    || value.dataUrl.length > 256_000
    || !/^data:image\/png;base64,[A-Za-z0-9+/]+={0,2}$/u.test(value.dataUrl)) {
    throw new Error('Invalid Fleet pairing QR code.');
  }
  return { dataUrl: value.dataUrl };
}

export function decodeFleetAppServerStatuses(value: unknown): FleetAppServerStatus[] {
  if (!Array.isArray(value) || !value.every((entry) => isRecord(entry)
    && nonEmptyString(entry.id)
    && (entry.state === 'online' || entry.state === 'offline' || entry.state === 'issue')
    && nonEmptyString(entry.message))) {
    throw new Error('Invalid Fleet app-server statuses.');
  }
  return value.map((entry: Record<string, unknown>) => ({
    id: entry.id as string,
    state: entry.state as FleetAppServerStatus['state'],
    message: entry.message as string
  }));
}

export interface FleetBrowser {
  id: string;
  name: string;
  kind: 'embedded' | 'external';
  enabled: boolean;
}

export const DEFAULT_BROWSER_HOME_PAGE = 'https://duckduckgo.com/';

export interface FleetRemoteSessionSummary {
  id: string;
  workspaceId: string;
  title: string;
  status: string;
  prompt: string;
  updatedAt: string;
  automation: boolean;
}

export interface FleetRemoteCatalog {
  serverId: string;
  providerCatalog: BealeAppServerProviderCatalog;
  workspaces: Array<{ id: string; workspaceId: string; name: string; runCount: number }>;
  sessions: FleetRemoteSessionSummary[];
  machines: FleetMachine[];
  browsers: FleetBrowser[];
  workspaceTargets: Record<string, string[]>;
  requiredWorkspaceIds: string[];
  optionalWorkspaceIds: string[];
  hasBaseVm: boolean;
}

/** Client-neutral New Research input for a session owned by another app-server. */
export interface FleetRemoteResearchInput {
  promptMarkdown: string;
  targetIds: string[];
  mode?: string;
  goal?: { objective?: string };
  provider?: AppServerSessionLaunchProvider;
  shellSafetyMode?: string;
  guardianEnabled?: boolean;
  workflowId?: string;
  collaboration?: Record<string, unknown>;
  repeatSchedule?: AppServerRepeatSchedule;
}

export interface FleetRemotePromptExpansionInput {
  promptMarkdown: string;
  phase?: string;
  provider?: { id: string; model: string };
}

export interface FleetRemotePromptExpansionResult {
  phase: string;
  promptMarkdown: string;
}

export interface FleetRemoteGoalSuggestions {
  phase: string;
  suggestions: string[];
  cacheStatus?: "stale";
}

export function decodeFleetRemotePromptExpansionInput(value: unknown): FleetRemotePromptExpansionInput {
  if (!isRecord(value) || !nonEmptyString(value.promptMarkdown) || value.promptMarkdown.length > 12_000
    || (value.phase !== undefined && (typeof value.phase !== 'string' || value.phase.length > 128))
    || (value.provider !== undefined && (!isRecord(value.provider)
      || !nonEmptyString(value.provider.id) || !nonEmptyString(value.provider.model)
      || value.provider.id.length > 100 || value.provider.model.length > 200))) {
    throw new Error('Invalid remote research prompt expansion input.');
  }
  return value as unknown as FleetRemotePromptExpansionInput;
}

export type AppServerRepeatSchedule = { type: 'none' } | {
  type: 'minutely' | 'hourly' | 'daily' | 'weekly' | 'monthly';
  interval: number;
};

export function decodeFleetRemoteResearchInput(value: unknown, workspaceId: string): FleetRemoteResearchInput {
  if (!isRecord(value) || !nonEmptyString(value.promptMarkdown) || value.promptMarkdown.length > 131_072
    || !Array.isArray(value.targetIds) || value.targetIds.length < 1 || value.targetIds.length > 16
    || !value.targetIds.every((id: unknown) => typeof id === 'string' && id.length > 0 && id.length <= 256)
    || new Set(value.targetIds).size !== value.targetIds.length) {
    throw new Error('Invalid remote research input.');
  }
  const launch = { workspaceId, promptMarkdown: value.promptMarkdown,
    machineId: value.targetIds.length > 1 ? 'local' : value.targetIds[0], targetIds: value.targetIds,
    ...(value.mode === undefined ? {} : { mode: value.mode }),
    ...(value.goal === undefined ? {} : { goal: value.goal }),
    ...(value.provider === undefined ? {} : { provider: value.provider }),
    ...(value.shellSafetyMode === undefined ? {} : { shellSafetyMode: value.shellSafetyMode }),
    ...(value.guardianEnabled === undefined ? {} : { guardianEnabled: value.guardianEnabled }),
    ...(value.workflowId === undefined ? {} : { workflowId: value.workflowId }),
    ...(value.collaboration === undefined ? {} : { collaboration: value.collaboration }) };
  if (value.repeatSchedule !== undefined) {
    const schedule = value.repeatSchedule;
    if (!isRecord(schedule) || (schedule.type !== 'none' && schedule.type !== 'minutely'
      && schedule.type !== 'hourly' && schedule.type !== 'daily' && schedule.type !== 'weekly'
      && schedule.type !== 'monthly') || (schedule.type !== 'none'
        && (!Number.isInteger(schedule.interval) || (schedule.interval as number) < 1 || (schedule.interval as number) > 99))) {
      throw new Error('Invalid remote research repeat schedule.');
    }
  }
  decodeAppServerSessionLaunchRequest({ launchVersion: APP_SERVER_SESSION_LAUNCH_VERSION, launch });
  return value as unknown as FleetRemoteResearchInput;
}

export interface FleetSshTestInput {
  machineId: string;
  sshHost: string;
  sshUser: string;
  sshIdentityFile?: string;
  sshKnownHostsFile?: string;
}

export interface FleetSshTestResult {
  success: boolean;
  message: string;
}

export function decodeFleetSshTestInput(value: unknown): FleetSshTestInput {
  if (!isRecord(value) || typeof value.machineId !== 'string' || !value.machineId
    || typeof value.sshHost !== 'string' || typeof value.sshUser !== 'string'
    || (value.sshIdentityFile !== undefined && typeof value.sshIdentityFile !== 'string')
    || (value.sshKnownHostsFile !== undefined && typeof value.sshKnownHostsFile !== 'string')) {
    throw new Error('Invalid Fleet SSH test settings.');
  }
  return {
    machineId: value.machineId,
    sshHost: value.sshHost,
    sshUser: value.sshUser,
    ...(value.sshIdentityFile === undefined ? {} : { sshIdentityFile: value.sshIdentityFile }),
    ...(value.sshKnownHostsFile === undefined ? {} : { sshKnownHostsFile: value.sshKnownHostsFile }),
  };
}

export interface FleetState {
  machineId: string;
  role: FleetRole;
  enabled: boolean;
  available: boolean;
  error: string | null;
  machines: FleetMachine[];
  remoteMachines: FleetMachine[];
  appServers: FleetAppServer[];
  browsers: FleetBrowser[];
  browserHomePage: string;
  workspaceTargets: Record<string, string[]>;
  primary: { name: string; sshHost: string | null } | null;
  requiredWorkspaceIds: string[];
  optionalWorkspaceIds: string[];
  lastMachineByWorkspace: Record<string, string>;
}

export const CLAIM_BOARD_MATURITIES = ["refuted", "lead", "observed", "reproduced", "verified"] as const;
export type ClaimBoardMaturity = (typeof CLAIM_BOARD_MATURITIES)[number];

export interface ClaimBoardTransitionRequest {
  workspaceId: string;
  claimId: string;
  expectedRevision: number;
  targetMaturity: ClaimBoardMaturity;
}

export function decodeClaimBoardTransitionRequest(value: unknown): ClaimBoardTransitionRequest {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("Claim board transition input is required.");
  const input = value as Record<string, unknown>;
  if (typeof input.workspaceId !== "string" || !input.workspaceId.trim()) throw new Error("workspaceId is required.");
  if (typeof input.claimId !== "string" || !input.claimId.trim()) throw new Error("claimId is required.");
  if (!Number.isSafeInteger(input.expectedRevision) || Number(input.expectedRevision) < 1) throw new Error("expectedRevision must be a positive integer.");
  if (typeof input.targetMaturity !== "string" || !(CLAIM_BOARD_MATURITIES as readonly string[]).includes(input.targetMaturity)) {
    throw new Error("targetMaturity must name a Findings board state.");
  }
  return {
    workspaceId: input.workspaceId,
    claimId: input.claimId,
    expectedRevision: Number(input.expectedRevision),
    targetMaturity: input.targetMaturity as ClaimBoardMaturity,
  };
}

export type WorkspaceProjectRequest =
  | { workspaceId: string; action: 'status' | 'sync' | 'export' | 'rebuild-index' | 'release-index' }
  | { workspaceId: string; action: 'import'; path: string; expectedRevision: number };

export type { WorkspaceProject, WorkspaceProjectHealth, WorkspaceCheckpointResult } from '@beale/research-agent';

export function decodeWorkspaceProjectRequest(value: unknown): WorkspaceProjectRequest {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Workspace project input is required.');
  const input = value as Record<string, unknown>;
  if (typeof input.workspaceId !== 'string' || !input.workspaceId.trim()) throw new Error('workspaceId is required.');
  if (input.action === 'status' || input.action === 'sync' || input.action === 'export' || input.action === 'rebuild-index' || input.action === 'release-index') return { workspaceId: input.workspaceId, action: input.action };
  if (input.action === 'import' && typeof input.path === 'string' && input.path.length > 0 && input.path.length < 1024
    && Number.isSafeInteger(input.expectedRevision) && Number(input.expectedRevision) > 0) return { workspaceId: input.workspaceId, action: 'import', path: input.path, expectedRevision: Number(input.expectedRevision) };
  throw new Error('Expected status, sync, export, rebuild-index, release-index, or import with a path and positive expectedRevision.');
}

export interface AppServerProtocolErrorDetail {
  code: string;
  message: string;
  retryable: boolean;
}

interface AppServerProtocolEnvelopeBase {
  protocol: typeof APP_SERVER_PROTOCOL_NAME;
  protocolVersion: typeof APP_SERVER_PROTOCOL_VERSION;
  operation: AppServerProtocolOperation;
  requestId?: string;
}

export interface AppServerProtocolSuccess<T = unknown> extends AppServerProtocolEnvelopeBase {
  ok: true;
  result: T;
}

export interface AppServerProtocolFailure extends AppServerProtocolEnvelopeBase {
  ok: false;
  error: AppServerProtocolErrorDetail;
}

export type AppServerProtocolEnvelope<T = unknown> = AppServerProtocolSuccess<T> | AppServerProtocolFailure;

export interface AppServerProtocolDescriptor {
  protocol: typeof APP_SERVER_PROTOCOL_NAME;
  protocolVersion: typeof APP_SERVER_PROTOCOL_VERSION;
  operations: readonly AppServerProtocolOperation[];
  contractVersion: typeof APP_SERVER_CONTRACT_VERSION;
  runtime: {
    name: typeof APP_SERVER_PROTOCOL_NAME;
    version: typeof APP_SERVER_RUNTIME_VERSION;
    buildId: string;
    nodeVersion: string;
  };
  schemas: {
    protocol: 1;
    session: 1;
    memorySummary: 14;
    finding: 7;
    campaignGraph: 5;
    goalSuggestions: 1;
  };
  capabilities: typeof APP_SERVER_PROTOCOL_CAPABILITIES;
  transports: {
    appServer: {
      path: typeof BEALE_APP_SERVER_OPERATIONS_PATH;
      authentication: "operator-bearer";
      framing: "json";
      errors: "http-problem";
    };
    websocket: {
      path: typeof APP_SERVER_PROTOCOL_WEBSOCKET_PATH;
      authentication: "bearer";
      framing: "json-message";
      errors: "protocol-error-message";
      correlation: "request-id";
      capabilities: typeof APP_SERVER_PROTOCOL_WEBSOCKET_CAPABILITIES;
    };
  };
}

export interface AppServerTransportBootstrap {
  protocolVersion: typeof APP_SERVER_PROTOCOL_VERSION;
  transport: "websocket";
  url: string;
  sessionId: string;
}

export interface AppServerClientHello {
  protocolVersion: typeof APP_SERVER_PROTOCOL_VERSION;
  type: "client.hello";
  sessionId: string;
  client: { name: string; version: string };
}

export interface AppServerServerHello {
  protocolVersion: typeof APP_SERVER_PROTOCOL_VERSION;
  type: "server.hello";
  sessionId: string;
  server: { name: typeof APP_SERVER_PROTOCOL_NAME; version: string; buildId: string };
  contractVersion: typeof APP_SERVER_CONTRACT_VERSION;
  schemas: AppServerProtocolDescriptor["schemas"];
  capabilities: typeof APP_SERVER_PROTOCOL_WEBSOCKET_CAPABILITIES;
}

export interface AppServerSessionControl<TControl extends Record<string, unknown> = Record<string, unknown>> {
  protocolVersion: typeof APP_SERVER_PROTOCOL_VERSION;
  type: "session.control";
  sessionId: string;
  requestId: string;
  control: TControl & { requestId: string };
}

export interface AppServerSessionEvent<TEvent extends Record<string, unknown> = Record<string, unknown>> {
  protocolVersion: typeof APP_SERVER_PROTOCOL_VERSION;
  type: "session.event";
  sessionId: string;
  event: TEvent;
}

export interface AppServerWebSocketProtocolError {
  protocolVersion: typeof APP_SERVER_PROTOCOL_VERSION;
  type: "protocol.error";
  sessionId: string;
  requestId?: string;
  error: AppServerProtocolErrorDetail;
  /** Retained in protocol v1 for clients that consumed the original WebSocket error shape. */
  message: string;
}

/** @deprecated Use AppServerWebSocketProtocolError for the complete current DTO. */
export interface AppServerProtocolError {
  protocolVersion: typeof APP_SERVER_PROTOCOL_VERSION;
  type: "protocol.error";
  sessionId: string;
  message: string;
}

export type AppServerClientMessage = AppServerClientHello | AppServerSessionControl;
export type AppServerServerMessage = AppServerServerHello | AppServerSessionEvent | AppServerWebSocketProtocolError;

/**
 * Versioned, client-independent launch contract consumed by the Beale
 * app-server. Clients describe session intent; the app-server owns the
 * app-server hosted-runtime argument and worker-environment mapping.
 */
export const APP_SERVER_SESSION_LAUNCH_VERSION = 3 as const;

export const APP_SERVER_PROVIDER_RISK_ACKNOWLEDGEMENTS = [
  "openai-codex",
  "anthropic",
  "xai",
  "zai",
  "openrouter",
] as const;

export type AppServerProviderRiskAcknowledgement =
  (typeof APP_SERVER_PROVIDER_RISK_ACKNOWLEDGEMENTS)[number];

export type AppServerProviderAuthenticationMethod = "subscription" | "api_key";

export interface AppServerSessionLaunchProvider {
  id?: string;
  model?: string;
  reasoningEffort?: string;
  fastMode?: boolean;
  /** Accepted for older clients; the host's configured Daybreak mode controls OpenAI requests. */
  daybreakBlue?: boolean;
}

export interface AppServerSessionLaunchContinuation {
  resumeAttemptId?: string;
  resumeFromInitialAttempt?: boolean;
  fallbackPrompt: string;
}

export interface AppServerSessionLaunchIntent {
  /** Beale's durable workspace id, never a host filesystem path. */
  workspaceId: string;
  machineId?: string;
  /** Selected execution and browser targets for one research session. */
  targetIds?: readonly string[];
  /** Host-owned browser page for a session opened through a connected primary. */
  browserRelay?: boolean;
  fleetOwnerMachineId?: string;
  /** Provider routing preferences of the workspace-owning primary for a brokered Fleet guest. */
  brokerAuthenticationPreferences?: Readonly<Record<string, AppServerProviderAuthenticationMethod>>;
  brokerRiskAcknowledgements?: readonly AppServerProviderRiskAcknowledgement[];
  mode?: string;
  attemptId?: string;
  promptMarkdown: string;
  goal?: { objective?: string };
  provider?: AppServerSessionLaunchProvider;
  shellSafetyMode?: string;
  guardianEnabled?: boolean;
  workflowId?: string;
  researchProfileId?: string;
  researchProfileHash?: string;
  collaboration?: Record<string, unknown>;
  repeatSchedule?: AppServerRepeatSchedule;
  continuation?: AppServerSessionLaunchContinuation;
  generateTitle?: boolean;
  /** Enables agent-recorded harness feedback for sessions started by the development desktop. */
  devHarnessFeedback?: boolean;
  /** Host-only endpoint used by Beale sessions to load workspace introspection tools. */
  introspection?: {
    url: string;
    token: string;
    runtimeMode?: "isolated" | "standard";
  };
}

export interface AppServerSessionLaunchRequest {
  launchVersion: typeof APP_SERVER_SESSION_LAUNCH_VERSION;
  sessionId?: string;
  launch: AppServerSessionLaunchIntent;
}

export function decodeAppServerSessionLaunchRequest(value: unknown): AppServerSessionLaunchRequest {
  if (!isRecord(value)) throw new Error("Session request body must be a JSON object.");
  if (value.launchVersion !== APP_SERVER_SESSION_LAUNCH_VERSION) {
    throw new Error(`launchVersion must be ${APP_SERVER_SESSION_LAUNCH_VERSION}.`);
  }
  optionalBoundedString(value, "sessionId", 128);
  const launch = requiredRecord(value, "launch");
  requiredBoundedString(launch, "workspaceId", 256);
  optionalBoundedString(launch, "machineId", 256);
  if (launch.targetIds !== undefined) {
    if (!Array.isArray(launch.targetIds) || launch.targetIds.length < 1 || launch.targetIds.length > 16
      || launch.targetIds.some((id: unknown) => typeof id !== "string" || id.length < 1 || id.length > 256)
      || new Set(launch.targetIds).size !== launch.targetIds.length) {
      throw new Error("launch.targetIds must contain 1 to 16 distinct target IDs.");
    }
    if (launch.targetIds.length > 1 && launch.machineId !== "local") {
      throw new Error("Multi-target sessions must run on the workspace app-server.");
    }
    if (launch.targetIds.length === 1 && launch.machineId !== undefined && launch.machineId !== launch.targetIds[0]) {
      throw new Error("The session target must match machineId.");
    }
  }
  if (launch.browserRelay !== undefined && typeof launch.browserRelay !== "boolean") {
    throw new Error("launch.browserRelay must be a boolean.");
  }
  if (launch.repeatSchedule !== undefined) {
    const schedule = requiredRecord(launch, "repeatSchedule");
    if (schedule.type !== "none" && schedule.type !== "minutely" && schedule.type !== "hourly"
      && schedule.type !== "daily" && schedule.type !== "weekly" && schedule.type !== "monthly") {
      throw new Error("Invalid session repeat schedule.");
    }
    if (schedule.type !== "none" && (!Number.isInteger(schedule.interval)
      || (schedule.interval as number) < 1 || (schedule.interval as number) > 99)) {
      throw new Error("Invalid session repeat interval.");
    }
  }
  optionalBoundedString(launch, "fleetOwnerMachineId", 128);
  if (launch.brokerAuthenticationPreferences !== undefined) {
    const preferences = requiredRecord(launch, "brokerAuthenticationPreferences");
    for (const [provider, method] of Object.entries(preferences)) {
      if (!APP_SERVER_PROVIDER_RISK_ACKNOWLEDGEMENTS.includes(provider as AppServerProviderRiskAcknowledgement)
        || (method !== "subscription" && method !== "api_key")) {
        throw new Error("Invalid broker provider authentication preference.");
      }
    }
  }
  if (launch.brokerRiskAcknowledgements !== undefined
    && (!Array.isArray(launch.brokerRiskAcknowledgements)
      || launch.brokerRiskAcknowledgements.some((value: unknown) =>
        !APP_SERVER_PROVIDER_RISK_ACKNOWLEDGEMENTS.includes(value as AppServerProviderRiskAcknowledgement)))) {
    throw new Error("Invalid broker provider risk acknowledgements.");
  }
  optionalBoundedString(launch, "mode", 128);
  optionalBoundedString(launch, "attemptId", 128);
  requiredBoundedString(launch, "promptMarkdown", 131_072);
  optionalBoundedString(launch, "shellSafetyMode", 64);
  if (launch.guardianEnabled !== undefined && typeof launch.guardianEnabled !== "boolean") {
    throw new Error("launch.guardianEnabled must be a boolean.");
  }
  optionalBoundedString(launch, "workflowId", 256);
  optionalBoundedString(launch, "researchProfileId", 256);
  optionalBoundedString(launch, "researchProfileHash", 256);
  if (launch.generateTitle !== undefined && typeof launch.generateTitle !== "boolean") {
    throw new Error("launch.generateTitle must be a boolean.");
  }
  if (launch.devHarnessFeedback !== undefined && typeof launch.devHarnessFeedback !== "boolean") {
    throw new Error("launch.devHarnessFeedback must be a boolean.");
  }
  if (launch.introspection !== undefined) {
    const introspection = requiredRecord(launch, "introspection");
    requiredBoundedString(introspection, "url", 2_048);
    requiredBoundedString(introspection, "token", 4_096);
    if (
      introspection.runtimeMode !== undefined
      && introspection.runtimeMode !== "isolated"
      && introspection.runtimeMode !== "standard"
    ) {
      throw new Error("launch.introspection.runtimeMode must be isolated or standard.");
    }
  }

  if (launch.goal !== undefined) {
    const goal = requiredRecord(launch, "goal");
    optionalBoundedString(goal, "objective", 131_072);
  }

  if (launch.provider !== undefined) {
    const provider = requiredRecord(launch, "provider");
    optionalBoundedString(provider, "id", 128);
    optionalBoundedString(provider, "model", 512);
    optionalBoundedString(provider, "reasoningEffort", 64);
    if (provider.fastMode !== undefined && typeof provider.fastMode !== "boolean") {
      throw new Error("launch.provider.fastMode must be a boolean.");
    }
    if (provider.daybreakBlue !== undefined && typeof provider.daybreakBlue !== "boolean") {
      throw new Error("launch.provider.daybreakBlue must be a boolean.");
    }
    if (provider.daybreakBlue !== undefined && provider.id !== undefined && provider.id !== "openai-codex") {
      throw new Error("launch.provider.daybreakBlue requires the openai-codex provider.");
    }
  }

  if (launch.collaboration !== undefined && !isRecord(launch.collaboration)) {
    throw new Error("launch.collaboration must be an object.");
  }
  if (launch.continuation !== undefined) {
    const continuation = requiredRecord(launch, "continuation");
    optionalBoundedString(continuation, "resumeAttemptId", 128);
    requiredBoundedString(continuation, "fallbackPrompt", 262_144);
    if (continuation.resumeFromInitialAttempt !== undefined
      && typeof continuation.resumeFromInitialAttempt !== "boolean") {
      throw new Error("launch.continuation.resumeFromInitialAttempt must be a boolean.");
    }
  }

  return value as unknown as AppServerSessionLaunchRequest;
}

export interface AppServerProtocolArguments {
  args: readonly string[];
  requestId?: string;
}

export function parseAppServerProtocolArguments(argv: readonly string[]): AppServerProtocolArguments {
  const args: string[] = [];
  let requestId: string | undefined;
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg !== "--request-id") {
      if (arg !== undefined) args.push(arg);
      continue;
    }
    if (requestId !== undefined) throw new Error("--request-id may only be provided once.");
    const value = argv[index + 1];
    if (!value?.trim()) throw new Error("--request-id requires a non-empty value.");
    if (value.length > APP_SERVER_PROTOCOL_MAX_REQUEST_ID_LENGTH) {
      throw new Error(`--request-id must not exceed ${APP_SERVER_PROTOCOL_MAX_REQUEST_ID_LENGTH} characters.`);
    }
    requestId = value;
    index += 1;
  }
  return { args, ...(requestId ? { requestId } : {}) };
}

export function appServerProtocolSuccess<T>(operation: AppServerProtocolOperation, result: T, requestId?: string): AppServerProtocolSuccess<T> {
  return {
    protocol: APP_SERVER_PROTOCOL_NAME, protocolVersion: APP_SERVER_PROTOCOL_VERSION,
    operation, ok: true, result, ...(requestId ? { requestId } : {}),
  };
}

export function appServerProtocolFailure(
  operation: AppServerProtocolOperation,
  code: string,
  message: string,
  retryable = false,
  requestId?: string,
): AppServerProtocolFailure {
  return {
    protocol: APP_SERVER_PROTOCOL_NAME, protocolVersion: APP_SERVER_PROTOCOL_VERSION,
    operation,
    ok: false,
    error: appServerProtocolErrorDetail(code, message, retryable),
    ...(requestId ? { requestId } : {}),
  };
}

export function appServerProtocolErrorDetail(
  code: string,
  message: string,
  retryable = false,
): AppServerProtocolErrorDetail {
  return { code, message, retryable };
}

export function appServerProtocolDescriptor(): AppServerProtocolDescriptor {
  return {
    protocol: APP_SERVER_PROTOCOL_NAME,
    protocolVersion: APP_SERVER_PROTOCOL_VERSION,
    operations: APP_SERVER_PROTOCOL_OPERATIONS,
    contractVersion: APP_SERVER_CONTRACT_VERSION,
    runtime: {
      name: APP_SERVER_PROTOCOL_NAME,
      version: APP_SERVER_RUNTIME_VERSION,
      buildId: appServerRuntimeBuildId(),
      nodeVersion: process.version,
    },
    schemas: { protocol: 1, session: 1, memorySummary: 14, finding: 7, campaignGraph: 5, goalSuggestions: 1 },
    capabilities: APP_SERVER_PROTOCOL_CAPABILITIES,
    transports: {
      appServer: {
        path: BEALE_APP_SERVER_OPERATIONS_PATH,
        authentication: "operator-bearer",
        framing: "json",
        errors: "http-problem",
      },
      websocket: {
        path: APP_SERVER_PROTOCOL_WEBSOCKET_PATH,
        authentication: "bearer",
        framing: "json-message",
        errors: "protocol-error-message",
        correlation: "request-id",
        capabilities: APP_SERVER_PROTOCOL_WEBSOCKET_CAPABILITIES,
      },
    },
  };
}

function appServerRuntimeBuildId(): string {
  const configured = process.env.APP_SERVER_BUILD_ID?.trim();
  if (configured) return configured;
  try {
    return createHash("sha256").update(readFileSync(fileURLToPath(import.meta.url))).digest("hex").slice(0, 24);
  } catch {
    return `${APP_SERVER_RUNTIME_VERSION}-unknown`;
  }
}

export function appServerTransportBootstrap(url: string, sessionId: string): AppServerTransportBootstrap {
  return { protocolVersion: APP_SERVER_PROTOCOL_VERSION, transport: "websocket", url, sessionId };
}

export function appServerServerHello(sessionId: string, serverVersion: string): AppServerServerHello {
  return {
    protocolVersion: APP_SERVER_PROTOCOL_VERSION,
    type: "server.hello",
    sessionId,
    server: { name: APP_SERVER_PROTOCOL_NAME, version: serverVersion, buildId: appServerRuntimeBuildId() },
    contractVersion: APP_SERVER_CONTRACT_VERSION,
    schemas: { protocol: 1, session: 1, memorySummary: 14, finding: 7, campaignGraph: 5, goalSuggestions: 1 },
    capabilities: APP_SERVER_PROTOCOL_WEBSOCKET_CAPABILITIES,
  };
}

export function appServerSessionEvent<TEvent extends Record<string, unknown>>(
  sessionId: string,
  event: TEvent,
): AppServerSessionEvent<TEvent> {
  return { protocolVersion: APP_SERVER_PROTOCOL_VERSION, type: "session.event", sessionId, event };
}

export function appServerWebSocketProtocolError(
  sessionId: string,
  code: string,
  message: string,
  requestId?: string,
): AppServerWebSocketProtocolError {
  return {
    protocolVersion: APP_SERVER_PROTOCOL_VERSION,
    type: "protocol.error",
    sessionId,
    ...(requestId ? { requestId } : {}),
    error: appServerProtocolErrorDetail(code, message),
    message,
  };
}

export function decodeAppServerProtocolEnvelope(value: unknown): AppServerProtocolEnvelope {
  if (!isRecord(value) || value.protocol !== APP_SERVER_PROTOCOL_NAME
    || value.protocolVersion !== APP_SERVER_PROTOCOL_VERSION
    || !isProtocolOperation(value.operation) || typeof value.ok !== "boolean") {
    throw new Error("Invalid or unsupported app-server protocol envelope.");
  }
  validateOptionalRequestId(value.requestId);
  if (value.ok === true) {
    if (!("result" in value)) throw new Error("app-server protocol success is missing result.");
    return value as unknown as AppServerProtocolSuccess;
  }
  validateError(value.error);
  return value as unknown as AppServerProtocolFailure;
}

export function decodeBealeAppServerHealth(value: unknown): BealeAppServerHealth {
  if (!isRecord(value) || value.ok !== true
    || value.controlVersion !== BEALE_APP_SERVER_CONTROL_VERSION
    || value.contractTimestamp !== BEALE_APP_SERVER_CONTRACT_TIMESTAMP
    || !sameAppServerCapabilities(value.capabilities)) {
    throw new Error("Invalid or incompatible Beale app-server health response.");
  }
  return value as unknown as BealeAppServerHealth;
}

export function decodeBealeAppServerDescriptor(value: unknown): BealeAppServerDescriptor {
  decodeBealeAppServerHealth(value);
  if (!isRecord(value) || value.sessionLaunchVersion !== APP_SERVER_SESSION_LAUNCH_VERSION
    || value.appServerProtocolVersion !== APP_SERVER_PROTOCOL_VERSION
    || !isRecord(value.endpoints)
    || value.endpoints.sessions !== BEALE_APP_SERVER_SESSIONS_PATH
    || value.endpoints.workspaces !== BEALE_APP_SERVER_WORKSPACES_PATH
    || value.endpoints.providers !== BEALE_APP_SERVER_PROVIDERS_PATH
    || value.endpoints.shutdown !== BEALE_APP_SERVER_SHUTDOWN_PATH
    || !isRecord(value.limits)
    || !positiveInteger(value.limits.requestBodyBytes)
    || !positiveInteger(value.limits.frameBytes)
    || value.limits.replayBytes !== BEALE_APP_SERVER_MAX_REPLAY_BYTES
    || value.limits.replayFrames !== BEALE_APP_SERVER_MAX_REPLAY_FRAMES) {
    throw new Error("Invalid or incompatible Beale app-server descriptor.");
  }
  return value as unknown as BealeAppServerDescriptor;
}

export function decodeBealeAppServerProviderCatalog(value: unknown): BealeAppServerProviderCatalog {
  if (!isRecord(value) || value.controlVersion !== BEALE_APP_SERVER_CONTROL_VERSION
    || !(value.defaultProviderId === null || nonEmptyString(value.defaultProviderId))
    || !Array.isArray(value.providers)) {
    throw new Error("Invalid Beale app-server provider catalog.");
  }
  const providers = value.providers.map((candidate) => {
    if (!isRecord(candidate)
      || !nonEmptyString(candidate.providerId)
      || !nonEmptyString(candidate.providerName)
      || (candidate.riskAcknowledged !== undefined && typeof candidate.riskAcknowledged !== 'boolean')
      || !(candidate.defaultLeadModel === null || nonEmptyString(candidate.defaultLeadModel))
      || !(candidate.defaultSubagentModel === null || nonEmptyString(candidate.defaultSubagentModel))
      || !(candidate.defaultReasoningEffort === null || nonEmptyString(candidate.defaultReasoningEffort))
      || !Array.isArray(candidate.models)) {
      throw new Error("Invalid Beale app-server provider catalog entry.");
    }
    const models = candidate.models.map((model) => {
      if (!isRecord(model) || !nonEmptyString(model.id) || !nonEmptyString(model.name)
        || typeof model.reasoning !== "boolean" || !Array.isArray(model.effortLevels)
        || !model.effortLevels.every(nonEmptyString)) {
        throw new Error("Invalid Beale app-server provider model.");
      }
      return model as unknown as BealeAppServerProviderModel;
    });
    return { ...candidate, models } as unknown as BealeAppServerProviderCatalogEntry;
  });
  return {
    controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
    defaultProviderId: value.defaultProviderId as string | null,
    providers,
  };
}

export function decodeBealeProviderUsageLimits(value: unknown): BealeProviderUsageLimits {
  const providerIds: readonly string[] = ["openai-codex", "xai", "anthropic", "zai", "openrouter"];
  const statuses: readonly string[] = ["available", "unsupported", "not_configured", "unavailable"];
  if (!isRecord(value)
    || !providerIds.includes(String(value.providerId))
    || !nonEmptyString(value.checkedAt)
    || !Number.isFinite(Date.parse(value.checkedAt))
    || !statuses.includes(String(value.status))
    || !(value.detail === null || typeof value.detail === "string")
    || !Array.isArray(value.windows)) {
    throw new Error("Invalid Beale provider usage limits.");
  }
  const windows = value.windows.map((window) => {
    if (!isRecord(window)
      || !nonEmptyString(window.id)
      || !nonEmptyString(window.label)
      || typeof window.usedPercent !== "number"
      || !Number.isFinite(window.usedPercent)
      || window.usedPercent < 0 || window.usedPercent > 100
      || !(window.resetsAt === null || (nonEmptyString(window.resetsAt) && Number.isFinite(Date.parse(window.resetsAt))))
      || !(window.durationMinutes === null || (typeof window.durationMinutes === "number" && Number.isFinite(window.durationMinutes) && window.durationMinutes > 0))) {
      throw new Error("Invalid Beale provider usage window.");
    }
    return window as unknown as BealeProviderUsageWindow;
  });
  return { ...value, windows } as unknown as BealeProviderUsageLimits;
}

export function decodeBealeAppServerSessionCatalogEntry(value: unknown): BealeAppServerSessionCatalogEntry {
  if (!isRecord(value) || !nonEmptyString(value.sessionId) || !isAppServerSessionState(value.state)
    || !nonEmptyString(value.startedAt)
    || !(value.endedAt === null || nonEmptyString(value.endedAt))
    || !(value.exitCode === null || Number.isInteger(value.exitCode))
    || typeof value.clientConnected !== "boolean"
    || !(value.diagnostic === null || typeof value.diagnostic === "string")
    || !isRecord(value.replay)
    || !nonNegativeInteger(value.replay.bufferedFrames)
    || !nonNegativeInteger(value.replay.bufferedBytes)
    || !nonNegativeInteger(value.replay.droppedFrames)) {
    throw new Error("Invalid Beale app-server session catalog entry.");
  }
  return value as unknown as BealeAppServerSessionCatalogEntry;
}

export function decodeBealeAppServerSessionCatalog(value: unknown): BealeAppServerSessionCatalog {
  if (!isRecord(value) || value.controlVersion !== BEALE_APP_SERVER_CONTROL_VERSION
    || !Array.isArray(value.sessions)) {
    throw new Error("Invalid Beale app-server session catalog.");
  }
  return {
    controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
    sessions: value.sessions.map(decodeBealeAppServerSessionCatalogEntry),
  };
}

export function decodeBealeAppServerSessionResult(value: unknown): BealeAppServerSessionResult {
  if (!isRecord(value) || value.controlVersion !== BEALE_APP_SERVER_CONTROL_VERSION) {
    throw new Error("Invalid Beale app-server session response.");
  }
  return {
    controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
    session: decodeBealeAppServerSessionCatalogEntry(value.session),
  };
}

export function decodeBealeAppServerSessionStartResult(value: unknown): BealeAppServerSessionStartResult {
  if (!isRecord(value) || value.controlVersion !== BEALE_APP_SERVER_CONTROL_VERSION
    || !nonEmptyString(value.attemptId) || !isRecord(value.transport)
    || value.transport.protocolVersion !== APP_SERVER_PROTOCOL_VERSION
    || value.transport.authentication !== "bearer"
    || !nonEmptyString(value.transport.token)
    || value.transport.reconnect !== "replay") {
    throw new Error("Invalid Beale app-server session start response.");
  }
  const session = decodeBealeAppServerSessionCatalogEntry(value.session);
  const expectedTransportPath = `${BEALE_APP_SERVER_SESSIONS_PATH}/${encodeURIComponent(session.sessionId)}/transport`;
  if (value.transport.path !== expectedTransportPath) {
    throw new Error("Invalid Beale app-server session transport path.");
  }
  return {
    controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
    session,
    attemptId: value.attemptId,
    transport: value.transport as unknown as BealeAppServerSessionTransport,
  };
}

export function decodeBealeAppServerSessionContinuationRequest(
  value: unknown,
): BealeAppServerSessionContinuationRequest {
  if (!isRecord(value)) throw new Error("Session continuation body must be a JSON object.");
  requiredBoundedString(value, "workspaceId", 256);
  requiredBoundedString(value, "instruction", 131_072);
  return value as unknown as BealeAppServerSessionContinuationRequest;
}

export function decodeBealeAppServerSessionAttachResult(value: unknown): BealeAppServerSessionAttachResult {
  if (!isRecord(value) || value.controlVersion !== BEALE_APP_SERVER_CONTROL_VERSION
    || !isRecord(value.transport)
    || value.transport.protocolVersion !== APP_SERVER_PROTOCOL_VERSION
    || value.transport.authentication !== "bearer"
    || !nonEmptyString(value.transport.token)
    || value.transport.reconnect !== "replay") {
    throw new Error("Invalid Beale app-server session attachment response.");
  }
  const session = decodeBealeAppServerSessionCatalogEntry(value.session);
  const expectedTransportPath = `${BEALE_APP_SERVER_SESSIONS_PATH}/${encodeURIComponent(session.sessionId)}/transport`;
  if (value.transport.path !== expectedTransportPath) {
    throw new Error("Invalid Beale app-server session transport path.");
  }
  return {
    controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
    session,
    transport: value.transport as unknown as BealeAppServerSessionTransport,
  };
}

export function decodeBealeAppServerSessionStopResult(value: unknown): BealeAppServerSessionStopResult {
  if (!isRecord(value) || value.controlVersion !== BEALE_APP_SERVER_CONTROL_VERSION
    || typeof value.stopped !== "boolean" || !nonEmptyString(value.sessionId)) {
    throw new Error("Invalid Beale app-server session stop response.");
  }
  return value as unknown as BealeAppServerSessionStopResult;
}

export function decodeBealeAppServerSessionControlResult(value: unknown): BealeAppServerSessionControlResult {
  if (!isRecord(value) || value.controlVersion !== BEALE_APP_SERVER_CONTROL_VERSION
    || value.accepted !== true || !nonEmptyString(value.sessionId)
    || !validRequestId(value.requestId)
    || (value.type !== "steer" && value.type !== "pause" && value.type !== "resume" && value.type !== "stop")) {
    throw new Error("Invalid Beale app-server session control response.");
  }
  return value as unknown as BealeAppServerSessionControlResult;
}

export function decodeBealeAppServerShutdownResult(value: unknown): BealeAppServerShutdownResult {
  if (!isRecord(value) || value.controlVersion !== BEALE_APP_SERVER_CONTROL_VERSION
    || value.shuttingDown !== true) {
    throw new Error("Invalid Beale app-server shutdown response.");
  }
  return value as unknown as BealeAppServerShutdownResult;
}

export function decodeBealeAppServerErrorResponse(value: unknown): BealeAppServerErrorResponse {
  if (!isRecord(value) || value.controlVersion !== BEALE_APP_SERVER_CONTROL_VERSION) {
    throw new Error("Invalid Beale app-server error response.");
  }
  validateError(value.error);
  return value as unknown as BealeAppServerErrorResponse;
}

export function decodeAppServerTransportBootstrap(value: unknown): AppServerTransportBootstrap {
  if (!isRecord(value) || value.protocolVersion !== APP_SERVER_PROTOCOL_VERSION
    || value.transport !== "websocket" || !nonEmptyString(value.url) || !nonEmptyString(value.sessionId)) {
    throw new Error("Invalid or unsupported app-server transport bootstrap.");
  }
  return value as unknown as AppServerTransportBootstrap;
}

export function decodeAppServerClientMessage(value: unknown): AppServerClientMessage {
  validateMessageBase(value);
  if (value.type === "client.hello") {
    if (!isRecord(value.client) || !nonEmptyString(value.client.name) || !nonEmptyString(value.client.version)) {
      throw new Error("The client.hello message requires client name and version.");
    }
    return value as unknown as AppServerClientHello;
  }
  if (value.type === "session.control") {
    if (!validRequestId(value.requestId) || !isRecord(value.control)) {
      throw new Error("The session.control message requires requestId and control.");
    }
    if (value.control.requestId !== value.requestId) throw new Error("Control request IDs must match.");
    return value as unknown as AppServerSessionControl;
  }
  throw new Error("Unsupported app-server client message type.");
}

export function decodeAppServerServerMessage(value: unknown): AppServerServerMessage {
  validateMessageBase(value);
  if (value.type === "server.hello") {
    if (!isRecord(value.server) || value.server.name !== APP_SERVER_PROTOCOL_NAME
      || !nonEmptyString(value.server.version) || !nonEmptyString(value.server.buildId)
      || value.contractVersion !== APP_SERVER_CONTRACT_VERSION
      || !validSchemaDescriptor(value.schemas) || !sameCapabilities(value.capabilities)) {
      throw new Error("The server.hello message has invalid server metadata or capabilities.");
    }
    return value as unknown as AppServerServerHello;
  }
  if (value.type === "session.event") {
    if (!isRecord(value.event)) throw new Error("The session.event message requires an event.");
    return value as unknown as AppServerSessionEvent;
  }
  if (value.type === "protocol.error") {
    validateOptionalRequestId(value.requestId);
    const legacyMessage = nonEmptyString(value.message) ? value.message : undefined;
    const error = isValidError(value.error)
      ? value.error
      : legacyMessage ? { code: "protocol_error", message: legacyMessage, retryable: false } : undefined;
    if (!error) throw new Error("The protocol.error message requires a valid error.");
    return {
      protocolVersion: APP_SERVER_PROTOCOL_VERSION,
      type: "protocol.error",
      sessionId: value.sessionId,
      ...(nonEmptyString(value.requestId) ? { requestId: value.requestId } : {}),
      error,
      message: legacyMessage ?? error.message,
    };
  }
  throw new Error("Unsupported app-server server message type.");
}

function validSchemaDescriptor(value: unknown): value is AppServerProtocolDescriptor["schemas"] {
  return isRecord(value) && value.protocol === 1 && value.session === 1
    && value.memorySummary === 14 && value.finding === 7 && value.campaignGraph === 5;
}

export interface BealeWorkspaceResearchClaimDuplicate {
  id: string;
  projection: "lead" | "finding";
  maturity: string;
  rating: ResearchClaimRating;
  classification: string;
  title: string;
  status: string;
  revision: number;
  markedAt: string;
}

function validateMessageBase(value: unknown): asserts value is Record<string, unknown> & { sessionId: string; type: string } {
  if (!isRecord(value) || value.protocolVersion !== APP_SERVER_PROTOCOL_VERSION
    || !nonEmptyString(value.type) || !nonEmptyString(value.sessionId)) {
    throw new Error("Invalid app-server WebSocket message.");
  }
}

function validateOptionalRequestId(value: unknown): void {
  if (value !== undefined && !validRequestId(value)) {
    throw new Error(`app-server protocol requestId must be non-empty and at most ${APP_SERVER_PROTOCOL_MAX_REQUEST_ID_LENGTH} characters.`);
  }
}

function validateError(value: unknown): asserts value is AppServerProtocolErrorDetail {
  if (!isValidError(value)) throw new Error("app-server protocol failure is missing a valid error.");
}

function isValidError(value: unknown): value is AppServerProtocolErrorDetail {
  return isRecord(value) && nonEmptyString(value.code)
    && typeof value.message === "string" && typeof value.retryable === "boolean";
}

function sameCapabilities(value: unknown): value is typeof APP_SERVER_PROTOCOL_WEBSOCKET_CAPABILITIES {
  return Array.isArray(value) && value.length === APP_SERVER_PROTOCOL_WEBSOCKET_CAPABILITIES.length
    && APP_SERVER_PROTOCOL_WEBSOCKET_CAPABILITIES.every((capability, index) => value[index] === capability);
}

function isProtocolOperation(value: unknown): value is AppServerProtocolOperation {
  return typeof value === "string" && (APP_SERVER_PROTOCOL_OPERATIONS as readonly string[]).includes(value);
}

function sameAppServerCapabilities(value: unknown): value is typeof BEALE_APP_SERVER_CAPABILITIES {
  return Array.isArray(value) && value.length === BEALE_APP_SERVER_CAPABILITIES.length
    && BEALE_APP_SERVER_CAPABILITIES.every((capability, index) => value[index] === capability);
}

function isAppServerSessionState(value: unknown): value is BealeAppServerSessionState {
  return value === "starting" || value === "running" || value === "completed"
    || value === "failed" || value === "stopped";
}

function positiveInteger(value: unknown): value is number {
  return Number.isInteger(value) && (value as number) > 0;
}

function nonNegativeInteger(value: unknown): value is number {
  return Number.isInteger(value) && (value as number) >= 0;
}

function requiredRecord(record: Record<string, unknown>, key: string): Record<string, unknown> {
  const value = record[key];
  if (!isRecord(value)) throw new Error(`${key} must be a JSON object.`);
  return value;
}

function requiredBoundedString(record: Record<string, unknown>, key: string, maxLength: number): string {
  const value = record[key];
  if (typeof value !== "string" || !value.trim() || value.length > maxLength) {
    throw new Error(`${key} must be a non-empty string of at most ${maxLength} characters.`);
  }
  return value;
}

function optionalBoundedString(record: Record<string, unknown>, key: string, maxLength: number): void {
  const value = record[key];
  if (value === undefined) return;
  requiredBoundedString(record, key, maxLength);
}

function stringArray(
  record: Record<string, unknown>,
  key: string,
  maxEntries: number,
  maxValueLength: number,
  allowed?: readonly string[],
): void {
  const value = record[key];
  if (value === undefined) return;
  if (!Array.isArray(value) || value.length > maxEntries || value.some((entry) =>
    typeof entry !== "string" || !entry.trim() || entry.length > maxValueLength
      || (allowed !== undefined && !allowed.includes(entry)))) {
    throw new Error(`${key} must be an array of at most ${maxEntries} supported, non-empty strings.`);
  }
}

function stringMap(
  record: Record<string, unknown>,
  key: string,
  maxEntries: number,
  maxKeyLength: number,
  maxValueLength: number,
  allowedValues?: readonly string[],
): void {
  const value = record[key];
  if (value === undefined) return;
  if (!isRecord(value)) throw new Error(`${key} must be a JSON object of string values.`);
  const entries = Object.entries(value);
  if (entries.length > maxEntries || entries.some(([entryKey, entryValue]) =>
    !entryKey.trim() || entryKey.length > maxKeyLength
      || typeof entryValue !== "string" || !entryValue.trim() || entryValue.length > maxValueLength
      || (allowedValues !== undefined && !allowedValues.includes(entryValue)))) {
    throw new Error(`${key} contains an invalid key or value.`);
  }
}

function nonEmptyString(value: unknown): value is string {
  return typeof value === "string" && Boolean(value.trim());
}

function validRequestId(value: unknown): value is string {
  return nonEmptyString(value) && value.length <= APP_SERVER_PROTOCOL_MAX_REQUEST_ID_LENGTH;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}
