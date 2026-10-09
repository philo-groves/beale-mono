import { spawn, execFile, type ChildProcess } from 'node:child_process';
import { createServer } from 'node:net';
import { hostname, platform } from 'node:os';
import { promisify } from 'node:util';
import {
  BEALE_APP_SERVER_CONTRACT_TIMESTAMP,
  BEALE_APP_SERVER_CONTROL_VERSION,
  BEALE_APP_SERVER_OPERATIONS_PATH,
  BEALE_APP_SERVER_SUPERVISOR_LOCAL_PORT,
  BEALE_APP_SERVER_SUPERVISOR_RESTART_PATH,
} from '@beale/app-server-runtime/protocol';
import type { AppServerHostWorkspace } from './hostRegistry.js';
import { FleetService } from './fleet.js';
import { FleetWorkspaceStore, type FleetFile } from './fleetWorkspace.js';
import { sshConnectionArgs } from './fleetSsh.js';

const execFileAsync = promisify(execFile);
const CONNECT_TIMEOUT_MS = 30_000;

export interface FleetRemoteEndpoint {
  machineId: string;
  url: string;
  operatorToken: string;
  remoteMachineId?: string;
  ownerMachineId?: string;
}

interface Tunnel extends FleetRemoteEndpoint { process: ChildProcess }
const activeTunnelProcesses = new Set<ChildProcess>();
process.once('exit', () => { for (const child of activeTunnelProcesses) child.kill(); });

export class FleetRemoteController {
  private readonly tunnels = new Map<string, Tunnel>();
  private readonly completions = new Map<string, Promise<{ imported: number; conflicts: number; candidateRecords: number }>>();

  public constructor(private readonly fleet: FleetService, private readonly files: FleetWorkspaceStore) {}

  public async restartGuestAppServer(machineId: string): Promise<{ restarted: true }> {
    const state = await this.fleet.state();
    if (!state.enabled || state.role !== 'primary') throw new Error('Fleet is unavailable on this primary machine.');
    const connection = await this.fleet.connection(machineId);
    if (connection.machine.base) throw new Error('A Fleet base VM must not be started or restarted.');
    const sshArgs = sshConnectionArgs(connection.sshHost, connection.sshUser,
      connection.sshIdentityFile, connection.sshKnownHostsFile, connection.machine.backend);
    const readCommand = platform() === 'win32'
      ? `powershell.exe -NoProfile -NonInteractive -EncodedCommand ${Buffer.from("$discovery=Join-Path $HOME '.beale/app-server.json'; if (Test-Path $discovery) { (Get-Content -Raw $discovery | ConvertFrom-Json).operatorToken } else { Get-Content -Raw (Join-Path $HOME '.beale/app-server.token') }", 'utf16le').toString('base64')}`
      : 'if test -f .beale/app-server.json; then cat .beale/app-server.json; else cat .beale/app-server.token; fi';
    const { stdout } = await execFileAsync('ssh', [...sshArgs, readCommand], {
      timeout: CONNECT_TIMEOUT_MS, maxBuffer: 100_000, encoding: 'utf8', windowsHide: true
    });
    const raw = stdout.trim();
    const token = raw.startsWith('{') ? (JSON.parse(raw) as { operatorToken?: unknown }).operatorToken : raw;
    if (typeof token !== 'string' || !token || token.length > 4_096) throw new Error('The guest restart token is unavailable.');
    const localPort = await freePort();
    const child = spawn('ssh', [
      '-o', 'ExitOnForwardFailure=yes', ...sshArgs.slice(0, -1),
      '-N', '-L', `127.0.0.1:${localPort}:127.0.0.1:${BEALE_APP_SERVER_SUPERVISOR_LOCAL_PORT}`,
      sshArgs.at(-1)!,
    ], { stdio: 'ignore', windowsHide: true });
    activeTunnelProcesses.add(child);
    let tunnelError: Error | null = null;
    child.once('error', (error) => { tunnelError = error; activeTunnelProcesses.delete(child); });
    child.once('exit', () => activeTunnelProcesses.delete(child));
    try {
      const url = `http://127.0.0.1:${localPort}`;
      const deadline = Date.now() + CONNECT_TIMEOUT_MS;
      let ready = false;
      while (Date.now() < deadline && child.exitCode === null && !tunnelError) {
        try {
          const response = await fetch(`${url}/health`, { signal: AbortSignal.timeout(1_000) });
          if (response.ok) { ready = true; break; }
        } catch { /* SSH forwarding may still be starting. */ }
        await delay(250);
      }
      if (tunnelError) throw tunnelError;
      if (!ready) throw new Error('The guest app-server restart supervisor is unavailable over SSH.');
      const response = await fetch(url + BEALE_APP_SERVER_SUPERVISOR_RESTART_PATH, {
        method: 'POST', headers: { authorization: `Bearer ${token}` }, signal: AbortSignal.timeout(90_000)
      });
      if (!response.ok) throw new Error(`The guest app-server restart supervisor refused the request (HTTP ${response.status}).`);
      return { restarted: true };
    } finally {
      child.kill();
      activeTunnelProcesses.delete(child);
    }
  }

  public async prepare(workspace: AppServerHostWorkspace, runId: string, machineId: string, ownerMachineId = this.fleet.machineId()): Promise<FleetRemoteEndpoint> {
    const remote = parseRemoteMachineId(machineId);
    if (remote) return this.prepareRemote(workspace, runId, machineId, remote.serverId, remote.machineId);
    const state = await this.fleet.state();
    if (state.role !== 'primary' || !state.enabled || !state.available) throw new Error('Fleet is unavailable on this primary machine.');
    const machine = state.machines.find((candidate) => candidate.id === machineId);
    if (!machine || machine.state === 'unknown') throw new Error('Select a Fleet VM.');
    if (!machine.sshConfigured) throw new Error('Set an SSH user for this base in local Fleet settings before launching research.');
    const owner = { machineId: ownerMachineId, sessionId: runId,
      ...(ownerMachineId !== this.fleet.machineId() ? { workspaceId: workspace.workspaceId } : {}) };
    const workerId = machine.base
      ? (await this.fleet.cloneForSession(machineId, owner)).id : machineId;
    if (!machine.base) await this.fleet.reserve(workerId, owner);
    try {
    if (machine.base || machine.state !== 'running') {
      await this.fleet.start(workerId, owner);
      const deadline = Date.now() + 90_000;
      while (Date.now() < deadline) {
        const current = await this.fleet.state();
        if (current.machines.some((candidate) => candidate.id === workerId && candidate.state === 'running')) break;
        await delay(1_000);
      }
    }
    const endpoint = await this.connectWithRetry(workerId, runId, 120_000, ownerMachineId);
    const sourceFiles = this.files.sourceFiles(workspace);
    const prior = await remoteOperation<{ files: FleetFile[] }>(endpoint, 'fleet.stage', {
      action: 'begin', workspaceId: workspace.workspaceId, primaryName: hostname(),
    });
    const remoteHashes = new Map(prior.files.map((file) => [file.path, file.sha256]));
    for (const file of sourceFiles) {
      if (remoteHashes.get(file.path) === file.sha256) continue;
      for (let offset = 0; offset < file.size; offset += 192 * 1024) {
        await remoteOperation(endpoint, 'fleet.stage', {
          action: 'write', workspaceId: workspace.workspaceId, path: file.path,
          offset, data: this.files.readSourceChunk(workspace.workspacePath, file.path, offset),
        });
      }
      if (file.size === 0) await remoteOperation(endpoint, 'fleet.stage', { action: 'write', workspaceId: workspace.workspaceId, path: file.path, offset: 0, data: '' });
      await remoteOperation(endpoint, 'fleet.stage', { action: 'seal', workspaceId: workspace.workspaceId, path: file.path, sha256: file.sha256 });
    }
    await remoteOperation(endpoint, 'fleet.stage', {
      action: 'finish', workspaceId: workspace.workspaceId, name: workspace.name,
      researchKitId: workspace.researchKitId,
    });
    this.files.saveBaseline({ workspaceId: workspace.workspaceId, workspacePath: workspace.workspacePath, machineId: workerId,
      ...(machine.base ? { selectedBaseId: machineId } : {}), runId, ownerMachineId,
      files: Object.fromEntries(sourceFiles.map((file) => [file.path, file.sha256])) });
    return endpoint;
    } catch (error) {
      if (machine.base) await this.fleet.stop(workerId, owner).catch(() => undefined);
      await this.fleet.release(workerId, owner).catch(() => undefined);
      throw error;
    }
  }

  public complete(runId: string): Promise<{ imported: number; conflicts: number; candidateRecords: number }> {
    const pending = this.completions.get(runId);
    if (pending) return pending;
    const completion = this.completeOnce(runId);
    this.completions.set(runId, completion);
    void completion.finally(() => { if (this.completions.get(runId) === completion) this.completions.delete(runId); }).catch(() => undefined);
    return completion;
  }

  private async completeOnce(runId: string): Promise<{ imported: number; conflicts: number; candidateRecords: number }> {
    const baseline = this.files.readBaseline(runId);
    if (baseline.completedResult) return baseline.completedResult;
    if (baseline.remoteServerId) return this.completeRemote(baseline);
    const ownerMachineId = baseline.ownerMachineId ?? this.fleet.machineId();
    const endpoint = await this.connectWithRetry(baseline.machineId, runId, 45_000, ownerMachineId);
    const exportList = await remoteOperation<{ files: FleetFile[] }>(endpoint, 'fleet.export', { action: 'list', workspaceId: baseline.workspaceId });
    let imported = 0;
    let conflicts = 0;
    let candidateRecords = 0;
    for (const file of exportList.files) {
      if (baseline.files[file.path] === file.sha256) continue;
      if (file.path === 'references/research-index.json' || file.path.startsWith('traces/')) continue;
      const result = await this.files.importGuestFile(baseline, file, async (offset) => {
        const result = await remoteOperation<{ data: string; size: number }>(endpoint, 'fleet.export', {
          action: 'read', workspaceId: baseline.workspaceId, path: file.path, offset,
        });
        if (result.size !== file.size) throw new Error('Fleet result changed during transfer.');
        return result.data;
      });
      if (result === 'conflict') conflicts += 1;
      else if (result === 'candidate') candidateRecords += 1;
      else imported += 1;
    }
    if (ownerMachineId === this.fleet.machineId() && this.fleet.isSessionClone(baseline.machineId)) {
      await this.fleet.stop(baseline.machineId, { machineId: ownerMachineId, sessionId: runId });
    }
    if (ownerMachineId === this.fleet.machineId()) {
      await this.fleet.release(baseline.machineId, { machineId: ownerMachineId, sessionId: runId });
    }
    const result = { imported, conflicts, candidateRecords };
    this.files.saveBaseline({ ...baseline, completedResult: result });
    return result;
  }

  public async validateLocal(workspaceId: string, mode: string | null): Promise<void> {
    if (mode === 'quick-chat') return;
    const state = await this.fleet.state();
    if (!state.enabled || state.role !== 'primary') return;
    const required = state.requiredWorkspaceIds.includes(workspaceId)
      || !state.optionalWorkspaceIds.includes(workspaceId)
        && (state.machines.some((machine) => machine.base)
          || (await this.fleet.remoteMachines()).some((machine) => machine.base));
    if (required) throw new Error('This workspace requires a Fleet VM. Select a VM or disable the requirement in workspace Settings.');
  }

  public async connect(machineId: string, runId: string, ownerMachineId = this.fleet.machineId()): Promise<FleetRemoteEndpoint> {
    const baseline = this.files.baselineIfExists(runId);
    if (baseline?.selectedBaseId === machineId) {
      machineId = baseline.remoteServerId && baseline.remoteMachineId
        ? `remote:${baseline.remoteServerId}:${baseline.remoteMachineId}` : baseline.machineId;
    }
    const remote = parseRemoteMachineId(machineId);
    if (remote) {
      const server = await this.fleet.connectedRemoteAppServer(remote.serverId);
      await this.fleet.callRemote(remote.serverId, 'fleet.connect', {
        machineId: remote.machineId, runId, ownerMachineId, proxy: true,
      });
      return { machineId, url: server.url, operatorToken: server.operatorToken,
        remoteMachineId: remote.machineId, ownerMachineId };
    }
    this.fleet.assertReserved(machineId, { machineId: ownerMachineId, sessionId: runId });
    const existing = this.tunnels.get(machineId);
    if (existing && existing.process.exitCode === null) {
      if (await healthy(existing)) return existing;
      existing.process.kill();
      this.tunnels.delete(machineId);
    }
    const connection = await this.fleet.connection(machineId);
    const sshArgs = sshConnectionArgs(connection.sshHost, connection.sshUser, connection.sshIdentityFile, connection.sshKnownHostsFile, connection.machine.backend);
    const readCommand = platform() === 'win32'
      ? `powershell.exe -NoProfile -NonInteractive -EncodedCommand ${Buffer.from("Get-Content -Raw (Join-Path $HOME '.beale/app-server.json')", 'utf16le').toString('base64')}`
      : 'cat .beale/app-server.json';
    const { stdout } = await execFileAsync('ssh', [...sshArgs, readCommand], { timeout: CONNECT_TIMEOUT_MS, maxBuffer: 100_000, encoding: 'utf8', windowsHide: true });
    const discovery: unknown = JSON.parse(stdout);
    if (!isRecord(discovery) || discovery.contractTimestamp !== BEALE_APP_SERVER_CONTRACT_TIMESTAMP
      || typeof discovery.port !== 'number' || discovery.port < 1 || discovery.port > 65535
      || typeof discovery.operatorToken !== 'string' || !discovery.operatorToken) {
      throw new Error('Guest Beale app-server is unavailable or has an incompatible control contract.');
    }
    const localPort = await freePort();
    const child = spawn('ssh', [
      '-o', 'ExitOnForwardFailure=yes', ...sshArgs.slice(0, -1),
      '-N', '-L', `127.0.0.1:${localPort}:127.0.0.1:${discovery.port}`,
      sshArgs.at(-1)!,
    ], { stdio: 'ignore', windowsHide: true });
    activeTunnelProcesses.add(child);
    let spawnError: Error | null = null;
    child.once('error', (error) => { spawnError = error; });
    child.once('exit', () => {
      activeTunnelProcesses.delete(child);
      if (this.tunnels.get(machineId)?.process === child) this.tunnels.delete(machineId);
    });
    const tunnel: Tunnel = { machineId, url: `http://127.0.0.1:${localPort}`, operatorToken: discovery.operatorToken, process: child };
    const deadline = Date.now() + CONNECT_TIMEOUT_MS;
    while (Date.now() < deadline) {
      if (spawnError) throw spawnError;
      if (child.exitCode !== null) throw new Error('SSH could not forward the guest Beale app-server port.');
      if (await healthy(tunnel)) {
        const state = await remoteOperation<{ role: string; enabled: boolean }>(tunnel, 'fleet.state', {});
        if (state.role !== 'guest' || !state.enabled) {
          child.kill();
          throw new Error('The selected VM is not configured as an enabled Fleet guest.');
        }
        this.tunnels.set(machineId, tunnel);
        return tunnel;
      }
      await delay(250);
    }
    child.kill();
    throw new Error('Timed out connecting to the guest Beale app-server over SSH.');
  }

  private async prepareRemote(
    workspace: AppServerHostWorkspace, runId: string, machineId: string, serverId: string, remoteMachineId: string,
  ): Promise<FleetRemoteEndpoint> {
    const server = await this.fleet.connectedRemoteAppServer(serverId);
    const remoteState = await this.fleet.callRemote<{ enabled: boolean; role: string; machines: Array<{ id: string; base: boolean; sshConfigured: boolean; state: string; owner?: { machineId: string; sessionId: string } | null }> }>(serverId, 'fleet.state', {});
    const machine = remoteState.machines.find((candidate) => candidate.id === remoteMachineId);
    if (!remoteState.enabled || remoteState.role !== 'primary' || !machine || machine.state === 'unknown') {
      throw new Error('The remote app server does not have this Fleet VM.');
    }
    if (!machine.sshConfigured) throw new Error('Set an SSH user for this base in the remote machine’s local Fleet settings.');
    if (machine.owner && (machine.owner.machineId !== this.fleet.machineId() || machine.owner.sessionId !== runId)) {
      throw new Error('The remote VM is reserved by another machine or session.');
    }
    const workerId = machine.base
      ? (await this.fleet.callRemote<{ machineId: string }>(serverId, 'fleet.clone_for_session', {
        baseId: remoteMachineId, ownerMachineId: this.fleet.machineId(), sessionId: runId,
        workspaceId: workspace.workspaceId,
      })).machineId
      : remoteMachineId;
    if (!machine.base) await this.fleet.callRemote(serverId, 'fleet.reserve', {
      machineId: workerId, ownerMachineId: this.fleet.machineId(), sessionId: runId,
      workspaceId: workspace.workspaceId,
    });
    try {
    const sourceFiles = this.files.sourceFiles(workspace);
    const prior = await this.fleet.callRemote<{ files: FleetFile[] }>(serverId, 'fleet.relay_stage', {
      action: 'begin', workspaceId: workspace.workspaceId,
    });
    const hashes = new Map(prior.files.map((file) => [file.path, file.sha256]));
    for (const file of sourceFiles) {
      if (hashes.get(file.path) === file.sha256) continue;
      for (let offset = 0; offset < file.size; offset += 192 * 1024) {
        await this.fleet.callRemote(serverId, 'fleet.relay_stage', {
          action: 'write', workspaceId: workspace.workspaceId, path: file.path, offset,
          data: this.files.readSourceChunk(workspace.workspacePath, file.path, offset),
        });
      }
      if (file.size === 0) await this.fleet.callRemote(serverId, 'fleet.relay_stage', {
        action: 'write', workspaceId: workspace.workspaceId, path: file.path, offset: 0, data: '',
      });
      await this.fleet.callRemote(serverId, 'fleet.relay_stage', {
        action: 'seal', workspaceId: workspace.workspaceId, path: file.path, sha256: file.sha256,
      });
    }
    await this.fleet.callRemote(serverId, 'fleet.relay_stage', {
      action: 'finish', workspaceId: workspace.workspaceId, name: workspace.name, researchKitId: workspace.researchKitId,
    });
    this.files.saveBaseline({ workspaceId: workspace.workspaceId, workspacePath: workspace.workspacePath,
      machineId, ...(machine.base ? { selectedBaseId: machineId } : {}),
      remoteServerId: serverId, remoteMachineId: workerId, runId, ownerMachineId: this.fleet.machineId(),
      files: Object.fromEntries(sourceFiles.map((file) => [file.path, file.sha256])) });
    return { machineId, url: server.url, operatorToken: server.operatorToken,
      remoteMachineId: workerId, ownerMachineId: this.fleet.machineId() };
    } catch (error) {
      await this.fleet.callRemote(serverId, 'fleet.release', {
        machineId: workerId, ownerMachineId: this.fleet.machineId(), sessionId: runId,
      }).catch(() => undefined);
      throw error;
    }
  }

  private async completeRemote(baseline: import('./fleetWorkspace.js').FleetTransferBaseline): Promise<{ imported: number; conflicts: number; candidateRecords: number }> {
    const serverId = baseline.remoteServerId!;
    if (!baseline.remoteCompleted) {
      await this.fleet.callRemote(serverId, 'fleet.complete', { runId: baseline.runId });
      baseline.remoteCompleted = true;
      this.files.saveBaseline(baseline);
    }
    const exportList = await this.fleet.callRemote<{ files: FleetFile[] }>(serverId, 'fleet.relay_export', { action: 'list', workspaceId: baseline.workspaceId });
    let imported = 0; let conflicts = 0; let candidateRecords = 0;
    for (const file of exportList.files) {
      if (baseline.files[file.path] === file.sha256 || file.path === 'references/research-index.json' || file.path.startsWith('traces/')) continue;
      const result = await this.files.importGuestFile(baseline, file, async (offset) => {
        const chunk = await this.fleet.callRemote<{ data: string; size: number }>(serverId, 'fleet.relay_export', {
          action: 'read', workspaceId: baseline.workspaceId, path: file.path, offset,
        });
        if (chunk.size !== file.size) throw new Error('Remote Fleet result changed during transfer.');
        return chunk.data;
      });
      if (result === 'conflict') conflicts += 1;
      else if (result === 'candidate') candidateRecords += 1;
      else imported += 1;
    }
    const result = { imported, conflicts, candidateRecords };
    await this.fleet.callRemote(serverId, 'fleet.release', {
      machineId: baseline.remoteMachineId, ownerMachineId: this.fleet.machineId(), sessionId: baseline.runId,
    });
    this.files.saveBaseline({ ...baseline, completedResult: result });
    return result;
  }

  private async connectWithRetry(machineId: string, runId: string, timeoutMs: number, ownerMachineId = this.fleet.machineId()): Promise<FleetRemoteEndpoint> {
    const deadline = Date.now() + timeoutMs;
    let lastError: unknown = null;
    do {
      try { return await this.connect(machineId, runId, ownerMachineId); }
      catch (error) { lastError = error; }
      if (Date.now() >= deadline) break;
      await delay(Math.min(2_000, deadline - Date.now()));
    } while (Date.now() < deadline);
    throw new Error(lastError ? 'Fleet could not connect to the guest. Check its SSH host key, user, and running Beale app-server.' : 'Fleet guest connection timed out.');
  }
}

async function remoteOperation<T>(endpoint: FleetRemoteEndpoint, operation: 'fleet.state' | 'fleet.stage' | 'fleet.export', input: Record<string, unknown>): Promise<T> {
  const response = await fetch(endpoint.url + BEALE_APP_SERVER_OPERATIONS_PATH, {
    method: 'POST', headers: { authorization: `Bearer ${endpoint.operatorToken}`, 'content-type': 'application/json' },
    body: JSON.stringify({ operation, input }), signal: AbortSignal.timeout(60_000),
  });
  if (!response.ok) throw new Error(`Guest Fleet ${operation} failed (${response.status}). Check the guest Fleet role and workspace setup.`);
  const payload: unknown = await response.json();
  if (!isRecord(payload) || payload.controlVersion !== BEALE_APP_SERVER_CONTROL_VERSION) throw new Error('Guest Fleet response has an incompatible control version.');
  return payload.result as T;
}

async function healthy(endpoint: FleetRemoteEndpoint): Promise<boolean> {
  try {
    const response = await fetch(endpoint.url + '/health', { signal: AbortSignal.timeout(1_000) });
    const body: unknown = await response.json();
    return response.ok && isRecord(body) && body.ok === true
      && body.contractTimestamp === BEALE_APP_SERVER_CONTRACT_TIMESTAMP;
  } catch { return false; }
}

function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      if (!address || typeof address === 'string') { server.close(); reject(new Error('Could not allocate Fleet tunnel port.')); return; }
      server.close(() => resolve(address.port));
    });
  });
}
function delay(milliseconds: number): Promise<void> { return new Promise((resolve) => setTimeout(resolve, milliseconds)); }
function isRecord(value: unknown): value is Record<string, unknown> { return typeof value === 'object' && value !== null && !Array.isArray(value); }
function parseRemoteMachineId(value: string): { serverId: string; machineId: string } | null {
  const match = /^remote:([A-Za-z0-9][A-Za-z0-9._-]{0,127}):(tart|hyper-v):([A-Za-z0-9][A-Za-z0-9._-]{0,127})$/u.exec(value);
  return match ? { serverId: match[1]!, machineId: `${match[2]}:${match[3]}` } : null;
}
