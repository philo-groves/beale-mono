import { execFile, spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { chmodSync, existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { homedir, platform } from 'node:os';
import { dirname, join } from 'node:path';
import { promisify } from 'node:util';
import type { FleetBackend, FleetMachine, FleetSshTestInput, FleetSshTestResult, FleetState } from '@beale/app-server-runtime/protocol';
import { sshConnectionArgs } from './fleetSsh.js';

const execFileAsync = promisify(execFile);
const VM_NAME = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/u;
const WORKSPACE_ID = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,255}$/u;

interface FleetMachineConfig {
  base: boolean;
  privilege: 'standard' | 'elevated';
  sshHost: string | null;
  sshUser: string | null;
  sshIdentityFile: string | null;
  sshKnownHostsFile: string | null;
}

interface FleetConfiguration {
  version: 1;
  role: 'primary' | 'guest';
  enabled: boolean;
  primary: { name: string; sshHost: string | null } | null;
  requiredWorkspaceIds: string[];
  optionalWorkspaceIds: string[];
  lastMachineByWorkspace: Record<string, string>;
  machines: Record<string, FleetMachineConfig>;
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
  public constructor(
    private readonly path = join(homedir(), '.beale', 'fleet.json'),
    private readonly hostPlatform = platform(),
    private readonly runner: FleetCommandRunner = defaultRunner,
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
          privilege: saved?.privilege ?? 'standard',
          sshConfigured: Boolean(saved?.sshUser),
          sshIdentityConfigured: Boolean(saved?.sshIdentityFile),
          sshKnownHostsConfigured: Boolean(saved?.sshKnownHostsFile),
          sshHost: saved?.sshHost ?? null,
          sshUser: saved?.sshUser ?? null,
        };
      }).sort((a, b) => a.name.localeCompare(b.name) || a.id.localeCompare(b.id));
      return this.project(config, machines, true, null);
    } catch (error) {
      return this.project(config, [], false, boundedError(error));
    }
  }

  public async configure(input: Record<string, unknown>): Promise<FleetState> {
    const config = this.read();
    const action = requiredText(input.action, 'Fleet action');
    if (action === 'set-role') {
      if (input.role !== 'primary' && input.role !== 'guest') throw new Error('Fleet role must be primary or guest.');
      config.role = input.role;
    } else if (action === 'set-enabled') {
      if (typeof input.enabled !== 'boolean') throw new Error('Fleet enabled must be boolean.');
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
      const machineId = requiredText(input.machineId, 'Machine ID');
      const state = await this.state();
      if (!state.machines.some((machine) => machine.id === machineId && !machine.base && machine.sshConfigured && machine.state !== 'unknown')) {
        throw new Error('Select an existing runnable Fleet machine.');
      }
      config.lastMachineByWorkspace[workspaceId] = machineId;
    } else if (action === 'set-machine') {
      const machineId = requiredText(input.machineId, 'Machine ID');
      const state = await this.state();
      if (!state.machines.some((machine) => machine.id === machineId)) throw new Error('Unknown Fleet machine.');
      const prior = config.machines[machineId] ?? defaultMachineConfig();
      const base = input.base === undefined ? prior.base : input.base;
      const privilege = input.privilege === undefined ? prior.privilege : input.privilege;
      if (typeof base !== 'boolean') throw new Error('Base must be boolean.');
      if (privilege !== 'standard' && privilege !== 'elevated') throw new Error('Privilege must be standard or elevated.');
      if (base && state.machines.find((machine) => machine.id === machineId)?.state !== 'stopped') {
        throw new Error('Stop a VM before marking it as a Fleet base.');
      }
      config.machines[machineId] = {
        base,
        privilege,
        sshHost: input.sshHost === undefined ? prior.sshHost : optionalText(input.sshHost),
        sshUser: input.sshUser === undefined ? prior.sshUser : optionalText(input.sshUser),
        sshIdentityFile: input.sshIdentityFile === undefined ? prior.sshIdentityFile : optionalText(input.sshIdentityFile),
        sshKnownHostsFile: input.sshKnownHostsFile === undefined ? prior.sshKnownHostsFile : optionalText(input.sshKnownHostsFile),
      };
    } else {
      throw new Error('Unsupported Fleet configuration action.');
    }
    this.write(config);
    return this.state();
  }

  public async clone(baseId: string, name: string): Promise<FleetState> {
    const config = this.read();
    this.requirePrimaryEnabled(config);
    const targetName = validVmName(name);
    const state = await this.state();
    const base = state.machines.find((machine) => machine.id === baseId);
    if (!base?.base || base.state !== 'stopped') throw new Error('Clone source must be a stopped, registered base VM.');
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
    config.machines[clone.id] = { ...config.machines[baseId]!, base: false, sshHost: null };
    this.write(config);
    return this.state();
  }

  public async start(machineId: string): Promise<FleetState> {
    const { machine } = await this.runnable(machineId);
    if (machine.state === 'running') return this.state();
    if (machine.backend === 'tart') this.runner.launch('tart', ['run', machine.name, '--no-graphics']);
    else await this.powershell(`Start-VM -Name ${powershellString(machine.name)} -ErrorAction Stop`, 120_000);
    return this.state();
  }

  public async stop(machineId: string): Promise<FleetState> {
    const { machine } = await this.runnable(machineId);
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
    const args = sshConnectionArgs(host, user, identity, knownHosts, machine.backend);
    try {
      await this.runner.run('ssh', [...args.slice(0, -1), '-o', 'NumberOfPasswordPrompts=0', args.at(-1)!, 'true'], 15_000);
      return { success: true, message: `Key-only SSH succeeded at ${host}. These settings have not been saved.` };
    } catch (error) {
      const detail = sshFailureDetail(error);
      return { success: false, message: `${sshFailureMessage(error)} Address checked: ${host}.${detail ? ` SSH detail: ${detail}` : ''}` };
    }
  }

  public async connection(machineId: string): Promise<{ machine: FleetMachine; sshHost: string; sshUser: string; sshIdentityFile: string | null; sshKnownHostsFile: string | null }> {
    const { machine, config } = await this.runnable(machineId);
    const saved = config.machines[machineId];
    if (machine.state !== 'running' || !saved?.sshUser) throw new Error('Fleet VM is not running or SSH user is not configured.');
    const sshHost = saved.sshHost ?? await this.discoverIp(machine);
    if (!sshHost) throw new Error('Fleet could not determine the running VM SSH address.');
    return { machine, sshHost, sshUser: saved.sshUser, sshIdentityFile: saved.sshIdentityFile, sshKnownHostsFile: saved.sshKnownHostsFile };
  }

  private async runnable(machineId: string): Promise<{ machine: FleetMachine; config: FleetConfiguration }> {
    const config = this.read();
    this.requirePrimaryEnabled(config);
    const state = await this.state();
    const machine = state.machines.find((candidate) => candidate.id === machineId);
    if (!machine || machine.base) throw new Error('Select an existing runnable Fleet VM.');
    return { machine, config };
  }

  private requirePrimaryEnabled(config: FleetConfiguration): void {
    if (config.role !== 'primary' || !config.enabled) throw new Error('Fleet is unavailable on this Beale instance.');
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
          base: false, privilege: 'standard' as const, sshConfigured: false, sshIdentityConfigured: false, sshKnownHostsConfigured: false, sshHost: null, sshUser: null }];
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
        base: false, privilege: 'standard' as const, sshConfigured: false, sshIdentityConfigured: false, sshKnownHostsConfigured: false, sshHost: null, sshUser: null }];
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
      primary: config.primary,
      requiredWorkspaceIds: [...config.requiredWorkspaceIds],
      optionalWorkspaceIds: [...config.optionalWorkspaceIds],
      lastMachineByWorkspace: { ...config.lastMachineByWorkspace },
    };
  }

  private read(): FleetConfiguration {
    if (!existsSync(this.path)) return defaultConfig();
    try {
      const parsed: unknown = JSON.parse(readFileSync(this.path, 'utf8'));
      if (!isRecord(parsed) || parsed.version !== 1) throw new Error('Unsupported Fleet configuration.');
      const config = defaultConfig();
      if (parsed.role === 'primary' || parsed.role === 'guest') config.role = parsed.role;
      config.enabled = parsed.enabled !== false;
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
      if (isRecord(parsed.machines)) {
        for (const [key, value] of Object.entries(parsed.machines)) {
          if (!isRecord(value)) continue;
          config.machines[key] = {
            base: value.base === true,
            privilege: value.privilege === 'elevated' ? 'elevated' : 'standard',
            sshHost: optionalText(value.sshHost),
            sshUser: optionalText(value.sshUser),
            sshIdentityFile: optionalText(value.sshIdentityFile),
            sshKnownHostsFile: optionalText(value.sshKnownHostsFile),
          };
        }
      }
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

function defaultMachineConfig(): FleetMachineConfig {
  return { base: false, privilege: 'standard', sshHost: null, sshUser: null, sshIdentityFile: null, sshKnownHostsFile: null };
}

function defaultConfig(): FleetConfiguration {
  return { version: 1, role: 'primary', enabled: true, primary: null, requiredWorkspaceIds: [], optionalWorkspaceIds: [], lastMachineByWorkspace: {}, machines: {} };
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
