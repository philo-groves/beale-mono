import { spawn, spawnSync } from 'node:child_process';
import { existsSync, readdirSync, statSync } from 'node:fs';
import { homedir } from 'node:os';
import { basename, dirname, win32 } from 'node:path';
import { readClaudeSubscriptionRateLimits, readXaiSubscriptionAccessToken } from '@beale/research-agent';
import type { BealeProviderLimitId, BealeProviderUsageLimits, BealeProviderUsageWindow } from '@beale/app-server-runtime/protocol';
import { resolveAppServerCodexAuthFile } from './sessionLaunch.js';

const SESSION_MINUTES = 5 * 60;
const WEEK_MINUTES = 7 * 24 * 60;
const MONTH_MINUTES = 30 * 24 * 60;

type Probe = (providerId: BealeProviderLimitId) => Promise<BealeProviderUsageLimits>;

export class ProviderUsageLimitsService {
  private readonly cache = new Map<BealeProviderLimitId, { expiresAt: number; result: BealeProviderUsageLimits }>();
  private readonly pending = new Map<BealeProviderLimitId, Promise<BealeProviderUsageLimits>>();

  public constructor(private readonly probe: Probe = probeProviderUsageLimits) {}

  public async read(providerId: BealeProviderLimitId, refresh = false): Promise<BealeProviderUsageLimits> {
    const cached = this.cache.get(providerId);
    if (!refresh && cached && cached.expiresAt > Date.now()) return cached.result;
    const inFlight = this.pending.get(providerId);
    if (inFlight) return inFlight;
    const request = this.probe(providerId).then((result) => {
      this.cache.set(providerId, { expiresAt: Date.now() + (result.status === 'available' ? 5 * 60_000 : 30_000), result });
      return result;
    }).finally(() => this.pending.delete(providerId));
    this.pending.set(providerId, request);
    return request;
  }
}

function snapshot(providerId: BealeProviderLimitId, status: BealeProviderUsageLimits['status'], windows: BealeProviderUsageWindow[] = [], detail: string | null = null): BealeProviderUsageLimits {
  return { providerId, checkedAt: new Date().toISOString(), status, detail, windows };
}

function finitePercent(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? Math.max(0, Math.min(100, value)) : null;
}

function resetIso(value: unknown, epochSeconds = false): string | null {
  const millis = epochSeconds && typeof value === 'number' ? value * 1000 : typeof value === 'string' ? Date.parse(value) : NaN;
  return Number.isFinite(millis) && millis > 0 ? new Date(millis).toISOString() : null;
}

function record(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : null;
}

function codexWindow(id: string, value: unknown, fallbackMinutes: number): BealeProviderUsageWindow | null {
  const data = record(value);
  const usedPercent = finitePercent(data?.usedPercent);
  if (!data || usedPercent === null) return null;
  const durationMinutes = typeof data.windowDurationMins === 'number' && Number.isFinite(data.windowDurationMins) && data.windowDurationMins > 0
    ? data.windowDurationMins : fallbackMinutes;
  const label = durationMinutes >= MONTH_MINUTES ? 'Monthly Limit' : durationMinutes >= WEEK_MINUTES ? 'Weekly Limit' : 'Session Limit';
  return { id, label, usedPercent, resetsAt: resetIso(data.resetsAt, true), durationMinutes };
}

export function codexRateLimitsToSnapshot(value: unknown): BealeProviderUsageLimits {
  const result = record(value);
  const byId = record(result?.rateLimitsByLimitId);
  const limits = record(byId?.codex) ?? record(result?.rateLimits);
  if (!limits) return snapshot('openai-codex', 'unavailable', [], 'Codex did not report subscription limits.');
  if (limits.limitId && limits.limitId !== 'codex') return snapshot('openai-codex', 'unavailable', [], 'Codex did not report its main usage limit.');
  const monthly = limits.planType === 'free' || limits.planType === 'go';
  const windows = [
    codexWindow('primary', limits.primary, monthly ? MONTH_MINUTES : SESSION_MINUTES),
    codexWindow('secondary', limits.secondary, WEEK_MINUTES),
  ].filter((window): window is BealeProviderUsageWindow => window !== null);
  return snapshot('openai-codex', 'available', windows, windows.length ? null : 'No subscription windows were reported.');
}

export function xaiBillingToSnapshot(value: unknown): BealeProviderUsageLimits {
  const config = record(record(value)?.config);
  const period = record(config?.currentPeriod);
  const periodType = typeof period?.type === 'string' ? period.type.replace(/^USAGE_PERIOD_TYPE_/, '') : '';
  const resetsAt = resetIso(period?.end);
  const reportedPercent = finitePercent(config?.creditUsagePercent);
  // Proto3 JSON omits zero-valued scalar fields, including creditUsagePercent.
  const usedPercent = reportedPercent ?? (config && !Object.hasOwn(config, 'creditUsagePercent')
    && (periodType === 'WEEKLY' || periodType === 'MONTHLY') && resetsAt ? 0 : null);
  if (usedPercent === null) return snapshot('xai', 'unavailable', [], 'xAI did not report a current subscription usage window.');
  const label = periodType === 'WEEKLY' ? 'Weekly Limit' : periodType === 'MONTHLY' ? 'Monthly Limit' : 'Subscription Limit';
  const durationMinutes = periodType === 'WEEKLY' ? WEEK_MINUTES : periodType === 'MONTHLY' ? MONTH_MINUTES : null;
  return snapshot('xai', 'available', [{ id: 'subscription', label, usedPercent, resetsAt, durationMinutes }]);
}

export function claudeRateLimitsToSnapshot(value: Awaited<ReturnType<typeof readClaudeSubscriptionRateLimits>>): BealeProviderUsageLimits {
  if (!value.available) return snapshot('anthropic', 'unsupported', [], 'Claude did not report subscription limits for this account.');
  const windows: BealeProviderUsageWindow[] = [];
  for (const [id, label, minutes, limit] of [
    ['five_hour', 'Session Limit', SESSION_MINUTES, value.fiveHour],
    ['seven_day', 'Weekly Limit', WEEK_MINUTES, value.sevenDay],
  ] as const) {
    const usedPercent = finitePercent(limit?.utilization);
    if (usedPercent !== null) windows.push({ id, label, usedPercent, resetsAt: resetIso(limit?.resetsAt), durationMinutes: minutes });
  }
  return snapshot('anthropic', 'available', windows, windows.length ? null : 'No subscription windows were reported.');
}

async function readCodexRateLimits(): Promise<unknown> {
  const authFile = resolveAppServerCodexAuthFile();
  if (!authFile) return null;
  if (basename(authFile) !== 'auth.json') throw new Error('Codex usage needs an auth.json credential file.');
  const child = spawn(resolveCodexUsageCommand(), ['app-server', '--listen', 'stdio://'], {
    stdio: ['pipe', 'pipe', 'ignore'],
    windowsHide: true,
    env: { ...process.env, CODEX_HOME: dirname(authFile) },
  });
  return await new Promise<unknown>((resolve, reject) => {
    let done = false;
    let buffer = '';
    const finish = (error?: Error, result?: unknown) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      child.kill();
      if (error) reject(error);
      else resolve(result);
    };
    const timer = setTimeout(() => finish(new Error('Codex usage request timed out.')), 8_000);
    child.once('error', () => finish(new Error('Codex could not be started to read usage.')));
    child.once('exit', () => finish(new Error('Codex exited before reporting usage.')));
    child.stdin.on('error', () => finish(new Error('Codex closed the usage connection.')));
    child.stdout.on('data', (chunk: Buffer) => {
      buffer += chunk.toString('utf8');
      if (buffer.length > 1_000_000) return finish(new Error('Codex usage response was too large.'));
      for (let newline = buffer.indexOf('\n'); newline >= 0; newline = buffer.indexOf('\n')) {
        const line = buffer.slice(0, newline);
        buffer = buffer.slice(newline + 1);
        let message: Record<string, unknown> | null;
        try { message = record(JSON.parse(line)); } catch { continue; }
        if (message?.id === 1) {
          if (message.error) return finish(new Error('Codex rejected usage initialization.'));
          child.stdin.write(`${JSON.stringify({ method: 'initialized' })}\n`);
          child.stdin.write(`${JSON.stringify({ id: 2, method: 'account/rateLimits/read', params: null })}\n`);
        } else if (message?.id === 2) {
          if (message.error) return finish(new Error('Codex could not read usage limits.'));
          return finish(undefined, message.result);
        }
      }
    });
    child.stdin.write(`${JSON.stringify({ id: 1, method: 'initialize', params: { clientInfo: { name: 'beale', title: 'Beale', version: '0.9.5' }, capabilities: null } })}\n`);
  });
}

export function resolveCodexUsageCommand(
  environment: NodeJS.ProcessEnv = process.env,
  platform = process.platform,
  userHome = homedir()
): string {
  const configured = environment.BEALE_CODEX_COMMAND?.trim();
  if (configured) return configured;
  return platform === 'win32' ? findWindowsCodexDesktopCommand(environment, userHome) || 'codex' : 'codex';
}

function findWindowsCodexDesktopCommand(environment: NodeJS.ProcessEnv, userHome: string): string | null {
  const localAppData = environment.LOCALAPPDATA?.trim() || win32.join(userHome, 'AppData', 'Local');
  const binRoot = win32.join(localAppData, 'OpenAI', 'Codex', 'bin');
  try {
    const installed = readdirSync(binRoot, { withFileTypes: true })
      .filter((entry) => entry.isDirectory())
      .map((entry) => win32.join(binRoot, entry.name, 'codex.exe'))
      .filter((candidate) => existsSync(candidate))
      .map((candidate) => ({ candidate, modifiedAt: statSync(candidate).mtimeMs }))
      .sort((left, right) => right.modifiedAt - left.modifiedAt)[0]?.candidate;
    if (installed) return installed;
  } catch { /* Codex Desktop may not have a per-user bin directory. */ }

  const systemRoot = environment.SystemRoot?.trim() || 'C:\\Windows';
  const powershell = win32.join(systemRoot, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
  const script = '$package = Get-AppxPackage -Name OpenAI.Codex -ErrorAction SilentlyContinue | Sort-Object Version -Descending | Select-Object -First 1; if ($null -ne $package) { Write-Output $package.InstallLocation }';
  const result = spawnSync(powershell, ['-NoProfile', '-NonInteractive', '-Command', script], {
    encoding: 'utf8', timeout: 3_000, windowsHide: true, env: environment
  });
  if (result.error || result.status !== 0) return null;
  const installLocation = result.stdout.trim().split(/\r?\n/u).filter(Boolean).at(-1);
  if (!installLocation) return null;
  const command = win32.join(installLocation, 'app', 'resources', 'codex.exe');
  return existsSync(command) ? command : null;
}

async function probeProviderUsageLimits(providerId: BealeProviderLimitId): Promise<BealeProviderUsageLimits> {
  if (providerId === 'zai' || providerId === 'openrouter') {
    return snapshot(providerId, 'unsupported', [], 'This provider does not report subscription limit windows to Beale.');
  }
  try {
    if (providerId === 'openai-codex') {
      const authFile = resolveAppServerCodexAuthFile();
      if (!authFile || !existsSync(authFile)) return snapshot(providerId, 'not_configured', [], 'Sign in with a Codex subscription to see limits.');
      const result = await readCodexRateLimits();
      return codexRateLimitsToSnapshot(result);
    }
    if (providerId === 'xai') {
      const token = await readXaiSubscriptionAccessToken();
      if (!token) return snapshot(providerId, 'not_configured', [], 'Sign in with an xAI subscription to see limits.');
      const response = await fetch('https://cli-chat-proxy.grok.com/v1/billing?format=credits', {
        headers: { authorization: `Bearer ${token}`, 'x-xai-token-auth': 'xai-grok-cli', accept: 'application/json' },
        signal: AbortSignal.timeout(8_000),
      });
      if (!response.ok) throw new Error('xAI did not return subscription usage.');
      return xaiBillingToSnapshot(await response.json() as unknown);
    }
    return claudeRateLimitsToSnapshot(await readClaudeSubscriptionRateLimits());
  } catch {
    return snapshot(providerId, 'unavailable', [], 'Could not refresh subscription limits. Try again later.');
  }
}
