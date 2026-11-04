import { readFileSync, statSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

interface GuestVersion {
  build: string;
  buildLabEx: string;
  architecture: string;
  channel: string;
  track: string;
  observedAt: string;
}

interface ReleaseReference {
  build: string;
  architecture: string;
  channel: string;
  track: string;
  sourceUrl: string;
  publishedAt: string;
  checkedAt: string;
}

export interface CanaryCheckInput {
  sessionStartedAt: string;
  guest: GuestVersion;
  latest: ReleaseReference | null;
}

export interface CanaryCheckResult {
  status: 'behind' | 'matches-reference' | 'unknown';
  message: string;
  warnings: string[];
  guestBuild: string | null;
  latestBuild: string | null;
  sourceUrl: string | null;
  sourceCheckedAt: string | null;
  evaluatedAt: string;
}

const DAY_MS = 24 * 60 * 60 * 1000;
const UNKNOWN = 'Canary freshness is unknown; do not label the VM current.';

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function text(value: unknown, max = 256): value is string {
  return typeof value === 'string' && value.trim().length > 0 && value.length <= max;
}

function version(value: unknown): [number, number] | null {
  if (typeof value !== 'string' || !/^\d{4,6}\.\d{1,7}$/u.test(value)) return null;
  const [build, revision] = value.split('.').map(Number);
  return [build!, revision!];
}

function time(value: unknown): number {
  return typeof value === 'string' && /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z$/u.test(value)
    ? Date.parse(value) : NaN;
}

function publicationTime(value: unknown): number {
  if (typeof value === 'string' && /^\d{4}-\d{2}-\d{2}$/u.test(value)) {
    const parsed = Date.parse(`${value}T00:00:00.000Z`);
    return Number.isFinite(parsed) && new Date(parsed).toISOString().slice(0, 10) === value ? parsed : NaN;
  }
  return time(value);
}

function officialReleaseUrl(value: unknown): value is string {
  if (!text(value, 2048)) return false;
  try {
    const url = new URL(value);
    if (url.protocol !== 'https:' || url.username || url.password || url.port) return false;
    return (url.hostname === 'learn.microsoft.com' && /^\/en-us\/windows-insider\/flight-hub\/?$/u.test(url.pathname))
      || (url.hostname === 'blogs.windows.com' && url.pathname.startsWith('/windows-insider/'));
  } catch { return false; }
}

/** Compare supplied observations only; the caller must obtain and retain live source evidence. */
export function checkCanary(input: unknown, now = Date.now()): CanaryCheckResult {
  const result: CanaryCheckResult = {
    status: 'unknown', message: UNKNOWN, warnings: [], guestBuild: null, latestBuild: null,
    sourceUrl: null, sourceCheckedAt: null, evaluatedAt: new Date(now).toISOString()
  };
  if (!record(input) || !record(input.guest)) {
    result.warnings.push('Guest observations and sessionStartedAt are required.');
    return result;
  }
  const guest = input.guest;
  const sessionStart = time(input.sessionStartedAt);
  const observedAt = time(guest.observedAt);
  const guestVersion = version(guest.build);
  if (!guestVersion || !text(guest.buildLabEx)
    || !text(guest.architecture) || !text(guest.channel) || !text(guest.track)) {
    result.warnings.push('Read the guest build, UBR, BuildLabEx, architecture, channel, and track; missing values are not defaults.');
    return result;
  }
  result.guestBuild = guest.build as string;
  if (!Number.isFinite(sessionStart) || sessionStart > now || !Number.isFinite(observedAt)
    || observedAt < sessionStart || observedAt > now || now - observedAt > DAY_MS) {
    result.warnings.push('Guest observations must be from this session, within 24 hours, and not future-dated.');
    return result;
  }
  if (!record(input.latest)) {
    result.warnings.push('Fetch the applicable Microsoft release reference now; offline or ambiguous coverage cannot establish freshness.');
    return result;
  }
  const latest = input.latest;
  const latestVersion = version(latest.build);
  const checkedAt = time(latest.checkedAt);
  const publishedAt = publicationTime(latest.publishedAt);
  if (!latestVersion || !officialReleaseUrl(latest.sourceUrl)
    || !Number.isFinite(checkedAt) || checkedAt < sessionStart || checkedAt > now || now - checkedAt > DAY_MS
    || !Number.isFinite(publishedAt) || publishedAt > checkedAt) {
    result.warnings.push('A full build/revision and dated official release reference fetched during this session within 24 hours are required.');
    return result;
  }
  result.latestBuild = latest.build as string;
  result.sourceUrl = latest.sourceUrl;
  result.sourceCheckedAt = latest.checkedAt as string;
  if (!text(latest.channel) || !text(latest.track) || !text(latest.architecture)
    || latest.channel.trim().toLowerCase() !== guest.channel.trim().toLowerCase()
    || latest.track.trim().toLowerCase() !== guest.track.trim().toLowerCase()
    || latest.architecture.trim().toLowerCase() !== guest.architecture.trim().toLowerCase()) {
    result.warnings.push('Architecture, channel, or track differs or is missing. Resolve the applicable release; do not compare unrelated build streams.');
    return result;
  }
  const channel = guest.channel.trim().toLowerCase();
  if (channel !== 'canary' && !/^experimental(?: \([a-z0-9 -]+\))?$/u.test(channel)) {
    result.warnings.push('This is not an identified Canary or Experimental stream. Confirm the requested environment.');
    return result;
  }
  if (channel !== 'canary') {
    result.warnings.push('The stream is Experimental while the bounty guidance names Canary. Confirm the current Microsoft channel mapping and eligibility; a version match does not resolve this.');
  }
  const difference = guestVersion[0] - latestVersion[0] || guestVersion[1] - latestVersion[1];
  if (difference < 0) {
    result.status = 'behind';
    result.message = `Warning: guest ${result.guestBuild} is behind reference ${result.latestBuild}. Update the selected VM before final validation.`;
    result.warnings.push(result.message);
  } else if (difference > 0) {
    result.warnings.push('The guest is ahead of the supplied reference. Check for stale release data, a different branch, or a channel transition; do not downgrade automatically.');
  } else {
    result.status = 'matches-reference';
    result.message = `Guest ${result.guestBuild} matches the supplied release reference. Source applicability and bounty eligibility are not certified by this comparison.`;
  }
  return result;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    if (process.argv.length !== 3) throw new Error('Usage: node check-canary.ts <input.json>');
    const inputPath = process.argv[2]!;
    const stats = statSync(inputPath);
    if (!stats.isFile() || stats.size > 64 * 1024) throw new Error('Input must be a JSON file no larger than 64 KiB.');
    process.stdout.write(`${JSON.stringify(checkCanary(JSON.parse(readFileSync(inputPath, 'utf8'))), null, 2)}\n`);
  } catch {
    process.stdout.write(`${JSON.stringify({ status: 'unknown', message: 'Cannot read the check input. Use: node check-canary.ts <input.json> with a JSON file no larger than 64 KiB.' })}\n`);
    process.exitCode = 2;
  }
}
