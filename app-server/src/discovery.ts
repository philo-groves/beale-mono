import { randomBytes } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, renameSync, unlinkSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { basename, dirname, extname, join } from 'node:path';

export const DISCOVERY_RECORD_VERSION = 1 as const;
export type AppServerHostMode = 'tray' | 'headless';

export interface AppServerDiscoveryRecord {
  version: typeof DISCOVERY_RECORD_VERSION;
  contractTimestamp: string | null;
  hostMode?: AppServerHostMode;
  pid: number;
  host: string;
  port: number;
  localUrl?: string;
  url: string;
  operatorToken: string;
  startedAt: string;
}

export function defaultDiscoveryPath(): string {
  return join(homedir(), '.beale', 'app-server.json');
}

export function discoveryLockPath(discoveryPath: string): string {
  return `${discoveryPath}.lock`;
}

export function acquireDiscoveryLock(discoveryPath: string, pid: number): boolean {
  const path = discoveryLockPath(discoveryPath);
  mkdirSync(dirname(path), { recursive: true });
  for (let attempt = 0; attempt < 3; attempt += 1) {
    try {
      writeFileSync(path, `${JSON.stringify({ pid })}\n`, { encoding: 'utf8', flag: 'wx', mode: 0o600 });
      return true;
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error;
      const ownerPid = readDiscoveryLockPid(path);
      if (ownerPid === pid) return true;
      if (ownerPid && isProcessAlive(ownerPid)) return false;
      try {
        unlinkSync(path);
      } catch (unlinkError) {
        if ((unlinkError as NodeJS.ErrnoException).code !== 'ENOENT') return false;
      }
    }
  }
  return false;
}

export function releaseDiscoveryLock(discoveryPath: string, pid: number): boolean {
  const path = discoveryLockPath(discoveryPath);
  if (readDiscoveryLockPid(path) !== pid) return false;
  try {
    unlinkSync(path);
    return true;
  } catch {
    return false;
  }
}

export function generateOperatorToken(): string {
  return randomBytes(32).toString('hex');
}

export function operatorTokenPath(discoveryPath: string = defaultDiscoveryPath()): string {
  const extension = extname(discoveryPath);
  const stem = basename(discoveryPath, extension);
  return join(dirname(discoveryPath), `${stem}.token`);
}

export function readOrCreateOperatorToken(path: string): string {
  mkdirSync(dirname(path), { recursive: true });
  const existing = readOperatorToken(path);
  if (existing) return existing;
  if (existsSync(path)) {
    throw new Error(`Beale app-server operator token file is invalid: ${path}`);
  }
  const generated = generateOperatorToken();
  try {
    writeFileSync(path, `${generated}\n`, { encoding: 'utf8', flag: 'wx', mode: 0o600 });
    return generated;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error;
    const raced = readOperatorToken(path);
    if (raced) return raced;
    throw new Error(`Beale app-server operator token file is invalid: ${path}`);
  }
}

export function writeDiscoveryRecord(record: AppServerDiscoveryRecord, path: string): void {
  mkdirSync(dirname(path), { recursive: true });
  const temporary = `${path}.${process.pid}.tmp`;
  writeFileSync(temporary, `${JSON.stringify(record, null, 2)}\n`, {
    encoding: 'utf8',
    mode: 0o600
  });
  renameSync(temporary, path);
}

export function readDiscoveryRecord(path: string): AppServerDiscoveryRecord | null {
  if (!existsSync(path)) return null;
  let raw: unknown;
  try {
    raw = JSON.parse(readFileSync(path, 'utf8'));
  } catch {
    return null;
  }
  if (!isDiscoveryRecord(raw)) return null;
  return {
    ...raw,
    contractTimestamp: typeof raw.contractTimestamp === 'string' ? raw.contractTimestamp : null
  };
}

export function clearDiscoveryRecord(path: string, pid: number): boolean {
  const record = readDiscoveryRecord(path);
  if (!record || record.pid !== pid) return false;
  try {
    unlinkSync(path);
    return true;
  } catch {
    return false;
  }
}

export function isProcessAlive(pid: number): boolean {
  if (!Number.isInteger(pid) || pid <= 0) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === 'EPERM';
  }
}

function isDiscoveryRecord(value: unknown): value is AppServerDiscoveryRecord {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
  const record = value as Record<string, unknown>;
  return record.version === DISCOVERY_RECORD_VERSION
    && (record.hostMode === undefined || record.hostMode === 'tray' || record.hostMode === 'headless')
    && Number.isInteger(record.pid)
    && typeof record.host === 'string'
    && Number.isInteger(record.port)
    && (record.localUrl === undefined || typeof record.localUrl === 'string')
    && typeof record.url === 'string'
    && typeof record.operatorToken === 'string'
    && typeof record.startedAt === 'string';
}

function readOperatorToken(path: string): string | null {
  if (!existsSync(path)) return null;
  try {
    const value = readFileSync(path, 'utf8').trim();
    return /^[A-Za-z0-9_-]{32,256}$/u.test(value) ? value : null;
  } catch {
    return null;
  }
}

function readDiscoveryLockPid(path: string): number | null {
  if (!existsSync(path)) return null;
  try {
    const value = JSON.parse(readFileSync(path, 'utf8')) as { pid?: unknown };
    return Number.isInteger(value.pid) && Number(value.pid) > 0 ? Number(value.pid) : null;
  } catch {
    return null;
  }
}
