import { readFile } from 'node:fs/promises';
import { existsSync, readFileSync } from 'node:fs';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { homedir } from 'node:os';
import { basename, dirname, join } from 'node:path';
import {
  BEALE_APP_SERVER_CAPABILITIES,
  BEALE_APP_SERVER_CONTROL_VERSION,
  BEALE_APP_SERVER_CONTRACT_TIMESTAMP,
  BEALE_APP_SERVER_OPERATIONS_PATH,
  BEALE_APP_SERVER_SHUTDOWN_PATH,
  appServerProtocolFailure,
  appServerProtocolSuccess,
  type AppServerProtocolOperation
} from './protocol.js';
import { installUndiciTypeOfServiceCompatibility } from './node-network-compatibility.js';

installUndiciTypeOfServiceCompatibility();

interface DiscoveryRecord { localUrl?: string; url: string; operatorToken: string }

export async function runAppServerClient(argv: readonly string[], requestId?: string): Promise<void> {
  const operation = operationForArguments(argv);
  if (!operation) throw new Error(`Unsupported app-server client command: ${argv.slice(0, 2).join(' ')}`);
  try {
    const input = operation === 'provider.complete' ? await readStandardInput() : await readInputOption(argv);
    const discovery = await ensureDiscovery();
    const response = await fetch(`${discovery.localUrl?.trim() || discovery.url}${BEALE_APP_SERVER_OPERATIONS_PATH}`, {
      method: 'POST',
      headers: { authorization: `Bearer ${discovery.operatorToken}`, 'content-type': 'application/json' },
      body: JSON.stringify({ operation, args: argv, ...(input !== undefined ? { input } : {}), ...profileSelection(input) })
    });
    const payload = await response.json() as {
      controlVersion?: unknown;
      result?: unknown;
      error?: string | { code?: unknown; message?: unknown; retryable?: unknown };
    };
    if (!response.ok) {
      const structuredError = typeof payload.error === 'object' && payload.error !== null ? payload.error : null;
      process.stdout.write(`${JSON.stringify(appServerProtocolFailure(
        operation,
        typeof structuredError?.code === 'string' ? structuredError.code : 'app_server_operation_failed',
        typeof structuredError?.message === 'string'
          ? structuredError.message
          : typeof payload.error === 'string' ? payload.error : `HTTP ${response.status}`,
        structuredError?.retryable === true,
        requestId
      ))}\n`);
      process.exitCode = 1;
      return;
    }
    if (payload.controlVersion !== BEALE_APP_SERVER_CONTROL_VERSION) throw new Error('App-server control version mismatch.');
    process.stdout.write(`${JSON.stringify(appServerProtocolSuccess(operation, payload.result, requestId))}\n`);
  } catch (error) {
    process.stdout.write(`${JSON.stringify(appServerProtocolFailure(
      operation, 'app_server_operation_failed', error instanceof Error ? error.message : String(error), false, requestId
    ))}\n`);
    process.exitCode = 1;
  }
}

export async function runAppServerUtilityClient(argv: readonly string[]): Promise<void> {
  const operation = utilityOperationForArguments(argv);
  if (!operation) throw new Error(`Unsupported app-server client command: ${argv.slice(0, 3).join(' ')}`);
  const discovery = await ensureDiscovery();
  const response = await fetch(`${discovery.localUrl?.trim() || discovery.url}${BEALE_APP_SERVER_OPERATIONS_PATH}`, {
    method: 'POST',
    headers: { authorization: `Bearer ${discovery.operatorToken}`, 'content-type': 'application/json' },
    body: JSON.stringify({ operation, args: argv, ...profileSelection() })
  });
  const payload = await response.json() as { controlVersion?: unknown; result?: unknown; error?: string | { message?: unknown } };
  const structuredError = typeof payload.error === 'object' && payload.error !== null ? payload.error : null;
  if (!response.ok) throw new Error(typeof structuredError?.message === 'string'
    ? structuredError.message
    : typeof payload.error === 'string' ? payload.error : `HTTP ${response.status}`);
  if (payload.controlVersion !== BEALE_APP_SERVER_CONTROL_VERSION) throw new Error('App-server control version mismatch.');
  renderUtilityResult(operation, payload.result, argv.includes('--json'));
}

function readDiscovery(): DiscoveryRecord {
  const path = process.env.BEALE_APP_SERVER_STATE_FILE?.trim() || join(homedir(), '.beale', 'app-server.json');
  const value = JSON.parse(readFileSync(path, 'utf8')) as Partial<DiscoveryRecord>;
  if ((!value.localUrl && !value.url) || !value.operatorToken) throw new Error('The Beale app-server discovery record is invalid.');
  return value as DiscoveryRecord;
}

async function ensureDiscovery(): Promise<DiscoveryRecord> {
  const current = tryReadDiscovery();
  if (current) {
    const compatibility = await inspectCompatibility(current);
    if (compatibility === 'compatible') return current;
    if (compatibility === 'client_older') {
      throw new Error(
        `The app-server contract is newer than this client (${BEALE_APP_SERVER_CONTRACT_TIMESTAMP}). Restart or update Beale before retrying.`
      );
    }
    if (compatibility === 'server_older') await stopOlderAppServer(current);
  }
  const headlessEntry = fileURLToPath(new URL('../../../app-server/dist/headlessMain.js', import.meta.url));
  if (!existsSync(headlessEntry)) throw new Error('The Beale app-server is not built. Run pnpm build first.');
  const child = spawn(process.execPath, [headlessEntry], {
    detached: true,
    stdio: 'ignore',
    windowsHide: true,
    env: { ...process.env, ELECTRON_RUN_AS_NODE: '1' }
  });
  child.unref();
  const deadline = Date.now() + 15_000;
  while (Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 100));
    const discovery = tryReadDiscovery();
    if (discovery && await inspectCompatibility(discovery) === 'compatible') return discovery;
  }
  throw new Error('The Beale app-server did not become ready.');
}

function tryReadDiscovery(): DiscoveryRecord | null {
  try { return readDiscovery(); } catch { return null; }
}

type AppServerCompatibility = 'compatible' | 'server_older' | 'client_older' | 'unreachable';

async function inspectCompatibility(discovery: DiscoveryRecord): Promise<AppServerCompatibility> {
  try {
    const response = await fetch(`${discovery.localUrl?.trim() || discovery.url}/health`, { signal: AbortSignal.timeout(1_000) });
    if (!response.ok) return 'unreachable';
    const payload = await response.json() as Record<string, unknown>;
    const timestamp = typeof payload.contractTimestamp === 'string' ? payload.contractTimestamp : null;
    if (!timestamp || timestamp < BEALE_APP_SERVER_CONTRACT_TIMESTAMP) return 'server_older';
    if (timestamp > BEALE_APP_SERVER_CONTRACT_TIMESTAMP) return 'client_older';
    if (payload.controlVersion !== BEALE_APP_SERVER_CONTROL_VERSION || !Array.isArray(payload.capabilities)) {
      return 'server_older';
    }
    const capabilities = new Set(payload.capabilities.filter((value): value is string => typeof value === 'string'));
    return BEALE_APP_SERVER_CAPABILITIES.every((capability) => capabilities.has(capability))
      ? 'compatible'
      : 'server_older';
  } catch {
    return 'unreachable';
  }
}

async function stopOlderAppServer(discovery: DiscoveryRecord): Promise<void> {
  let response: Response;
  try {
    response = await fetch(`${discovery.localUrl?.trim() || discovery.url}${BEALE_APP_SERVER_SHUTDOWN_PATH}`, {
      method: 'POST',
      headers: { authorization: `Bearer ${discovery.operatorToken}` },
      signal: AbortSignal.timeout(2_000)
    });
  } catch (error) {
    throw new Error(`The older Beale app-server could not be stopped: ${error instanceof Error ? error.message : String(error)}`);
  }
  if (!response.ok) {
    const payload = await response.json().catch(() => null) as { error?: string | { message?: unknown } } | null;
    const structuredError = typeof payload?.error === 'object' && payload.error !== null ? payload.error : null;
    const message = typeof structuredError?.message === 'string'
      ? structuredError.message
      : typeof payload?.error === 'string' ? payload.error : null;
    const detail = message ? ` ${message}` : '';
    throw new Error(`The older Beale app-server refused a safe restart (HTTP ${response.status}).${detail}`);
  }
  const deadline = Date.now() + 5_000;
  while (Date.now() < deadline) {
    if (await inspectCompatibility(discovery) === 'unreachable') return;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error('The older Beale app-server did not stop within 5 seconds.');
}

async function readInputOption(argv: readonly string[]): Promise<unknown | undefined> {
  const index = argv.indexOf('--input');
  const path = index >= 0 ? argv[index + 1] : undefined;
  return path ? JSON.parse(await readFile(path, 'utf8')) as unknown : undefined;
}

async function readStandardInput(): Promise<unknown> {
  let raw = '';
  process.stdin.setEncoding('utf8');
  for await (const chunk of process.stdin) raw += String(chunk);
  return JSON.parse(raw) as unknown;
}

function profileSelection(input?: unknown): { profileId?: string } {
  // A profile carried by typed input supports the first operation for a new
  // workspace. The app-server still rejects it if its authoritative registry
  // already associates that workspace with another profile.
  const inputProfileId = profileIdFromInput(input);
  if (inputProfileId) return { profileId: inputProfileId };
  if (workspaceIdFromInput(input)) return {};
  const configuredProfileId = process.env.APP_SERVER_PROFILE_ID?.trim();
  if (configuredProfileId) return { profileId: configuredProfileId };
  const databasePath = process.env.APP_SERVER_DATABASE_PATH?.trim();
  if (!databasePath) return {};
  const directory = dirname(databasePath);
  const parent = basename(directory);
  // A profile-isolated canonical path carries an unambiguous profile ID. An
  // arbitrary override does not: workspace-scoped operations must let the
  // app-server registry select the workspace's actual profile.
  return basename(dirname(directory)) === 'profiles' ? { profileId: parent } : {};
}

function workspaceIdFromInput(input: unknown): string | null {
  if (!input || typeof input !== 'object' || Array.isArray(input)) return null;
  const workspaceId = (input as Record<string, unknown>).workspaceId;
  return typeof workspaceId === 'string' && workspaceId.trim() ? workspaceId.trim() : null;
}

function profileIdFromInput(input: unknown): string | null {
  if (!input || typeof input !== 'object' || Array.isArray(input)) return null;
  const record = input as Record<string, unknown>;
  if (typeof record.researchProfileId === 'string' && record.researchProfileId.trim()) {
    return record.researchProfileId.trim();
  }
  const profile = record.researchProfile;
  if (!profile || typeof profile !== 'object' || Array.isArray(profile)) return null;
  const profileId = (profile as Record<string, unknown>).profileId;
  return typeof profileId === 'string' && profileId.trim() ? profileId.trim() : null;
}

function operationForArguments(argv: readonly string[]): AppServerProtocolOperation | null {
  const [group, command] = argv;
  if (group === 'protocol' && command === 'describe') return 'protocol.describe';
  if (group === 'complete') return 'provider.complete';
  if (group === 'workspace' && command === 'state') return 'workspace.state';
  if (group === 'registry' && command === 'state') return 'registry.state';
  const normalized = command?.replaceAll('-', '_');
  if (group === 'session' && normalized) return `session.${normalized}` as AppServerProtocolOperation;
  if (group === 'knowledge') {
    const mapping: Record<string, AppServerProtocolOperation> = {
      summary: 'memory.summary', notification_feed: 'memory.notification_feed', dreaming_prepare: 'dreaming.prepare',
      dreaming_parse_plan: 'dreaming.parse_plan', dreaming_apply: 'dreaming.apply', dreaming_record_failure: 'dreaming.record_failure',
      dreaming_restore: 'dreaming.restore', runbook_get: 'runbook.get', report_list: 'report.list', report_get: 'report.get', artifact_resolve: 'artifact.resolve'
    };
    return normalized ? mapping[normalized] ?? null : null;
  }
  if (group === 'harness') {
    const mapping: Record<string, AppServerProtocolOperation> = {
      model_job_resolve: 'model_job.resolve', provider_describe: 'provider.describe', source_inspect: 'source.inspect',
      source_materialize: 'source.materialize', plugin_list: 'plugin.list', plugin_add_filesystem: 'plugin.add_filesystem',
      plugin_add_repository: 'plugin.add_repository', plugin_set_enabled: 'plugin.set_enabled', plugin_remove: 'plugin.remove',
      plugin_runtime: 'plugin.runtime', maintenance_summary: 'maintenance.summary', maintenance_run: 'maintenance.run'
    };
    return normalized ? mapping[normalized] ?? null : null;
  }
  return null;
}

function utilityOperationForArguments(argv: readonly string[]): AppServerProtocolOperation | null {
  const [group, command] = argv;
  if (group === 'profile' && command === 'resolve') return 'profile.resolve';
  if (group === 'auth' && command === 'list') return 'auth.list';
  if (group === 'auth' && command === 'status') return 'auth.status';
  if (group === 'auth' && command === 'verify') return 'auth.verify';
  if (group === 'auth' && command === 'logout') return 'auth.logout';
  if (group === 'models' && command === 'list') return 'model.list';
  if (group === 'tools' && command === 'list') return 'tools.list';
  if (group === 'tools' && command === 'config') return 'tools.config';
  if (group === 'config' && command === 'show') return 'config.show';
  if (group === 'config' && command === 'set') return 'config.set';
  return null;
}

function renderUtilityResult(operation: AppServerProtocolOperation, value: unknown, json: boolean): void {
  if (json || operation === 'profile.resolve' || operation === 'tools.list'
    || operation === 'tools.config' || operation === 'config.show' || operation === 'config.set'
    || operation === 'model.list') {
    process.stdout.write(`${JSON.stringify(value, null, 2)}\n`);
    return;
  }
  if (operation === 'auth.list' && Array.isArray(value)) {
    for (const entry of value) {
      const provider = entry as { id?: unknown; name?: unknown; authMethods?: unknown };
      process.stdout.write(`${String(provider.id)}\t${String(provider.name)}\t${Array.isArray(provider.authMethods) ? provider.authMethods.join(', ') : ''}\n`);
    }
    return;
  }
  if (operation === 'auth.status') {
    const status = value as { authFile?: unknown; providers?: Array<{ id?: unknown; name?: unknown; authMethods?: unknown; storedCredentialType?: unknown }> };
    process.stdout.write(`Auth file: ${String(status.authFile ?? '')}\n`);
    for (const provider of status.providers ?? []) {
      process.stdout.write(`${String(provider.id)}\t${String(provider.name)}\t${Array.isArray(provider.authMethods) ? provider.authMethods.join(', ') : ''}\t${String(provider.storedCredentialType ?? 'not stored')}\n`);
    }
    return;
  }
  if (operation === 'auth.verify') {
    const result = value as { providerName?: unknown; providerId?: unknown; modelId?: unknown; configured?: unknown; source?: unknown };
    const source = result.source ? ` via ${String(result.source)}` : '';
    process.stdout.write(`${String(result.providerName)} (${String(result.providerId)}) model ${String(result.modelId)}: ${result.configured ? `configured${source}` : 'not configured'}\n`);
    return;
  }
  if (operation === 'auth.logout') {
    process.stdout.write(`Removed stored credentials for ${String((value as { providerId?: unknown }).providerId)}.\n`);
    return;
  }
  process.stdout.write(`${JSON.stringify(value)}\n`);
}
