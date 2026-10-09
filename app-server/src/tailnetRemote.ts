import { execFile } from 'node:child_process';
import { lookup } from 'node:dns/promises';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { promisify } from 'node:util';

const execFileAsync = promisify(execFile);
const MAGIC_DNS = /^[a-z0-9-]+(?:\.[a-z0-9-]+)+\.ts\.net$/u;

export interface TailnetRunner {
  run(command: string, args: readonly string[], timeoutMs: number): Promise<string>;
}

export function normalizeTailnetOrigin(value: unknown): string {
  if (typeof value !== 'string') throw new Error('Enter a Tailscale HTTPS app-server URL.');
  let url: URL;
  try { url = new URL(value); }
  catch { throw new Error('Enter a Tailscale HTTPS app-server URL.'); }
  if (url.protocol !== 'https:' || !MAGIC_DNS.test(url.hostname)
    || url.username || url.password || url.pathname !== '/' || url.search || url.hash) {
    throw new Error('Enter an HTTPS Tailscale MagicDNS app-server origin.');
  }
  return url.origin;
}

export async function verifySameTailnet(
  origin: string,
  runner: TailnetRunner,
  resolveAddresses: (host: string) => Promise<string[]> = async (host) => (await lookup(host, { all: true })).map((item) => item.address),
): Promise<void> {
  const host = new URL(normalizeTailnetOrigin(origin)).hostname;
  const raw = await runTailscaleCommand(['status', '--json'], runner);
  let parsed: unknown;
  try { parsed = JSON.parse(raw); }
  catch { throw new Error('Tailscale returned an invalid status response.'); }
  if (!record(parsed) || parsed.BackendState !== 'Running' || !record(parsed.Self)) {
    throw new Error('Tailscale must be connected before using a remote app server.');
  }
  const selfName = dnsName(parsed.Self.DNSName);
  if (!selfName || !MAGIC_DNS.test(selfName)) throw new Error('Tailscale did not report this machine’s MagicDNS name.');
  const tailnet = selfName.slice(selfName.indexOf('.') + 1);
  if (host.slice(host.indexOf('.') + 1) !== tailnet) throw new Error('The remote app server is outside this Tailscale network.');
  const peers = record(parsed.Peer) ? Object.values(parsed.Peer) : [];
  const peer = peers.find((item) => record(item) && dnsName(item.DNSName) === host);
  if (!record(peer) || peer.Online !== true || !Array.isArray(peer.TailscaleIPs)) {
    throw new Error('The remote app server is not an online peer on this Tailscale network.');
  }
  const peerAddresses = new Set(peer.TailscaleIPs.filter((item): item is string => typeof item === 'string'));
  const resolved = await resolveAddresses(host);
  if (resolved.length === 0 || resolved.some((address) => !peerAddresses.has(address))) {
    throw new Error('The remote app-server hostname did not resolve to its Tailscale peer address.');
  }
}

export async function runTailscaleCommand(args: readonly string[], runner: TailnetRunner = systemTailnetRunner): Promise<string> {
  let lastError: unknown = null;
  for (const command of tailscaleCandidates()) {
    try { return await runner.run(command, args, 15_000); }
    catch (error) {
      lastError = error;
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') break;
    }
  }
  throw new Error(lastError instanceof Error ? `Tailscale status is unavailable: ${lastError.message}` : 'Tailscale was not found.');
}

function tailscaleCandidates(): string[] {
  const configured = process.env.BEALE_TAILSCALE_COMMAND?.trim();
  const mac = process.platform === 'darwin'
    ? ['/Applications/Tailscale.app/Contents/MacOS/Tailscale', '/opt/homebrew/bin/tailscale', '/usr/local/bin/tailscale'] : [];
  const windows = process.platform === 'win32' && process.env.ProgramFiles
    ? [join(process.env.ProgramFiles, 'Tailscale', 'tailscale.exe')] : [];
  return [...new Set([configured, ...mac, ...windows, process.platform === 'win32' ? 'tailscale.exe' : 'tailscale']
    .filter((candidate): candidate is string => Boolean(candidate)))]
    .filter((candidate) => !candidate.includes('/') && !candidate.includes('\\') || existsSync(candidate));
}

function dnsName(value: unknown): string | null {
  return typeof value === 'string' ? value.trim().toLowerCase().replace(/\.$/u, '') : null;
}

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export const systemTailnetRunner: TailnetRunner = {
  async run(command, args, timeoutMs) {
    const { stdout } = await execFileAsync(command, [...args], { timeout: timeoutMs, maxBuffer: 2_000_000, encoding: 'utf8', windowsHide: true });
    return stdout;
  },
};
