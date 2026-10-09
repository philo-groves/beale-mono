import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http';
import { randomBytes, randomUUID, timingSafeEqual } from 'node:crypto';
import type { AddressInfo } from 'node:net';
import type { Duplex } from 'node:stream';
import { WebSocket, WebSocketServer } from 'ws';
import { installPreBealeEnvironmentAliases } from '@beale/research-agent/legacy-compatibility';
import { installUndiciTypeOfServiceCompatibility } from '@beale/app-server-runtime/node-network-compatibility';
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
  APP_SERVER_PROTOCOL_VERSION,
  APP_SERVER_PROTOCOL_OPERATIONS,
  APP_SERVER_SESSION_LAUNCH_VERSION,
  decodeAppServerClientMessage,
  decodeBealeAppServerSessionContinuationRequest,
  decodeBealeAppServerSessionControlRequest,
  decodeAppServerSessionLaunchRequest,
  appServerServerHello,
  appServerSessionEvent,
  type BealeAppServerDescriptor,
  type BealeAppServerHealth,
  type BealeAppServerSessionCatalog,
  type BealeAppServerSessionCatalogEntry,
  type BealeAppServerSessionAttachResult,
  type BealeAppServerSessionResult,
  type BealeAppServerSessionStartResult,
  type BealeAppServerSessionStopResult,
  type BealeAppServerSessionControlResult,
  type BealeAppServerShutdownResult,
  type AppServerSessionLaunchRequest
} from '@beale/app-server-runtime/protocol';
import {
  generateSessionToken,
  spawnAppServerSession,
  type AppServerSession,
  type SpawnAppServerSessionOptions
} from './appServerSession.js';
import {
  clearDiscoveryRecord,
  generateOperatorToken,
  operatorTokenPath,
  readOrCreateOperatorToken,
  writeDiscoveryRecord,
  type AppServerDiscoveryRecord,
  type AppServerHostMode
} from './discovery.js';
import { prepareAppServerSessionLaunch } from './sessionLaunch.js';
import {
  AppServerHostService,
  type AppServerStartupRecoveryResult,
  type PreparedAppServerSession
} from './hostService.js';
import {
  DEFAULT_LONG_SESSION_RECOVERY_ATTEMPTS,
  inspectAppServerSessionCompletion,
  longSessionRecoveryDelayMs,
  longSessionRecoveryFallbackPrompt
} from './sessionRecovery.js';
import { AppServerWorkerDatabaseCoordinator } from './workerDatabaseBroker.js';
import { FleetBrowserService } from './fleetBrowser.js';

installUndiciTypeOfServiceCompatibility();

const DEFAULT_HOST = '127.0.0.1';
const MAX_REQUEST_BODY_BYTES = 524_288;
const MAX_FRAME_BYTES = 1_048_576;
const SESSION_ID_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;
const MAX_RETAINED_TERMINAL_SESSIONS = 50;
const MAX_ERROR_DETAIL_CHARS = 1_000;
const DEFAULT_SESSION_STARTUP_TIMEOUT_MS = 60_000;
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
  spawnSession?: (options: SpawnAppServerSessionOptions) => Promise<AppServerSession>;
  recoverInterruptedOnStart?: boolean;
  automationScheduler?: false | {
    scanIntervalMs?: number;
    now?: () => Date;
  };
  longSessionRecovery?: false | {
    maxAttempts?: number;
    delayMs?: (recoveryNumber: number) => number;
  };
  /** Maximum time from worker launch to runtime readiness. */
  sessionStartupTimeoutMs?: number;
}

export type SessionStartRequest = AppServerSessionLaunchRequest;

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
  readonly request: AppServerSessionLaunchRequest;
  readonly clientTokens: Set<string>;
  readonly startedAt: string;
  session: AppServerSession | null;
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
  startupDiagnostic: string | null;
  unsubscribeSessionEvents: (() => void) | null;
  currentAttemptId: string;
  currentAttemptWasInitial: boolean;
  recoveryCount: number;
  recoveryTimer: NodeJS.Timeout | null;
  introspectionToken: string | null;
  readonly recentActivity: string[];
}

interface ResidentIntrospectionBinding {
  sessionId: string;
  workspaceId: string;
}

function isTerminal(state: SessionState): boolean {
  return state === 'completed' || state === 'failed' || state === 'stopped';
}

export async function startAppServer(options: AppServerOptions = {}): Promise<AppServerHandle> {
  installPreBealeEnvironmentAliases();
  const host = options.host ?? DEFAULT_HOST;
  const publicUrl = options.publicUrl ? normalizePublicUrl(options.publicUrl) : null;
  const operatorToken = options.operatorToken?.trim()
    || (options.discoveryFile
      ? readOrCreateOperatorToken(operatorTokenPath(options.discoveryFile))
      : generateOperatorToken());
  const databaseCoordinator = new AppServerWorkerDatabaseCoordinator();
  const hostService = options.hostService ?? new AppServerHostService({ databaseCoordinator });
  const spawnSession = options.spawnSession ?? ((spawnOptions: SpawnAppServerSessionOptions) => (
    spawnAppServerSession({ ...spawnOptions, databaseCoordinator })
  ));
  const recoveryOptions = options.longSessionRecovery === false
    ? null
    : options.longSessionRecovery ?? {};
  const maxRecoveryAttempts = recoveryOptions
    ? boundedRecoveryAttempts(recoveryOptions.maxAttempts)
    : 0;
  const recoveryDelay = recoveryOptions?.delayMs ?? longSessionRecoveryDelayMs;
  const sessionStartupTimeoutMs = boundedSessionStartupTimeout(options.sessionStartupTimeoutMs);
  const automationScheduler = options.automationScheduler === false
    ? null
    : options.automationScheduler ?? {};
  const sessions = new Map<string, SessionRuntime>();
  const fleetBrowser = new FleetBrowserService();
  const fleetProxySessions = new Map<string, { endpoint: { url: string; operatorToken: string }; clientTokens: Set<string>; completed: boolean;
    workspaceId: string | null; prompt: string; state: string; startedAt: string }>();
  const introspectionBindings = new Map<string, ResidentIntrospectionBinding>();
  let discoveryRecord: AppServerDiscoveryRecord | null = null;
  let automationTimer: NodeJS.Timeout | null = null;
  let fleetProxyTimer: NodeJS.Timeout | null = null;
  let fleetProxyScanInProgress = false;
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
      const browserUrl = new URL(request.url ?? '/', 'http://localhost');
      const agentBrowser = /^\/v1\/fleet-browser\/([^/]+)\/devtools\/page\/([^/]+)$/.exec(browserUrl.pathname);
      if (agentBrowser) {
        const sessionId = decodeURIComponent(agentBrowser[1]!);
        if (!isLoopbackRequest(request) || !fleetBrowser.authorized(sessionId, browserUrl.searchParams.get('token') ?? '')) {
          rejectUpgrade(socket, 401, 'Fleet browser session token is required.'); return;
        }
        wss.handleUpgrade(request, socket, head, (clientSocket) => {
          void fleetBrowser.agentSocket(sessionId, decodeURIComponent(agentBrowser[2]!), clientSocket)
            .catch(() => clientSocket.close(1011, 'Fleet browser unavailable'));
        });
        return;
      }
      const viewerMatch = /^\/v1\/sessions\/([^/]+)\/browser$/.exec(browserUrl.pathname);
      const proxyMatch = /^\/v1\/sessions\/([^/]+)\/(?:transport|browser)$/.exec(browserUrl.pathname);
      const proxy = proxyMatch ? fleetProxySessions.get(decodeURIComponent(proxyMatch[1] ?? '')) : null;
      if (proxy) {
        const token = [...proxy.clientTokens].find((value) => authorizedBearer(request.headers.authorization, value));
        if (!token) { rejectUpgrade(socket, 401, 'A valid bearer token is required.'); return; }
        wss.handleUpgrade(request, socket, head, (clientSocket) => attachFleetProxyClient(proxy, token, request.url ?? '', clientSocket));
        return;
      }
      if (viewerMatch) {
        const sessionId = decodeURIComponent(viewerMatch[1]!);
        const runtime = sessions.get(sessionId);
        if (!runtime || !fleetBrowser.hasSession(sessionId)
          || ![...runtime.clientTokens].some((token) => authorizedBearer(request.headers.authorization, token))) {
          rejectUpgrade(socket, 401, 'A session token is required.'); return;
        }
        wss.handleUpgrade(request, socket, head, (clientSocket) => {
          void fleetBrowser.viewerSocket(sessionId, clientSocket)
            .catch((error: unknown) => { if (clientSocket.readyState === WebSocket.OPEN) {
              clientSocket.send(JSON.stringify({ type: 'browser.error', message: error instanceof Error ? error.message : String(error) }));
              clientSocket.close(1011, 'Fleet browser unavailable');
            } });
        });
        return;
      }
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

  hostService.resumeFleetModelBrokers?.();

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
  fleetProxyTimer = setInterval(() => void scanFleetProxySessions(), 5_000);
  fleetProxyTimer.unref();

  function requireOperator(request: IncomingMessage): void {
    if (!authorizedBearer(request.headers.authorization, operatorToken)) {
      throw new HttpError(401, 'An operator bearer token is required for this operation.');
    }
  }

  function requireResidentIntrospection(request: IncomingMessage): ResidentIntrospectionBinding {
    for (const [token, binding] of introspectionBindings) {
      if (authorizedBearer(request.headers.authorization, token)) return binding;
    }
    throw new HttpError(401, 'A valid Beale introspection bearer token is required.');
  }

  async function invokeResidentIntrospectionTool(
    binding: ResidentIntrospectionBinding,
    tool: string,
    args: Record<string, unknown>
  ): Promise<unknown> {
    if (tool === 'list_workspaces') return hostService.listWorkspaces();
    if (tool === 'get_workspace') {
      const requested = residentText(args.registryWorkspaceId);
      const workspace = hostService.listWorkspaces().workspaces.find((candidate) => (
        requested ? candidate.id === requested || candidate.workspaceId === requested : candidate.workspaceId === binding.workspaceId
      ));
      if (!workspace || workspace.workspaceId !== binding.workspaceId) {
        throw new Error('Resident introspection can only access the automation workspace.');
      }
      return { workspace };
    }
    if (tool === 'list_sessions') {
      const requestedStatus = residentText(args.status);
      const rawLimit = typeof args.limit === 'number' ? args.limit : 50;
      const limit = Number.isFinite(rawLimit) ? Math.max(1, Math.min(200, Math.floor(rawLimit))) : 50;
      const projected = [...sessions.values()]
        .filter((runtime) => runtime.sessionId !== binding.sessionId)
        .filter((runtime) => runtime.request.launch.workspaceId === binding.workspaceId)
        .sort((left, right) => right.startedAt.localeCompare(left.startedAt))
        .map((runtime) => ({
          ...catalogEntry(runtime),
          runId: runtime.sessionId,
          status: runtime.state === 'running' ? 'active' : runtime.state === 'starting' ? 'queued' : runtime.state
        }))
        .filter((session) => !requestedStatus || session.status === requestedStatus)
        .slice(0, limit);
      return { sessions: projected };
    }
    if (tool === 'stop_session') {
      const sessionId = residentRequiredText(args.runId, 'runId');
      const runtime = sessions.get(sessionId);
      if (!runtime || runtime.request.launch.workspaceId !== binding.workspaceId || runtime.sessionId === binding.sessionId) {
        throw new Error(`No other active session in this workspace matched ${sessionId}.`);
      }
      return { runId: sessionId, stopped: stopSession(sessionId) === true };
    }
    if (tool === 'launch_session') {
      const explicit = isRecord(args.startRunInput) ? args.startRunInput : args;
      const promptMarkdown = residentRequiredText(explicit.promptMarkdown, 'promptMarkdown');
      const providerId = residentText(explicit.provider);
      const model = residentText(explicit.model);
      const reasoningEffort = residentText(explicit.reasoningEffort);
      const fastMode = explicit.fastMode === true;
      const workflowId = residentText(explicit.workflowId);
      const requestedWorkspace = residentText(args.registryWorkspaceId);
      if (requestedWorkspace) {
        const match = hostService.listWorkspaces().workspaces.find((workspace) => (
          workspace.id === requestedWorkspace || workspace.workspaceId === requestedWorkspace
        ));
        if (!match || match.workspaceId !== binding.workspaceId) {
          throw new Error('Resident introspection can only launch sessions in the automation workspace.');
        }
      }
      const goalObjective = residentText(explicit.goalObjective);
      const started = await startSession({
        launchVersion: APP_SERVER_SESSION_LAUNCH_VERSION,
        launch: {
          workspaceId: binding.workspaceId,
          promptMarkdown,
          ...(providerId || model || reasoningEffort || fastMode
            ? {
                provider: {
                  ...(providerId ? { id: providerId } : {}),
                  ...(model ? { model } : {}),
                  ...(reasoningEffort ? { reasoningEffort } : {}),
                  ...(fastMode ? { fastMode: true } : {})
                }
              }
            : {}),
          shellSafetyMode: residentText(explicit.shellSafetyMode) || 'auto_review',
          ...(workflowId ? { workflowId } : {}),
          ...(explicit.goalEnabled === true
            ? { goal: { ...(goalObjective ? { objective: goalObjective } : {}) } }
            : {})
        }
      }, false);
      return { runId: started.session.sessionId, session: started.session };
    }
    throw new Error(`Beale introspection tool ${tool} is unavailable from a resident automation.`);
  }

  function residentText(value: unknown): string {
    return typeof value === 'string' ? value.trim() : '';
  }

  function residentRequiredText(value: unknown, name: string): string {
    const normalized = residentText(value);
    if (!normalized) throw new Error(`${name} is required.`);
    return normalized;
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
      sessionLaunchVersion: APP_SERVER_SESSION_LAUNCH_VERSION,
      appServerProtocolVersion: APP_SERVER_PROTOCOL_VERSION,
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
    const browserMatch = /^\/v1\/fleet-browser\/([^/]+)\/(json\/list|json\/version|contexts|contexts\/default\/open)$/.exec(url.pathname);
    if (browserMatch) {
      const sessionId = decodeURIComponent(browserMatch[1]!);
      if (!isLoopbackRequest(request) || !fleetBrowser.authorized(sessionId, url.searchParams.get('token') ?? '')) {
        throw new HttpError(403, 'Fleet browser session token is required.');
      }
      const kind = browserMatch[2]!;
      if (kind === 'contexts/default/open' && request.method === 'POST') {
        sendJson(response, 200, { id: 'default', label: 'Default' }); return;
      }
      if (request.method !== 'GET' || kind === 'contexts/default/open') throw new HttpError(405, 'Unsupported Fleet browser request.');
      const result = await fleetBrowser.discovery(sessionId, kind === 'json/list' ? 'list' : kind === 'json/version' ? 'version' : 'contexts', `http://127.0.0.1:${address.port}`);
      sendJson(response, 200, result);
      return;
    }
    const brokerMatch = /^\/v1\/fleet-model-broker\/([A-Za-z0-9][A-Za-z0-9._-]{0,127})(?:\/([A-Za-z0-9][A-Za-z0-9._-]{0,127}))?$/u.exec(url.pathname);
    if (brokerMatch) {
      if (request.socket.remoteAddress !== '127.0.0.1' && request.socket.remoteAddress !== '::1') {
        throw new HttpError(403, 'The Fleet model broker is available on loopback only.');
      }
      const token = /^Bearer (.+)$/u.exec(request.headers.authorization ?? '')?.[1] ?? '';
      const sessionId = brokerMatch[1]!;
      if (request.method === 'POST' && !brokerMatch[2]) {
        const body = await readJsonBody(request, 16 * 1024 * 1024);
        if (!isRecord(body) || typeof body.id !== 'string' || !isRecord(body.model)) {
          throw new HttpError(400, 'A model broker request is required.');
        }
        hostService.brokerSubmit(sessionId, token, body as unknown as import('@beale/research-agent').ModelBrokerRequest);
        sendJson(response, 200, { submitted: true });
        return;
      }
      if (request.method === 'GET' && brokerMatch[2]) {
        sendJson(response, 200, hostService.brokerResult(sessionId, token, brokerMatch[2]));
        return;
      }
      throw new HttpError(405, 'Unsupported model broker request.');
    }
    if (request.method === 'POST' && url.pathname === '/v1/introspection/tool') {
      const binding = requireResidentIntrospection(request);
      const body = await readJsonBody(request);
      if (!isRecord(body) || typeof body.tool !== 'string' || !isRecord(body.args)) {
        sendJson(response, 400, { ok: false, error: 'A Beale introspection tool and arguments are required.' });
        return;
      }
      try {
        sendJson(response, 200, {
          ok: true,
          result: await invokeResidentIntrospectionTool(binding, body.tool, body.args)
        });
      } catch (error) {
        sendJson(response, 400, {
          ok: false,
          error: error instanceof Error ? error.message : String(error)
        });
      }
      return;
    }
    requireOperator(request);
    const proxiedSessionId = proxySessionId(url.pathname);
    const proxied = proxiedSessionId ? fleetProxySessions.get(proxiedSessionId) : null;
    if (proxied) {
      await proxyFleetHttp(proxied, request, response, url, proxiedSessionId!);
      return;
    }
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
        sessions: [...listSessions(), ...[...fleetProxySessions].map(([sessionId, proxy]): BealeAppServerSessionCatalogEntry => ({
          sessionId, state: proxySessionState(proxy.state), startedAt: proxy.startedAt, endedAt: proxy.completed ? new Date().toISOString() : null,
          exitCode: null, clientConnected: false, diagnostic: null,
          replay: { bufferedFrames: 0, bufferedBytes: 0, droppedFrames: 0 },
        }))]
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
        || !APP_SERVER_PROTOCOL_OPERATIONS.includes(body.operation as never)) {
        throw new HttpError(400, 'A supported app-server operation is required.');
      }
      if (body.args !== undefined && (!Array.isArray(body.args) || body.args.some((value) => typeof value !== 'string'))) {
        throw new HttpError(400, 'Operation args must be an array of strings.');
      }
      if (body.operation === 'fleet.connect' && isRecord(body.input) && body.input.proxy === true) {
        const runId = typeof body.input.runId === 'string' ? body.input.runId : '';
        const endpoint = await hostCall(() => hostService.executeOperation({ operation: 'fleet.connect', input: body.input })) as { url?: unknown; operatorToken?: unknown };
        if (typeof endpoint.url !== 'string' || typeof endpoint.operatorToken !== 'string') throw new HttpError(502, 'Fleet guest connection is unavailable.');
        const attach = await fetch(`${endpoint.url}${BEALE_APP_SERVER_SESSIONS_PATH}/${encodeURIComponent(runId)}/attachments`, {
          method: 'POST', headers: { authorization: `Bearer ${endpoint.operatorToken}` }, signal: AbortSignal.timeout(30_000),
        });
        if (!attach.ok) throw new HttpError(502, 'Fleet guest session could not be reattached.');
        const attached: unknown = await attach.json();
        const token = isRecord(attached) && isRecord(attached.transport) ? attached.transport.token : null;
        if (typeof token !== 'string' || !token) throw new HttpError(502, 'Fleet guest session returned no transport token.');
        const existingProxy = fleetProxySessions.get(runId);
        fleetProxySessions.set(runId, { endpoint: { url: endpoint.url, operatorToken: endpoint.operatorToken },
          clientTokens: new Set([...(existingProxy?.clientTokens ?? []), token]),
          completed: existingProxy?.completed ?? false, workspaceId: existingProxy?.workspaceId ?? null,
          prompt: existingProxy?.prompt ?? '', state: existingProxy?.state ?? 'running',
          startedAt: existingProxy?.startedAt ?? new Date().toISOString() });
        sendJson(response, 200, { controlVersion: BEALE_APP_SERVER_CONTROL_VERSION, result: { machineId: body.input.machineId, url: baseUrl } });
        return;
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
            operation: body.operation as (typeof APP_SERVER_PROTOCOL_OPERATIONS)[number],
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
      const workspaceId = pathPart(workspaceSessionsMatch, 1);
      const catalog = await hostCall(() => hostService.workspaceSessions(
        workspaceId,
        queryInteger(url, 'limit', 200)
      ));
      const proxied = [...fleetProxySessions].flatMap(([sessionId, proxy]) => proxy.workspaceId === workspaceId ? [{
        id: sessionId, workspaceId, title: proxy.prompt.slice(0, 90), prompt: proxy.prompt, status: proxy.state,
        createdAt: proxy.startedAt, attempts: [{ startedAt: proxy.startedAt }], metadata: {},
      }] : []);
      sendJson(response, 200, isRecord(catalog) && Array.isArray(catalog.result)
        ? { ...catalog, result: [...proxied, ...catalog.result] } : catalog);
      return;
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
          ...(url.searchParams.get('beforeEventId') ? { beforeEventId: url.searchParams.get('beforeEventId')! } : {}),
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
    const continuationMatch = /^\/v1\/sessions\/([^/]+)\/continuations$/.exec(url.pathname);
    if (continuationMatch && request.method === 'POST') {
      const sessionId = decodeURIComponent(continuationMatch[1] ?? '');
      const current = sessions.get(sessionId);
      if (current && !isTerminal(current.state)) {
        throw new HttpError(409, `Session ${sessionId} is still active.`);
      }
      let continuation;
      try {
        continuation = decodeBealeAppServerSessionContinuationRequest(await readJsonBody(request));
      } catch (error) {
        throw new HttpError(400, error instanceof Error ? error.message : 'Invalid session continuation request.');
      }
      const launchRequest = await hostCall(() => hostService.createSessionContinuationRequest(
        continuation.workspaceId,
        sessionId,
        continuation.instruction
      ));
      const started = await startSession(
        launchRequest,
        false,
        () => hostCall(() => hostService.recordSessionContinuationInstruction(launchRequest))
      );
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
    const sessionControlMatch = /^\/v1\/sessions\/([^/]+)\/control$/.exec(url.pathname);
    if (sessionControlMatch && request.method === 'POST') {
      const sessionId = decodeURIComponent(sessionControlMatch[1] ?? '');
      const runtime = sessions.get(sessionId);
      if (!runtime) throw new HttpError(404, `Unknown session: ${sessionId}`);
      if (isTerminal(runtime.state)) throw new HttpError(410, `Session ${sessionId} has already ended.`);
      let control;
      try {
        control = decodeBealeAppServerSessionControlRequest(await readJsonBody(request));
      } catch (error) {
        throw new HttpError(400, error instanceof Error ? error.message : 'Invalid session control request.');
      }
      const requestId = randomUUID();
      const message = { schemaVersion: 1 as const, requestId, ...control };
      if (control.type === 'stop') {
        requestRuntimeStop(runtime, message);
      } else if (runtime.session && runtime.state === 'running') {
        runtime.session.sendControl(message);
      } else {
        if (runtime.pendingControls.length >= 128) runtime.pendingControls.shift();
        runtime.pendingControls.push(message);
      }
      const result: BealeAppServerSessionControlResult = {
        controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
        accepted: true,
        sessionId,
        requestId,
        type: control.type
      };
      sendJson(response, 202, result);
      return;
    }
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

  function normalizeSessionRequest(input: unknown): { sessionId: string; request: AppServerSessionLaunchRequest } {
    let request: AppServerSessionLaunchRequest;
    try {
      request = decodeAppServerSessionLaunchRequest(input);
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

  async function startSession(
    input: unknown,
    residentIntrospection = false,
    beforeLaunch?: () => Promise<void>
  ): Promise<StartedSession> {
    const normalized = normalizeSessionRequest(input);
    let request = normalized.request;
    if (request.launch.machineId && request.launch.machineId !== 'local' && request.launch.fleetOwnerMachineId) {
      const fleetState = await hostCall(() => hostService.executeOperation({ operation: 'fleet.state' })) as { role?: unknown };
      if (fleetState.role === 'primary') {
        const endpoint = await hostCall(() => hostService.executeOperation({ operation: 'fleet.prepare', input: {
          workspaceId: request.launch.workspaceId, runId: normalized.sessionId,
          machineId: request.launch.machineId, ownerMachineId: request.launch.fleetOwnerMachineId,
        } })) as { url?: unknown; operatorToken?: unknown; machineId?: unknown };
        if (typeof endpoint.url !== 'string' || typeof endpoint.operatorToken !== 'string'
          || typeof endpoint.machineId !== 'string') throw new HttpError(502, 'Fleet guest connection is unavailable.');
        let guestRejected = false;
        try {
        const guestResponse = await fetch(endpoint.url + BEALE_APP_SERVER_SESSIONS_PATH, {
          method: 'POST', headers: { authorization: `Bearer ${endpoint.operatorToken}`, 'content-type': 'application/json' },
          body: JSON.stringify({ ...request, sessionId: normalized.sessionId,
            launch: { ...request.launch, machineId: endpoint.machineId } }), redirect: 'error', signal: AbortSignal.timeout(90_000),
        });
        if (!guestResponse.ok) { guestRejected = true; throw new HttpError(502, `Fleet guest rejected the session (${guestResponse.status}).`); }
        const started: unknown = await guestResponse.json();
        const startedRecord = isRecord(started) ? started : null;
        const token = startedRecord && isRecord(startedRecord.transport) ? startedRecord.transport.token : null;
        const guestSession = startedRecord && isRecord(startedRecord.session) ? startedRecord.session : null;
        if (typeof token !== 'string' || !token || guestSession?.sessionId !== normalized.sessionId) throw new HttpError(502, 'Fleet guest returned an invalid session.');
        fleetProxySessions.set(normalized.sessionId, { endpoint: { url: endpoint.url, operatorToken: endpoint.operatorToken }, clientTokens: new Set([token]),
          completed: false, workspaceId: request.launch.workspaceId, prompt: request.launch.promptMarkdown, state: 'running', startedAt: new Date().toISOString() });
        return started as StartedSession;
        } catch (error) {
          if (guestRejected) {
            const owner = { machineId: endpoint.machineId, ownerMachineId: request.launch.fleetOwnerMachineId, sessionId: normalized.sessionId };
            await hostCall(() => hostService.executeOperation({ operation: 'fleet.stop', input: owner })).catch(() => undefined);
            await hostCall(() => hostService.executeOperation({ operation: 'fleet.release', input: owner })).catch(() => undefined);
          }
          throw error;
        }
      }
    }
    let residentIntrospectionToken: string | null = null;
    if (residentIntrospection && !request.launch.introspection) {
      residentIntrospectionToken = generateSessionToken();
      request = {
        ...request,
        launch: {
          ...request.launch,
          introspection: {
            url: `${localUrl}/v1/introspection`,
            token: residentIntrospectionToken,
            runtimeMode: 'standard'
          }
        }
      };
      introspectionBindings.set(residentIntrospectionToken, {
        sessionId: normalized.sessionId,
        workspaceId: request.launch.workspaceId
      });
    }
    const prepared = await (async () => {
      try {
        const result = await hostCall(() => hostService.prepareSession(request, normalized.sessionId));
        await beforeLaunch?.();
        return result;
      } catch (error) {
        if (residentIntrospectionToken) introspectionBindings.delete(residentIntrospectionToken);
        throw error;
      }
    })();
    const { sessionId, attemptId } = prepared;
    if (hostService.brokerToken?.(sessionId)) {
      const preferences = prepared.launch.provider.authenticationPreferences;
      const providers = [prepared.launch.provider.id];
      const collaboration = request.launch.collaboration;
      if (isRecord(collaboration) && Array.isArray(collaboration.providers)) {
        for (const candidate of collaboration.providers) {
          if (isRecord(candidate) && candidate.enabled === true && typeof candidate.provider === 'string') {
            providers.push(candidate.provider);
          }
        }
      }
      if (providers.some((provider) => provider === 'anthropic'
        || provider === 'zai' && preferences.zai !== 'api_key')) {
        throw new HttpError(409, 'This Fleet session uses a provider-specific subscription SDK that the model broker cannot yet run. Select a Pi-backed provider or Z.ai API key authentication.');
      }
    }
    const existing = sessions.get(sessionId);
    if (existing && !isTerminal(existing.state)) {
      if (residentIntrospectionToken) introspectionBindings.delete(residentIntrospectionToken);
      throw new HttpError(409, `Session ${sessionId} already exists.`);
    }
    if (existing) {
      sessions.delete(sessionId);
    }
    const effectiveRequest = request;
    const runtime = createSessionRuntime(effectiveRequest, prepared);
    sessions.set(sessionId, runtime);
    evictOldestTerminalSessions();
    try {
      await launchPreparedSession(runtime, prepared, effectiveRequest.launch.continuation === undefined);
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
      if (runtime.introspectionToken) introspectionBindings.delete(runtime.introspectionToken);
      notifyChange();
      const detail = error instanceof Error ? error.message : String(error);
      const diagnostic = runtime.diagnostic ?? detail;
      runtime.diagnostic = boundedDiagnostic(diagnostic);
      await recordSessionLaunchFailure(runtime, diagnostic);
      throw new HttpError(502, `app-server session failed to start: ${diagnostic}`);
    }
  }

  async function scanFleetProxySessions(): Promise<void> {
    if (closing || fleetProxyScanInProgress) return;
    fleetProxyScanInProgress = true;
    try {
    for (const [runId, proxy] of fleetProxySessions) {
      if (proxy.completed) continue;
      try {
        const response = await fetch(`${proxy.endpoint.url}${BEALE_APP_SERVER_SESSIONS_PATH}/${encodeURIComponent(runId)}`, {
          headers: { authorization: `Bearer ${proxy.endpoint.operatorToken}` }, signal: AbortSignal.timeout(3_000),
        });
        if (!response.ok) continue;
        const body: unknown = await response.json();
        const state = isRecord(body) && isRecord(body.session) ? body.session.state : null;
        if (typeof state === 'string') proxy.state = state;
        if (state !== 'completed' && state !== 'failed' && state !== 'stopped') continue;
        await hostCall(() => hostService.executeOperation({ operation: 'fleet.complete', input: { runId } }));
        proxy.completed = true;
      } catch { /* Retry after the guest or transfer becomes available. */ }
    }
    } finally { fleetProxyScanInProgress = false; }
  }

  async function scanDueAutomations(): Promise<void> {
    if (closing || automationScanInProgress) return;
    const service = hostService as AppServerHostService & {
      dueAutomations?: (at?: Date) => Promise<Array<{ request: AppServerSessionLaunchRequest }>>;
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
          await startSession(automation.request, true);
        } catch {
          // A later scan retries transient preparation or provider failures.
        }
      }
    } finally {
      automationScanInProgress = false;
    }
  }

  function createSessionRuntime(
    request: AppServerSessionLaunchRequest,
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
      startupDiagnostic: null,
      unsubscribeSessionEvents: null,
      currentAttemptId: prepared.attemptId,
      currentAttemptWasInitial: false,
      recoveryCount: 0,
      recoveryTimer: null,
      introspectionToken: request.launch.introspection?.runtimeMode === 'standard'
        && request.launch.introspection.url === `${localUrl}/v1/introspection`
        ? request.launch.introspection.token
        : null,
      recentActivity: []
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
        const detail = error instanceof Error ? error.message : String(error);
        await recordSessionLaunchFailure(runtime, detail);
        finishRuntime(
          runtime,
          'failed',
          null,
          `app-server startup recovery failed to launch: ${detail}`
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
    deliverClientFrame(runtime, Buffer.from(JSON.stringify(appServerSessionEvent(runtime.sessionId, event))));
  }

  async function launchPreparedSession(
    runtime: SessionRuntime,
    prepared: PreparedAppServerSession,
    attemptWasInitial = false
  ): Promise<void> {
    const { args, env } = prepareAppServerSessionLaunch(prepared.launch);
    if (runtime.request.launch.machineId && runtime.request.launch.machineId !== 'local') {
      const token = fleetBrowser.tokenFor(runtime.sessionId);
      env.BEALE_FLEET_BROWSER_ENDPOINT = `http://127.0.0.1:${address.port}/v1/fleet-browser/${encodeURIComponent(runtime.sessionId)}?token=${token}`;
    }
    delete env.APP_SERVER_MODEL_BROKER_URL;
    delete env.APP_SERVER_MODEL_BROKER_TOKEN;
    const brokerToken = hostService.brokerToken?.(runtime.sessionId);
    if (brokerToken) {
      for (const name of ['OPENAI_API_KEY', 'ANTHROPIC_API_KEY', 'XAI_API_KEY', 'ZAI_API_KEY',
        'OPENROUTER_API_KEY', 'APP_SERVER_CODEX_AUTH_FILE', 'BEALE_OPENAI_CODEX_AUTH_FILE']) {
        delete env[name];
      }
      env.APP_SERVER_MODEL_BROKER_URL = `http://127.0.0.1:${address.port}/v1/fleet-model-broker/${encodeURIComponent(runtime.sessionId)}`;
      env.APP_SERVER_MODEL_BROKER_TOKEN = brokerToken;
    }
    hostService.setWorkspaceSessionActive?.(runtime.request.launch.workspaceId, runtime.sessionId, true);
    if (runtime.stopRequested || sessions.get(runtime.sessionId) !== runtime) {
      hostService.setWorkspaceSessionActive?.(runtime.request.launch.workspaceId, runtime.sessionId, false);
      return;
    }
    let session: AppServerSession;
    try { session = await spawnSession({ sessionId: runtime.sessionId, args, env }); }
    catch (error) {
      hostService.setWorkspaceSessionActive?.(runtime.request.launch.workspaceId, runtime.sessionId, false);
      throw error;
    }
    if (runtime.stopRequested || sessions.get(runtime.sessionId) !== runtime) {
      session.stop();
      void session.waitExit().then(async () => {
        hostService.setWorkspaceSessionActive?.(runtime.request.launch.workspaceId, runtime.sessionId, false);
      });
      return;
    }
    runtime.session = session;
    runtime.currentAttemptId = prepared.attemptId;
    runtime.currentAttemptWasInitial = attemptWasInitial;
    runtime.handshakeFrame ??= Buffer.from(JSON.stringify(appServerServerHello(runtime.sessionId, '0.1.0')));
    runtime.unsubscribeSessionEvents = session.onEvent((event) => {
      observeSessionControlState(runtime, event);
      const recoveryActivity = recoveryActivityFromEvent(event);
      if (recoveryActivity) {
        runtime.recentActivity.push(recoveryActivity);
        if (runtime.recentActivity.length > 20) runtime.recentActivity.splice(0, runtime.recentActivity.length - 20);
      }
      deliverClientFrame(runtime, Buffer.from(JSON.stringify(appServerSessionEvent(runtime.sessionId, event))));
    });
    runtime.state = 'starting';
    runtime.endedAt = null;
    runtime.exitCode = null;
    runtime.diagnostic = null;
    runtime.startupDiagnostic = null;
    void session.waitExit().then((result) => handleSessionExit(runtime, session, prepared, result));
    if (!session.waitReady) {
      markRuntimeReady(runtime, session);
      return;
    }
    void waitForRuntimeReady(runtime, session, session.waitReady(), sessionStartupTimeoutMs);
  }

  function markRuntimeReady(runtime: SessionRuntime, session: AppServerSession): void {
    if (runtime.session !== session || sessions.get(runtime.sessionId) !== runtime || runtime.stopRequested) return;
    runtime.state = 'running';
    runtime.diagnostic = null;
    runtime.startupDiagnostic = null;
    for (const control of runtime.pendingControls.splice(0)) session.sendControl(control);
    notifyChange();
  }

  async function waitForRuntimeReady(
    runtime: SessionRuntime,
    session: AppServerSession,
    ready: Promise<void>,
    timeoutMs: number
  ): Promise<void> {
    let timeout: NodeJS.Timeout | null = null;
    try {
      await Promise.race([
        ready,
        new Promise<never>((_resolve, reject) => {
          timeout = setTimeout(() => reject(new Error(
            `Research runtime initialization did not become ready within ${Math.ceil(timeoutMs / 1_000)} seconds. No model request was sent.`
          )), timeoutMs);
          timeout.unref();
        })
      ]);
      markRuntimeReady(runtime, session);
    } catch (error) {
      if (runtime.session !== session || sessions.get(runtime.sessionId) !== runtime || runtime.stopRequested) return;
      const detail = error instanceof Error ? error.message : String(error);
      runtime.startupDiagnostic = boundedDiagnostic(
        `${detail} Retry the session; if this repeats, restart the Beale app-server.`
      );
      runtime.diagnostic = runtime.startupDiagnostic;
      const event = {
        schemaVersion: 1,
        kind: 'model.output',
        timestamp: new Date().toISOString(),
        payload: {
          eventId: `startup:${runtime.sessionId}:failed`,
          phase: 'completed',
          messagePhase: 'commentary',
          text: runtime.diagnostic,
          agentPath: '/root',
          responseId: `startup-failed-${runtime.sessionId}`,
          itemId: 'startup:failed'
        }
      };
      deliverClientFrame(runtime, Buffer.from(JSON.stringify(appServerSessionEvent(runtime.sessionId, event))));
      notifyChange();
      await recordSessionLaunchFailure(runtime, runtime.startupDiagnostic ?? detail);
      session.stop();
    } finally {
      if (timeout) clearTimeout(timeout);
    }
  }

  function observeSessionControlState(runtime: SessionRuntime, event: Record<string, unknown>): void {
    if (event.kind !== 'agent.event' || !isRecord(event.payload)) return;
    if (event.payload.eventType !== 'control.received' || event.payload.accepted !== true) return;
    const type = event.payload.type;
    if (type !== 'pause' && type !== 'resume' && type !== 'stop') return;
    if (runtime.stopRequested) return;
    if (type === 'stop') {
      if (runtime.startupDiagnostic) return;
      // An accepted worker-side stop must arm the same forced-exit fallback as HTTP/WS stop.
      requestRuntimeStop(runtime);
      return;
    }
    void recordSessionControlState(
      runtime,
      type === 'pause' ? 'paused' : 'active'
    );
  }

  async function recordSessionControlState(
    runtime: SessionRuntime,
    state: 'active' | 'paused' | 'stopped'
  ): Promise<void> {
    const service = hostService as AppServerHostService & {
      recordSessionControlState?: (input: {
        request: AppServerSessionLaunchRequest;
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

  async function recordSessionLaunchFailure(runtime: SessionRuntime, diagnostic: string): Promise<void> {
    const service = hostService as AppServerHostService & {
      recordSessionLaunchFailure?: (input: {
        request: AppServerSessionLaunchRequest;
        sessionId: string;
        attemptId: string;
        diagnostic: string;
      }) => Promise<void>;
    };
    try {
      await service.recordSessionLaunchFailure?.({
        request: runtime.request,
        sessionId: runtime.sessionId,
        attemptId: runtime.currentAttemptId,
        diagnostic
      });
    } catch (error) {
      runtime.diagnostic = boundedDiagnostic(
        `${diagnostic} Canonical launch-failure finalization also failed: ${error instanceof Error ? error.message : String(error)}`
      );
      notifyChange();
    }
  }

  async function handleSessionExit(
    runtime: SessionRuntime,
    session: AppServerSession,
    prepared: PreparedAppServerSession,
    result: { code: number | null; stderr: string }
  ): Promise<void> {
    if (runtime.session !== session || sessions.get(runtime.sessionId) !== runtime) return;
    runtime.unsubscribeSessionEvents?.();
    runtime.unsubscribeSessionEvents = null;
    runtime.session = null;
    const completion = await inspectAppServerSessionCompletion({
      code: result.code,
      stderr: result.stderr,
      capturePath: prepared.launch.capturePath,
      stopRequested: runtime.stopRequested
    });
    const startupDiagnostic = runtime.startupDiagnostic;
    hostService.setWorkspaceSessionActive?.(runtime.request.launch.workspaceId, runtime.sessionId, false);
    if (sessions.get(runtime.sessionId) !== runtime) return;
    if (startupDiagnostic) {
      finishRuntime(runtime, 'failed', result.code === 0 ? 1 : result.code, startupDiagnostic);
      return;
    }
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
        void recoverSession(runtime, prepared, completion.diagnostic ?? 'Unexpected app-server worker failure.');
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
    const fallbackPrompt = longSessionRecoveryFallbackPrompt(
      runtime.request.launch.promptMarkdown,
      diagnostic,
      runtime.recentActivity,
    );
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
      await recordSessionLaunchFailure(runtime, detail);
      finishRuntime(
        runtime,
        runtime.stopRequested ? 'stopped' : 'failed',
        null,
        runtime.stopRequested ? null : `app-server recovery failed to start: ${detail}`
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
    void recordSessionTerminalState(runtime, state);
  }

  async function recordSessionTerminalState(runtime: SessionRuntime, state: 'completed' | 'failed' | 'stopped'): Promise<void> {
    const service = hostService as AppServerHostService & {
      recordSessionTerminalState?: (input: {
        request: AppServerSessionLaunchRequest;
        sessionId: string;
        attemptId: string;
        state: 'completed' | 'failed' | 'stopped';
      }) => Promise<void>;
    };
    try {
      await service.recordSessionTerminalState?.({
        request: runtime.request, sessionId: runtime.sessionId, attemptId: runtime.currentAttemptId, state
      });
      notifyChange();
    } catch (error) {
      runtime.diagnostic = boundedDiagnostic(
        `${runtime.diagnostic ? `${runtime.diagnostic} ` : ''}Could not persist ${state} session state: ${error instanceof Error ? error.message : String(error)}`
      );
      notifyChange();
    }
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
    deliverClientFrame(runtime, Buffer.from(JSON.stringify(appServerSessionEvent(runtime.sessionId, event))));
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
    requestRuntimeStop(runtime);
    return true;
  }

  function requestRuntimeStop(
    runtime: SessionRuntime,
    control?: Record<string, unknown>
  ): void {
    if (!runtime.stopRequested) {
      runtime.stopRequested = true;
      void recordSessionControlState(runtime, 'stopped');
    }
    const session = runtime.session;
    if (!session) {
      finishRuntime(runtime, 'stopped', null, null);
      return;
    }
    if (control) {
      try {
        session.sendControl(control);
      } catch {
        // The bounded stop fallback below remains authoritative if delivery races worker exit.
      }
    }
    session.stop();
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

  async function proxyFleetHttp(
    proxy: { endpoint: { url: string; operatorToken: string }; clientTokens: Set<string> },
    request: IncomingMessage, response: ServerResponse, url: URL, sessionId: string,
  ): Promise<void> {
    const body = request.method === 'POST' ? await readProxyBody(request) : undefined;
    const upstream = await fetch(proxy.endpoint.url + url.pathname + url.search, {
      method: request.method ?? 'GET',
      headers: { authorization: `Bearer ${proxy.endpoint.operatorToken}`,
        ...(body ? { 'content-type': request.headers['content-type'] ?? 'application/json' } : {}) },
      ...(body ? { body } : {}), signal: AbortSignal.timeout(60_000),
    });
    const payload: unknown = await upstream.json().catch(() => null);
    if (request.method === 'POST' && (url.pathname === `${BEALE_APP_SERVER_SESSIONS_PATH}/${encodeURIComponent(sessionId)}/attachments`
      || url.pathname === `${BEALE_APP_SERVER_SESSIONS_PATH}/${encodeURIComponent(sessionId)}/continuations`)
      && isRecord(payload) && isRecord(payload.transport) && typeof payload.transport.token === 'string') {
      proxy.clientTokens.add(payload.transport.token);
    }
    sendJson(response, upstream.status, payload);
  }

  function attachFleetProxyClient(
    proxy: { endpoint: { url: string; operatorToken: string }; clientTokens: Set<string> },
    token: string, path: string, client: WebSocket,
  ): void {
    const upstreamUrl = new URL(path, proxy.endpoint.url);
    upstreamUrl.protocol = upstreamUrl.protocol === 'https:' ? 'wss:' : 'ws:';
    const upstream = new WebSocket(upstreamUrl, { headers: { authorization: `Bearer ${token}` }, maxPayload: MAX_FRAME_BYTES });
    const queued: Array<{ frame: Buffer; binary: boolean }> = [];
    let queuedBytes = 0;
    client.on('message', (data, binary) => {
      const frame = toBuffer(data);
      if (frame.byteLength > MAX_FRAME_BYTES) { client.close(1009); return; }
      if (upstream.readyState === WebSocket.OPEN) upstream.send(frame, { binary });
      else if (upstream.readyState === WebSocket.CONNECTING && queuedBytes + frame.byteLength <= MAX_FRAME_BYTES * 4) {
        queued.push({ frame, binary }); queuedBytes += frame.byteLength;
      } else client.close(1011, 'Fleet guest transport unavailable');
    });
    upstream.on('open', () => {
      for (const entry of queued) upstream.send(entry.frame, { binary: entry.binary });
      queued.length = 0;
    });
    upstream.on('message', (data, binary) => { if (client.readyState === WebSocket.OPEN) client.send(data, { binary }); });
    upstream.on('close', () => { if (client.readyState === WebSocket.OPEN) client.close(1001); });
    upstream.on('error', () => { if (client.readyState === WebSocket.OPEN) client.close(1011); });
    client.on('close', () => { if (upstream.readyState === WebSocket.OPEN || upstream.readyState === WebSocket.CONNECTING) upstream.close(); });
    client.on('error', () => upstream.terminate());
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
      message = decodeAppServerClientMessage(JSON.parse(frame.toString('utf8')) as unknown);
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
      requestRuntimeStop(runtime, control);
    } else if (runtime.session && runtime.state === 'running') {
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
    if (runtime.introspectionToken) introspectionBindings.delete(runtime.introspectionToken);
  }

  function sessionTransport(runtime: SessionRuntime, token: string): BealeAppServerSessionStartResult['transport'] {
    return {
      path: `${BEALE_APP_SERVER_SESSIONS_PATH}/${encodeURIComponent(runtime.sessionId)}/transport`,
      protocolVersion: APP_SERVER_PROTOCOL_VERSION,
      authentication: 'bearer',
      token,
      reconnect: 'replay'
    };
  }

  async function close(): Promise<void> {
    closing = true;
    fleetBrowser.close();
    hostService.stopFleetModelBrokers?.();
    if (automationTimer) clearInterval(automationTimer);
    automationTimer = null;
    if (fleetProxyTimer) clearInterval(fleetProxyTimer);
    fleetProxyTimer = null;
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

function recoveryActivityFromEvent(event: Record<string, unknown>): string | null {
  if (!isRecord(event.payload)) return null;
  const payload = event.payload;
  if (event.kind === 'model.output' && payload.phase === 'completed' && typeof payload.text === 'string') {
    const text = payload.text.replace(/\s+/gu, ' ').trim();
    return text ? `commentary: ${text.slice(0, 600)}` : null;
  }
  if (event.kind === 'tool.requested' || event.kind === 'tool.observed'
    || payload.type === 'tool_execution_end' || payload.eventType === 'tool_execution_end') {
    const toolName = typeof payload.toolName === 'string' ? payload.toolName : 'tool';
    const status = typeof payload.status === 'string' ? payload.status : event.kind;
    const summary = typeof payload.summary === 'string'
      ? ` — ${payload.summary.replace(/\s+/gu, ' ').trim().slice(0, 400)}`
      : '';
    return `${toolName}: ${status}${summary}`;
  }
  return null;
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

async function readJsonBody(request: IncomingMessage, limit = MAX_REQUEST_BODY_BYTES): Promise<unknown> {
  const chunks: Buffer[] = [];
  let total = 0;
  for await (const chunk of request) {
    const buffer = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk as ArrayBufferLike);
    total += buffer.byteLength;
    if (total > limit) {
      throw new HttpError(413, `Request body exceeds ${limit} bytes.`);
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

async function readProxyBody(request: IncomingMessage): Promise<Buffer | undefined> {
  const chunks: Buffer[] = [];
  let total = 0;
  for await (const chunk of request) {
    const buffer = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk as ArrayBufferLike);
    total += buffer.byteLength;
    if (total > MAX_REQUEST_BODY_BYTES) throw new HttpError(413, 'Fleet proxy request body is too large.');
    chunks.push(buffer);
  }
  return total ? Buffer.concat(chunks) : undefined;
}

function proxySessionId(pathname: string): string | null {
  const direct = /^\/v1\/sessions\/([^/]+)(?:\/[^/]+)?$/u.exec(pathname);
  const canonical = /^\/v1\/workspaces\/[^/]+\/sessions\/([^/]+)\/(?:update|events|collaboration|captures|event-details)$/u.exec(pathname);
  const encoded = direct?.[1] ?? canonical?.[1];
  if (!encoded) return null;
  try { return decodeURIComponent(encoded); } catch { return null; }
}

function proxySessionState(value: string): BealeAppServerSessionCatalogEntry['state'] {
  return value === 'starting' || value === 'running' || value === 'completed' || value === 'failed' || value === 'stopped'
    ? value : 'running';
}

function sendJson(response: ServerResponse, status: number, payload: unknown): void {
  const body = JSON.stringify(payload);
  response.writeHead(status, { 'content-type': 'application/json', 'content-length': Buffer.byteLength(body) });
  response.end(body);
}

function isLoopbackRequest(request: IncomingMessage): boolean {
  return request.socket.remoteAddress === '127.0.0.1' || request.socket.remoteAddress === '::1';
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
  if (status === 502) return 'app_server_failure';
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
        'app-server database integrity failed. Stop active writers and restore a verified backup or run SQLite recovery against the configured database before retrying. The original database must be preserved until recovery is validated.',
        { code: 'database_corrupt', retryable: false });
    }
    if (/failed (?:its|the) integrity check/iu.test(message)) {
      throw new HttpError(500,
        'app-server session integrity validation failed. Stop active writers, preserve the database, and restore or repair the affected session data before retrying.',
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

function boundedSessionStartupTimeout(value: number | undefined): number {
  if (value === undefined) return DEFAULT_SESSION_STARTUP_TIMEOUT_MS;
  if (!Number.isFinite(value) || value <= 0) {
    throw new Error('sessionStartupTimeoutMs must be a positive number.');
  }
  return Math.min(Math.floor(value), 10 * 60_000);
}
