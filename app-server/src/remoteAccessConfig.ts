import { readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';

const REMOTE_ACCESS_CONFIG_VERSION = 1;
const MAGIC_DNS_PATTERN = /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]*[a-z0-9])?)+\.ts\.net$/u;

export interface PersistedRemoteAccessLaunchOptions {
  host: '127.0.0.1';
  port: number;
  publicUrl: string;
}

/**
 * Restores Desktop's persisted Tailscale Serve configuration when the tray
 * host starts independently. Explicit launch environment variables still win
 * in trayMain, while disabled or malformed records fail closed to loopback.
 */
export function readPersistedRemoteAccessLaunchOptions(
  configPath = process.env.BEALE_APP_SERVER_REMOTE_ACCESS_FILE?.trim()
    || join(homedir(), '.beale', 'app-server-remote-access.json')
): PersistedRemoteAccessLaunchOptions | null {
  try {
    const value: unknown = JSON.parse(readFileSync(configPath, 'utf8'));
    if (!isRecord(value) || value.version !== REMOTE_ACCESS_CONFIG_VERSION || value.enabled !== true) return null;
    const magicDnsName = typeof value.magicDnsName === 'string'
      ? value.magicDnsName.trim().toLowerCase().replace(/\.$/u, '')
      : '';
    if (!MAGIC_DNS_PATTERN.test(magicDnsName)) return null;
    const localPort = validPort(value.localPort);
    const httpsPort = validPort(value.httpsPort);
    if (localPort === null || httpsPort === null) return null;
    return {
      host: '127.0.0.1',
      port: localPort,
      publicUrl: `https://${magicDnsName}:${httpsPort}`
    };
  } catch {
    return null;
  }
}

function validPort(value: unknown): number | null {
  return Number.isInteger(value) && Number(value) > 0 && Number(value) <= 65_535
    ? Number(value)
    : null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}
