import { spawn, execFile, type ChildProcess } from 'node:child_process';
import { createServer } from 'node:net';
import { hostname, platform } from 'node:os';
import { promisify } from 'node:util';
import {
  BEALE_APP_SERVER_CONTRACT_TIMESTAMP,
  BEALE_APP_SERVER_CONTROL_VERSION,
  BEALE_APP_SERVER_OPERATIONS_PATH,
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
}

interface Tunnel extends FleetRemoteEndpoint { process: ChildProcess }
const activeTunnelProcesses = new Set<ChildProcess>();
process.once('exit', () => { for (const child of activeTunnelProcesses) child.kill(); });

export class FleetRemoteController {
  private readonly tunnels = new Map<string, Tunnel>();

  public constructor(private readonly fleet: FleetService, private readonly files: FleetWorkspaceStore) {}

  public async prepare(workspace: AppServerHostWorkspace, runId: string, machineId: string): Promise<FleetRemoteEndpoint> {
    const state = await this.fleet.state();
    if (state.role !== 'primary' || !state.enabled || !state.available) throw new Error('Fleet is unavailable on this primary machine.');
    const machine = state.machines.find((candidate) => candidate.id === machineId);
    if (!machine || machine.base || !machine.sshConfigured || machine.state === 'unknown') throw new Error('Select a configured, runnable Fleet VM.');
    if (machine.state !== 'running') {
      await this.fleet.start(machineId);
      const deadline = Date.now() + 90_000;
      while (Date.now() < deadline) {
        const current = await this.fleet.state();
        if (current.machines.some((candidate) => candidate.id === machineId && candidate.state === 'running')) break;
        await delay(1_000);
      }
    }
    const endpoint = await this.connectWithRetry(machineId, 120_000);
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
    this.files.saveBaseline({ workspaceId: workspace.workspaceId, workspacePath: workspace.workspacePath, machineId, runId,
      files: Object.fromEntries(sourceFiles.map((file) => [file.path, file.sha256])) });
    return endpoint;
  }

  public async complete(runId: string): Promise<{ imported: number; conflicts: number; candidateRecords: number }> {
    const baseline = this.files.readBaseline(runId);
    const endpoint = await this.connectWithRetry(baseline.machineId, 45_000);
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
    return { imported, conflicts, candidateRecords };
  }

  public async validateLocal(workspaceId: string, mode: string | null): Promise<void> {
    if (mode === 'quick-chat') return;
    const state = await this.fleet.state();
    if (!state.enabled || state.role !== 'primary') return;
    const required = state.requiredWorkspaceIds.includes(workspaceId)
      || !state.optionalWorkspaceIds.includes(workspaceId)
        && state.machines.some((machine) => machine.base);
    if (required) throw new Error('This workspace requires a Fleet VM. Select a VM or disable the requirement in Fleet settings.');
  }

  public async connect(machineId: string): Promise<FleetRemoteEndpoint> {
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

  private async connectWithRetry(machineId: string, timeoutMs: number): Promise<FleetRemoteEndpoint> {
    const deadline = Date.now() + timeoutMs;
    let lastError: unknown = null;
    do {
      try { return await this.connect(machineId); }
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
