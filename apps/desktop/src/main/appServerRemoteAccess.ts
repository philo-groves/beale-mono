import { execFile } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join } from 'node:path';
import { promisify } from 'node:util';
import type {
  AppServerRemoteAccessSettings,
  AppServerRemoteAccessUpdate
} from '@shared/types';

const execFileAsync = promisify(execFile);
const CONFIG_VERSION = 1 as const;
export const APP_SERVER_LOCAL_PORT = 47_173;
export const APP_SERVER_TAILSCALE_HTTPS_PORT = 47_174;
const MAGIC_DNS_PATTERN = /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]*[a-z0-9])?)+\.ts\.net$/u;

interface PersistedRemoteAccessConfig {
  version: typeof CONFIG_VERSION;
  enabled: boolean;
  magicDnsName: string;
  localPort: number;
  httpsPort: number;
}

interface TailscaleStatus {
  BackendState?: unknown;
  Self?: { DNSName?: unknown };
}

export type TailscaleCommandRunner = (args: string[]) => Promise<string>;

export function appServerRemoteAccessConfigPath(): string {
  return process.env.BEALE_APP_SERVER_REMOTE_ACCESS_FILE?.trim()
    || join(homedir(), '.beale', 'app-server-remote-access.json');
}

export function readAppServerRemoteAccessSettings(): AppServerRemoteAccessSettings {
  const config = readConfig();
  return settingsFromConfig(config, config.enabled ? 'configured' : 'disabled', null);
}

export async function detectAppServerMagicDnsName(
  runner: TailscaleCommandRunner = runTailscale
): Promise<AppServerRemoteAccessSettings> {
  const config = readConfig();
  try {
    const detected = await detectMagicDnsName(runner);
    return settingsFromConfig(
      { ...config, magicDnsName: detected },
      config.enabled ? 'configured' : 'available',
      null
    );
  } catch (error) {
    return settingsFromConfig(config, 'unavailable', errorMessage(error));
  }
}

export async function updateAppServerRemoteAccess(
  update: AppServerRemoteAccessUpdate,
  runner: TailscaleCommandRunner = runTailscale
): Promise<AppServerRemoteAccessSettings> {
  if (typeof update.enabled !== 'boolean') throw new Error('Remote access enabled must be a boolean.');
  const current = readConfig();
  let magicDnsName = normalizeMagicDnsName(update.magicDnsName ?? current.magicDnsName);

  if (update.enabled && !magicDnsName) {
    magicDnsName = await detectMagicDnsName(runner);
  }
  if (update.enabled && !MAGIC_DNS_PATTERN.test(magicDnsName)) {
    throw new Error('Enter the full Tailscale MagicDNS name ending in .ts.net.');
  }

  const next: PersistedRemoteAccessConfig = {
    version: CONFIG_VERSION,
    enabled: update.enabled,
    magicDnsName,
    localPort: APP_SERVER_LOCAL_PORT,
    httpsPort: APP_SERVER_TAILSCALE_HTTPS_PORT
  };

  if (next.enabled) {
    await runner([
      'serve',
      '--bg',
      '--yes',
      `--https=${next.httpsPort}`,
      `http://127.0.0.1:${next.localPort}`
    ]);
  } else if (current.enabled) {
    await runner(['serve', `--https=${current.httpsPort}`, 'off']);
  }

  writeConfig(next);
  return settingsFromConfig(next, next.enabled ? 'configured' : 'disabled', null);
}

export function appServerRemoteAccessLaunchEnvironment(): NodeJS.ProcessEnv {
  const config = readConfig();
  if (!config.enabled || !MAGIC_DNS_PATTERN.test(config.magicDnsName)) return {};
  return {
    BEALE_APP_SERVER_HOST: '127.0.0.1',
    BEALE_APP_SERVER_PORT: String(config.localPort),
    BEALE_APP_SERVER_PUBLIC_URL: publicUrl(config)
  };
}

function readConfig(): PersistedRemoteAccessConfig {
  const fallback: PersistedRemoteAccessConfig = {
    version: CONFIG_VERSION,
    enabled: false,
    magicDnsName: '',
    localPort: APP_SERVER_LOCAL_PORT,
    httpsPort: APP_SERVER_TAILSCALE_HTTPS_PORT
  };
  try {
    const parsed: unknown = JSON.parse(readFileSync(appServerRemoteAccessConfigPath(), 'utf8'));
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return fallback;
    const record = parsed as Record<string, unknown>;
    return {
      ...fallback,
      enabled: record.enabled === true,
      magicDnsName: normalizeMagicDnsName(typeof record.magicDnsName === 'string' ? record.magicDnsName : '')
    };
  } catch {
    return fallback;
  }
}

function writeConfig(config: PersistedRemoteAccessConfig): void {
  const path = appServerRemoteAccessConfigPath();
  mkdirSync(dirname(path), { recursive: true });
  const temporary = `${path}.${process.pid}.tmp`;
  writeFileSync(temporary, `${JSON.stringify(config, null, 2)}\n`, { encoding: 'utf8', mode: 0o600 });
  renameSync(temporary, path);
}

function settingsFromConfig(
  config: PersistedRemoteAccessConfig,
  status: AppServerRemoteAccessSettings['status'],
  detail: string | null
): AppServerRemoteAccessSettings {
  return {
    enabled: config.enabled,
    magicDnsName: config.magicDnsName,
    localPort: config.localPort,
    httpsPort: config.httpsPort,
    publicUrl: config.magicDnsName ? publicUrl(config) : null,
    status,
    detail
  };
}

function publicUrl(config: PersistedRemoteAccessConfig): string {
  return `https://${config.magicDnsName}:${config.httpsPort}`;
}

async function detectMagicDnsName(runner: TailscaleCommandRunner): Promise<string> {
  const output = await runner(['status', '--json']);
  let status: TailscaleStatus;
  try {
    status = JSON.parse(output) as TailscaleStatus;
  } catch {
    throw new Error('Tailscale returned an invalid status response.');
  }
  if (status.BackendState !== 'Running') {
    throw new Error('Tailscale is not connected on this machine.');
  }
  const name = normalizeMagicDnsName(typeof status.Self?.DNSName === 'string' ? status.Self.DNSName : '');
  if (!MAGIC_DNS_PATTERN.test(name)) {
    throw new Error('Tailscale did not report a MagicDNS name for this machine.');
  }
  return name;
}

async function runTailscale(args: string[]): Promise<string> {
  const errors: string[] = [];
  for (const command of tailscaleCommandCandidates()) {
    try {
      const result = await execFileAsync(command, args, {
        encoding: 'utf8',
        timeout: 15_000,
        windowsHide: true,
        maxBuffer: 1024 * 1024
      });
      return result.stdout;
    } catch (error) {
      const code = (error as NodeJS.ErrnoException).code;
      if (code === 'ENOENT') continue;
      errors.push(errorMessage(error));
    }
  }
  throw new Error(errors.at(-1) ?? 'The Tailscale command-line client was not found.');
}

function tailscaleCommandCandidates(): string[] {
  const configured = process.env.BEALE_TAILSCALE_COMMAND?.trim();
  const candidates = [
    configured,
    ...(process.platform === 'darwin'
      ? [
          '/Applications/Tailscale.app/Contents/MacOS/Tailscale',
          '/opt/homebrew/bin/tailscale',
          '/usr/local/bin/tailscale'
        ]
      : []),
    ...(process.platform === 'win32' && process.env.ProgramFiles
      ? [join(process.env.ProgramFiles, 'Tailscale', 'tailscale.exe')]
      : []),
    process.platform === 'win32' ? 'tailscale.exe' : 'tailscale'
  ].filter((candidate): candidate is string => Boolean(candidate));
  return [...new Set(candidates)].filter((candidate) => !candidate.includes('/') || existsSync(candidate));
}

function normalizeMagicDnsName(value: string): string {
  return value.trim().toLowerCase().replace(/\.$/u, '');
}

function errorMessage(error: unknown): string {
  if (!(error instanceof Error)) return String(error);
  const stderr = 'stderr' in error && typeof error.stderr === 'string' ? error.stderr.trim() : '';
  return stderr || error.message;
}
