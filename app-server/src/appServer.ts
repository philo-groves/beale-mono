import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http';
import { randomBytes, timingSafeEqual } from 'node:crypto';
import type { AddressInfo } from 'node:net';
import type { Duplex } from 'node:stream';
import { WebSocket, WebSocketServer } from 'ws';
import {
  BEALE_APP_SERVER_CAPABILITIES,
  BEALE_APP_SERVER_CONTROL_VERSION,
  BEALE_APP_SERVER_CONTRACT_TIMESTAMP,
  BEALE_APP_SERVER_MAX_REPLAY_BYTES,
  BEALE_APP_SERVER_MAX_REPLAY_FRAMES,
  BEALE_APP_SERVER_OPERATIONS_PATH,
  BEALE_APP_SERVER_PROVIDERS_PATH,
  BEALE_APP_SERVER_SERVER_PATH,
  BEALE_APP_SERVER_SESSIONS_PATH,
  BEALE_APP_SERVER_SHUTDOWN_PATH,
  BEALE_APP_SERVER_WORKSPACES_PATH,
  HONEYCRISP_PROTOCOL_VERSION,
  HONEYCRISP_PROTOCOL_OPERATIONS,
  HONEYCRISP_SESSION_LAUNCH_VERSION,
  decodeHoneycrispClientMessage,
  decodeHoneycrispSessionLaunchRequest,
  honeycrispServerHello,
  honeycrispSessionEvent,
  type BealeAppServerDescriptor,
  type BealeAppServerHealth,
  type BealeAppServerSessionCatalog,
  type BealeAppServerSessionCatalogEntry,
  type BealeAppServerSessionAttachResult,
  type BealeAppServerSessionResult,
  type BealeAppServerSessionStartResult,
  type BealeAppServerSessionStopResult,
  type BealeAppServerShutdownResult,
  type HoneycrispSessionLaunchRequest
} from 'honeycrisp/protocol';
import {
  generateSessionToken,
  spawnHoneycrispSession,
  type HoneycrispSession,
  type SpawnHoneycrispSessionOptions
} from './honeycrispSession.js';
import {
  clearDiscoveryRecord,
  generateOperatorToken,
  operatorTokenPath,
  readOrCreateOperatorToken,
  writeDiscoveryRecord,
  type AppServerDiscoveryRecord,
  type AppServerHostMode
} from './discovery.js';
import { prepareHoneycrispSessionLaunch } from './sessionLaunch.js';
import {
  AppServerHostService,
  type AppServerStartupRecoveryResult,
  type PreparedAppServerSession
} from './hostService.js';
import {
  DEFAULT_LONG_SESSION_RECOVERY_ATTEMPTS,
  inspectHoneycrispSessionCompletion,
  longSessionRecoveryDelayMs,
  longSessionRecoveryFallbackPrompt
} from './sessionRecovery.js';

const DEFAULT_HOST = '127.0.0.1';
const MAX_REQUEST_BODY_BYTES = 524_288;
const MAX_FRAME_BYTES = 1_048_576;
const SESSION_ID_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;
const MAX_RETAINED_TERMINAL_SESSIONS = 50;
const MAX_ERROR_DETAIL_CHARS = 1_000;
export const APP_SERVER_CAPABILITIES = BEALE_APP_SERVER_CAPABILITIES;

export interface AppServerOptions {
  host?: string;
  port?: number;
  publicUrl?: string;
  operatorToken?: string;
  discoveryFile?: string;
  hostMode?: AppServerHostMode;
  onChange?: () => void;
  onShutdownRequested?: () => void;
  hostService?: AppServerHostService;
  spawnSession?: (options: SpawnHoneycrispSessionOptions) => Promise<HoneycrispSession>;
  recoverInterruptedOnStart?: boolean;
  automationScheduler?: false | {
    scanIntervalMs?: number;
    now?: () => Date;
  };
  longSessionRecovery?: false | {
    maxAttempts?: number;
    delayMs?: (recoveryNumber: number) => number;
  };
}

export type SessionStartRequest = HoneycrispSessionLaunchRequest;

export type StartedSession = BealeAppServerSessionStartResult;
export type SessionCatalogEntry = BealeAppServerSessionCatalogEntry;

export interface AppServerHandle {
  host: string;
  port: number;
  url: string;
  operatorToken: string;
  startSession(request: SessionStartRequest): Promise<StartedSession>;
  recoverInterruptedSessions(): Promise<AppServerStartupRecoverySummary>;
  listSessions(): SessionCatalogEntry[];
  stopSession(sessionId: string): boolean;
  close(): Promise<void>;
}

export interface AppServerStartupRecoverySummary {
  interruptedSessions: number;
  startedSessions: number;
  skippedSessions: number;
  failedSessions: number;
  errors: string[];
}

export class HttpError extends Error {
  readonly status: number;
  readonly code: string | undefined;
  readonly retryable: boolean | undefined;

  constructor(status: number, message: string, options: { code?: string; retryable?: boolean } = {}) {
    super(message);
    this.name = 'HttpError';
    this.status = status;
    this.code = options.code;
    this.retryable = options.retryable;
  }
}

type SessionState = SessionCatalogEntry['state'];

interface SessionRuntime {
  readonly sessionId: string;
  readonly request: HoneycrispSessionLaunchRequest;
  readonly clientTokens: Set<string>;
  readonly startedAt: string;
  session: HoneycrispSession | null;
  readonly clientSockets: Set<WebSocket>;
  readonly readyClientSockets: Set<WebSocket>;
  readonly pendingClientFrames: Buffer[];
  pendingClientBytes: number;
  droppedClientFrames: number;
  readonly pendingControls: Record<string, unknown>[];
  handshakeFrame: Buffer | null;
  state: SessionState;
  endedAt: string | null;
  exitCode: number | null;
  stopRequested: boolean;
  diagnostic: string | null;
  unsubscribeSessionEvents: (() => void) | null;
  currentAttemptId: string;
  currentAttemptWasInitial: boolean;
  recoveryCount: number;
  recoveryTimer: NodeJS.Timeout | null;
}

function isTerminal(state: SessionState): boolean {
  return state === 'completed' || state === 'failed' || state === 'stopped';
}

export async function startAppServer(options: AppServerOptions = {}): Promise<AppServerHandle> {
  const host = options.host ?? DEFAULT_HOST;
  const publicUrl = options.publicUrl ? normalizePublicUrl(options.publicUrl) : null;
  const operatorToken = options.operatorToken?.trim()
    || (options.discoveryFile
      ? readOrCreateOperatorToken(operatorTokenPath(options.discoveryFile))
      : generateOperatorToken());
  const hostService = options.hostService ?? new AppServerHostService();
  const spawnSession = options.spawnSession ?? spawnHoneycrispSession;
  const recoveryOptions = options.longSessionRecovery === false
    ? null
    : options.longSessionRecovery ?? {};
  const maxRecoveryAttempts = recoveryOptions
    ? boundedRecoveryAttempts(recoveryOptions.maxAttempts)
    : 0;
  const recoveryDelay = recoveryOptions?.delayMs ?? longSessionRecoveryDelayMs;
  const automationScheduler = options.automationScheduler === false
    ? null
    : options.automationScheduler ?? {};
  const sessions = new Map<string, SessionRuntime>();
  let discoveryRecord: AppServerDiscoveryRecord | null = null;
  let automationTimer: NodeJS.Timeout | null = null;
  let automationScanInProgress = false;
  let closing = false;

  const notifyChange = (): void => {
    try {
      options.onChange?.();
    } catch {
      // Host change callbacks must never break server bookkeeping.
    }
  };

  const httpServer: Server = createServer((request, response) => {
    handleHttpRequest(request, response).catch((error: unknown) => {
      respondWithError(response, error);
    });
  });
  httpServer.on('clientError', (_error, socket) => {
    socket.destroy();
  });

  const wss = new WebSocketServer({ noServer: true, maxPayload: MAX_FRAME_BYTES });
  httpServer.on('upgrade', (request, socket, head) => {
    try {
      const runtime = authenticateUpgrade(request);
      if (!runtime) {
        rejectUpgrade(socket, 404, 'Unknown session.');
        return;
      }
      if (isTerminal(runtime.state)) {
        rejectUpgrade(socket, 410, 'This session has already ended.');
        return;
      }
      wss.handleUpgrade(request, socket, head, (clientSocket) => {
        attachFacadeClient(runtime, clientSocket);
      });
    } catch {
      rejectUpgrade(socket, 401, 'A valid bearer token is required.');
    }
  });

  await new Promise<void>((resolve, reject) => {
    httpServer.once('error', reject);
    httpServer.listen(options.port ?? 0, host, () => resolve());
  });
  const address = httpServer.address() as AddressInfo;
  const localUrl = `http://${urlHost(host)}:${address.port}`;
  const baseUrl = publicUrl
    ? publicUrl
    : localUrl;

  if (options.recoverInterruptedOnStart) {
    await recoverInterruptedSessions();
  }

  if (options.discoveryFile) {
    discoveryRecord = {
      version: 1,
      contractTimestamp: BEALE_APP_SERVER_CONTRACT_TIMESTAMP,
      ...(options.hostMode ? { hostMode: options.hostMode } : {}),
      pid: process.pid,
      host,
      port: address.port,
      localUrl,
      url: baseUrl,
      operatorToken,
      startedAt: new Date().toISOString()
    };
    writeDiscoveryRecord(discoveryRecord, options.discoveryFile);
  }

  if (automationScheduler) {
    const scanIntervalMs = boundedAutomationScanInterval(automationScheduler.scanIntervalMs);
    automationTimer = setInterval(() => void scanDueAutomations(), scanIntervalMs);
    automationTimer.unref();
    setImmediate(() => void scanDueAutomations());
  }

  function requireOperator(request: IncomingMessage): void {
    if (!authorizedBearer(request.headers.authorization, operatorToken)) {
      throw new HttpError(401, 'An operator bearer token is required for this operation.');
    }
  }

  function healthResponse(): BealeAppServerHealth {
    return {
      ok: true,
      controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
      contractTimestamp: BEALE_APP_SERVER_CONTRACT_TIMESTAMP,
      capabilities: BEALE_APP_SERVER_CAPABILITIES
    };
  }

  function descriptorResponse(): BealeAppServerDescriptor {
    return {
      ...healthResponse(),
      sessionLaunchVersion: HONEYCRISP_SESSION_LAUNCH_VERSION,
      honeycrispProtocolVersion: HONEYCRISP_PROTOCOL_VERSION,
      endpoints: {
        sessions: BEALE_APP_SERVER_SESSIONS_PATH,
        workspaces: BEALE_APP_SERVER_WORKSPACES_PATH,
        providers: BEALE_APP_SERVER_PROVIDERS_PATH,
        operations: BEALE_APP_SERVER_OPERATIONS_PATH,
        shutdown: BEALE_APP_SERVER_SHUTDOWN_PATH
      },
      limits: {
        requestBodyBytes: MAX_REQUEST_BODY_BYTES,
        frameBytes: MAX_FRAME_BYTES,
        replayBytes: BEALE_APP_SERVER_MAX_REPLAY_BYTES,
        replayFrames: BEALE_APP_SERVER_MAX_REPLAY_FRAMES
      }
    };
  }

  async function handleHttpRequest(request: IncomingMessage, response: ServerResponse): Promise<void> {
    const url = new URL(request.url ?? '/', 'http://localhost');
    if (request.method === 'GET' && url.pathname === '/health') {
      sendJson(response, 200, healthResponse());
      return;
    }
    requireOperator(request);
    if (request.method === 'GET' && url.pathname === BEALE_APP_SERVER_PROVIDERS_PATH) {
      sendJson(response, 200, hostService.providerCatalog());
      return;
    }
    if (request.method === 'GET' && url.pathname === BEALE_APP_SERVER_SERVER_PATH) {
      sendJson(response, 200, descriptorResponse());
      return;
    }
    if (request.method === 'GET' && url.pathname === BEALE_APP_SERVER_SESSIONS_PATH) {
      const catalog: BealeAppServerSessionCatalog = {
        controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
        sessions: listSessions()
      };
      sendJson(response, 200, catalog);
      return;
    }
    if (request.method === 'GET' && url.pathname === BEALE_APP_SERVER_WORKSPACES_PATH) {
      sendJson(response, 200, hostService.listWorkspaces());
      return;
    }
    if (request.method === 'POST' && url.pathname === BEALE_APP_SERVER_OPERATIONS_PATH) {
      const body = await readJsonBody(request);
      if (!isRecord(body) || typeof body.operation !== 'string'
        || !HONEYCRISP_PROTOCOL_OPERATIONS.includes(body.operation as never)) {
        throw new HttpError(400, 'A supported Honeycrisp operation is required.');
      }
      if (body.args !== undefined && (!Array.isArray(body.args) || body.args.some((value) => typeof value !== 'string'))) {
        throw new HttpError(400, 'Operation args must be an array of strings.');
      }
      const controller = new AbortController();
      const abortDisconnectedOperation = (): void => {
        if (!response.writableEnded) controller.abort();
      };
      response.once('close', abortDisconnectedOperation);
      try {
        sendJson(response, 200, {
          controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
          result: await hostCall(() => hostService.executeOperation({
            operation: body.operation as (typeof HONEYCRISP_PROTOCOL_OPERATIONS)[number],
            ...(Array.isArray(body.args) ? { args: body.args as string[] } : {}),
            ...(body.input !== undefined ? { input: body.input } : {}),
            ...(typeof body.profileId === 'string' && body.profileId.trim() ? { profileId: body.profileId.trim() } : {}),
            signal: controller.signal
          }))
        });
      } finally {
        response.off('close', abortDisconnectedOperation);
      }
      return;
    }
    const workspaceMemoryMatch = request.method === 'GET'
      ? /^\/v1\/workspaces\/([^/]+)\/memory$/.exec(url.pathname)
      : null;
    if (workspaceMemoryMatch) {
      sendJson(response, 200, await hostCall(() => hostService.workspaceMemory(pathPart(workspaceMemoryMatch, 1))));
      return;
    }
    const workspaceMemoryNotificationsMatch = request.method === 'GET'
      ? /^\/v1\/workspaces\/([^/]+)\/memory-notifications$/.exec(url.pathname)
      : null;
    if (workspaceMemoryNotificationsMatch) {
      sendJson(response, 200, await hostCall(() => hostService.workspaceMemoryNotifications(
        pathPart(workspaceMemoryNotificationsMatch, 1),
        url.searchParams.get('sessionId') || undefined
      )));
      return;
    }
    const workspaceSessionsMatch = request.method === 'GET'
      ? /^\/v1\/workspaces\/([^/]+)\/sessions$/.exec(url.pathname)
      : null;
    if (workspaceSessionsMatch) {
      sendJson(response, 200, await hostCall(() => hostService.workspaceSessions(
        pathPart(workspaceSessionsMatch, 1),
        queryInteger(url, 'limit', 200)
      )));
      return;
    }
    const workspaceChannelsMatch = /^\/v1\/workspaces\/([^/]+)\/channels$/.exec(url.pathname);
    if (workspaceChannelsMatch) {
      const workspaceId = pathPart(workspaceChannelsMatch, 1);
      if (request.method === 'GET') {
        sendJson(response, 200, await hostCall(() => hostService.workspaceChannels(
          workspaceId,
          queryInteger(url, 'limit', 200),
          queryBoolean(url, 'archived')
        )));
        return;
      }
      if (request.method === 'POST') {
        const body = await readJsonBody(request);
        sendJson(response, 201, await hostCall(() => hostService.createWorkspaceChannel(
          workspaceId,
          isRecord(body) ? body : {}
        )));
        return;
      }
    }
    const workspaceChannelArchiveMatch = /^\/v1\/workspaces\/([^/]+)\/channels\/([^/]+)\/(archive|restore)$/.exec(url.pathname);
    if (workspaceChannelArchiveMatch && request.method === 'POST') {
      const workspaceId = pathPart(workspaceChannelArchiveMatch, 1);
      const channel = pathPart(workspaceChannelArchiveMatch, 2);
      const action = workspaceChannelArchiveMatch[3];
      sendJson(response, 200, await hostCall(() => action === 'archive'
        ? hostService.archiveWorkspaceChannel(workspaceId, channel)
        : hostService.restoreWorkspaceChannel(workspaceId, channel)));
      return;
    }
    const workspaceChannelMatch = /^\/v1\/workspaces\/([^/]+)\/channels\/([^/]+)$/.exec(url.pathname);
    if (workspaceChannelMatch) {
      const workspaceId = pathPart(workspaceChannelMatch, 1);
      const channel = pathPart(workspaceChannelMatch, 2);
      if (request.method === 'GET') {
        sendJson(response, 200, await hostCall(() => hostService.workspaceChannel(
          workspaceId,
          channel,
          queryInteger(url, 'messageLimit', 500)
        )));
        return;
      }
      if (request.method === 'POST') {
        const body = await readJsonBody(request);
        sendJson(response, 201, await hostCall(() => hostService.postWorkspaceChannelMessage(
          workspaceId,
          channel,
          isRecord(body) ? body : {}
        )));
        return;
      }
      if (request.method === 'DELETE') {
        sendJson(response, 200, await hostCall(() => hostService.deleteWorkspaceChannel(workspaceId, channel)));
        return;
      }
    }
    const canonicalSessionMatch = /^\/v1\/workspaces\/([^/]+)\/sessions\/([^/]+)\/(update|events|collaboration|captures|event-details)$/.exec(url.pathname);
    if (canonicalSessionMatch) {
      const workspaceId = pathPart(canonicalSessionMatch, 1);
      const sessionId = pathPart(canonicalSessionMatch, 2);
      const operation = canonicalSessionMatch[3];
      if (request.method === 'GET' && operation === 'update') {
        sendJson(response, 200, await hostCall(() => hostService.sessionUpdate(workspaceId, sessionId, {
          ...(url.searchParams.get('afterEventId') ? { afterEventId: url.searchParams.get('afterEventId')! } : {}),
          tail: queryBoolean(url, 'tail'),
          limit: queryInteger(url, 'limit', 200),
          maxBytes: queryInteger(url, 'maxBytes', 1_000_000)
        })));
        return;
      }
      if (request.method === 'GET' && operation === 'events') {
        sendJson(response, 200, await hostCall(() => hostService.sessionEvents(workspaceId, sessionId, {
          ...(url.searchParams.get('stream') ? { stream: url.searchParams.get('stream')! } : {}),
          ...(url.searchParams.get('afterEventId') ? { afterEventId: url.searchParams.get('afterEventId')! } : {}),
          tail: queryBoolean(url, 'tail'),
          limit: queryInteger(url, 'limit', 200),
          maxBytes: queryInteger(url, 'maxBytes', 1_000_000)
        })));
        return;
      }
      if (request.method === 'GET' && operation === 'collaboration') {
        sendJson(response, 200, await hostCall(() => hostService.sessionCollaboration(
          workspaceId,
          sessionId,
          queryInteger(url, 'messageLimit', 200)
        )));
        return;
      }
      if (request.method === 'GET' && operation === 'captures') {
        sendJson(response, 200, await hostCall(() => hostService.sessionCaptures(workspaceId, sessionId)));
        return;
      }
      if (request.method === 'POST' && operation === 'event-details') {
        const body = await readJsonBody(request);
        const eventIds = isRecord(body) && Array.isArray(body.eventIds)
          ? body.eventIds.filter((value): value is string => typeof value === 'string' && Boolean(value.trim()))
          : [];
        sendJson(response, 200, await hostCall(() => hostService.sessionEventDetails(
          workspaceId,
          sessionId,
          eventIds
        )));
        return;
      }
    }
    if (request.method === 'POST' && url.pathname === BEALE_APP_SERVER_SESSIONS_PATH) {
      const body = await readJsonBody(request);
      const started = await startSession(body);
      sendJson(response, 201, started);
      return;
    }
    if (request.method === 'POST' && url.pathname === BEALE_APP_SERVER_SHUTDOWN_PATH) {
      if (!options.onShutdownRequested) {
        throw new HttpError(501, 'This app-server host does not support control-plane shutdown.');
      }
      const activeSessions = [...sessions.values()].filter((runtime) => !isTerminal(runtime.state));
      if (activeSessions.length > 0) {
        throw new HttpError(
          409,
          `The Beale app-server cannot restart while ${activeSessions.length} research ${activeSessions.length === 1 ? 'session is' : 'sessions are'} active.`,
          { code: 'sessions_active', retryable: true }
        );
      }
      const result: BealeAppServerShutdownResult = {
        controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
        shuttingDown: true
      };
      sendJson(response, 202, result);
      setImmediate(() => options.onShutdownRequested?.());
      return;
    }
    const attachmentMatch = /^\/v1\/sessions\/([^/]+)\/attachments$/.exec(url.pathname);
    if (attachmentMatch && request.method === 'POST') {
      const sessionId = decodeURIComponent(attachmentMatch[1] ?? '');
      const runtime = sessions.get(sessionId);
      if (!runtime) throw new HttpError(404, `Unknown session: ${sessionId}`);
      if (isTerminal(runtime.state)) throw new HttpError(410, `Session ${sessionId} has already ended.`);
      const token = generateSessionToken();
      runtime.clientTokens.add(token);
      const result: BealeAppServerSessionAttachResult = {
        controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
        session: catalogEntry(runtime),
        transport: sessionTransport(runtime, token)
      };
      sendJson(response, 201, result);
      return;
    }
    const sessionMatch = /^\/v1\/sessions\/([^/]+)$/.exec(url.pathname);
    if (sessionMatch && request.method === 'GET') {
      const sessionId = decodeURIComponent(sessionMatch[1] ?? '');
      const session = sessionEntry(sessionId);
      if (!session) throw new HttpError(404, `Unknown session: ${sessionId}`);
      const result: BealeAppServerSessionResult = {
        controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
        session
      };
      sendJson(response, 200, result);
      return;
    }
    if (sessionMatch && request.method === 'DELETE') {
      const sessionId = decodeURIComponent(sessionMatch[1] ?? '');
      const stopped = stopSession(sessionId);
      if (stopped === null) {
        throw new HttpError(404, `Unknown session: ${sessionId}`);
      }
      const result: BealeAppServerSessionStopResult = {
        controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
        stopped,
        sessionId
      };
      sendJson(response, stopped ? 202 : 200, result);
      return;
    }
    throw new HttpError(404, 'Not found.');
  }

  function normalizeSessionRequest(input: unknown): { sessionId: string; request: HoneycrispSessionLaunchRequest } {
    let request: HoneycrispSessionLaunchRequest;
    try {
      request = decodeHoneycrispSessionLaunchRequest(input);
    } catch (error) {
      throw new HttpError(400, error instanceof Error ? error.message : 'Invalid session launch request.');
    }
    const sessionId = request.sessionId
      ? request.sessionId
      : `session-${randomBytes(8).toString('hex')}`;
    if (!SESSION_ID_PATTERN.test(sessionId)) {
      throw new HttpError(400, 'sessionId must match [A-Za-z0-9][A-Za-z0-9._-]{0,127}.');
    }
    return { sessionId, request };
  }

  async function startSession(input: unknown): Promise<StartedSession> {
    const normalized = normalizeSessionRequest(input);
    const prepared = await hostCall(() => hostService.prepareSession(
      normalized.request,
      normalized.sessionId
    ));
    const { sessionId, attemptId } = prepared;
    const existing = sessions.get(sessionId);
    if (existing && !isTerminal(existing.state)) {
      throw new HttpError(409, `Session ${sessionId} already exists.`);
    }
    if (existing) {
      sessions.delete(sessionId);
    }
    const runtime = createSessionRuntime(normalized.request, prepared);
    sessions.set(sessionId, runtime);
    evictOldestTerminalSessions();
    try {
      await launchPreparedSession(runtime, prepared, normalized.request.launch.continuation === undefined);
      notifyChange();
      return {
        controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
        session: sessionEntry(sessionId)!,
        attemptId,
        transport: sessionTransport(runtime, [...runtime.clientTokens][0]!)
      };
    } catch (error) {
      runtime.state = 'failed';
      runtime.endedAt = new Date().toISOString();
      notifyChange();
      const detail = error instanceof Error ? error.message : String(error);
      runtime.diagnostic = boundedDiagnostic(detail);
      throw new HttpError(502, `Honeycrisp session failed to start: ${detail}`);
    }
  }

  async function scanDueAutomations(): Promise<void> {
    if (closing || automationScanInProgress) return;
    const service = hostService as AppServerHostService & {
      dueAutomations?: (at?: Date) => Promise<Array<{ request: HoneycrispSessionLaunchRequest }>>;
    };
    if (typeof service.dueAutomations !== 'function') return;
    automationScanInProgress = true;
    try {
      const due = await service.dueAutomations(automationScheduler?.now?.() ?? new Date());
      for (const automation of due) {
        if (closing) break;
        const sessionId = automation.request.sessionId;
        if (!sessionId) continue;
        const runtime = sessions.get(sessionId);
        if (runtime && !isTerminal(runtime.state)) continue;
        try {
          await startSession(automation.request);
        } catch {
          // A later scan retries transient preparation or provider failures.
        }
      }
    } finally {
      automationScanInProgress = false;
    }
  }

  function createSessionRuntime(
    request: HoneycrispSessionLaunchRequest,
    prepared: PreparedAppServerSession
  ): SessionRuntime {
    return {
      sessionId: prepared.sessionId,
      request,
      clientTokens: new Set([generateSessionToken()]),
      startedAt: new Date().toISOString(),
      session: null,
      clientSockets: new Set(),
      readyClientSockets: new Set(),
      pendingClientFrames: [],
      pendingClientBytes: 0,
      droppedClientFrames: 0,
      pendingControls: [],
      handshakeFrame: null,
      state: 'starting',
      endedAt: null,
      exitCode: null,
      stopRequested: false,
      diagnostic: null,
      unsubscribeSessionEvents: null,
      currentAttemptId: prepared.attemptId,
      currentAttemptWasInitial: false,
      recoveryCount: 0,
      recoveryTimer: null
    };
  }

  async function recoverInterruptedSessions(): Promise<AppServerStartupRecoverySummary> {
    const service = hostService as AppServerHostService & {
      recoverInterruptedSessions?: () => Promise<AppServerStartupRecoveryResult>;
    };
    if (!service.recoverInterruptedSessions) {
      return {
        interruptedSessions: 0,
        startedSessions: 0,
        skippedSessions: 0,
        failedSessions: 0,
        errors: []
      };
    }
    const result = await service.recoverInterruptedSessions();
    let startedSessions = 0;
    let failedSessions = 0;
    const errors = [...result.errors];
    for (const candidate of result.recovered) {
      const { prepared, request } = candidate;
      if (sessions.has(prepared.sessionId)) {
        failedSessions += 1;
        errors.push(`${prepared.sessionId}: an app-server runtime already owns this session.`);
        continue;
      }
      const runtime = createSessionRuntime(request, prepared);
      sessions.set(prepared.sessionId, runtime);
      try {
        await launchPreparedSession(runtime, prepared, false);
        emitStartupRecoveryCommentary(runtime);
        startedSessions += 1;
      } catch (error) {
        failedSessions += 1;
        finishRuntime(
          runtime,
          'failed',
          null,
          `Honeycrisp startup recovery failed to launch: ${error instanceof Error ? error.message : String(error)}`
        );
      }
    }
    evictOldestTerminalSessions();
    notifyChange();
    return {
      interruptedSessions: result.interruptedSessions,
      startedSessions,
      skippedSessions: result.skippedSessions,
      failedSessions,
      errors
    };
  }

  function emitStartupRecoveryCommentary(runtime: SessionRuntime): void {
    const event = {
      schemaVersion: 1,
      kind: 'model.output',
      timestamp: new Date().toISOString(),
      payload: {
        phase: 'completed',
        messagePhase: 'commentary',
        text: 'The previous app-server session ended unexpectedly. I restored its durable attempt state and am continuing automatically.',
        agentPath: '/root',
        responseId: `startup-recovery-${runtime.sessionId}`,
        itemId: 'text:0'
      }
    };
    deliverClientFrame(runtime, Buffer.from(JSON.stringify(honeycrispSessionEvent(runtime.sessionId, event))));
  }

  async function launchPreparedSession(
    runtime: SessionRuntime,
    prepared: PreparedAppServerSession,
    attemptWasInitial = false
  ): Promise<void> {
    const { args, env } = prepareHoneycrispSessionLaunch(prepared.launch);
    const session = await spawnSession({ sessionId: runtime.sessionId, args, env });
    if (runtime.stopRequested || sessions.get(runtime.sessionId) !== runtime) {
      session.stop();
      return;
    }
    runtime.session = session;
    runtime.currentAttemptId = prepared.attemptId;
    runtime.currentAttemptWasInitial = attemptWasInitial;
    runtime.handshakeFrame ??= Buffer.from(JSON.stringify(honeycrispServerHello(runtime.sessionId, '0.1.0')));
    runtime.unsubscribeSessionEvents = session.onEvent((event) => {
      observeSessionControlState(runtime, event);
      deliverClientFrame(runtime, Buffer.from(JSON.stringify(honeycrispSessionEvent(runtime.sessionId, event))));
    });
    runtime.state = 'running';
    runtime.endedAt = null;
    runtime.exitCode = null;
    runtime.diagnostic = null;
    for (const control of runtime.pendingControls.splice(0)) session.sendControl(control);
    void session.waitExit().then((result) => handleSessionExit(runtime, session, prepared, result));
  }

  function observeSessionControlState(runtime: SessionRuntime, event: Record<string, unknown>): void {
    if (event.kind !== 'agent.event' || !isRecord(event.payload)) return;
    if (event.payload.eventType !== 'control.received' || event.payload.accepted !== true) return;
    const type = event.payload.type;
    if (type !== 'pause' && type !== 'resume' && type !== 'stop') return;
    if (type === 'stop') runtime.stopRequested = true;
    void recordSessionControlState(
      runtime,
      type === 'pause' ? 'paused' : type === 'resume' ? 'active' : 'stopped'
    );
  }

  async function recordSessionControlState(
    runtime: SessionRuntime,
    state: 'active' | 'paused' | 'stopped'
  ): Promise<void> {
    const service = hostService as AppServerHostService & {
      recordSessionControlState?: (input: {
        request: HoneycrispSessionLaunchRequest;
        sessionId: string;
        attemptId: string;
        state: 'active' | 'paused' | 'stopped';
      }) => Promise<void>;
    };
    try {
      await service.recordSessionControlState?.({
        request: runtime.request,
        sessionId: runtime.sessionId,
        attemptId: runtime.currentAttemptId,
        state
      });
    } catch (error) {
      runtime.diagnostic = boundedDiagnostic(
        `Could not persist ${state} session control state: ${error instanceof Error ? error.message : String(error)}`
      );
      notifyChange();
    }
  }

  async function handleSessionExit(
    runtime: SessionRuntime,
    session: HoneycrispSession,
    prepared: PreparedAppServerSession,
    result: { code: number | null; stderr: string }
  ): Promise<void> {
    if (runtime.session !== session || sessions.get(runtime.sessionId) !== runtime) return;
    runtime.unsubscribeSessionEvents?.();
    runtime.unsubscribeSessionEvents = null;
    runtime.session = null;
    const completion = await inspectHoneycrispSessionCompletion({
      code: result.code,
      stderr: result.stderr,
      capturePath: prepared.launch.capturePath,
      stopRequested: runtime.stopRequested
    });
    if (sessions.get(runtime.sessionId) !== runtime) return;
    if (runtime.stopRequested) {
      finishRuntime(runtime, 'stopped', result.code, null);
      return;
    }
    if (completion.succeeded) {
      finishRuntime(runtime, 'completed', 0, null);
      return;
    }
    if (completion.recoverable && runtime.recoveryCount < maxRecoveryAttempts) {
      runtime.recoveryCount += 1;
      const recoveryNumber = runtime.recoveryCount;
      const delayMs = Math.max(0, recoveryDelay(recoveryNumber));
      emitRecoveryCommentary(runtime, recoveryNumber, maxRecoveryAttempts);
      notifyChange();
      runtime.recoveryTimer = setTimeout(() => {
        runtime.recoveryTimer = null;
        void recoverSession(runtime, prepared, completion.diagnostic ?? 'Unexpected Honeycrisp worker failure.');
      }, delayMs);
      runtime.recoveryTimer.unref();
      return;
    }
    finishRuntime(runtime, 'failed', result.code === 0 ? 1 : result.code, completion.diagnostic);
  }

  async function recoverSession(
    runtime: SessionRuntime,
    previous: PreparedAppServerSession,
    diagnostic: string
  ): Promise<void> {
    if (runtime.stopRequested || sessions.get(runtime.sessionId) !== runtime) {
      if (sessions.get(runtime.sessionId) === runtime) finishRuntime(runtime, 'stopped', null, null);
      return;
    }
    const fallbackPrompt = longSessionRecoveryFallbackPrompt(runtime.request.launch.promptMarkdown, diagnostic);
    try {
      const recoveryInput = {
        request: runtime.request,
        sessionId: runtime.sessionId,
        previousAttemptId: previous.attemptId,
        previousAttemptWasInitial: runtime.currentAttemptWasInitial,
        fallbackPrompt
      };
      const service = hostService as AppServerHostService & {
        prepareSessionRecovery?: (input: typeof recoveryInput) => Promise<PreparedAppServerSession>;
      };
      const prepared = service.prepareSessionRecovery
        ? await service.prepareSessionRecovery(recoveryInput)
        : await service.prepareSession({
            ...runtime.request,
            sessionId: runtime.sessionId,
            launch: {
              ...runtime.request.launch,
              attemptId: `attempt-${randomBytes(8).toString('hex')}`,
              generateTitle: false,
              continuation: {
                resumeAttemptId: previous.attemptId,
                resumeFromInitialAttempt: runtime.currentAttemptWasInitial,
                fallbackPrompt
              }
            }
          }, runtime.sessionId);
      if (runtime.stopRequested || sessions.get(runtime.sessionId) !== runtime) return;
      await launchPreparedSession(runtime, prepared, false);
      notifyChange();
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      finishRuntime(
        runtime,
        runtime.stopRequested ? 'stopped' : 'failed',
        null,
        runtime.stopRequested ? null : `Honeycrisp recovery failed to start: ${detail}`
      );
    }
  }

  function finishRuntime(
    runtime: SessionRuntime,
    state: Extract<SessionState, 'completed' | 'failed' | 'stopped'>,
    exitCode: number | null,
    diagnostic: string | null
  ): void {
    if (runtime.recoveryTimer) clearTimeout(runtime.recoveryTimer);
    runtime.recoveryTimer = null;
    runtime.endedAt = new Date().toISOString();
    runtime.exitCode = exitCode;
    runtime.state = state;
    runtime.diagnostic = state === 'failed' ? boundedDiagnostic(diagnostic ?? '') : null;
    teardownRuntime(runtime, state === 'completed' ? 1000 : 1011);
    notifyChange();
  }

  function emitRecoveryCommentary(
    runtime: SessionRuntime,
    recoveryNumber: number,
    maximum: number
  ): void {
    const event = {
      schemaVersion: 1,
      kind: 'model.output',
      timestamp: new Date().toISOString(),
      payload: {
        phase: 'completed',
        messagePhase: 'commentary',
        text: `The provider session ended unexpectedly. I’m restoring the durable attempt state and continuing automatically (recovery ${recoveryNumber} of ${maximum}).`,
        agentPath: '/root',
        responseId: `session-recovery-${runtime.sessionId}-${recoveryNumber}`,
        itemId: 'text:0'
      }
    };
    deliverClientFrame(runtime, Buffer.from(JSON.stringify(honeycrispSessionEvent(runtime.sessionId, event))));
  }

  /**
   * Returns true when a running session was stopped, false when a retained
   * terminal record was removed, or null when the id is unknown.
   */
  function stopSession(sessionId: string): boolean | null {
    const runtime = sessions.get(sessionId);
    if (!runtime) return null;
    if (isTerminal(runtime.state)) {
      sessions.delete(sessionId);
      notifyChange();
      return false;
    }
    runtime.stopRequested = true;
    void recordSessionControlState(runtime, 'stopped');
    if (!runtime.session) {
      finishRuntime(runtime, 'stopped', null, null);
      return true;
    }
    runtime.session?.stop();
    return true;
  }

  function listSessions(): SessionCatalogEntry[] {
    return [...sessions.values()]
      .sort((a, b) => b.startedAt.localeCompare(a.startedAt))
      .map(catalogEntry);
  }

  function sessionEntry(sessionId: string): SessionCatalogEntry | null {
    const runtime = sessions.get(sessionId);
    return runtime ? catalogEntry(runtime) : null;
  }

  function catalogEntry(runtime: SessionRuntime): SessionCatalogEntry {
    return {
      sessionId: runtime.sessionId,
      state: runtime.state,
      startedAt: runtime.startedAt,
      endedAt: runtime.endedAt,
      exitCode: runtime.exitCode,
      diagnostic: runtime.diagnostic,
      clientConnected: [...runtime.readyClientSockets].some((socket) => socket.readyState === WebSocket.OPEN),
      replay: {
        bufferedFrames: runtime.pendingClientFrames.length,
        bufferedBytes: runtime.pendingClientBytes,
        droppedFrames: runtime.droppedClientFrames
      }
    };
  }

  function evictOldestTerminalSessions(): void {
    const terminal = [...sessions.values()]
      .filter((runtime) => isTerminal(runtime.state))
      .sort((a, b) => (a.endedAt ?? '').localeCompare(b.endedAt ?? ''));
    while (terminal.length > MAX_RETAINED_TERMINAL_SESSIONS) {
      const oldest = terminal.shift();
      if (!oldest) break;
      sessions.delete(oldest.sessionId);
    }
  }

  function authenticateUpgrade(request: IncomingMessage): SessionRuntime | null {
    const url = new URL(request.url ?? '/', 'http://localhost');
    const match = /^\/v1\/sessions\/([^/]+)\/transport$/.exec(url.pathname);
    if (!match) return null;
    const runtime = sessions.get(decodeURIComponent(match[1] ?? ''));
    if (!runtime) return null;
    if (![...runtime.clientTokens].some((token) => authorizedBearer(request.headers.authorization, token))) {
      throw new Error('invalid token');
    }
    return runtime;
  }

  function attachFacadeClient(runtime: SessionRuntime, clientSocket: WebSocket): void {
    runtime.clientSockets.add(clientSocket);
    let receivedClientHello = false;
    notifyChange();
    clientSocket.on('close', () => {
      runtime.readyClientSockets.delete(clientSocket);
      if (runtime.clientSockets.delete(clientSocket)) {
        notifyChange();
      }
    });
    clientSocket.on('message', (data: unknown) => {
      const frame = toBuffer(data);
      if (frame.byteLength > MAX_FRAME_BYTES) {
        clientSocket.close(1009);
        return;
      }
      receivedClientHello = handleDirectClientFrame(runtime, clientSocket, frame, receivedClientHello);
    });
    clientSocket.on('error', () => clientSocket.terminate());
  }

  function handleDirectClientFrame(
    runtime: SessionRuntime,
    clientSocket: WebSocket,
    frame: Buffer,
    receivedClientHello: boolean
  ): boolean {
    let message;
    try {
      message = decodeHoneycrispClientMessage(JSON.parse(frame.toString('utf8')) as unknown);
    } catch {
      clientSocket.close(1002, 'invalid protocol message');
      return receivedClientHello;
    }
    if (message.sessionId !== runtime.sessionId) {
      clientSocket.close(1002, 'session mismatch');
      return receivedClientHello;
    }
    if (!receivedClientHello) {
      if (message.type !== 'client.hello') {
        clientSocket.close(1002, 'client hello required');
        return false;
      }
      if (runtime.handshakeFrame) clientSocket.send(runtime.handshakeFrame);
      runtime.readyClientSockets.add(clientSocket);
      flushPendingClientFrames(runtime, clientSocket);
      notifyChange();
      return true;
    }
    if (message.type !== 'session.control') {
      clientSocket.close(1002, 'session control required');
      return true;
    }
    const control = message.control as unknown as Record<string, unknown>;
    if (control.type === 'stop') {
      runtime.stopRequested = true;
      void recordSessionControlState(runtime, 'stopped');
    }
    if (runtime.session) {
      runtime.session.sendControl(control);
    } else {
      if (runtime.pendingControls.length >= 128) runtime.pendingControls.shift();
      runtime.pendingControls.push(control);
    }
    return true;
  }

  function deliverClientFrame(runtime: SessionRuntime, frame: Buffer): void {
    let delivered = false;
    for (const client of runtime.readyClientSockets) {
      if (client.readyState !== WebSocket.OPEN) continue;
      client.send(frame);
      delivered = true;
    }
    if (!delivered) queueClientFrame(runtime, frame);
  }

  function flushPendingClientFrames(runtime: SessionRuntime, clientSocket: WebSocket): void {
    for (const frame of runtime.pendingClientFrames.splice(0)) clientSocket.send(frame);
    runtime.pendingClientBytes = 0;
  }

  function queueClientFrame(runtime: SessionRuntime, frame: Buffer): void {
    const copy = Buffer.from(frame);
    while (runtime.pendingClientFrames.length > 0 && (
      runtime.pendingClientFrames.length >= BEALE_APP_SERVER_MAX_REPLAY_FRAMES
      || runtime.pendingClientBytes + copy.byteLength > BEALE_APP_SERVER_MAX_REPLAY_BYTES
    )) {
      const dropped = runtime.pendingClientFrames.shift();
      if (!dropped) break;
      runtime.pendingClientBytes -= dropped.byteLength;
      runtime.droppedClientFrames += 1;
    }
    if (copy.byteLength > BEALE_APP_SERVER_MAX_REPLAY_BYTES) {
      runtime.droppedClientFrames += 1;
      return;
    }
    runtime.pendingClientFrames.push(copy);
    runtime.pendingClientBytes += copy.byteLength;
  }

  function teardownRuntime(runtime: SessionRuntime, clientCode: number): void {
    runtime.unsubscribeSessionEvents?.();
    runtime.unsubscribeSessionEvents = null;
    for (const client of runtime.clientSockets) {
      if (client.readyState === WebSocket.OPEN) client.close(clientCode);
    }
    runtime.readyClientSockets.clear();
  }

  function sessionTransport(runtime: SessionRuntime, token: string): BealeAppServerSessionStartResult['transport'] {
    return {
      path: `${BEALE_APP_SERVER_SESSIONS_PATH}/${encodeURIComponent(runtime.sessionId)}/transport`,
      protocolVersion: HONEYCRISP_PROTOCOL_VERSION,
      authentication: 'bearer',
      token,
      reconnect: 'replay'
    };
  }

  async function close(): Promise<void> {
    closing = true;
    if (automationTimer) clearInterval(automationTimer);
    automationTimer = null;
    for (const runtime of [...sessions.values()]) {
      if (runtime.recoveryTimer) clearTimeout(runtime.recoveryTimer);
      runtime.recoveryTimer = null;
      if (!isTerminal(runtime.state)) {
        // Stop only the process-local worker. The canonical attempt deliberately
        // remains active so the next app-server incarnation classifies this as
        // an interruption and continues it. Explicit session pause/stop controls
        // are persisted separately and therefore remain excluded from recovery.
        runtime.stopRequested = true;
        runtime.session?.stop();
      }
      teardownRuntime(runtime, 1001);
    }
    sessions.clear();
    await new Promise<void>((resolve) => {
      wss.close(() => resolve());
    });
    await new Promise<void>((resolve) => {
      httpServer.close(() => resolve());
    });
    if (options.discoveryFile && discoveryRecord) {
      clearDiscoveryRecord(options.discoveryFile, discoveryRecord.pid);
    }
    notifyChange();
  }

  return {
    host,
    port: address.port,
    url: baseUrl,
    operatorToken,
    startSession,
    recoverInterruptedSessions,
    listSessions,
    stopSession: (sessionId) => stopSession(sessionId) === true,
    close
  };
}

function boundedDiagnostic(value: string): string | null {
  const normalized = value.trim();
  return normalized ? normalized.slice(-MAX_ERROR_DETAIL_CHARS) : null;
}

function boundedRecoveryAttempts(value: number | undefined): number {
  if (value === undefined) return DEFAULT_LONG_SESSION_RECOVERY_ATTEMPTS;
  if (!Number.isFinite(value)) return DEFAULT_LONG_SESSION_RECOVERY_ATTEMPTS;
  return Math.max(0, Math.min(5, Math.floor(value)));
}

function boundedAutomationScanInterval(value: number | undefined): number {
  if (value === undefined || !Number.isFinite(value)) return 30_000;
  return Math.max(10, Math.min(300_000, Math.floor(value)));
}

function rejectUpgrade(socket: Duplex, status: number, message: string): void {
  socket.write(`HTTP/1.1 ${status} ${message}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n`);
  socket.destroy();
}

async function readJsonBody(request: IncomingMessage): Promise<unknown> {
  const chunks: Buffer[] = [];
  let total = 0;
  for await (const chunk of request) {
    const buffer = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk as ArrayBufferLike);
    total += buffer.byteLength;
    if (total > MAX_REQUEST_BODY_BYTES) {
      throw new HttpError(413, `Request body exceeds ${MAX_REQUEST_BODY_BYTES} bytes.`);
    }
    chunks.push(buffer);
  }
  const raw = Buffer.concat(chunks).toString('utf8').trim();
  if (!raw) throw new HttpError(400, 'Request body is required.');
  try {
    return JSON.parse(raw);
  } catch {
    throw new HttpError(400, 'Request body must be valid JSON.');
  }
}

function sendJson(response: ServerResponse, status: number, payload: unknown): void {
  const body = JSON.stringify(payload);
  response.writeHead(status, { 'content-type': 'application/json', 'content-length': Buffer.byteLength(body) });
  response.end(body);
}

function respondWithError(response: ServerResponse, error: unknown): void {
  if (response.headersSent) {
    response.destroy();
    return;
  }
  const status = error instanceof HttpError ? error.status : 500;
  const message = error instanceof Error ? error.message : 'Unexpected server error.';
  sendJson(response, status, {
    controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
    error: {
      code: error instanceof HttpError && error.code ? error.code : httpErrorCode(status),
      // Process and tool failures put the actionable terminal diagnostic at
      // the end of stderr. Preserve that tail across the HTTP boundary.
      message: message.slice(-MAX_ERROR_DETAIL_CHARS),
      retryable: error instanceof HttpError && error.retryable !== undefined
        ? error.retryable
        : status === 429 || status === 502 || status === 503 || status === 504
    }
  });
}

function authorizedBearer(header: string | undefined, expectedToken: string): boolean {
  if (!header?.startsWith('Bearer ')) return false;
  const actual = Buffer.from(header.slice('Bearer '.length).trim(), 'utf8');
  const expected = Buffer.from(expectedToken, 'utf8');
  return actual.length === expected.length && timingSafeEqual(actual, expected);
}

function httpErrorCode(status: number): string {
  if (status === 400) return 'invalid_request';
  if (status === 401) return 'unauthorized';
  if (status === 404) return 'not_found';
  if (status === 409) return 'conflict';
  if (status === 410) return 'gone';
  if (status === 413) return 'request_too_large';
  if (status === 429) return 'rate_limited';
  if (status === 501) return 'unsupported';
  if (status === 502) return 'honeycrisp_failure';
  if (status === 503 || status === 504) return 'temporarily_unavailable';
  return 'internal_error';
}

function toBuffer(data: unknown): Buffer {
  if (Buffer.isBuffer(data)) return data;
  if (Array.isArray(data)) return Buffer.concat(data.map((part) => toBuffer(part)));
  return Buffer.from(data as ArrayBufferLike);
}

async function hostCall<T>(operation: () => Promise<T> | T): Promise<T> {
  try {
    return await operation();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (/database disk image is malformed|file is not a database|database corruption|SQLITE_CORRUPT|SQLITE_NOTADB/iu.test(message)) {
      throw new HttpError(500,
        'Honeycrisp database integrity failed. Stop active writers and restore a verified backup or run SQLite recovery against the configured database before retrying. The original database must be preserved until recovery is validated.',
        { code: 'database_corrupt', retryable: false });
    }
    if (/failed (?:its|the) integrity check/iu.test(message)) {
      throw new HttpError(500,
        'Honeycrisp session integrity validation failed. Stop active writers, preserve the database, and restore or repair the affected session data before retrying.',
        { code: 'session_integrity_failed', retryable: false });
    }
    if (/workspace is not registered|does not belong to workspace/iu.test(message)) throw new HttpError(404, message);
    if (/required|unsupported|must be|no Lead provider/iu.test(message)) throw new HttpError(400, message);
    if (/timed out/iu.test(message)) throw new HttpError(504, message);
    if (/database is locked|temporarily unavailable/iu.test(message)) throw new HttpError(503, message);
    throw error;
  }
}

function pathPart(match: RegExpExecArray, index: number): string {
  const value = match[index];
  if (!value) throw new HttpError(400, 'A required path segment is missing.');
  return decodeURIComponent(value);
}

function queryInteger(url: URL, name: string, fallback: number): number {
  const value = url.searchParams.get(name);
  if (value === null) return fallback;
  const parsed = Number(value);
  if (!Number.isInteger(parsed) || parsed <= 0) {
    throw new HttpError(400, `${name} must be a positive integer.`);
  }
  return parsed;
}

function queryBoolean(url: URL, name: string): boolean {
  const value = url.searchParams.get(name);
  return value === '1' || value === 'true';
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function normalizePublicUrl(value: string): string {
  let parsed: URL;
  try {
    parsed = new URL(value);
  } catch {
    throw new Error('BEALE_APP_SERVER_PUBLIC_URL must be an absolute HTTP or HTTPS URL.');
  }
  if ((parsed.protocol !== 'http:' && parsed.protocol !== 'https:')
    || parsed.username || parsed.password || parsed.search || parsed.hash
    || parsed.pathname !== '/') {
    throw new Error('BEALE_APP_SERVER_PUBLIC_URL must be an HTTP or HTTPS origin without credentials, a path, query, or fragment.');
  }
  return parsed.origin;
}

function urlHost(host: string): string {
  const normalized = host === '0.0.0.0' || host === '::' ? '127.0.0.1' : host;
  return normalized.includes(':') && !normalized.startsWith('[') ? `[${normalized}]` : normalized;
}
