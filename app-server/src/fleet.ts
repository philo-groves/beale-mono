import { execFile, spawn } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { chmodSync, existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { homedir, platform } from 'node:os';
import { dirname, join } from 'node:path';
import { promisify } from 'node:util';
import { APP_SERVER_SESSION_LAUNCH_VERSION, BEALE_APP_SERVER_CONTRACT_TIMESTAMP, BEALE_APP_SERVER_CONTROL_VERSION, BEALE_APP_SERVER_OPERATIONS_PATH, BEALE_APP_SERVER_PROVIDERS_PATH, BEALE_APP_SERVER_PROVIDER_LIMITS_PATH, BEALE_APP_SERVER_QUICK_CHATS_PATH, BEALE_APP_SERVER_SERVER_PATH, BEALE_APP_SERVER_WORKSPACES_PATH, BEALE_APP_SERVER_SESSIONS_PATH, BEALE_APP_SERVER_SUPERVISOR_HTTPS_PORT, BEALE_APP_SERVER_SUPERVISOR_RESTART_PATH, DEFAULT_BROWSER_HOME_PAGE, bealeAppServerWorkspaceDetailsPath, decodeBealeAppServerDescriptor, decodeBealeAppServerProviderCatalog, decodeBealeProviderUsageLimits, decodeBealeAppServerQuickChatList, decodeBealeAppServerQuickChatStartInput, decodeBealeAppServerQuickChatStartResult, decodeBealeAppServerWorkspaceDetails, decodeBealeAppServerWorkspaceDocument, decodeGeneratedResearchGoalSuggestions, type AppServerProtocolOperation, type BealeAppServerQuickChatList, type BealeAppServerQuickChatStartResult, type BealeAppServerWorkspaceDetails, type BealeAppServerWorkspaceDocument, type BealeProviderLimitId, type BealeProviderUsageLimits, type FleetAppServerStatus, type FleetBackend, type FleetMachine, type FleetRemoteCatalog, type FleetRemoteGoalSuggestions, type FleetRemotePromptExpansionInput, type FleetRemotePromptExpansionResult, type FleetRemoteResearchInput, type FleetSshTestInput, type FleetSshTestResult, type FleetState } from '@beale/app-server-runtime/protocol';
import { sshConnectionArgs } from './fleetSsh.js';
import { detectFleetBrowsers } from './fleetBrowsers.js';
import { normalizeTailnetOrigin, verifySameTailnet } from './tailnetRemote.js';
import {
  FLEET_SANDBOX_PROVIDERS,
  FLEET_SANDBOX_PROVIDER_DETAILS,
  createFleetSandbox,
  destroyFleetSandbox,
  fleetSandboxProvider,
  isFleetSandboxProvider,
  type FleetSandboxReference,
  type FleetSandboxProviderResolver,
} from '@beale/research-agent';

const execFileAsync = promisify(execFile);
const VM_NAME = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/u;
const WORKSPACE_ID = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,255}$/u;
const VM_TARGET_ID = /^(?:local|browser:[a-z0-9-]+|vm:(?:run|clone):(?:tart|hyper-v):[A-Za-z0-9._:-]+)$/u;
const SANDBOX_TARGET_ID = new RegExp(`^sandbox:(?:${FLEET_SANDBOX_PROVIDERS.join('|')})$`, 'u');
/** The working directory Sandbox SDK uses for commands in every provider. */
const SANDBOX_GUEST_CWD = '/workspace';

function isWorkspaceTargetId(id: string): boolean {
  return VM_TARGET_ID.test(id) || SANDBOX_TARGET_ID.test(id);
}

/** What a session worker needs to run commands on one Fleet target. */
export type FleetRemoteShell =
  | { sshArgs: string[]; guestPlatform: 'win32' | 'darwin' | 'linux'; guestCwd: string }
  | { sandbox: FleetSandboxReference; guestPlatform: 'linux'; guestCwd: string };

/** A target prepared for one session; `machineId` is the ownership record released when it ends. */
export type FleetSessionTarget = { machineId: string } & FleetRemoteShell;

export class FleetIncompleteResponseError extends Error {
  constructor(readonly operation: AppServerProtocolOperation, cause?: unknown) {
    super(`Remote app-server ${operation} returned an incomplete JSON response. Check the current state before retrying a control action.`, { cause });
    this.name = 'FleetIncompleteResponseError';
  }
}

export function normalizeBrowserHomePage(value: unknown): string {
  if (typeof value !== 'string' || value.length > 2048) throw new Error('Browser home page must be an HTTP or HTTPS URL.');
  try {
    const url = new URL(value.trim());
    if ((url.protocol !== 'http:' && url.protocol !== 'https:') || !url.hostname || url.username || url.password) throw new Error();
    return url.href;
  } catch {
    throw new Error('Browser home page must be an HTTP or HTTPS URL.');
  }
}

interface FleetMachineConfig {
  base: boolean;
  canRun: boolean;
  canClone: boolean;
  guestPlatform: 'win32' | 'darwin' | 'linux';
  sessionClone: boolean;
  privilege: 'standard' | 'elevated';
  sshHost: string | null;
  sshUser: string | null;
  sshIdentityFile: string | null;
  sshKnownHostsFile: string | null;
}

interface FleetConfiguration {
  version: 3;
  machineId: string;
  role: 'primary' | 'guest';
  enabled: boolean;
  primary: { name: string; sshHost: string | null } | null;
  requiredWorkspaceIds: string[];
  optionalWorkspaceIds: string[];
  lastMachineByWorkspace: Record<string, string>;
  workspaceTargets: Record<string, string[]>;
  browserEnabled: Record<string, boolean>;
  /** Sandbox SDK providers offered as targets. Off until the operator enables one. */
  sandboxEnabled: Record<string, boolean>;
  browserHomePage: string;
  machines: Record<string, FleetMachineConfig>;
  appServers: Record<string, { name: string; url: string; operatorToken: string }>;
  owners: Record<string, { machineId: string; sessionId: string; workspaceId?: string }>;
}

export interface FleetCommandRunner {
  run(command: string, args: readonly string[], timeoutMs: number): Promise<string>;
  launch(command: string, args: readonly string[]): void;
}

const defaultRunner: FleetCommandRunner = {
  async run(command, args, timeoutMs) {
    const { stdout } = await execFileAsync(command, [...args], {
      timeout: timeoutMs,
      maxBuffer: 2_000_000,
      windowsHide: true,
      encoding: 'utf8',
    });
    return stdout;
  },
  launch(command, args) {
    const child = spawn(command, [...args], { detached: true, stdio: 'ignore', windowsHide: true });
    child.once('error', () => undefined);
    child.unref();
  },
};

export class FleetService {
  private ownershipQueue: Promise<void> = Promise.resolve();
  private readonly sessionClones = new Map<string, Promise<FleetMachine>>();
  private readonly validatedRemotePeers = new Map<string, { url: string; at: number }>();
  public constructor(
    private readonly path = join(homedir(), '.beale', 'fleet.json'),
    private readonly hostPlatform = platform(),
    private readonly runner: FleetCommandRunner = defaultRunner,
    private readonly sandboxProvider: FleetSandboxProviderResolver = fleetSandboxProvider,
  ) {}

  public async state(): Promise<FleetState> {
    const config = this.read();
    if (config.role === 'guest') return this.project(config, [], false, null);
    const backend = this.backend();
    if (!backend) return this.project(config, [], false, 'Fleet supports Tart on macOS and Hyper-V on Windows.');
    try {
      const discovered = await this.discover(backend);
      const machines = discovered.map((machine): FleetMachine => {
        const saved = config.machines[machine.id];
        return {
          ...machine,
          base: saved?.base === true,
          canRun: saved?.canRun ?? saved?.base !== true,
          canClone: saved?.canClone ?? saved?.base === true,
          guestPlatform: saved?.guestPlatform ?? (machine.backend === 'hyper-v' ? 'win32' : 'darwin'),
          privilege: saved?.privilege ?? 'standard',
          sshConfigured: Boolean(saved?.sshUser),
          sshIdentityConfigured: Boolean(saved?.sshIdentityFile),
          sshKnownHostsConfigured: Boolean(saved?.sshKnownHostsFile),
          sshHost: saved?.sshHost ?? null,
          sshUser: saved?.sshUser ?? null,
          owner: config.owners[machine.id]
            ? { machineId: config.owners[machine.id]!.machineId, sessionId: config.owners[machine.id]!.sessionId } : null,
        };
      }).sort((a, b) => a.name.localeCompare(b.name) || a.id.localeCompare(b.id));
      return this.project(config, machines, true, null);
    } catch (error) {
      return this.project(config, [], false, boundedError(error));
    }
  }

  public async stateForWorkspace(workspaceId: string): Promise<FleetState> {
    validWorkspaceId(workspaceId);
    const config = this.read();
    const targets = config.workspaceTargets[workspaceId] ?? ['local', 'browser:in-agent'];
    return config.enabled && targets.some((target) => target.startsWith('vm:'))
      ? this.state()
      : this.project(config, [], false, null);
  }

  public async configure(input: Record<string, unknown>): Promise<FleetState> {
    return this.exclusive(() => this.configureUnlocked(input));
  }

  public browserHomePage(): string {
    return this.read().browserHomePage;
  }

  private async configureUnlocked(input: Record<string, unknown>): Promise<FleetState> {
    const config = this.read();
    const action = requiredText(input.action, 'Fleet action');
    if (action === 'set-role') {
      if (input.role !== 'primary' && input.role !== 'guest') throw new Error('Fleet role must be primary or guest.');
      if (input.role !== config.role && Object.keys(config.owners).length) throw new Error('Release active VM sessions before changing the Fleet role.');
      config.role = input.role;
    } else if (action === 'set-enabled') {
      if (typeof input.enabled !== 'boolean') throw new Error('Fleet enabled must be boolean.');
      if (!input.enabled && Object.keys(config.owners).length) throw new Error('Release active VM sessions before disabling Fleet.');
      config.enabled = input.enabled;
    } else if (action === 'set-primary') {
      config.primary = {
        name: requiredText(input.name, 'Primary name'),
        sshHost: optionalText(input.sshHost),
      };
    } else if (action === 'set-required') {
      const workspaceId = validWorkspaceId(input.workspaceId);
      if (typeof input.required !== 'boolean') throw new Error('Fleet required must be boolean.');
      config.requiredWorkspaceIds = input.required
        ? [...new Set([...config.requiredWorkspaceIds, workspaceId])]
        : config.requiredWorkspaceIds.filter((candidate) => candidate !== workspaceId);
      config.optionalWorkspaceIds = input.required
        ? config.optionalWorkspaceIds.filter((candidate) => candidate !== workspaceId)
        : [...new Set([...config.optionalWorkspaceIds, workspaceId])];
    } else if (action === 'select-machine') {
      const workspaceId = validWorkspaceId(input.workspaceId);
      const machineId = requiredText(input.machineId, 'Target ID');
      const state = await this.stateForWorkspace(workspaceId);
      const selectable = (machineId === 'local' || state.enabled)
        && (config.workspaceTargets[workspaceId] ?? ['local', 'browser:in-agent']).includes(machineId)
        && (machineId === 'local'
          || (SANDBOX_TARGET_ID.test(machineId) && config.sandboxEnabled[machineId.slice('sandbox:'.length)] === true)
          || state.browsers.some((browser) => machineId === `browser:${browser.id}` && browser.enabled)
          || state.machines.some((machine) => (machineId === `vm:run:${machine.id}` && machine.canRun)
            || (machineId === `vm:clone:${machine.id}` && machine.canClone && machine.state === 'stopped')));
      if (!selectable) {
        throw new Error('Select an existing runnable Fleet machine.');
      }
      config.lastMachineByWorkspace[workspaceId] = machineId;
    } else if (action === 'set-workspace-targets') {
      const workspaceId = validWorkspaceId(input.workspaceId);
      if (!Array.isArray(input.targetIds) || input.targetIds.length > 128
        || input.targetIds.some((id) => typeof id !== 'string' || id.length > 300 || !isWorkspaceTargetId(id))) {
        throw new Error('Workspace targets must be a bounded list of Browser, Fleet VM, or sandbox IDs.');
      }
      const state = await this.state();
      const targetIds = input.targetIds as string[];
      for (const id of targetIds) {
        if (id === 'local') continue;
        if (id.startsWith('sandbox:')) {
          if (config.sandboxEnabled[id.slice('sandbox:'.length)] !== true) throw new Error('Workspace sandbox target is unavailable.');
          continue;
        }
        if (id.startsWith('browser:')) {
          if (!state.browsers.some((browser) => browser.id === id.slice(8) && browser.enabled)) throw new Error('Workspace browser target is unavailable.');
          continue;
        }
        const match = /^vm:(run|clone):(.+)$/u.exec(id);
        const machine = state.machines.find((candidate) => candidate.id === match?.[2]);
        if (!machine || (match?.[1] === 'run' ? !machine.canRun : !machine.canClone)) throw new Error('Workspace VM target is unavailable.');
      }
      config.workspaceTargets[workspaceId] = [...new Set(targetIds)];
    } else if (action === 'set-sandbox-enabled') {
      const provider = input.provider;
      if (!isFleetSandboxProvider(provider) || typeof input.enabled !== 'boolean') throw new Error('Select a supported Fleet sandbox provider.');
      if (!input.enabled && Object.keys(config.owners).some((id) => id.startsWith(`sandbox:${provider}:`))) {
        throw new Error('Release active sandbox sessions before disabling this provider.');
      }
      config.sandboxEnabled[provider] = input.enabled;
    } else if (action === 'set-browser-enabled') {
      const browserId = requiredText(input.browserId, 'Browser ID');
      if (typeof input.enabled !== 'boolean' || !detectFleetBrowsers(this.hostPlatform).some((browser) => browser.id === browserId)) {
        throw new Error('Select an installed Fleet browser.');
      }
      config.browserEnabled[browserId] = input.enabled;
    } else if (action === 'set-browser-home-page') {
      config.browserHomePage = normalizeBrowserHomePage(input.url);
    } else if (action === 'set-machine') {
      const machineId = requiredText(input.machineId, 'Machine ID');
      const state = await this.state();
      if (!state.machines.some((machine) => machine.id === machineId)) throw new Error('Unknown Fleet machine.');
      if (config.owners[machineId]) throw new Error('VM settings cannot change while a session owns the VM.');
      const prior = config.machines[machineId] ?? { ...defaultMachineConfig(), guestPlatform: machineId.startsWith('tart:') ? 'darwin' as const : 'win32' as const };
      const base = input.base === undefined ? prior.base : input.base;
      const canRun = input.canRun === undefined ? input.base === undefined ? prior.canRun : !base : input.canRun;
      const canClone = input.canClone === undefined ? input.base === undefined ? prior.canClone : base : input.canClone;
      const guestPlatform = input.guestPlatform === undefined ? prior.guestPlatform : input.guestPlatform;
      const privilege = input.privilege === undefined ? prior.privilege : input.privilege;
      if (typeof base !== 'boolean') throw new Error('Base must be boolean.');
      if (typeof canRun !== 'boolean' || typeof canClone !== 'boolean') throw new Error('VM target permissions must be boolean.');
      if (guestPlatform !== 'win32' && guestPlatform !== 'darwin' && guestPlatform !== 'linux') throw new Error('Select a supported VM guest operating system.');
      if (privilege !== 'standard' && privilege !== 'elevated') throw new Error('Privilege must be standard or elevated.');
      if (base && state.machines.find((machine) => machine.id === machineId)?.state !== 'stopped') {
        throw new Error('Stop a VM before marking it as a Fleet base.');
      }
      config.machines[machineId] = {
        base,
        canRun,
        canClone,
        guestPlatform,
        sessionClone: base ? false : prior.sessionClone,
        privilege,
        sshHost: input.sshHost === undefined ? prior.sshHost : optionalText(input.sshHost),
        sshUser: input.sshUser === undefined ? prior.sshUser : optionalText(input.sshUser),
        sshIdentityFile: input.sshIdentityFile === undefined ? prior.sshIdentityFile : optionalText(input.sshIdentityFile),
        sshKnownHostsFile: input.sshKnownHostsFile === undefined ? prior.sshKnownHostsFile : optionalText(input.sshKnownHostsFile),
      };
    } else if (action === 'set-app-server') {
      this.requirePrimaryEnabled(config);
      const id = requiredText(input.serverId, 'App server ID');
      if (!VM_NAME.test(id)) throw new Error('Invalid app server ID.');
      const name = requiredText(input.name, 'App server name');
      const url = normalizeTailnetOrigin(input.url);
      const token = typeof input.operatorToken === 'string' && input.operatorToken.trim()
        ? input.operatorToken.trim() : config.appServers[id]?.operatorToken;
      if (!token || token.length > 4096) throw new Error('An app-server operator token is required.');
      await this.validateRemoteAppServer(url, token);
      config.appServers[id] = { name, url, operatorToken: token };
      this.validatedRemotePeers.delete(id);
    } else if (action === 'remove-app-server') {
      this.requirePrimaryEnabled(config);
      const id = requiredText(input.serverId, 'App server ID');
      delete config.appServers[id];
      this.validatedRemotePeers.delete(id);
    } else {
      throw new Error('Unsupported Fleet configuration action.');
    }
    this.write(config);
    return action === 'select-machine'
      ? this.stateForWorkspace(validWorkspaceId(input.workspaceId))
      : this.state();
  }

  public machineId(): string { return this.read().machineId; }

  public remoteAppServer(serverId: string): { id: string; name: string; url: string; operatorToken: string } {
    const config = this.read();
    this.requirePrimaryEnabled(config);
    const server = config.appServers[serverId];
    if (!server) throw new Error('Unknown Fleet app server.');
    return { id: serverId, ...server };
  }

  public async testAppServer(input: Record<string, unknown>): Promise<{ success: boolean; message: string }> {
    try {
      const url = normalizeTailnetOrigin(input.url);
      const serverId = typeof input.serverId === 'string' ? input.serverId : '';
      const token = typeof input.operatorToken === 'string' && input.operatorToken.trim()
        ? input.operatorToken.trim() : this.read().appServers[serverId]?.operatorToken ?? '';
      if (!token || token.length > 4096) throw new Error('An app-server operator token is required.');
      await this.validateRemoteAppServer(url, token);
      return { success: true, message: 'Connected to a compatible app server on this Tailscale network.' };
    } catch (error) {
      return { success: false, message: boundedError(error) };
    }
  }

  public async connectedRemoteAppServer(serverId: string): Promise<{ id: string; name: string; url: string; operatorToken: string }> {
    const server = this.remoteAppServer(serverId);
    const cached = this.validatedRemotePeers.get(serverId);
    if (!cached || cached.url !== server.url || Date.now() - cached.at > 5_000) {
      await this.validateRemoteAppServer(server.url, server.operatorToken);
      this.validatedRemotePeers.set(serverId, { url: server.url, at: Date.now() });
    }
    return server;
  }

  public async restartRemoteAppServer(serverId: string): Promise<{ restarted: true }> {
    const server = this.remoteAppServer(serverId);
    await verifySameTailnet(server.url, this.runner);
    const endpoint = new URL(server.url);
    endpoint.port = String(BEALE_APP_SERVER_SUPERVISOR_HTTPS_PORT);
    const response = await fetch(endpoint.origin + BEALE_APP_SERVER_SUPERVISOR_RESTART_PATH, {
      method: 'POST', headers: { authorization: `Bearer ${server.operatorToken}` },
      redirect: 'error', signal: AbortSignal.timeout(90_000),
    });
    if (!response.ok) throw new Error(`The remote app-server restart supervisor refused the request (HTTP ${response.status}).`);
    try {
      await this.validateRemoteAppServer(server.url, server.operatorToken);
    } catch (error) {
      throw new Error(`The remote app server restarted, but ${boundedError(error)}`);
    }
    this.validatedRemotePeers.delete(serverId);
    return { restarted: true };
  }

  public async callRemote<T>(serverId: string, operation: AppServerProtocolOperation, input: Record<string, unknown>, timeoutMs = 5 * 60_000): Promise<T> {
    const server = await this.connectedRemoteAppServer(serverId);
    const statusRead = operation === 'workspace.research_track' && input.action === 'status';
    for (let attempt = 0; ; attempt += 1) {
      const response = await fetch(server.url + BEALE_APP_SERVER_OPERATIONS_PATH, {
        method: 'POST', headers: { authorization: `Bearer ${server.operatorToken}`, 'content-type': 'application/json' },
        body: JSON.stringify({ operation, input }), redirect: 'error', signal: AbortSignal.timeout(timeoutMs),
      });
      if (!response.ok) throw new Error(`Remote app-server ${operation} failed (${response.status}).`);
      let payload: unknown;
      try { payload = await response.json(); }
      catch (error) {
        if (statusRead && attempt === 0) continue;
        throw new FleetIncompleteResponseError(operation, error);
      }
      if (!isRecord(payload) || payload.controlVersion !== BEALE_APP_SERVER_CONTROL_VERSION
        || !Object.hasOwn(payload, 'result')) {
        throw new Error('Remote app-server response has an incompatible control contract.');
      }
      return payload.result as T;
    }
  }

  public async remoteMachines(): Promise<FleetMachine[]> {
    const ids = Object.keys(this.read().appServers);
    const results = await Promise.allSettled(ids.map(async (serverId): Promise<FleetMachine[]> => {
      const state = await this.callRemote<FleetState>(serverId, 'fleet.state', {}, 12_000);
      if (!state.enabled || state.role !== 'primary' || !state.available) return [];
      return state.machines.filter((machine) => machine.base && machine.state === 'stopped')
        .map((machine) => ({ ...machine, id: `remote:${serverId}:${machine.id}`, name: `${this.remoteAppServer(serverId).name} / ${machine.name}` }));
    }));
    if (ids.length && results.every((result) => result.status === 'rejected')) {
      throw new Error('Could not load VM inventory from any saved app server. Check their connections in Fleet.');
    }
    return results.flatMap((result) => result.status === 'fulfilled' ? result.value : [])
      .sort((left, right) => left.name.localeCompare(right.name));
  }

  public async appServerStatuses(): Promise<FleetAppServerStatus[]> {
    const config = this.read();
    if (!config.enabled || config.role !== 'primary') return [];
    return Promise.all(Object.entries(config.appServers).map(([id, server]) => probeFleetAppServerStatus(
      id, server.url, server.operatorToken, (url) => verifySameTailnet(url, this.runner)
    )));
  }

  public async remoteCatalog(serverId: string): Promise<FleetRemoteCatalog> {
    const server = await this.connectedRemoteAppServer(serverId);
    const fleetState = await this.callRemote<FleetState>(serverId, 'fleet.state', {});
    const providerCatalog = decodeBealeAppServerProviderCatalog(await this.remoteGet(server, BEALE_APP_SERVER_PROVIDERS_PATH));
    const workspacesResult = await this.remoteGet(server, BEALE_APP_SERVER_WORKSPACES_PATH);
    if (!isRecord(workspacesResult) || !Array.isArray(workspacesResult.workspaces)) throw new Error('Remote workspace catalog is invalid.');
    const workspaces = workspacesResult.workspaces.flatMap((value: unknown) => {
      if (!isRecord(value) || typeof value.id !== 'string' || typeof value.workspaceId !== 'string' || typeof value.name !== 'string') return [];
      return [{ id: value.id, workspaceId: value.workspaceId, name: value.name, runCount: typeof value.runCount === 'number' ? value.runCount : 0 }];
    });
    const sessions = (await Promise.all(workspaces.map(async (workspace) => {
      const payload = await this.remoteGet(server, `${BEALE_APP_SERVER_WORKSPACES_PATH}/${encodeURIComponent(workspace.workspaceId)}/sessions?limit=500`);
      if (!isRecord(payload) || !Array.isArray(payload.result)) return [];
      return payload.result.flatMap((value: unknown) => {
        if (!isRecord(value) || typeof value.id !== 'string') return [];
        const metadata = isRecord(value.metadata) ? value.metadata : {};
        const bealeRun = isRecord(metadata.bealeRun) ? metadata.bealeRun : {};
        const budget = isRecord(bealeRun.budget) ? bealeRun.budget : {};
        const schedule = isRecord(budget.repeatSchedule) ? budget.repeatSchedule : {};
        const attempts = Array.isArray(value.attempts) ? value.attempts : [];
        const lastAttempt = attempts.at(-1);
        return [{ id: value.id, workspaceId: workspace.workspaceId,
          title: typeof value.title === 'string' && value.title.trim() ? value.title : typeof value.prompt === 'string' ? value.prompt.slice(0, 90) : value.id,
          status: typeof value.status === 'string' ? value.status : 'unknown',
          prompt: typeof value.prompt === 'string' ? value.prompt : '',
          updatedAt: isRecord(lastAttempt) && typeof lastAttempt.startedAt === 'string' ? lastAttempt.startedAt : typeof value.createdAt === 'string' ? value.createdAt : '',
          automation: typeof schedule.type === 'string' && schedule.type !== 'none' }];
      });
    }))).flat();
    return { serverId, providerCatalog, workspaces, sessions: sessions.sort((left, right) => right.updatedAt.localeCompare(left.updatedAt)),
      machines: fleetState.machines,
      browsers: fleetState.browsers,
      workspaceTargets: fleetState.workspaceTargets,
      requiredWorkspaceIds: fleetState.requiredWorkspaceIds, optionalWorkspaceIds: fleetState.optionalWorkspaceIds,
      hasBaseVm: fleetState.machines.some((machine) => machine.base) };
  }

  public async remoteProviderUsageLimits(serverId: string, providerId: BealeProviderLimitId, refresh: boolean): Promise<BealeProviderUsageLimits> {
    const server = await this.connectedRemoteAppServer(serverId);
    const query = new URLSearchParams({ provider: providerId, ...(refresh ? { refresh: '1' } : {}) });
    const limits = decodeBealeProviderUsageLimits(await this.remoteGet(server, `${BEALE_APP_SERVER_PROVIDER_LIMITS_PATH}?${query}`));
    if (limits.providerId !== providerId) throw new Error('Remote app server returned limits for a different provider.');
    return limits;
  }

  public async remoteWorkspaceDetails(serverId: string, workspaceId: string): Promise<BealeAppServerWorkspaceDetails> {
    if (!WORKSPACE_ID.test(workspaceId)) throw new Error('Invalid workspace ID.');
    const server = await this.connectedRemoteAppServer(serverId);
    return decodeBealeAppServerWorkspaceDetails(await this.remoteGet(
      server, bealeAppServerWorkspaceDetailsPath(workspaceId)
    ));
  }

  public async remoteWorkspaceDocument(serverId: string, workspaceId: string, kind: 'runbook' | 'report', resourceId: string): Promise<BealeAppServerWorkspaceDocument> {
    if (!WORKSPACE_ID.test(workspaceId) || !resourceId.trim() || !['runbook', 'report'].includes(kind)) {
      throw new Error('Invalid remote workspace document request.');
    }
    const document = await this.callRemote<unknown>(serverId, kind === 'runbook' ? 'runbook.get' : 'report.get', {
      workspaceId, [kind === 'runbook' ? 'runbookId' : 'reportId']: resourceId
    });
    if (!isRecord(document)) throw new Error('Remote app server returned an invalid workspace document.');
    const sanitized = kind === 'runbook' && Array.isArray(document.cells)
      ? { ...document, cells: document.cells.map((cell) => {
        if (!isRecord(cell) || !isRecord(cell.executor) || cell.executor.kind !== 'tart-vm') return cell;
        return { ...cell, executor: { ...cell.executor, workspacePath: null } };
      }) }
      : document;
    return decodeBealeAppServerWorkspaceDocument({ schemaVersion: 1, kind, resourceId, document: sanitized });
  }

  public async remoteQuickChats(serverId: string): Promise<BealeAppServerQuickChatList> {
    const server = await this.connectedRemoteAppServer(serverId);
    return decodeBealeAppServerQuickChatList(await this.remoteGet(server, BEALE_APP_SERVER_QUICK_CHATS_PATH));
  }

  public async remoteStartQuickChat(serverId: string, input: unknown): Promise<BealeAppServerQuickChatStartResult> {
    const quickChat = decodeBealeAppServerQuickChatStartInput(input);
    const server = await this.connectedRemoteAppServer(serverId);
    return decodeBealeAppServerQuickChatStartResult(await this.remotePost(server, BEALE_APP_SERVER_QUICK_CHATS_PATH, quickChat));
  }

  public async remoteContinueQuickChat(serverId: string, workspaceId: string, sessionId: string, instruction: string): Promise<{ sessionId: string }> {
    if (!WORKSPACE_ID.test(workspaceId) || !VM_NAME.test(sessionId) || !instruction.trim() || instruction.length > 131_072) {
      throw new Error('Invalid Quick Chat continuation.');
    }
    const server = await this.connectedRemoteAppServer(serverId);
    const payload = await this.remotePost(server, `${BEALE_APP_SERVER_SESSIONS_PATH}/${encodeURIComponent(sessionId)}/continuations`, {
      workspaceId, instruction: instruction.trim()
    });
    if (!isRecord(payload) || !isRecord(payload.session) || payload.session.sessionId !== sessionId) {
      throw new Error('Remote app server returned an invalid Quick Chat continuation.');
    }
    return { sessionId };
  }

  public async remoteSession(serverId: string, workspaceId: string, sessionId: string, beforeEventId: string | null = null): Promise<unknown> {
    const server = await this.connectedRemoteAppServer(serverId);
    const path = `${BEALE_APP_SERVER_WORKSPACES_PATH}/${encodeURIComponent(workspaceId)}/sessions/${encodeURIComponent(sessionId)}`;
    return beforeEventId
      ? this.remoteGet(server, `${path}/events?beforeEventId=${encodeURIComponent(beforeEventId)}&limit=200&maxBytes=1000000`)
      : this.remoteGet(server, `${path}/update?tail=true&limit=200&maxBytes=1000000`);
  }

  public async remoteBrowserAttach(serverId: string, sessionId: string): Promise<{ url: string; token: string }> {
    if (!VM_NAME.test(sessionId)) throw new Error('Invalid remote session identity.');
    const server = await this.connectedRemoteAppServer(serverId);
    const attachment = await this.remotePost(server, `${BEALE_APP_SERVER_SESSIONS_PATH}/${encodeURIComponent(sessionId)}/attachments`, {});
    const transport = isRecord(attachment) && isRecord(attachment.transport) ? attachment.transport : null;
    if (!transport || typeof transport.token !== 'string' || !transport.token) {
      throw new Error('The remote app server did not return a session attachment.');
    }
    const url = new URL(`${BEALE_APP_SERVER_SESSIONS_PATH}/${encodeURIComponent(sessionId)}/browser`, server.url);
    url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
    return { url: url.href, token: transport.token };
  }

  public async remoteLaunch(serverId: string, workspaceId: string, research: FleetRemoteResearchInput): Promise<{ sessionId: string }> {
    const server = await this.connectedRemoteAppServer(serverId);
    const promptMarkdown = research.promptMarkdown;
    if (!promptMarkdown.trim() || promptMarkdown.length > 131_072) throw new Error('Enter a research prompt.');
    const targetIds = [...research.targetIds];
    if (targetIds.length < 1 || targetIds.length > 16
      || targetIds.some((id) => typeof id !== 'string' || id.length < 1 || id.length > 256)
      || new Set(targetIds).size !== targetIds.length) {
      throw new Error('Select 1 to 16 distinct remote targets.');
    }
    if (research.repeatSchedule?.type !== undefined && research.repeatSchedule.type !== 'none'
      && targetIds.some((id) => id.startsWith('vm:') || id.startsWith('sandbox:'))) {
      throw new Error('Repeating sessions are available on Local and browser targets only.');
    }
    const fleetState = await this.callRemote<FleetState>(serverId, 'fleet.state', {});
    const allowed = fleetState.workspaceTargets[workspaceId] ?? ['local', 'browser:in-agent'];
    for (const machineId of targetIds) {
      if (!allowed.includes(machineId) && !(machineId === 'local' && allowed.includes('browser:in-agent'))) throw new Error('Select a target enabled for this remote workspace.');
      if (machineId !== 'local' && !fleetState.enabled) throw new Error('Fleet is disabled on the remote app server.');
      if (machineId.startsWith('browser:') && !fleetState.browsers.some((browser) => browser.enabled && machineId === `browser:${browser.id}`)) {
        throw new Error('Select an enabled remote browser target.');
      }
      if (machineId.startsWith('vm:') && !fleetState.machines.some((machine) =>
        machineId === `vm:run:${machine.id}` && machine.canRun
        || machineId === `vm:clone:${machine.id}` && machine.canClone && machine.state === 'stopped')) {
        throw new Error('Select an available remote VM target.');
      }
      if (machineId.startsWith('sandbox:') && !fleetState.sandboxes.some((sandbox) => sandbox.enabled && machineId === `sandbox:${sandbox.provider}`)) {
        throw new Error('Select an enabled remote sandbox target.');
      }
    }
    const payload = await this.remotePost(server, BEALE_APP_SERVER_SESSIONS_PATH, {
      launchVersion: APP_SERVER_SESSION_LAUNCH_VERSION, launch: { workspaceId, promptMarkdown: promptMarkdown.trim(),
        machineId: targetIds.length > 1 ? 'local' : targetIds[0], targetIds, browserRelay: true,
        ...(research.mode ? { mode: research.mode } : {}),
        ...(research.goal ? { goal: research.goal } : {}),
        ...(research.provider ? { provider: research.provider } : {}),
        ...(research.shellSafetyMode ? { shellSafetyMode: research.shellSafetyMode } : {}),
        ...(research.guardianEnabled === false ? { guardianEnabled: false } : {}),
        ...(research.workflowId ? { workflowId: research.workflowId } : {}),
        ...(research.collaboration ? { collaboration: research.collaboration } : {}),
        ...(research.repeatSchedule && research.repeatSchedule.type !== 'none' ? { repeatSchedule: research.repeatSchedule } : {}),
        generateTitle: true },
    });
    if (!isRecord(payload) || !isRecord(payload.session) || typeof payload.session.sessionId !== 'string') {
      throw new Error('Remote app server returned an invalid session start.');
    }
    return { sessionId: payload.session.sessionId };
  }

  public async remoteExpandPrompt(serverId: string, workspaceId: string, expansion: FleetRemotePromptExpansionInput): Promise<FleetRemotePromptExpansionResult> {
    if (!WORKSPACE_ID.test(workspaceId)) throw new Error('Invalid workspace ID.');
    const result = await this.callRemote<unknown>(serverId, 'prompt.expand', { workspaceId, ...expansion });
    if (!isRecord(result) || typeof result.phase !== 'string' || typeof result.promptMarkdown !== 'string'
      || !result.promptMarkdown.trim() || result.promptMarkdown.length > 131_072) {
      throw new Error('Remote app server returned an invalid expanded prompt.');
    }
    return { phase: result.phase, promptMarkdown: result.promptMarkdown };
  }

  public async remoteGoalSuggestions(serverId: string, workspaceId: string, phase: string, refresh: boolean, incremental = false): Promise<FleetRemoteGoalSuggestions> {
    if (!WORKSPACE_ID.test(workspaceId) || !VM_NAME.test(phase)) throw new Error('Invalid remote research suggestion request.');
    const result = decodeGeneratedResearchGoalSuggestions(await this.callRemote<unknown>(serverId, 'suggestion.generate', {
      workspaceId, phase, refresh, incremental
    }), phase);
    return { phase: result.phase, suggestions: result.suggestions,
      ...(result.cacheStatus === 'stale' ? { cacheStatus: 'stale' as const } : {}) };
  }

  public async remoteControl(serverId: string, sessionId: string, type: string, instruction: string | null): Promise<void> {
    if (!VM_NAME.test(sessionId) || !['pause', 'resume', 'stop', 'steer'].includes(type)) throw new Error('Invalid remote session control.');
    if (type === 'steer' && (!instruction?.trim() || instruction.length > 131_072)) throw new Error('Enter a steering instruction.');
    const server = await this.connectedRemoteAppServer(serverId);
    await this.remotePost(server, `${BEALE_APP_SERVER_SESSIONS_PATH}/${encodeURIComponent(sessionId)}/control`, {
      type, ...(type === 'steer' ? { instruction: instruction!.trim() } : {}),
    });
  }

  private async remoteGet(server: { url: string; operatorToken: string }, path: string): Promise<unknown> {
    const response = await fetch(server.url + path, { headers: { authorization: `Bearer ${server.operatorToken}` }, redirect: 'error', signal: AbortSignal.timeout(30_000) });
    if (!response.ok) throw new Error(`Remote app-server request failed (${response.status}).`);
    return response.json();
  }

  private async remotePost(server: { url: string; operatorToken: string }, path: string, body: unknown): Promise<unknown> {
    const response = await fetch(server.url + path, { method: 'POST', headers: { authorization: `Bearer ${server.operatorToken}`, 'content-type': 'application/json' },
      body: JSON.stringify(body), redirect: 'error', signal: AbortSignal.timeout(90_000) });
    if (!response.ok) throw new Error(`Remote app-server request failed (${response.status}).`);
    return response.json();
  }

  private async validateRemoteAppServer(url: string, token: string): Promise<void> {
    await verifySameTailnet(url, this.runner);
    const response = await fetch(url + BEALE_APP_SERVER_SERVER_PATH, {
      headers: { authorization: `Bearer ${token}` }, redirect: 'error', signal: AbortSignal.timeout(10_000),
    });
    if (!response.ok) throw new Error(`Remote app-server authentication failed (${response.status}).`);
    const descriptor: unknown = await response.json();
    assertCompatibleRemoteAppServerDescriptor(descriptor);
  }

  public async reserve(machineId: string, owner: { machineId: string; sessionId: string; workspaceId?: string }): Promise<FleetState> {
    return this.exclusive(async () => {
      validOwner(owner);
      const state = await this.state();
      const machine = state.machines.find((candidate) => candidate.id === machineId);
      if (!machine || !machine.canRun) throw new Error('Select a Fleet VM with Can Run enabled.');
      const config = this.read();
      this.requirePrimaryEnabled(config);
      const current = config.owners[machineId];
      if (current && (current.machineId !== owner.machineId || current.sessionId !== owner.sessionId)) {
        throw new Error('Fleet VM is already reserved by another machine or session.');
      }
      if (current?.workspaceId && owner.workspaceId && current.workspaceId !== owner.workspaceId) {
        throw new Error('Fleet VM is already staging another workspace for this session.');
      }
      if (owner.workspaceId && Object.entries(config.owners).some(([id, claim]) => id !== machineId
        && claim.workspaceId === owner.workspaceId && (claim.machineId !== owner.machineId || claim.sessionId !== owner.sessionId))) {
        throw new Error('Fleet workspace is already staged for another VM session.');
      }
      config.owners[machineId] = { ...current, ...owner };
      this.write(config);
      return this.state();
    });
  }

  public async release(machineId: string, owner: { machineId: string; sessionId: string }): Promise<FleetState> {
    return this.exclusive(async () => {
      validOwner(owner);
      const config = this.read();
      const current = config.owners[machineId];
      if (current && (current.machineId !== owner.machineId || current.sessionId !== owner.sessionId)) {
        throw new Error('Only the owning machine and session may release this Fleet VM.');
      }
      delete config.owners[machineId];
      this.write(config);
      return this.state();
    });
  }

  public assertReserved(machineId: string, owner: { machineId: string; sessionId: string }): void {
    validOwner(owner);
    const current = this.read().owners[machineId];
    if (!current || current.machineId !== owner.machineId || current.sessionId !== owner.sessionId) {
      throw new Error('Fleet VM is not reserved by this machine and session.');
    }
  }

  public isReservedBy(machineId: string, owner: { machineId: string; sessionId: string }): boolean {
    const current = this.read().owners[machineId];
    return current?.machineId === owner.machineId && current.sessionId === owner.sessionId;
  }

  private async exclusive<T>(work: () => Promise<T>): Promise<T> {
    const previous = this.ownershipQueue;
    let unlock: () => void = () => undefined;
    this.ownershipQueue = new Promise<void>((resolve) => { unlock = resolve; });
    await previous;
    try { return await work(); } finally { unlock(); }
  }

  public async clone(baseId: string, name: string): Promise<FleetState> {
    const config = this.read();
    this.requirePrimaryEnabled(config);
    const targetName = validVmName(name);
    const state = await this.state();
    const base = state.machines.find((machine) => machine.id === baseId);
    if (!base?.canClone || base.state !== 'stopped') throw new Error('Clone source must be a stopped VM with Can Clone enabled.');
    if (state.machines.some((machine) => machine.name === targetName)) throw new Error('Fleet VM name already exists.');
    if (base.backend === 'tart') {
      await this.runner.run('tart', ['clone', base.name, targetName], 30 * 60_000);
    } else {
      const exportPath = join(process.env.TEMP || process.env.TMP || homedir(), `beale-fleet-export-${randomUUID()}`);
      const script = [
        'try {',
        `$source = Get-VM -Name ${powershellString(base.name)} -ErrorAction Stop`,
        `Export-VM -VM $source -Path ${powershellString(exportPath)} -ErrorAction Stop`,
        `$config = Get-ChildItem -Path ${powershellString(exportPath)} -Filter *.vmcx -Recurse | Select-Object -First 1`,
        `if (-not $config) { throw 'Hyper-V export did not contain a VM configuration.' }`,
        `$copy = Import-VM -Path $config.FullName -Copy -GenerateNewId -ErrorAction Stop`,
        `Rename-VM -VM $copy -NewName ${powershellString(targetName)} -ErrorAction Stop`,
        '} finally {',
        `Remove-Item -LiteralPath ${powershellString(exportPath)} -Recurse -Force -ErrorAction SilentlyContinue`,
        '}',
      ].join('\n');
      await this.powershell(script, 30 * 60_000);
    }
    const next = await this.state();
    const clone = next.machines.find((machine) => machine.name === targetName);
    if (!clone) throw new Error('The VM was cloned but could not be found in Fleet inventory.');
    const latest = this.read();
    latest.machines[clone.id] = { ...config.machines[baseId]!, base: false, canRun: true, canClone: false, sshHost: null };
    this.write(latest);
    return this.state();
  }

  public async cloneForSession(
    baseId: string,
    owner: { machineId: string; sessionId: string; workspaceId?: string },
    skipMachineId?: string,
  ): Promise<FleetMachine> {
    const key = `${baseId}\0${owner.machineId}\0${owner.sessionId}\0${skipMachineId ?? ''}`;
    const pending = this.sessionClones.get(key);
    if (pending) return pending;
    const clone = this.cloneForSessionOnce(baseId, owner, skipMachineId);
    this.sessionClones.set(key, clone);
    void clone.finally(() => { if (this.sessionClones.get(key) === clone) this.sessionClones.delete(key); }).catch(() => undefined);
    return clone;
  }

  private async cloneForSessionOnce(
    baseId: string,
    owner: { machineId: string; sessionId: string; workspaceId?: string },
    skipMachineId?: string,
  ): Promise<FleetMachine> {
    validOwner(owner);
    const state = await this.state();
    const base = state.machines.find((machine) => machine.id === baseId);
    if (!base?.canClone || base.state !== 'stopped' || !base.sshConfigured) {
      throw new Error('Select a stopped Fleet VM with Can Clone and a configured SSH user.');
    }
    if (owner.workspaceId && Object.values(this.read().owners).some((claim) => claim.workspaceId === owner.workspaceId
      && (claim.machineId !== owner.machineId || claim.sessionId !== owner.sessionId))) {
      throw new Error('Fleet workspace is already staged for another VM session.');
    }
    const suffix = createHash('sha256').update(`${baseId}\0${owner.machineId}\0${owner.sessionId}`).digest('hex').slice(0, 16);
    const prefix = `beale-${base.name.slice(0, 64)}-${suffix}`;
    for (const existing of state.machines.filter((machine) => machine.name === prefix || machine.name.startsWith(`${prefix}-`))) {
      if (existing.id === skipMachineId || !existing.canRun || existing.base) continue;
      const claim = this.read().owners[existing.id];
      if ((claim?.machineId === owner.machineId && claim.sessionId === owner.sessionId) || !claim) {
        const config = this.read();
        const inherited = config.machines[baseId]!;
        const saved = config.machines[existing.id] ?? inherited;
        config.machines[existing.id] = { ...saved, base: false, canRun: true, canClone: false, sessionClone: true,
          sshUser: saved.sshUser ?? inherited.sshUser,
          sshIdentityFile: saved.sshIdentityFile ?? inherited.sshIdentityFile,
          sshKnownHostsFile: saved.sshKnownHostsFile ?? inherited.sshKnownHostsFile };
        this.write(config);
        if (!claim) await this.reserve(existing.id, owner);
        return existing;
      }
    }
    let name = prefix;
    for (let sequence = 2; state.machines.some((machine) => machine.name === name); sequence += 1) {
      if (sequence > 10_000) throw new Error('Fleet has too many clones for this session.');
      name = `${prefix}-${sequence}`;
    }
    const cloned = await this.clone(baseId, name);
    const worker = cloned.machines.find((machine) => machine.name === name);
    if (!worker) throw new Error('Fleet session clone could not be found.');
    const config = this.read();
    config.machines[worker.id] = { ...config.machines[worker.id]!, sessionClone: true };
    this.write(config);
    await this.reserve(worker.id, owner);
    return worker;
  }

  public isSessionClone(machineId: string): boolean {
    return this.read().machines[machineId]?.sessionClone === true;
  }

  public async prepareSessionTarget(targetId: string, workspaceId: string, sessionId: string, enforceAllowlist = true): Promise<FleetSessionTarget> {
    if (targetId.startsWith('sandbox:')) return this.prepareSandboxTarget(targetId, workspaceId, sessionId, enforceAllowlist);
    const match = /^vm:(run|clone):(.+)$/u.exec(targetId);
    if (!match) throw new Error('Invalid Fleet VM target.');
    if (enforceAllowlist && !(this.read().workspaceTargets[workspaceId] ?? ['local', 'browser:in-agent']).includes(targetId)) {
      throw new Error('This Fleet VM is not enabled for the workspace.');
    }
    const owner = { machineId: this.machineId(), sessionId };
    const selected = (await this.state()).machines.find((machine) => machine.id === match[2]);
    if (!selected || (match[1] === 'run' ? !selected.canRun : !selected.canClone)) throw new Error('Fleet VM target is unavailable.');
    let cloned = match[1] === 'clone';
    let machineId = cloned
      ? (await this.cloneForSession(selected.id, owner)).id
      : selected.id;
    for (let recovery = 0; ; recovery += 1) {
      try {
        if (!cloned) await this.reserve(machineId, owner);
        if (!this.read().machines[machineId]?.sshUser) throw new Error('Fleet VM SSH user is not configured.');
        await this.start(machineId, owner);
        for (let attempt = 0; attempt < 60; attempt += 1) {
          try {
            const connection = await this.connection(machineId);
            const args = sshConnectionArgs(connection.sshHost, connection.sshUser, connection.sshIdentityFile,
              connection.sshKnownHostsFile, connection.machine.backend, connection.sshHostKeyAlias);
            const guestCwd = (await this.runner.run('ssh', [...args,
              connection.machine.guestPlatform === 'win32'
                ? 'powershell.exe -NoProfile -NonInteractive -Command "(Get-Location).Path"'
                : 'pwd'], 5_000)).trim().split(/\r?\n/u)[0];
            if (!guestCwd) throw new Error('Fleet VM did not report its working directory.');
            return { machineId, sshArgs: args, guestPlatform: connection.machine.guestPlatform, guestCwd };
          } catch (error) {
            const permanentFailure = nonRetryableFleetSshFailure(error);
            if (permanentFailure && attempt >= 2) throw new Error(permanentFailure);
            if (attempt === 59) throw error;
            await new Promise((resolve) => setTimeout(resolve, 1_500));
          }
        }
        throw new Error('Fleet VM SSH connection is unavailable.');
      } catch (error) {
        const state = recovery === 0 ? await this.state().catch(() => null) : null;
        const failed = state?.machines.find((machine) => machine.id === machineId);
        const unusable = !failed || failed.state !== 'running' || !failed.sshConfigured;
        const allowed = this.read().workspaceTargets[workspaceId] ?? ['local', 'browser:in-agent'];
        const alternative = !cloned && unusable ? state?.machines.find((machine) => machine.canClone
          && machine.state === 'stopped' && machine.sshConfigured && machine.guestPlatform === selected.guestPlatform
          && allowed.includes(`vm:clone:${machine.id}`)) : null;
        const replacementBaseId = cloned ? selected.id : alternative?.id;
        const replace = recovery === 0 && unusable && Boolean(replacementBaseId);
        await this.releaseSessionTarget(machineId, sessionId).catch(() => undefined);
        if (!replace) throw error;
        if (cloned) {
          const config = this.read();
          if (config.machines[machineId]) {
            config.machines[machineId]!.canRun = false;
            this.write(config);
          }
        }
        machineId = (await this.cloneForSession(replacementBaseId!, owner, cloned ? machineId : undefined)).id;
        cloned = true;
      }
    }
  }

  public async releaseSessionTarget(machineId: string, sessionId: string): Promise<void> {
    const owner = { machineId: this.machineId(), sessionId };
    if (!this.isReservedBy(machineId, owner)) return;
    if (machineId.startsWith('sandbox:')) {
      // A failed delete keeps the ownership record, so the sandbox stays visible instead of leaking unnoticed.
      const [, provider, ...rest] = machineId.split(':');
      if (isFleetSandboxProvider(provider)) await destroyFleetSandbox(await this.sandboxProvider(provider), rest.join(':'));
    } else if (this.isSessionClone(machineId)) {
      await this.stop(machineId, owner).catch(() => undefined);
    }
    await this.release(machineId, owner);
  }

  /**
   * Gets or creates this session's sandbox on the provider and records the session as its owner.
   * The key ties the sandbox to this primary and session, so a retried launch reuses it.
   */
  private async prepareSandboxTarget(targetId: string, workspaceId: string, sessionId: string, enforceAllowlist: boolean): Promise<FleetSessionTarget> {
    const config = this.read();
    this.requirePrimaryEnabled(config);
    const provider = targetId.slice('sandbox:'.length);
    if (!isFleetSandboxProvider(provider) || config.sandboxEnabled[provider] !== true) throw new Error('Fleet sandbox target is unavailable.');
    if (enforceAllowlist && !(config.workspaceTargets[workspaceId] ?? ['local', 'browser:in-agent']).includes(targetId)) {
      throw new Error('This Fleet sandbox is not enabled for the workspace.');
    }
    const owner = { machineId: config.machineId, sessionId };
    validOwner(owner);
    const sandboxId = await createFleetSandbox(await this.sandboxProvider(provider),
      `beale:${config.machineId}:${sessionId}:${provider}`, { 'beale-session': sessionId });
    const machineId = `sandbox:${provider}:${sandboxId}`;
    await this.exclusive(async () => {
      const latest = this.read();
      latest.owners[machineId] = owner;
      this.write(latest);
    });
    return { machineId, sandbox: { provider, sandboxId }, guestPlatform: 'linux', guestCwd: SANDBOX_GUEST_CWD };
  }

  public async start(machineId: string, owner?: { machineId: string; sessionId: string }): Promise<FleetState> {
    const { machine } = await this.runnable(machineId);
    this.assertOwnership(machineId, owner);
    if (machine.state === 'running') return this.state();
    if (machine.backend === 'tart') this.runner.launch('tart', ['run', machine.name, '--no-graphics']);
    else await this.powershell(`Start-VM -Name ${powershellString(machine.name)} -ErrorAction Stop`, 120_000);
    return this.state();
  }

  public async stop(machineId: string, owner?: { machineId: string; sessionId: string }): Promise<FleetState> {
    const { machine } = await this.runnable(machineId);
    this.assertOwnership(machineId, owner);
    if (machine.state === 'stopped') return this.state();
    if (machine.backend === 'tart') await this.runner.run('tart', ['stop', machine.name], 120_000);
    else await this.powershell(`Stop-VM -Name ${powershellString(machine.name)} -Shutdown -ErrorAction Stop`, 120_000);
    return this.state();
  }

  public async testSsh(input: FleetSshTestInput): Promise<FleetSshTestResult> {
    const config = this.read();
    this.requirePrimaryEnabled(config);
    const machine = (await this.state()).machines.find((candidate) => candidate.id === input.machineId);
    if (!machine) throw new Error('Unknown Fleet machine.');
    if (machine.state !== 'running') {
      return { success: false, message: machine.base
        ? 'This base VM is stopped. Clone and start a worker to test its inherited SSH settings.'
        : 'Start this VM before testing SSH.' };
    }
    const saved = config.machines[input.machineId] ?? defaultMachineConfig();
    const host = optionalText(input.sshHost) ?? await this.discoverIp(machine);
    if (!host) return { success: false, message: 'Could not determine the VM IP address. Set an SSH host override or check VM networking.' };
    const user = optionalText(input.sshUser);
    if (!user) return { success: false, message: 'Enter the guest SSH user.' };
    const identity = input.sshIdentityFile === undefined ? saved.sshIdentityFile : optionalText(input.sshIdentityFile);
    const knownHosts = input.sshKnownHostsFile === undefined ? saved.sshKnownHostsFile : optionalText(input.sshKnownHostsFile);
    const hostKeyAlias = machine.backend === 'tart' && saved.sessionClone && !knownHosts
      ? sessionCloneHostKeyAlias(machine.id) : null;
    const args = sshConnectionArgs(host, user, identity, knownHosts, machine.backend, hostKeyAlias);
    try {
      await this.runner.run('ssh', [...args.slice(0, -1), '-o', 'NumberOfPasswordPrompts=0', args.at(-1)!, 'true'], 15_000);
      return { success: true, message: `Key-only SSH succeeded at ${host}. These settings have not been saved.` };
    } catch (error) {
      const detail = sshFailureDetail(error);
      return { success: false, message: `${sshFailureMessage(error)} Address checked: ${host}.${detail ? ` SSH detail: ${detail}` : ''}` };
    }
  }

  public async connection(machineId: string): Promise<{ machine: FleetMachine; sshHost: string; sshUser: string;
    sshIdentityFile: string | null; sshKnownHostsFile: string | null; sshHostKeyAlias: string | null }> {
    const { machine, config } = await this.runnable(machineId);
    const saved = config.machines[machineId];
    if (machine.state !== 'running' || !saved?.sshUser) throw new Error('Fleet VM is not running or SSH user is not configured.');
    const sshHost = saved.sshHost ?? await this.discoverIp(machine);
    if (!sshHost) throw new Error('Fleet could not determine the running VM SSH address.');
    const sshHostKeyAlias = machine.backend === 'tart' && saved.sessionClone && !saved.sshKnownHostsFile
      ? sessionCloneHostKeyAlias(machine.id) : null;
    return { machine, sshHost, sshUser: saved.sshUser, sshIdentityFile: saved.sshIdentityFile,
      sshKnownHostsFile: saved.sshKnownHostsFile, sshHostKeyAlias };
  }

  private async runnable(machineId: string): Promise<{ machine: FleetMachine; config: FleetConfiguration }> {
    const config = this.read();
    this.requirePrimaryEnabled(config);
    const state = await this.state();
    const machine = state.machines.find((candidate) => candidate.id === machineId);
    if (!machine || !machine.canRun) throw new Error('Select a Fleet VM with Can Run enabled.');
    return { machine, config };
  }

  private requirePrimaryEnabled(config: FleetConfiguration): void {
    if (config.role !== 'primary' || !config.enabled) throw new Error('Fleet is unavailable on this Beale instance.');
  }

  private assertOwnership(machineId: string, owner?: { machineId: string; sessionId: string }): void {
    const current = this.read().owners[machineId];
    if (!current) return;
    if (!owner || owner.machineId !== current.machineId || owner.sessionId !== current.sessionId) {
      throw new Error('Fleet VM is reserved by another machine or session.');
    }
  }

  private backend(): FleetBackend | null {
    return this.hostPlatform === 'darwin' ? 'tart' : this.hostPlatform === 'win32' ? 'hyper-v' : null;
  }

  private async discover(backend: FleetBackend): Promise<FleetMachine[]> {
    if (backend === 'tart') {
      const parsed: unknown = JSON.parse(await this.runner.run('tart', ['list', '--source', 'local', '--format', 'json'], 15_000));
      if (!Array.isArray(parsed)) throw new Error('Tart returned an invalid VM list.');
      return parsed.slice(0, 500).flatMap((item: unknown) => {
        if (!isRecord(item)) return [];
        const name = typeof item.Name === 'string' ? item.Name : item.name;
        if (typeof name !== 'string' || !VM_NAME.test(name)) return [];
        const running = item.Running === true || item.running === true;
        return [{ id: `tart:${name}`, name, backend, state: running ? 'running' as const : 'stopped' as const,
          base: false, canRun: false, canClone: false, guestPlatform: 'darwin' as const, privilege: 'standard' as const, sshConfigured: false, sshIdentityConfigured: false, sshKnownHostsConfigured: false, sshHost: null, sshUser: null, owner: null }];
      });
    }
    const output = await this.powershell("Get-VM | Select-Object Name, @{Name='VMId';Expression={$_.VMId.ToString()}}, @{Name='State';Expression={$_.State.ToString()}} | ConvertTo-Json -Compress", 15_000);
    const parsed: unknown = output.trim() ? JSON.parse(output) : [];
    const items: unknown[] = Array.isArray(parsed) ? parsed : [parsed];
    return items.slice(0, 500).flatMap((item) => {
      if (!isRecord(item) || typeof item.Name !== 'string' || !VM_NAME.test(item.Name)) return [];
      const id = typeof item.VMId === 'string' ? item.VMId : null;
      if (!id) return [];
      return [{ id: `hyper-v:${id}`, name: item.Name, backend, state: item.State === 'Running' ? 'running' as const : item.State === 'Off' ? 'stopped' as const : 'unknown' as const,
        base: false, canRun: false, canClone: false, guestPlatform: 'win32' as const, privilege: 'standard' as const, sshConfigured: false, sshIdentityConfigured: false, sshKnownHostsConfigured: false, sshHost: null, sshUser: null, owner: null }];
    });
  }

  private powershell(script: string, timeoutMs: number): Promise<string> {
    return this.runner.run('powershell.exe', ['-NoProfile', '-NonInteractive', '-EncodedCommand', Buffer.from(script, 'utf16le').toString('base64')], timeoutMs);
  }

  private async discoverIp(machine: FleetMachine): Promise<string | null> {
    if (machine.backend === 'tart') {
      try {
        const output = await this.runner.run('tart', ['ip', machine.name, '--resolver', 'agent', '--wait', '5'], 8_000);
        const ip = ipv4FromOutput(output);
        if (ip) return ip;
      } catch { /* Older or unconfigured guests may not provide Tart guest-agent IP resolution. */ }
      const output = await this.runner.run('tart', ['ip', machine.name, '--wait', '10'], 15_000);
      return ipv4FromOutput(output);
    }
    const script = `(Get-VMNetworkAdapter -VMName ${powershellString(machine.name)} -ErrorAction Stop | ForEach-Object IPAddresses | Where-Object { $_ -match '^\\d+\\.\\d+\\.\\d+\\.\\d+$' -and $_ -notmatch '^(127|169\\.254)\\.' } | Select-Object -First 1)`;
    return (await this.powershell(script, 30_000)).trim() || null;
  }

  private project(config: FleetConfiguration, machines: FleetMachine[], available: boolean, error: string | null): FleetState {
    return {
      role: config.role,
      enabled: config.enabled,
      available,
      error,
      machines,
      remoteMachines: [],
      machineId: config.machineId,
      appServers: Object.entries(config.appServers).map(([id, server]) => ({ id, name: server.name, url: server.url }))
        .sort((left, right) => left.name.localeCompare(right.name) || left.id.localeCompare(right.id)),
      browsers: detectFleetBrowsers(this.hostPlatform).map((browser) => ({ ...browser, enabled: config.browserEnabled[browser.id] !== false })),
      sandboxes: FLEET_SANDBOX_PROVIDERS.map((provider) => ({
        provider,
        name: FLEET_SANDBOX_PROVIDER_DETAILS[provider].name,
        enabled: config.sandboxEnabled[provider] === true,
        credentialsConfigured: FLEET_SANDBOX_PROVIDER_DETAILS[provider].credentials.every((name) => Boolean(process.env[name])),
      })),
      browserHomePage: config.browserHomePage,
      workspaceTargets: Object.fromEntries(Object.entries(config.workspaceTargets).map(([id, targets]) => [id, [...targets]])),
      primary: config.primary,
      requiredWorkspaceIds: [...config.requiredWorkspaceIds],
      optionalWorkspaceIds: [...config.optionalWorkspaceIds],
      lastMachineByWorkspace: { ...config.lastMachineByWorkspace },
    };
  }

  private read(): FleetConfiguration {
    if (!existsSync(this.path)) {
      const config = defaultConfig();
      this.write(config);
      return config;
    }
    try {
      const parsed: unknown = JSON.parse(readFileSync(this.path, 'utf8'));
      if (!isRecord(parsed) || (parsed.version !== 1 && parsed.version !== 2 && parsed.version !== 3)) throw new Error('Unsupported Fleet configuration.');
      const config = defaultConfig();
      if (typeof parsed.machineId === 'string' && VM_NAME.test(parsed.machineId)) config.machineId = parsed.machineId;
      if (parsed.role === 'primary' || parsed.role === 'guest') config.role = parsed.role;
      config.enabled = parsed.enabled !== false;
      if (typeof parsed.browserHomePage === 'string') {
        try { config.browserHomePage = normalizeBrowserHomePage(parsed.browserHomePage); } catch { /* Use the default for invalid saved URLs. */ }
      }
      if (isRecord(parsed.primary) && typeof parsed.primary.name === 'string') {
        config.primary = { name: parsed.primary.name, sshHost: optionalText(parsed.primary.sshHost) };
      }
      if (Array.isArray(parsed.requiredWorkspaceIds)) config.requiredWorkspaceIds = parsed.requiredWorkspaceIds.filter((value): value is string => typeof value === 'string' && WORKSPACE_ID.test(value));
      if (Array.isArray(parsed.optionalWorkspaceIds)) config.optionalWorkspaceIds = parsed.optionalWorkspaceIds.filter((value): value is string => typeof value === 'string' && WORKSPACE_ID.test(value));
      if (isRecord(parsed.lastMachineByWorkspace)) {
        for (const [key, value] of Object.entries(parsed.lastMachineByWorkspace)) {
          if (WORKSPACE_ID.test(key) && typeof value === 'string') config.lastMachineByWorkspace[key] = value;
        }
      }
      if (isRecord(parsed.workspaceTargets)) {
        for (const [key, value] of Object.entries(parsed.workspaceTargets)) {
          if (WORKSPACE_ID.test(key) && Array.isArray(value)) {
            config.workspaceTargets[key] = value.filter((id): id is string => typeof id === 'string'
              && id.length <= 300 && isWorkspaceTargetId(id)).slice(0, 128);
          }
        }
      }
      if (isRecord(parsed.browserEnabled)) {
        for (const [id, enabled] of Object.entries(parsed.browserEnabled)) {
          if (/^[a-z0-9-]+$/u.test(id) && typeof enabled === 'boolean') config.browserEnabled[id] = enabled;
        }
      }
      if (isRecord(parsed.sandboxEnabled)) {
        for (const [provider, enabled] of Object.entries(parsed.sandboxEnabled)) {
          if (isFleetSandboxProvider(provider) && typeof enabled === 'boolean') config.sandboxEnabled[provider] = enabled;
        }
      }
      if (isRecord(parsed.machines)) {
        for (const [key, value] of Object.entries(parsed.machines)) {
          if (!isRecord(value)) continue;
          config.machines[key] = {
            base: value.base === true,
            canRun: typeof value.canRun === 'boolean' ? value.canRun : value.base !== true,
            canClone: typeof value.canClone === 'boolean' ? value.canClone : value.base === true,
            guestPlatform: value.guestPlatform === 'win32' || value.guestPlatform === 'darwin' || value.guestPlatform === 'linux'
              ? value.guestPlatform : key.startsWith('hyper-v:') ? 'win32' : 'darwin',
            sessionClone: value.sessionClone === true,
            privilege: value.privilege === 'elevated' ? 'elevated' : 'standard',
            sshHost: optionalText(value.sshHost),
            sshUser: optionalText(value.sshUser),
            sshIdentityFile: optionalText(value.sshIdentityFile),
            sshKnownHostsFile: optionalText(value.sshKnownHostsFile),
          };
        }
      }
      if (isRecord(parsed.appServers)) {
        for (const [id, value] of Object.entries(parsed.appServers)) {
          if (!VM_NAME.test(id) || !isRecord(value) || typeof value.name !== 'string'
            || typeof value.operatorToken !== 'string' || !value.operatorToken) continue;
          try { config.appServers[id] = { name: value.name, url: normalizeTailnetOrigin(value.url), operatorToken: value.operatorToken }; }
          catch { /* Preserve valid remote entries without exposing malformed endpoints. */ }
        }
      }
      if (isRecord(parsed.owners)) {
        for (const [id, value] of Object.entries(parsed.owners)) {
          if (!isRecord(value)) continue;
          const owner = { machineId: value.machineId, sessionId: value.sessionId,
            ...(typeof value.workspaceId === 'string' ? { workspaceId: value.workspaceId } : {}) };
          try { validOwner(owner); config.owners[id] = owner as { machineId: string; sessionId: string; workspaceId?: string }; }
          catch { /* Ignore malformed ownership records. */ }
        }
      }
      if (parsed.version !== 3) this.write(config);
      return config;
    } catch {
      throw new Error('Fleet configuration is invalid; preserve the file and repair it before changing Fleet.');
    }
  }

  private write(config: FleetConfiguration): void {
    mkdirSync(dirname(this.path), { recursive: true, mode: 0o700 });
    const temporary = `${this.path}.${randomUUID()}.tmp`;
    writeFileSync(temporary, `${JSON.stringify(config, null, 2)}\n`, { mode: 0o600 });
    chmodSync(temporary, 0o600);
    renameSync(temporary, this.path);
  }
}

export async function probeFleetAppServerStatus(
  id: string,
  url: string,
  operatorToken: string,
  verifyPeer: (url: string) => Promise<void>,
  fetcher: typeof fetch = fetch
): Promise<FleetAppServerStatus> {
  const offline = (message: string): FleetAppServerStatus => ({ id, state: 'offline', message });
  const issue = (message: string): FleetAppServerStatus => ({ id, state: 'issue', message });
  try { await verifyPeer(url); }
  catch { return offline('Unavailable on this Tailscale network.'); }
  let response: Response;
  try {
    response = await fetcher(url + BEALE_APP_SERVER_SERVER_PATH, {
      headers: { authorization: `Bearer ${operatorToken}` },
      redirect: 'error', signal: AbortSignal.timeout(5_000)
    });
  } catch { return offline('Could not reach the app server.'); }
  if (response.status === 401 || response.status === 403) return issue('App-server authentication failed.');
  if (!response.ok) return issue(`App server responded with HTTP ${response.status}.`);
  try { decodeBealeAppServerDescriptor(await response.json()); }
  catch { return issue('Incompatible or invalid app-server contract.'); }
  return { id, state: 'online', message: 'Connected to a compatible app server.' };
}

export function assertCompatibleRemoteAppServerDescriptor(descriptor: unknown): void {
  if (!isRecord(descriptor) || descriptor.ok !== true) {
    throw new Error('The remote app server returned an invalid descriptor.');
  }
  const timestamp = descriptor.contractTimestamp;
  if (typeof timestamp === 'string' && /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/u.test(timestamp)
    && timestamp !== BEALE_APP_SERVER_CONTRACT_TIMESTAMP) {
    const detail = `(remote ${timestamp}; local ${BEALE_APP_SERVER_CONTRACT_TIMESTAMP})`;
    if (timestamp < BEALE_APP_SERVER_CONTRACT_TIMESTAMP) {
      throw new Error(`The remote app server is running an older control contract ${detail}. Run pnpm build on that machine, then restart its app-server.`);
    }
    throw new Error(`The remote app server is running a newer control contract ${detail}. Update and restart this Beale installation.`);
  }
  if (timestamp !== BEALE_APP_SERVER_CONTRACT_TIMESTAMP
    || descriptor.controlVersion !== BEALE_APP_SERVER_CONTROL_VERSION) {
    throw new Error('The remote app server has an incompatible control contract. Rebuild and restart the remote app-server.');
  }
}

function defaultMachineConfig(): FleetMachineConfig {
  return { base: false, canRun: true, canClone: false, guestPlatform: 'win32', sessionClone: false, privilege: 'standard', sshHost: null, sshUser: null, sshIdentityFile: null, sshKnownHostsFile: null };
}

function defaultConfig(): FleetConfiguration {
  return { version: 3, machineId: randomUUID(), role: 'primary', enabled: true, primary: null, requiredWorkspaceIds: [], optionalWorkspaceIds: [], lastMachineByWorkspace: {}, workspaceTargets: {}, browserEnabled: {}, sandboxEnabled: {}, browserHomePage: DEFAULT_BROWSER_HOME_PAGE, machines: {}, appServers: {}, owners: {} };
}

function validOwner(owner: { machineId: unknown; sessionId: unknown; workspaceId?: unknown }): void {
  if (typeof owner.machineId !== 'string' || !VM_NAME.test(owner.machineId)
    || typeof owner.sessionId !== 'string' || !VM_NAME.test(owner.sessionId)
    || owner.workspaceId !== undefined && (typeof owner.workspaceId !== 'string' || !WORKSPACE_ID.test(owner.workspaceId))) {
    throw new Error('Fleet owner requires a machine and session ID.');
  }
}


function requiredText(value: unknown, name: string): string {
  if (typeof value !== 'string' || !value.trim() || value.length > 512) throw new Error(`${name} is required.`);
  return value.trim();
}

function optionalText(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value.trim().slice(0, 512) : null;
}

function validVmName(value: unknown): string {
  if (typeof value !== 'string' || !VM_NAME.test(value)) throw new Error('VM name must use letters, digits, dots, underscores, or hyphens.');
  return value;
}

function validWorkspaceId(value: unknown): string {
  if (typeof value !== 'string' || !WORKSPACE_ID.test(value)) throw new Error('Invalid workspace ID.');
  return value;
}

function powershellString(value: string): string {
  return `'${value.replaceAll("'", "''")}'`;
}

function boundedError(error: unknown): string {
  return (error instanceof Error ? error.message : String(error)).slice(0, 500);
}

function ipv4FromOutput(output: string): string | null {
  return output.trim().split(/\s+/u).find((value) => /^\d{1,3}(?:\.\d{1,3}){3}$/u.test(value)) ?? null;
}

function sshFailureMessage(error: unknown): string {
  const stderr = isRecord(error) && typeof error.stderr === 'string' ? error.stderr : '';
  if (/identity file .*not accessible|no such identity/iu.test(stderr)) return 'SSH identity file could not be read. Check its path on the primary machine.';
  if (/host key verification failed|remote host identification has changed/iu.test(stderr)) return 'SSH host key verification failed. Review the VM entry in the known-hosts file before replacing it.';
  if (/permission denied/iu.test(stderr)) return 'SSH authentication failed. Check the guest user and private key.';
  if (/connection refused/iu.test(stderr)) return 'The VM refused SSH. Check that Remote Login is enabled in the guest.';
  if (/no route to host|network is unreachable/iu.test(stderr)) return platform() === 'darwin'
    ? 'The app-server could not reach this VM. If Terminal can connect, check Beale Local Network access in macOS System Settings.'
    : 'The app-server could not reach this VM. Check its network and host override.';
  if (/timed out/iu.test(stderr)) return 'The VM SSH address could not be reached. Check its network and host override.';
  return 'SSH failed. Check the guest user, identity file, known-hosts file, and Remote Login. These settings have not been saved.';
}

function nonRetryableFleetSshFailure(error: unknown): string | null {
  const stderr = isRecord(error) && typeof error.stderr === 'string' ? error.stderr : '';
  return /host key verification failed|remote host identification has changed/iu.test(stderr)
    ? sshFailureMessage(error) : null;
}

function sessionCloneHostKeyAlias(machineId: string): string {
  return `beale-vm-${createHash('sha256').update(machineId).digest('hex').slice(0, 24)}`;
}

function sshFailureDetail(error: unknown): string | null {
  if (!isRecord(error)) return null;
  const stderr = typeof error.stderr === 'string' ? error.stderr : '';
  const lastLine = stderr.split(/\r?\n/u).map((line) => line.trim()).filter(Boolean).at(-1);
  if (lastLine) return lastLine.replaceAll(homedir(), '~').replace(/[\x00-\x1f\x7f]/gu, '').slice(0, 240);
  if (error.killed === true) return 'SSH process exceeded the test timeout.';
  return null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}
