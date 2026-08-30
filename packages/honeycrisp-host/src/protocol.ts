import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import type {
  GeneratedResearchGoalSuggestions,
  ResearchChannelMessageKind,
  ResearchChannelSharedResourceKind,
  ResearchClaimRating,
  ResearchGoalSuggestionInput,
  ResearchGoalSuggestionSelectionInput,
} from "@honeycrisp/research-agent";

export type {
  ResearchChannelDetail,
  ResearchChannelMemberRecord,
  ResearchChannelMemberStatus,
  ResearchChannelMessageKind,
  ResearchChannelMessageRecord,
  ResearchChannelRecord,
  ResearchChannelSharedResourceKind,
  ResearchChannelSharedResourceRecord,
  ResearchChannelSummary,
  ResearchClaimRating,
  GeneratedResearchGoalSuggestions,
  ResearchGoalSuggestionInput,
  ResearchGoalSuggestionSelectionInput,
} from "@honeycrisp/research-agent";

export interface CreateResearchChannelInput {
  name: string;
  title?: string;
  topic: string;
}

export interface PostResearchChannelMessageInput {
  contentMarkdown: string;
  kind?: Exclude<ResearchChannelMessageKind, "system">;
  evidenceRefs?: string[];
}

export interface ShareResearchChannelResourceInput {
  kind: ResearchChannelSharedResourceKind;
  resourceId: string;
  title: string;
  note?: string;
}

export const HONEYCRISP_PROTOCOL_NAME = "honeycrisp" as const;
export const HONEYCRISP_PROTOCOL_VERSION = 1 as const;
export const HONEYCRISP_CONTRACT_VERSION = 12 as const;
export const HONEYCRISP_RUNTIME_VERSION = "0.1.0" as const;
export const HONEYCRISP_PROTOCOL_WEBSOCKET_PATH = "/v1/session" as const;
export const HONEYCRISP_PROTOCOL_BOOTSTRAP_PREFIX = "HONEYCRISP_TRANSPORT " as const;
/**
 * Bump this UTC timestamp whenever the Desktop/app-server control contract
 * changes. Both binaries compile the same value and compare it directionally.
 */
export const BEALE_APP_SERVER_CONTRACT_TIMESTAMP = "2026-08-28T22:00:00.000Z" as const;
export const BEALE_APP_SERVER_CONTROL_VERSION = 1 as const;
export const BEALE_APP_SERVER_CAPABILITIES = [
  "session.typed-launch.v2",
  "session.introspection-runtime.v1",
  "session.exit-diagnostics",
  "session.transport-path.v1",
  "session.reconnect.v1",
  "session.startup-recovery.v1",
  "session.multi-client.v1",
  "host.control.v1",
  "host.descriptor.v1",
  "host.provider-catalog.v1",
  "canonical.reads.v1",
  "session.commentary.v1",
  "memory.notifications.v3",
  "host.operations.v1",
  "source.clone-modes.v1",
  "maintenance.repository-consolidation.v1",
  "host.shutdown-guard.v1",
  "workspace.memory-backend.v1",
  "knowledge.campaign-tracks.v2",
  "knowledge.claims.v2",
  "knowledge.claim-security-tracking.v1",
  "workspace.channels.v2",
  "workspace.channels.archive.v1",
  "workspace.goal-suggestions.v1",
  "workspace.prompt-expansion.v1",
  "knowledge.report-content-revise.v1",
  "knowledge.report-triage-status.v1",
  "knowledge.report-packet-replace.v1",
  "knowledge.report-recording-replace.v1",
] as const;
export const BEALE_APP_SERVER_SERVER_PATH = "/v1/server" as const;
export const BEALE_APP_SERVER_SESSIONS_PATH = "/v1/sessions" as const;
export const BEALE_APP_SERVER_WORKSPACES_PATH = "/v1/workspaces" as const;
export const BEALE_APP_SERVER_PROVIDERS_PATH = "/v1/providers" as const;
export const BEALE_APP_SERVER_OPERATIONS_PATH = "/v1/operations" as const;
export const BEALE_APP_SERVER_SHUTDOWN_PATH = "/v1/server/shutdown" as const;
export const BEALE_APP_SERVER_MAX_REPLAY_BYTES = 4_194_304 as const;
export const BEALE_APP_SERVER_MAX_REPLAY_FRAMES = 256 as const;

export interface BealeAppServerHealth {
  ok: true;
  controlVersion: typeof BEALE_APP_SERVER_CONTROL_VERSION;
  contractTimestamp: typeof BEALE_APP_SERVER_CONTRACT_TIMESTAMP;
  capabilities: typeof BEALE_APP_SERVER_CAPABILITIES;
}

export interface BealeAppServerDescriptor extends BealeAppServerHealth {
  sessionLaunchVersion: typeof HONEYCRISP_SESSION_LAUNCH_VERSION;
  honeycrispProtocolVersion: typeof HONEYCRISP_PROTOCOL_VERSION;
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
  protocolVersion: typeof HONEYCRISP_PROTOCOL_VERSION;
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

export interface BealeAppServerShutdownResult {
  controlVersion: typeof BEALE_APP_SERVER_CONTROL_VERSION;
  shuttingDown: true;
}

export interface BealeAppServerErrorResponse {
  controlVersion: typeof BEALE_APP_SERVER_CONTROL_VERSION;
  error: HoneycrispProtocolErrorDetail;
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
}

export interface BealeWorkspaceMemoryCatalog {
  schemaVersion: 3;
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
  sessionIds: string[];
  projection: "lead" | "finding";
  maturity: string;
  freshness: string;
  workflow: string;
  rating: ResearchClaimRating;
  classification: string;
  componentClaimIds: string[];
  title: string;
  summary: string;
  impact: string;
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
  riskTreatment: "unreviewed" | "remediate" | "mitigated" | "accepted" | "transferred";
  cvssAssessments: Array<{
    version: "4.0" | "3.1";
    vector: string;
    score: number;
    nomenclature: "CVSS-B" | "CVSS-BT" | "CVSS-BE" | "CVSS-BTE" | "CVSS:3.1";
    assessedAt: string;
  }>;
  affectedAssetIds: string[];
  affectedVersions: Array<{ assetId: string | null; range: string; fixedVersion: string | null }>;
  externalReferences: Array<{ kind: string; identifier: string; url: string | null }>;
}

export const HONEYCRISP_PROTOCOL_WEBSOCKET_CAPABILITIES = ["session.events", "session.controls"] as const;
export const HONEYCRISP_PROTOCOL_MAX_REQUEST_ID_LENGTH = 200 as const;
export const HONEYCRISP_PROTOCOL_CAPABILITIES = [
  "knowledge.findings",
  "knowledge.claims.v2",
  "knowledge.claim_security_tracking",
  "knowledge.finding_staleness",
  "knowledge.campaign_graph",
  "knowledge.campaign_tracks.v2",
  "knowledge.evidence_gates",
  "session.append_only",
  "session.controls",
  "session.bounded_reads",
  "session.targeted_details",
  "workspace.channels.v2",
  "workspace.channels.archive.v1",
  "workspace.goal-suggestions.v1",
  "workspace.prompt-expansion.v1",
  "knowledge.report-content-revise.v1",
  "knowledge.report-triage-status.v1",
  "knowledge.report-packet-replace.v1",
  "knowledge.report-recording-replace.v1",
] as const;

/** @deprecated Import the protocol-named constants from `honeycrisp/protocol`. */
export const HONEYCRISP_TRANSPORT_PROTOCOL_VERSION = HONEYCRISP_PROTOCOL_VERSION;
/** @deprecated Import the protocol-named constants from `honeycrisp/protocol`. */
export const HONEYCRISP_TRANSPORT_PREFIX = HONEYCRISP_PROTOCOL_BOOTSTRAP_PREFIX;
/** @deprecated Import the protocol-named constants from `honeycrisp/protocol`. */
export const HONEYCRISP_TRANSPORT_PATH = HONEYCRISP_PROTOCOL_WEBSOCKET_PATH;

export const HONEYCRISP_PROTOCOL_OPERATIONS = [
  "protocol.describe", "session.create", "session.begin_attempt", "session.append_event", "session.append_event_receipt",
  "session.transition", "session.recover_interrupted", "session.import_capture", "session.get", "session.get_update", "session.events", "session.event_details",
  "session.collaboration", "session.captures", "session.capture", "session.list", "session.list_summaries",
  "channel.list", "channel.get", "channel.create", "channel.join", "channel.post", "channel.share", "channel.archive", "channel.restore", "channel.delete",
  "memory.summary", "memory.notification_feed", "dreaming.prepare", "dreaming.parse_plan", "dreaming.apply",
  "dreaming.record_failure", "dreaming.restore", "runbook.get", "report.get", "report.revise_content", "report.update_triage_status", "report.replace_packet", "report.replace_recording",
  "investigation.list", "investigation.get", "investigation.replay",
  "artifact.resolve", "provider.complete", "provider.describe", "model_job.resolve",
  "suggestion.generate", "suggestion.select", "prompt.expand",
  "profile.resolve", "auth.list", "auth.status", "auth.verify", "auth.logout", "model.list",
  "tools.list", "tools.config", "config.show", "config.set",
  "source.inspect", "source.materialize", "plugin.list", "plugin.add_filesystem",
  "plugin.add_repository", "plugin.set_enabled", "plugin.remove", "plugin.runtime",
  "maintenance.summary", "maintenance.run",
] as const;

export type HoneycrispProtocolOperation = (typeof HONEYCRISP_PROTOCOL_OPERATIONS)[number];

export interface HoneycrispProtocolErrorDetail {
  code: string;
  message: string;
  retryable: boolean;
}

interface HoneycrispProtocolEnvelopeBase {
  protocol: typeof HONEYCRISP_PROTOCOL_NAME;
  protocolVersion: typeof HONEYCRISP_PROTOCOL_VERSION;
  operation: HoneycrispProtocolOperation;
  requestId?: string;
}

export interface HoneycrispProtocolSuccess<T = unknown> extends HoneycrispProtocolEnvelopeBase {
  ok: true;
  result: T;
}

export interface HoneycrispProtocolFailure extends HoneycrispProtocolEnvelopeBase {
  ok: false;
  error: HoneycrispProtocolErrorDetail;
}

export type HoneycrispProtocolEnvelope<T = unknown> = HoneycrispProtocolSuccess<T> | HoneycrispProtocolFailure;

export interface HoneycrispProtocolDescriptor {
  protocol: typeof HONEYCRISP_PROTOCOL_NAME;
  protocolVersion: typeof HONEYCRISP_PROTOCOL_VERSION;
  operations: readonly HoneycrispProtocolOperation[];
  contractVersion: typeof HONEYCRISP_CONTRACT_VERSION;
  runtime: {
    name: typeof HONEYCRISP_PROTOCOL_NAME;
    version: typeof HONEYCRISP_RUNTIME_VERSION;
    buildId: string;
    nodeVersion: string;
  };
  schemas: {
    protocol: 1;
    session: 1;
    memorySummary: 11;
    finding: 4;
    campaignGraph: 4;
    goalSuggestions: 1;
  };
  capabilities: typeof HONEYCRISP_PROTOCOL_CAPABILITIES;
  transports: {
    appServer: {
      path: typeof BEALE_APP_SERVER_OPERATIONS_PATH;
      authentication: "operator-bearer";
      framing: "json";
      errors: "http-problem";
    };
    websocket: {
      path: typeof HONEYCRISP_PROTOCOL_WEBSOCKET_PATH;
      authentication: "bearer";
      framing: "json-message";
      errors: "protocol-error-message";
      correlation: "request-id";
      capabilities: typeof HONEYCRISP_PROTOCOL_WEBSOCKET_CAPABILITIES;
    };
  };
}

export interface HoneycrispTransportBootstrap {
  protocolVersion: typeof HONEYCRISP_PROTOCOL_VERSION;
  transport: "websocket";
  url: string;
  sessionId: string;
}

export interface HoneycrispClientHello {
  protocolVersion: typeof HONEYCRISP_PROTOCOL_VERSION;
  type: "client.hello";
  sessionId: string;
  client: { name: string; version: string };
}

export interface HoneycrispServerHello {
  protocolVersion: typeof HONEYCRISP_PROTOCOL_VERSION;
  type: "server.hello";
  sessionId: string;
  server: { name: typeof HONEYCRISP_PROTOCOL_NAME; version: string; buildId: string };
  contractVersion: typeof HONEYCRISP_CONTRACT_VERSION;
  schemas: HoneycrispProtocolDescriptor["schemas"];
  capabilities: typeof HONEYCRISP_PROTOCOL_WEBSOCKET_CAPABILITIES;
}

export interface HoneycrispSessionControl<TControl extends Record<string, unknown> = Record<string, unknown>> {
  protocolVersion: typeof HONEYCRISP_PROTOCOL_VERSION;
  type: "session.control";
  sessionId: string;
  requestId: string;
  control: TControl & { requestId: string };
}

export interface HoneycrispSessionEvent<TEvent extends Record<string, unknown> = Record<string, unknown>> {
  protocolVersion: typeof HONEYCRISP_PROTOCOL_VERSION;
  type: "session.event";
  sessionId: string;
  event: TEvent;
}

export interface HoneycrispWebSocketProtocolError {
  protocolVersion: typeof HONEYCRISP_PROTOCOL_VERSION;
  type: "protocol.error";
  sessionId: string;
  requestId?: string;
  error: HoneycrispProtocolErrorDetail;
  /** Retained in protocol v1 for clients that consumed the original WebSocket error shape. */
  message: string;
}

/** @deprecated Use HoneycrispWebSocketProtocolError for the complete current DTO. */
export interface HoneycrispProtocolError {
  protocolVersion: typeof HONEYCRISP_PROTOCOL_VERSION;
  type: "protocol.error";
  sessionId: string;
  message: string;
}

export type HoneycrispClientMessage = HoneycrispClientHello | HoneycrispSessionControl;
export type HoneycrispServerMessage = HoneycrispServerHello | HoneycrispSessionEvent | HoneycrispWebSocketProtocolError;

/**
 * Versioned, client-independent launch contract consumed by the Beale
 * app-server. Clients describe session intent; the app-server owns the
 * Honeycrisp hosted-runtime argument and worker-environment mapping.
 */
export const HONEYCRISP_SESSION_LAUNCH_VERSION = 2 as const;

export const HONEYCRISP_PROVIDER_RISK_ACKNOWLEDGEMENTS = [
  "openai-codex",
  "anthropic",
  "xai",
  "zai",
  "openrouter",
] as const;

export type HoneycrispProviderRiskAcknowledgement =
  (typeof HONEYCRISP_PROVIDER_RISK_ACKNOWLEDGEMENTS)[number];

export type HoneycrispProviderAuthenticationMethod = "subscription" | "api_key";

export interface HoneycrispSessionLaunchProvider {
  id?: string;
  model?: string;
  reasoningEffort?: string;
}

export interface HoneycrispSessionLaunchContinuation {
  resumeAttemptId?: string;
  resumeFromInitialAttempt?: boolean;
  fallbackPrompt: string;
}

export interface HoneycrispSessionLaunchIntent {
  /** Beale's durable workspace id, never a host filesystem path. */
  workspaceId: string;
  attemptId?: string;
  promptMarkdown: string;
  goal?: { objective?: string };
  provider?: HoneycrispSessionLaunchProvider;
  shellSafetyMode?: string;
  workflowId?: string;
  researchProfileId?: string;
  researchProfileHash?: string;
  collaboration?: Record<string, unknown>;
  continuation?: HoneycrispSessionLaunchContinuation;
  generateTitle?: boolean;
  /** Host-only endpoint used by Beale sessions to load workspace introspection tools. */
  introspection?: {
    url: string;
    token: string;
    runtimeMode?: "isolated" | "standard";
  };
}

export interface HoneycrispSessionLaunchRequest {
  launchVersion: typeof HONEYCRISP_SESSION_LAUNCH_VERSION;
  sessionId?: string;
  launch: HoneycrispSessionLaunchIntent;
}

export function decodeHoneycrispSessionLaunchRequest(value: unknown): HoneycrispSessionLaunchRequest {
  if (!isRecord(value)) throw new Error("Session request body must be a JSON object.");
  if (value.launchVersion !== HONEYCRISP_SESSION_LAUNCH_VERSION) {
    throw new Error(`launchVersion must be ${HONEYCRISP_SESSION_LAUNCH_VERSION}.`);
  }
  optionalBoundedString(value, "sessionId", 128);
  const launch = requiredRecord(value, "launch");
  requiredBoundedString(launch, "workspaceId", 256);
  optionalBoundedString(launch, "attemptId", 128);
  requiredBoundedString(launch, "promptMarkdown", 131_072);
  optionalBoundedString(launch, "shellSafetyMode", 64);
  optionalBoundedString(launch, "workflowId", 256);
  optionalBoundedString(launch, "researchProfileId", 256);
  optionalBoundedString(launch, "researchProfileHash", 256);
  if (launch.generateTitle !== undefined && typeof launch.generateTitle !== "boolean") {
    throw new Error("launch.generateTitle must be a boolean.");
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

  return value as unknown as HoneycrispSessionLaunchRequest;
}

export interface HoneycrispProtocolArguments {
  args: readonly string[];
  requestId?: string;
}

export function parseHoneycrispProtocolArguments(argv: readonly string[]): HoneycrispProtocolArguments {
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
    if (value.length > HONEYCRISP_PROTOCOL_MAX_REQUEST_ID_LENGTH) {
      throw new Error(`--request-id must not exceed ${HONEYCRISP_PROTOCOL_MAX_REQUEST_ID_LENGTH} characters.`);
    }
    requestId = value;
    index += 1;
  }
  return { args, ...(requestId ? { requestId } : {}) };
}

export function honeycrispProtocolSuccess<T>(operation: HoneycrispProtocolOperation, result: T, requestId?: string): HoneycrispProtocolSuccess<T> {
  return {
    protocol: HONEYCRISP_PROTOCOL_NAME, protocolVersion: HONEYCRISP_PROTOCOL_VERSION,
    operation, ok: true, result, ...(requestId ? { requestId } : {}),
  };
}

export function honeycrispProtocolFailure(
  operation: HoneycrispProtocolOperation,
  code: string,
  message: string,
  retryable = false,
  requestId?: string,
): HoneycrispProtocolFailure {
  return {
    protocol: HONEYCRISP_PROTOCOL_NAME, protocolVersion: HONEYCRISP_PROTOCOL_VERSION,
    operation,
    ok: false,
    error: honeycrispProtocolErrorDetail(code, message, retryable),
    ...(requestId ? { requestId } : {}),
  };
}

export function honeycrispProtocolErrorDetail(
  code: string,
  message: string,
  retryable = false,
): HoneycrispProtocolErrorDetail {
  return { code, message, retryable };
}

export function honeycrispProtocolDescriptor(): HoneycrispProtocolDescriptor {
  return {
    protocol: HONEYCRISP_PROTOCOL_NAME,
    protocolVersion: HONEYCRISP_PROTOCOL_VERSION,
    operations: HONEYCRISP_PROTOCOL_OPERATIONS,
    contractVersion: HONEYCRISP_CONTRACT_VERSION,
    runtime: {
      name: HONEYCRISP_PROTOCOL_NAME,
      version: HONEYCRISP_RUNTIME_VERSION,
      buildId: honeycrispRuntimeBuildId(),
      nodeVersion: process.version,
    },
    schemas: { protocol: 1, session: 1, memorySummary: 11, finding: 4, campaignGraph: 4, goalSuggestions: 1 },
    capabilities: HONEYCRISP_PROTOCOL_CAPABILITIES,
    transports: {
      appServer: {
        path: BEALE_APP_SERVER_OPERATIONS_PATH,
        authentication: "operator-bearer",
        framing: "json",
        errors: "http-problem",
      },
      websocket: {
        path: HONEYCRISP_PROTOCOL_WEBSOCKET_PATH,
        authentication: "bearer",
        framing: "json-message",
        errors: "protocol-error-message",
        correlation: "request-id",
        capabilities: HONEYCRISP_PROTOCOL_WEBSOCKET_CAPABILITIES,
      },
    },
  };
}

function honeycrispRuntimeBuildId(): string {
  const configured = process.env.HONEYCRISP_BUILD_ID?.trim();
  if (configured) return configured;
  try {
    return createHash("sha256").update(readFileSync(fileURLToPath(import.meta.url))).digest("hex").slice(0, 24);
  } catch {
    return `${HONEYCRISP_RUNTIME_VERSION}-unknown`;
  }
}

export function honeycrispTransportBootstrap(url: string, sessionId: string): HoneycrispTransportBootstrap {
  return { protocolVersion: HONEYCRISP_PROTOCOL_VERSION, transport: "websocket", url, sessionId };
}

export function honeycrispServerHello(sessionId: string, serverVersion: string): HoneycrispServerHello {
  return {
    protocolVersion: HONEYCRISP_PROTOCOL_VERSION,
    type: "server.hello",
    sessionId,
    server: { name: HONEYCRISP_PROTOCOL_NAME, version: serverVersion, buildId: honeycrispRuntimeBuildId() },
    contractVersion: HONEYCRISP_CONTRACT_VERSION,
    schemas: { protocol: 1, session: 1, memorySummary: 11, finding: 4, campaignGraph: 4, goalSuggestions: 1 },
    capabilities: HONEYCRISP_PROTOCOL_WEBSOCKET_CAPABILITIES,
  };
}

export function honeycrispSessionEvent<TEvent extends Record<string, unknown>>(
  sessionId: string,
  event: TEvent,
): HoneycrispSessionEvent<TEvent> {
  return { protocolVersion: HONEYCRISP_PROTOCOL_VERSION, type: "session.event", sessionId, event };
}

export function honeycrispWebSocketProtocolError(
  sessionId: string,
  code: string,
  message: string,
  requestId?: string,
): HoneycrispWebSocketProtocolError {
  return {
    protocolVersion: HONEYCRISP_PROTOCOL_VERSION,
    type: "protocol.error",
    sessionId,
    ...(requestId ? { requestId } : {}),
    error: honeycrispProtocolErrorDetail(code, message),
    message,
  };
}

export function decodeHoneycrispProtocolEnvelope(value: unknown): HoneycrispProtocolEnvelope {
  if (!isRecord(value) || value.protocol !== HONEYCRISP_PROTOCOL_NAME
    || value.protocolVersion !== HONEYCRISP_PROTOCOL_VERSION
    || !isProtocolOperation(value.operation) || typeof value.ok !== "boolean") {
    throw new Error("Invalid or unsupported Honeycrisp protocol envelope.");
  }
  validateOptionalRequestId(value.requestId);
  if (value.ok === true) {
    if (!("result" in value)) throw new Error("Honeycrisp protocol success is missing result.");
    return value as unknown as HoneycrispProtocolSuccess;
  }
  validateError(value.error);
  return value as unknown as HoneycrispProtocolFailure;
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
  if (!isRecord(value) || value.sessionLaunchVersion !== HONEYCRISP_SESSION_LAUNCH_VERSION
    || value.honeycrispProtocolVersion !== HONEYCRISP_PROTOCOL_VERSION
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
    || value.transport.protocolVersion !== HONEYCRISP_PROTOCOL_VERSION
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

export function decodeBealeAppServerSessionAttachResult(value: unknown): BealeAppServerSessionAttachResult {
  if (!isRecord(value) || value.controlVersion !== BEALE_APP_SERVER_CONTROL_VERSION
    || !isRecord(value.transport)
    || value.transport.protocolVersion !== HONEYCRISP_PROTOCOL_VERSION
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

export function decodeHoneycrispTransportBootstrap(value: unknown): HoneycrispTransportBootstrap {
  if (!isRecord(value) || value.protocolVersion !== HONEYCRISP_PROTOCOL_VERSION
    || value.transport !== "websocket" || !nonEmptyString(value.url) || !nonEmptyString(value.sessionId)) {
    throw new Error("Invalid or unsupported Honeycrisp transport bootstrap.");
  }
  return value as unknown as HoneycrispTransportBootstrap;
}

export function decodeHoneycrispClientMessage(value: unknown): HoneycrispClientMessage {
  validateMessageBase(value);
  if (value.type === "client.hello") {
    if (!isRecord(value.client) || !nonEmptyString(value.client.name) || !nonEmptyString(value.client.version)) {
      throw new Error("The client.hello message requires client name and version.");
    }
    return value as unknown as HoneycrispClientHello;
  }
  if (value.type === "session.control") {
    if (!validRequestId(value.requestId) || !isRecord(value.control)) {
      throw new Error("The session.control message requires requestId and control.");
    }
    if (value.control.requestId !== value.requestId) throw new Error("Control request IDs must match.");
    return value as unknown as HoneycrispSessionControl;
  }
  throw new Error("Unsupported Honeycrisp client message type.");
}

export function decodeHoneycrispServerMessage(value: unknown): HoneycrispServerMessage {
  validateMessageBase(value);
  if (value.type === "server.hello") {
    if (!isRecord(value.server) || value.server.name !== HONEYCRISP_PROTOCOL_NAME
      || !nonEmptyString(value.server.version) || !nonEmptyString(value.server.buildId)
      || value.contractVersion !== HONEYCRISP_CONTRACT_VERSION
      || !validSchemaDescriptor(value.schemas) || !sameCapabilities(value.capabilities)) {
      throw new Error("The server.hello message has invalid server metadata or capabilities.");
    }
    return value as unknown as HoneycrispServerHello;
  }
  if (value.type === "session.event") {
    if (!isRecord(value.event)) throw new Error("The session.event message requires an event.");
    return value as unknown as HoneycrispSessionEvent;
  }
  if (value.type === "protocol.error") {
    validateOptionalRequestId(value.requestId);
    const legacyMessage = nonEmptyString(value.message) ? value.message : undefined;
    const error = isValidError(value.error)
      ? value.error
      : legacyMessage ? { code: "protocol_error", message: legacyMessage, retryable: false } : undefined;
    if (!error) throw new Error("The protocol.error message requires a valid error.");
    return {
      protocolVersion: HONEYCRISP_PROTOCOL_VERSION,
      type: "protocol.error",
      sessionId: value.sessionId,
      ...(nonEmptyString(value.requestId) ? { requestId: value.requestId } : {}),
      error,
      message: legacyMessage ?? error.message,
    };
  }
  throw new Error("Unsupported Honeycrisp server message type.");
}

function validSchemaDescriptor(value: unknown): value is HoneycrispProtocolDescriptor["schemas"] {
  return isRecord(value) && value.protocol === 1 && value.session === 1
    && value.memorySummary === 11 && value.finding === 4 && value.campaignGraph === 4;
}

function validateMessageBase(value: unknown): asserts value is Record<string, unknown> & { sessionId: string; type: string } {
  if (!isRecord(value) || value.protocolVersion !== HONEYCRISP_PROTOCOL_VERSION
    || !nonEmptyString(value.type) || !nonEmptyString(value.sessionId)) {
    throw new Error("Invalid Honeycrisp WebSocket message.");
  }
}

function validateOptionalRequestId(value: unknown): void {
  if (value !== undefined && !validRequestId(value)) {
    throw new Error(`Honeycrisp protocol requestId must be non-empty and at most ${HONEYCRISP_PROTOCOL_MAX_REQUEST_ID_LENGTH} characters.`);
  }
}

function validateError(value: unknown): asserts value is HoneycrispProtocolErrorDetail {
  if (!isValidError(value)) throw new Error("Honeycrisp protocol failure is missing a valid error.");
}

function isValidError(value: unknown): value is HoneycrispProtocolErrorDetail {
  return isRecord(value) && nonEmptyString(value.code)
    && typeof value.message === "string" && typeof value.retryable === "boolean";
}

function sameCapabilities(value: unknown): value is typeof HONEYCRISP_PROTOCOL_WEBSOCKET_CAPABILITIES {
  return Array.isArray(value) && value.length === HONEYCRISP_PROTOCOL_WEBSOCKET_CAPABILITIES.length
    && HONEYCRISP_PROTOCOL_WEBSOCKET_CAPABILITIES.every((capability, index) => value[index] === capability);
}

function isProtocolOperation(value: unknown): value is HoneycrispProtocolOperation {
  return typeof value === "string" && (HONEYCRISP_PROTOCOL_OPERATIONS as readonly string[]).includes(value);
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
  return nonEmptyString(value) && value.length <= HONEYCRISP_PROTOCOL_MAX_REQUEST_ID_LENGTH;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}
