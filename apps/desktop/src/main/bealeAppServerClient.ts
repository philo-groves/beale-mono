import { spawn, spawnSync } from 'node:child_process';
import { chmodSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { join } from 'node:path';
import {
  BEALE_APP_SERVER_CAPABILITIES,
  BEALE_APP_SERVER_CONTROL_VERSION,
  BEALE_APP_SERVER_CONTRACT_TIMESTAMP,
  BEALE_APP_SERVER_OPERATIONS_PATH,
  BEALE_APP_SERVER_SESSIONS_PATH,
  BEALE_APP_SERVER_SHUTDOWN_PATH,
  decodeBealeAppServerSessionCatalog,
  decodeBealeAppServerSessionAttachResult,
  decodeBealeAppServerSessionResult,
  decodeBealeAppServerSessionStartResult,
  decodeBealeAppServerSessionStopResult,
  type BealeAppServerCanonicalResult,
  type BealeAppServerSessionCatalogEntry,
  type AppServerSessionLaunchRequest
  ,type AppServerProtocolOperation
} from '@beale/app-server-runtime/protocol';
import { resolveAppServerNodeCommand, resolveAppServerWorkspaceRoot } from './appServerInvocation';
import { appServerRemoteAccessLaunchEnvironment } from './appServerRemoteAccess';

const DEFAULT_HEALTH_TIMEOUT_MS = 2_000;
const DEFAULT_READY_TIMEOUT_MS = 20_000;
const SESSION_REQUEST_TIMEOUT_MS = 35_000;
const POLL_INTERVAL_MS = 250;
const APP_SERVER_SHUTDOWN_TIMEOUT_MS = 5_000;
const UNRESPONSIVE_APP_SERVER_GRACEFUL_TIMEOUT_MS = 1_500;
const REQUIRED_APP_SERVER_CAPABILITIES = BEALE_APP_SERVER_CAPABILITIES;
const APP_SERVER_LAUNCH_ENVIRONMENT_FLAG = '--beale-launch-environment-file';
const APP_SERVER_ATTACH_EXISTING_FLAG = '--attach-existing';
const APP_SERVER_LAUNCH_ENVIRONMENT_TTL_MS = 30_000;
const appServerEnsureRequests = new Map<string, Promise<BealeAppServerDiscovery>>();
const validatedAppServerDiscoveries = new Map<string, BealeAppServerDiscovery>();

export interface BealeAppServerDiscovery {
  version: number;
  contractTimestamp?: string | null;
  hostMode?: 'tray' | 'headless';
  pid: number;
  host: string;
  port: number;
  localUrl?: string;
  url: string;
  operatorToken: string;
  startedAt: string;
}

export interface AppServerSessionStartResult {
  sessionId: string;
  attemptId: string;
  url: string;
  token: string;
}

export interface AppServerSessionAttachment {
  sessionId: string;
  url: string;
  token: string;
}

export type AppServerCatalogEntry = BealeAppServerSessionCatalogEntry;

export interface EnsureBealeAppServerOptions {
  stateFile?: string;
  readyTimeoutMs?: number;
  healthTimeoutMs?: number;
}

export function bealeAppServerStateFilePath(): string {
  return process.env.BEALE_APP_SERVER_STATE_FILE?.trim() || join(homedir(), '.beale', 'app-server.json');
}

export function readBealeAppServerDiscovery(path: string = bealeAppServerStateFilePath()): BealeAppServerDiscovery | null {
  if (!existsSync(path)) return null;
  let raw: unknown;
  try {
    raw = JSON.parse(readFileSync(path, 'utf8'));
  } catch {
    return null;
  }
  if (!isDiscoveryRecord(raw)) return null;
  return raw;
}

export function readLiveBealeAppServerDiscovery(path: string = bealeAppServerStateFilePath()): BealeAppServerDiscovery | null {
  const record = readBealeAppServerDiscovery(path);
  return record && isBealeAppServerAlive(record) ? record : null;
}

export function isBealeAppServerAlive(record: BealeAppServerDiscovery): boolean {
  if (!Number.isInteger(record.pid) || record.pid <= 0) return false;
  try {
    process.kill(record.pid, 0);
    return true;
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === 'EPERM';
  }
}

/**
 * Returns a live app-server discovery record, launching the platform tray
 * host when available and the headless host otherwise. The
 * BEALE_APP_SERVER_COMMAND and BEALE_APP_SERVER_ARGS_JSON environment
 * variables override the launcher for custom setups.
 */
export function ensureBealeAppServerRunning(options: EnsureBealeAppServerOptions = {}): Promise<BealeAppServerDiscovery> {
  const stateFile = options.stateFile ?? bealeAppServerStateFilePath();
  const discovered = readLiveBealeAppServerDiscovery(stateFile);
  const validated = validatedAppServerDiscoveries.get(stateFile);
  if (discovered && validated && sameAppServerInstance(discovered, validated)) {
    return Promise.resolve(discovered);
  }
  const active = appServerEnsureRequests.get(stateFile);
  if (active) return active;
  const request = ensureBealeAppServerRunningOnce({ ...options, stateFile }).then((record) => {
    validatedAppServerDiscoveries.set(stateFile, record);
    return record;
  });
  appServerEnsureRequests.set(stateFile, request);
  void request.finally(() => {
    if (appServerEnsureRequests.get(stateFile) === request) appServerEnsureRequests.delete(stateFile);
  }).catch(() => undefined);
  return request;
}

async function ensureBealeAppServerRunningOnce(options: EnsureBealeAppServerOptions & { stateFile: string }): Promise<BealeAppServerDiscovery> {
  const stateFile = options.stateFile;
  const healthTimeoutMs = options.healthTimeoutMs ?? DEFAULT_HEALTH_TIMEOUT_MS;

  const existing = readBealeAppServerDiscovery(stateFile);
  if (existing && isBealeAppServerAlive(existing)) {
    const compatibility = await inspectAppServerCompatibility(existing, healthTimeoutMs);
    if (compatibility.status === 'compatible' && !shouldReplaceAppServerWithTray(existing)) return existing;
    if (compatibility.status === 'desktop_older' && compatibility.serverContractTimestamp) {
      throw desktopRestartRequired(compatibility.serverContractTimestamp);
    }
    const activeSessionCount = await fetchActiveAppServerSessionCount(existing, healthTimeoutMs);
    if (activeSessionCount === null) {
      if (compatibility.status !== 'unreachable') throw appServerSessionStateUnavailable();
      await stopUnresponsiveAppServer(existing, stateFile);
    } else {
      if (activeSessionCount > 0) {
        if (compatibility.status === 'compatible') {
          if (shouldLaunchAppServerTrayController(existing, activeSessionCount)) launchAppServerTrayController();
          return existing;
        }
        throw appServerRestartDeferred(activeSessionCount);
      }
      await stopAppServerForUpgrade(existing);
    }
  }

  const launch: LaunchDiagnostics = { stderrTail: '', launchError: null };
  launchAppServerProcess(launch);
  const deadline = Date.now() + (options.readyTimeoutMs ?? DEFAULT_READY_TIMEOUT_MS);
  let lastDetail = launch.launchError ?? 'no discovery record appeared';
  for (;;) {
    const record = readBealeAppServerDiscovery(stateFile);
    if (record && isBealeAppServerAlive(record)) {
      const compatibility = await inspectAppServerCompatibility(record, Math.min(healthTimeoutMs, 1_000));
      if (compatibility.status === 'compatible') {
        return record;
      }
      if (compatibility.status === 'desktop_older' && compatibility.serverContractTimestamp) {
        throw desktopRestartRequired(compatibility.serverContractTimestamp);
      }
      lastDetail = compatibility.status === 'server_older'
        ? 'launched instance advertised an older or incomplete control contract'
        : 'recorded instance did not answer /health';
      if (!isBealeAppServerAlive(record)) lastDetail = 'recorded instance exited';
    }
    if (Date.now() > deadline) {
      const stderr = launch.stderrTail.trim();
      const detail = [lastDetail, stderr ? `launcher stderr: ${stderr}` : null]
        .filter(Boolean)
        .join('; ');
      throw new Error(`The Beale app-server did not become ready within ${Math.round((options.readyTimeoutMs ?? DEFAULT_READY_TIMEOUT_MS) / 1_000)}s (${detail}).`);
    }
    await delay(POLL_INTERVAL_MS);
  }
}

export async function probeAppServerHealth(record: BealeAppServerDiscovery, timeoutMs: number): Promise<boolean> {
  try {
    const response = await fetch(`${appServerControlUrl(record)}/health`, { signal: AbortSignal.timeout(timeoutMs) });
    if (!response.ok) return false;
    const payload = (await response.json()) as { ok?: unknown };
    return payload.ok === true;
  } catch {
    return false;
  }
}

export async function startAppServerSession(
  record: BealeAppServerDiscovery,
  request: AppServerSessionLaunchRequest
): Promise<AppServerSessionStartResult> {
  const response = await fetch(`${appServerControlUrl(record)}${BEALE_APP_SERVER_SESSIONS_PATH}`, {
    method: 'POST',
    headers: {
      authorization: `Bearer ${record.operatorToken}`,
      'content-type': 'application/json'
    },
    body: JSON.stringify(request),
    signal: AbortSignal.timeout(SESSION_REQUEST_TIMEOUT_MS)
  });
  if (!response.ok) {
    const detail = await describeResponse(response);
    throw new Error(`The Beale app-server rejected the session request (${response.status}): ${detail}`);
  }
  const payload: unknown = await response.json().catch(() => null);
  let decoded;
  try {
    decoded = decodeBealeAppServerSessionStartResult(payload);
  } catch {
    throw new Error('The Beale app-server returned an invalid session start response.');
  }
  return {
    sessionId: decoded.session.sessionId,
    attemptId: decoded.attemptId,
    url: appServerWebSocketUrl(appServerControlUrl(record), decoded.transport.path),
    token: decoded.transport.token
  };
}

export async function stopAppServerSession(record: BealeAppServerDiscovery, sessionId: string): Promise<void> {
  const response = await fetch(`${appServerControlUrl(record)}${BEALE_APP_SERVER_SESSIONS_PATH}/${encodeURIComponent(sessionId)}`, {
    method: 'DELETE',
    headers: { authorization: `Bearer ${record.operatorToken}` },
    signal: AbortSignal.timeout(5_000)
  });
  if (!response.ok && response.status !== 404) {
    throw new Error(`The Beale app-server failed to stop session ${sessionId} (${response.status}).`);
  }
  if (response.ok) {
    const payload: unknown = await response.json().catch(() => null);
    const result = decodeBealeAppServerSessionStopResult(payload);
    if (result.sessionId !== sessionId) {
      throw new Error('The Beale app-server returned a mismatched session stop response.');
    }
  }
}

export async function attachAppServerSession(
  record: BealeAppServerDiscovery,
  sessionId: string
): Promise<AppServerSessionAttachment> {
  const response = await fetch(
    `${appServerControlUrl(record)}${BEALE_APP_SERVER_SESSIONS_PATH}/${encodeURIComponent(sessionId)}/attachments`,
    {
      method: 'POST',
      headers: { authorization: `Bearer ${record.operatorToken}` },
      signal: AbortSignal.timeout(SESSION_REQUEST_TIMEOUT_MS)
    }
  );
  if (!response.ok) {
    throw new Error(`The Beale app-server could not attach session ${sessionId} (${response.status}): ${await describeResponse(response)}`);
  }
  const decoded = decodeBealeAppServerSessionAttachResult(await response.json());
  if (decoded.session.sessionId !== sessionId) {
    throw new Error('The Beale app-server returned a mismatched session attachment.');
  }
  return {
    sessionId,
    url: appServerWebSocketUrl(appServerControlUrl(record), decoded.transport.path),
    token: decoded.transport.token
  };
}

export async function fetchAppServerSession(
  record: BealeAppServerDiscovery,
  sessionId: string,
  timeoutMs = 3_000
): Promise<AppServerCatalogEntry | null> {
  try {
    const response = await fetch(`${appServerControlUrl(record)}${BEALE_APP_SERVER_SESSIONS_PATH}/${encodeURIComponent(sessionId)}`, {
      headers: { authorization: `Bearer ${record.operatorToken}` },
      signal: AbortSignal.timeout(timeoutMs)
    });
    if (!response.ok) return null;
    const payload: unknown = await response.json().catch(() => null);
    return decodeBealeAppServerSessionResult(payload).session;
  } catch {
    return null;
  }
}

export async function fetchAppServerCanonicalResult<T>(
  record: BealeAppServerDiscovery,
  path: string,
  options: { method?: 'GET' | 'POST' | 'DELETE'; body?: unknown; signal?: AbortSignal } = {}
): Promise<T> {
  if (!path.startsWith('/v1/')) throw new Error('Canonical app-server paths must start with /v1/.');
  const response = await fetch(`${appServerControlUrl(record)}${path}`, {
    method: options.method ?? 'GET',
    headers: {
      authorization: `Bearer ${record.operatorToken}`,
      ...(options.body === undefined ? {} : { 'content-type': 'application/json' })
    },
    ...(options.body === undefined ? {} : { body: JSON.stringify(options.body) }),
    signal: options.signal ?? AbortSignal.timeout(SESSION_REQUEST_TIMEOUT_MS)
  });
  if (!response.ok) {
    const detail = await describeResponse(response);
    throw new Error(`The Beale app-server canonical request failed (${response.status}): ${detail}`);
  }
  const payload: unknown = await response.json().catch(() => null);
  if (!isCanonicalResult(payload)) {
    throw new Error('The Beale app-server returned an invalid canonical response.');
  }
  return payload.result as T;
}

export async function fetchAppServerCanonicalResultWithRecovery<T>(
  record: BealeAppServerDiscovery,
  path: string,
  options: { method?: 'GET' | 'POST' | 'DELETE'; body?: unknown; signal?: AbortSignal } = {}
): Promise<T> {
  try {
    return await fetchAppServerCanonicalResult<T>(record, path, options);
  } catch (error) {
    if (options.signal?.aborted || !isAppServerTransportFailure(error)) throw error;
    const recovered = await ensureBealeAppServerRunning();
    return fetchAppServerCanonicalResult<T>(recovered, path, options);
  }
}

/**
 * Reads an already-hosted session without entering app-server lifecycle
 * management. A transient socket failure may reattach to the current live
 * discovery record, but it must never launch or replace the process that owns
 * the session being read.
 */
export async function fetchExistingAppServerCanonicalResult<T>(
  record: BealeAppServerDiscovery,
  path: string,
  options: { method?: 'GET' | 'POST' | 'DELETE'; body?: unknown; signal?: AbortSignal } = {}
): Promise<T> {
  try {
    return await fetchAppServerCanonicalResult<T>(record, path, options);
  } catch (error) {
    if (options.signal?.aborted || !isAppServerTransportFailure(error)) throw error;
    await delay(100);
    options.signal?.throwIfAborted();
    const current = readLiveBealeAppServerDiscovery();
    if (!current) throw error;
    return fetchAppServerCanonicalResult<T>(current, path, options);
  }
}

export async function invokeAppServerOperation<T>(request: {
  operation: AppServerProtocolOperation;
  args?: readonly string[];
  input?: unknown;
  profileId?: string;
  signal?: AbortSignal;
}): Promise<T> {
  const server = await ensureBealeAppServerRunning();
  const response = await fetch(`${appServerControlUrl(server)}${BEALE_APP_SERVER_OPERATIONS_PATH}`, {
    method: 'POST',
    headers: { authorization: `Bearer ${server.operatorToken}`, 'content-type': 'application/json' },
    body: JSON.stringify({
      operation: request.operation,
      ...(request.args ? { args: request.args } : {}),
      ...(request.input !== undefined ? { input: request.input } : {}),
      ...(request.profileId ? { profileId: request.profileId } : {})
    }),
    signal: request.signal ?? AbortSignal.timeout(5 * 60_000)
  });
  if (!response.ok) throw new Error(`app-server ${request.operation} failed: ${await describeResponse(response)}`);
  const payload = await response.json() as { controlVersion?: unknown; result?: unknown };
  if (payload.controlVersion !== BEALE_APP_SERVER_CONTROL_VERSION) throw new Error('The app-server returned an incompatible operation response.');
  return payload.result as T;
}

function isAppServerTransportFailure(error: unknown): boolean {
  if (error instanceof TypeError) return true;
  if (!(error instanceof Error)) return false;
  const cause = error.cause;
  return cause instanceof Error && (
    cause.message.includes('ECONNRESET')
    || cause.message.includes('ECONNREFUSED')
    || cause.message.includes('UND_ERR_SOCKET')
  );
}

/**
 * Environment variables that belong to the launching host's debug tooling
 * (for example vitest instrumentation) and would break a plain detached
 * Node child if inherited.
 */
const CHILD_ENV_BLOCKLIST = ['NODE_OPTIONS', 'NODE_V8_COVERAGE'];
const LAUNCH_STDERR_TAIL_CHARS = 2_000;

interface LaunchDiagnostics {
  stderrTail: string;
  launchError: string | null;
}

interface AppServerLaunch {
  command: string;
  args: string[];
  trayHost: boolean;
  trayIconPath?: string;
  launchServices?: boolean;
}

function launchAppServerProcess(
  diagnostics: LaunchDiagnostics,
  options: { attachExisting?: boolean } = {}
): boolean {
  const childEnv: NodeJS.ProcessEnv = {
    ...appServerRemoteAccessLaunchEnvironment(),
    ...process.env
  };
  for (const key of CHILD_ENV_BLOCKLIST) delete childEnv[key];

  const configuredCommand = process.env.BEALE_APP_SERVER_COMMAND?.trim();
  const launch = configuredCommand
    ? { command: configuredCommand, args: parseEnvironmentArgs('BEALE_APP_SERVER_ARGS_JSON'), trayHost: false }
    : defaultAppServerLaunch();
  if (options.attachExisting && !launch.trayHost) return false;
  if (options.attachExisting) launch.args.push(APP_SERVER_ATTACH_EXISTING_FLAG);
  if (launch.trayIconPath && !childEnv.BEALE_APP_SERVER_ICON?.trim()) {
    childEnv.BEALE_APP_SERVER_ICON = launch.trayIconPath;
  }
  if (launch.trayHost) delete childEnv.ELECTRON_RUN_AS_NODE;

  let launchEnvironmentDirectory: string | null = null;
  if (launch.launchServices) {
    const launchEnvironment = writePrivateAppServerLaunchEnvironment(childEnv);
    launchEnvironmentDirectory = launchEnvironment.directory;
    launch.args.push(APP_SERVER_LAUNCH_ENVIRONMENT_FLAG, launchEnvironment.path);
  }

  const child = spawn(launch.command, launch.args, {
    detached: true,
    stdio: ['ignore', 'ignore', 'pipe'],
    windowsHide: true,
    env: childEnv
  });
  child.stderr?.setEncoding('utf8');
  child.stderr?.on('data', (chunk: string) => {
    diagnostics.stderrTail += chunk;
    if (diagnostics.stderrTail.length > LAUNCH_STDERR_TAIL_CHARS) {
      diagnostics.stderrTail = diagnostics.stderrTail.slice(-LAUNCH_STDERR_TAIL_CHARS);
    }
  });
  child.on('error', (error) => {
    diagnostics.launchError = `${launch.command}: ${error.message}`;
    if (launchEnvironmentDirectory) {
      rmSync(launchEnvironmentDirectory, { recursive: true, force: true });
      launchEnvironmentDirectory = null;
    }
  });
  if (launchEnvironmentDirectory) {
    const directory = launchEnvironmentDirectory;
    const cleanup = setTimeout(() => rmSync(directory, { recursive: true, force: true }), APP_SERVER_LAUNCH_ENVIRONMENT_TTL_MS);
    cleanup.unref();
  }
  if (process.platform !== 'win32') {
    child.unref();
  }
  return true;
}

function launchAppServerTrayController(): void {
  launchAppServerProcess({ stderrTail: '', launchError: null }, { attachExisting: true });
}

function defaultAppServerLaunch(): AppServerLaunch {
  const workspaceRoot = resolveAppServerWorkspaceRoot();
  if (!workspaceRoot) {
    throw new Error('The Beale app-server is not running and no workspace root was found to launch one. Start it with `pnpm --filter @beale/app-server start` or set BEALE_APP_SERVER_COMMAND.');
  }
  const appServerRoot = join(workspaceRoot, 'app-server');
  if (isAppServerTrayPlatform(process.platform) && typeof process.versions.electron === 'string') {
    const electronPath = process.platform === 'win32'
      ? ['dist', 'electron.exe']
      : ['dist', 'Electron.app'];
    const electronRuntime = [
      join(appServerRoot, 'node_modules', 'electron', ...electronPath),
      join(workspaceRoot, 'apps', 'desktop', 'node_modules', 'electron', ...electronPath)
    ].find((candidate) => existsSync(candidate));
    if (electronRuntime) {
      requireBuiltAppServerTrayEntry(appServerRoot);
      const trayIconPath = process.platform === 'darwin'
        ? join(workspaceRoot, 'apps', 'desktop', 'resources', 'MenuBarIcon.svg')
        : undefined;
      if (process.platform === 'darwin') {
        return {
          command: '/usr/bin/open',
          args: ['-n', '-g', '--stderr', '/dev/stderr', '-a', electronRuntime, '--args', appServerRoot],
          trayHost: true,
          launchServices: true,
          ...(trayIconPath && existsSync(trayIconPath) ? { trayIconPath } : {})
        };
      }
      return {
        command: electronRuntime,
        args: [appServerRoot],
        trayHost: true,
        ...(trayIconPath && existsSync(trayIconPath) ? { trayIconPath } : {})
      };
    }
  }
  const headlessEntry = join(appServerRoot, 'dist', 'headlessMain.js');
  if (!existsSync(headlessEntry)) {
    throw new Error(`The Beale app-server entry was not found at ${headlessEntry}. Build the workspace packages first.`);
  }
  return { command: resolveAppServerNodeCommand(), args: [headlessEntry], trayHost: false };
}

export function writePrivateAppServerLaunchEnvironment(environment: NodeJS.ProcessEnv): {
  directory: string;
  path: string;
} {
  const directory = mkdtempSync(join(tmpdir(), 'beale-app-server-launch-'));
  chmodSync(directory, 0o700);
  const path = join(directory, 'environment.json');
  writeFileSync(path, JSON.stringify(environment), { encoding: 'utf8', flag: 'wx', mode: 0o600 });
  return { directory, path };
}

export function isAppServerTrayPlatform(platform: NodeJS.Platform): boolean {
  return platform === 'darwin' || platform === 'win32';
}

export function requireBuiltAppServerTrayEntry(
  appServerRoot: string,
  pathExists: (path: string) => boolean = existsSync
): string {
  const trayEntry = join(appServerRoot, 'dist', 'trayBootstrap.js');
  if (!pathExists(trayEntry)) {
    throw new Error(`The Beale app-server tray entry was not found at ${trayEntry}. Build the workspace packages first.`);
  }
  return trayEntry;
}

export function shouldReplaceAppServerWithTray(
  record: BealeAppServerDiscovery,
  platform: NodeJS.Platform = process.platform,
  configuredCommand: string | undefined = process.env.BEALE_APP_SERVER_COMMAND
): boolean {
  return isAppServerTrayPlatform(platform)
    && !configuredCommand?.trim()
    && record.hostMode !== 'tray';
}

export function shouldLaunchAppServerTrayController(
  record: BealeAppServerDiscovery,
  activeSessionCount: number,
  platform: NodeJS.Platform = process.platform,
  configuredCommand: string | undefined = process.env.BEALE_APP_SERVER_COMMAND
): boolean {
  return activeSessionCount > 0
    && shouldReplaceAppServerWithTray(record, platform, configuredCommand);
}

function parseEnvironmentArgs(name: string): string[] {
  const raw = process.env[name]?.trim();
  if (!raw) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new Error(`${name} must be a JSON array of strings.`);
  }
  if (!Array.isArray(parsed) || parsed.some((value) => typeof value !== 'string')) {
    throw new Error(`${name} must be a JSON array of strings.`);
  }
  return parsed as string[];
}

async function describeResponse(response: Response): Promise<string> {
  const text = await response.text().catch(() => '');
  if (!text.trim()) return response.statusText || 'no detail';
  try {
    const parsed = JSON.parse(text) as { error?: unknown };
    if (typeof parsed.error === 'string') return parsed.error.slice(-500);
    if (isRecord(parsed.error) && typeof parsed.error.message === 'string') {
      return parsed.error.message.slice(-500);
    }
  } catch {
    // Fall through to the raw text.
  }
  return text.slice(-500);
}

function appServerWebSocketUrl(baseUrl: string, path: string): string {
  const base = new URL(baseUrl);
  base.protocol = base.protocol === 'https:' ? 'wss:' : 'ws:';
  return new URL(path, base).toString();
}

export function appServerControlUrl(record: BealeAppServerDiscovery): string {
  return record.localUrl?.trim() || record.url;
}

export async function restartBealeAppServer(): Promise<BealeAppServerDiscovery> {
  const existing = readBealeAppServerDiscovery();
  if (existing && isBealeAppServerAlive(existing)) await stopAppServerForUpgrade(existing);
  return ensureBealeAppServerRunning();
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function isDiscoveryRecord(value: unknown): value is BealeAppServerDiscovery {
  return isRecord(value)
    && typeof value.version === 'number'
    && (value.hostMode === undefined || value.hostMode === 'tray' || value.hostMode === 'headless')
    && typeof value.pid === 'number'
    && typeof value.host === 'string'
    && typeof value.port === 'number'
    && (value.localUrl === undefined || typeof value.localUrl === 'string')
    && typeof value.url === 'string'
    && typeof value.operatorToken === 'string'
    && typeof value.startedAt === 'string';
}

function sameAppServerInstance(first: BealeAppServerDiscovery, second: BealeAppServerDiscovery): boolean {
  return first.pid === second.pid
    && first.startedAt === second.startedAt
    && appServerControlUrl(first) === appServerControlUrl(second)
    && first.operatorToken === second.operatorToken;
}

function isCanonicalResult(value: unknown): value is BealeAppServerCanonicalResult {
  return isRecord(value)
    && value.controlVersion === BEALE_APP_SERVER_CONTROL_VERSION
    && isRecord(value.workspace)
    && typeof value.workspace.workspaceId === 'string'
    && 'result' in value;
}

export type AppServerCompatibilityStatus =
  | 'compatible'
  | 'server_older'
  | 'desktop_older'
  | 'unreachable';

export interface AppServerCompatibilityResult {
  status: AppServerCompatibilityStatus;
  serverContractTimestamp: string | null;
}

export class BealeDesktopRestartRequiredError extends Error {
  public constructor(public readonly serverContractTimestamp: string) {
    super(
      `The running Beale desktop contract (${BEALE_APP_SERVER_CONTRACT_TIMESTAMP}) is older than the app-server contract (${serverContractTimestamp}). Restart Beale to load the update.`
    );
    this.name = 'BealeDesktopRestartRequiredError';
  }
}

let desktopRestartRequiredHandler: ((error: BealeDesktopRestartRequiredError) => void) | null = null;

export function setBealeDesktopRestartRequiredHandler(
  handler: ((error: BealeDesktopRestartRequiredError) => void) | null
): void {
  desktopRestartRequiredHandler = handler;
}

export async function probeAppServerCompatibility(record: BealeAppServerDiscovery, timeoutMs: number): Promise<boolean> {
  return (await inspectAppServerCompatibility(record, timeoutMs)).status === 'compatible';
}

export async function inspectAppServerCompatibility(
  record: BealeAppServerDiscovery,
  timeoutMs: number
): Promise<AppServerCompatibilityResult> {
  try {
    const response = await fetch(`${appServerControlUrl(record)}/health`, { signal: AbortSignal.timeout(timeoutMs) });
    if (!response.ok) return { status: 'unreachable', serverContractTimestamp: null };
    const payload: unknown = await response.json();
    if (!isRecord(payload) || payload.ok !== true) {
      return { status: 'unreachable', serverContractTimestamp: null };
    }
    const serverContractTimestamp = validContractTimestamp(payload.contractTimestamp)
      ? payload.contractTimestamp
      : null;
    if (!serverContractTimestamp) {
      return { status: 'server_older', serverContractTimestamp: null };
    }
    if (serverContractTimestamp > BEALE_APP_SERVER_CONTRACT_TIMESTAMP) {
      return { status: 'desktop_older', serverContractTimestamp };
    }
    if (serverContractTimestamp < BEALE_APP_SERVER_CONTRACT_TIMESTAMP) {
      return { status: 'server_older', serverContractTimestamp };
    }
    if (payload.controlVersion !== BEALE_APP_SERVER_CONTROL_VERSION) {
      return { status: 'server_older', serverContractTimestamp };
    }
    if (!Array.isArray(payload.capabilities)) {
      return { status: 'server_older', serverContractTimestamp };
    }
    const capabilities = new Set(payload.capabilities.filter((value): value is string => typeof value === 'string'));
    return {
      status: REQUIRED_APP_SERVER_CAPABILITIES.every((capability) => capabilities.has(capability))
        ? 'compatible'
        : 'server_older',
      serverContractTimestamp
    };
  } catch {
    return { status: 'unreachable', serverContractTimestamp: null };
  }
}

async function stopAppServerForUpgrade(record: BealeAppServerDiscovery): Promise<void> {
  const activeSessionCount = await fetchActiveAppServerSessionCount(record, DEFAULT_HEALTH_TIMEOUT_MS);
  if (activeSessionCount === null) throw appServerSessionStateUnavailable();
  if (activeSessionCount > 0) throw appServerRestartDeferred(activeSessionCount);
  for (const path of [BEALE_APP_SERVER_SHUTDOWN_PATH, '/shutdown']) {
    try {
      const response = await fetch(`${appServerControlUrl(record)}${path}`, {
        method: 'POST',
        headers: { authorization: `Bearer ${record.operatorToken}` },
        signal: AbortSignal.timeout(2_000)
      });
      if (response.ok) break;
      if (response.status === 409) {
        throw new AppServerShutdownRefusedError(await describeResponse(response));
      }
    } catch (error) {
      if (error instanceof AppServerShutdownRefusedError) throw error;
      // Try the pre-versioned path before falling back to process termination.
    }
  }
  const gracefulDeadline = Date.now() + 2_000;
  while (Date.now() < gracefulDeadline && isBealeAppServerAlive(record)) {
    await delay(100);
  }
  if (isBealeAppServerAlive(record)) {
    if (record.pid === process.pid) {
      throw new Error('Refusing to terminate the current process while replacing the Beale app-server.');
    }
    try {
      process.kill(record.pid, 'SIGTERM');
    } catch (error) {
      if (isBealeAppServerAlive(record)) {
        throw new Error(`Unable to stop the older Beale app-server process ${record.pid}: ${error instanceof Error ? error.message : String(error)}`);
      }
    }
  }
  const deadline = Date.now() + APP_SERVER_SHUTDOWN_TIMEOUT_MS;
  while (Date.now() < deadline && isBealeAppServerAlive(record)) {
    await delay(100);
  }
  if (isBealeAppServerAlive(record)) {
    throw new Error(`The older Beale app-server process ${record.pid} did not stop.`);
  }
}

async function stopUnresponsiveAppServer(
  record: BealeAppServerDiscovery,
  stateFile: string
): Promise<void> {
  const current = readBealeAppServerDiscovery(stateFile);
  if (!current || !sameAppServerInstance(record, current)) {
    throw new Error('The unresponsive Beale app-server discovery changed before it could be restarted. Retry startup.');
  }
  if (readAppServerDiscoveryLockOwner(stateFile) !== record.pid) {
    throw new Error('The unresponsive Beale app-server does not own its discovery lock. Refusing to terminate an unverified process.');
  }
  if (record.pid === process.pid) {
    throw new Error('Refusing to terminate the current process while recovering the Beale app-server.');
  }
  try {
    process.kill(record.pid, 'SIGTERM');
  } catch (error) {
    if (isBealeAppServerAlive(record)) {
      throw new Error(`Unable to stop the unresponsive Beale app-server process ${record.pid}: ${error instanceof Error ? error.message : String(error)}`);
    }
  }
  const gracefulDeadline = Date.now() + UNRESPONSIVE_APP_SERVER_GRACEFUL_TIMEOUT_MS;
  while (Date.now() < gracefulDeadline && isBealeAppServerAlive(record)) {
    await delay(100);
  }
  if (isBealeAppServerAlive(record)) {
    if (!appServerOwnershipMatches(record, stateFile)) {
      throw new Error('The unresponsive Beale app-server ownership changed during shutdown. Refusing to force-terminate it.');
    }
    try {
      process.kill(record.pid, 'SIGKILL');
    } catch (error) {
      if (isBealeAppServerAlive(record)) {
        throw new Error(`Unable to force-stop the unresponsive Beale app-server process ${record.pid}: ${error instanceof Error ? error.message : String(error)}`);
      }
    }
  }
  const forcedDeadline = Date.now() + APP_SERVER_SHUTDOWN_TIMEOUT_MS;
  while (Date.now() < forcedDeadline && isBealeAppServerAlive(record)) {
    await delay(100);
  }
  if (isBealeAppServerAlive(record)) {
    throw new Error(`The unresponsive Beale app-server process ${record.pid} did not stop.`);
  }
}

function appServerOwnershipMatches(record: BealeAppServerDiscovery, stateFile: string): boolean {
  const current = readBealeAppServerDiscovery(stateFile);
  return Boolean(current && sameAppServerInstance(record, current))
    && readAppServerDiscoveryLockOwner(stateFile) === record.pid;
}

function readAppServerDiscoveryLockOwner(stateFile: string): number | null {
  try {
    const parsed = JSON.parse(readFileSync(`${stateFile}.lock`, 'utf8')) as { pid?: unknown };
    return Number.isInteger(parsed.pid) && Number(parsed.pid) > 0 ? Number(parsed.pid) : null;
  } catch {
    return null;
  }
}

async function fetchActiveAppServerSessionCount(
  record: BealeAppServerDiscovery,
  timeoutMs: number
): Promise<number | null> {
  try {
    const response = await fetch(`${appServerControlUrl(record)}${BEALE_APP_SERVER_SESSIONS_PATH}`, {
      headers: { authorization: `Bearer ${record.operatorToken}` },
      signal: AbortSignal.timeout(timeoutMs)
    });
    if (!response.ok) return null;
    const catalog = decodeBealeAppServerSessionCatalog(await response.json());
    return catalog.sessions.filter((session) => session.state === 'starting' || session.state === 'running').length;
  } catch {
    return null;
  }
}

function appServerSessionStateUnavailable(): Error {
  return new Error('The Beale app-server did not confirm that no research sessions are active. Refusing to restart it; retry after the existing server responds.');
}

function appServerRestartDeferred(activeSessionCount: number): Error {
  return new Error(
    `The Beale app-server cannot restart while ${activeSessionCount} research ${activeSessionCount === 1 ? 'session is' : 'sessions are'} active. Try again after the session finishes.`
  );
}

class AppServerShutdownRefusedError extends Error {
  public constructor(message: string) {
    super(message);
    this.name = 'AppServerShutdownRefusedError';
  }
}

function validContractTimestamp(value: unknown): value is string {
  return typeof value === 'string'
    && /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/.test(value)
    && Number.isFinite(Date.parse(value));
}

function desktopRestartRequired(serverContractTimestamp: string): BealeDesktopRestartRequiredError {
  const error = new BealeDesktopRestartRequiredError(serverContractTimestamp);
  desktopRestartRequiredHandler?.(error);
  return error;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}
