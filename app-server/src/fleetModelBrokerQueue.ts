import { createHash, randomBytes, randomUUID } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, readdirSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { ModelBrokerRequest } from '@beale/research-agent';

const ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/u;
const LEASE_MS = 30_000;
const MAX_REQUEST_BYTES = 16 * 1024 * 1024;
const MAX_RESULT_BYTES = 16 * 1024 * 1024;
const MAX_REQUESTS_PER_SESSION = 5_000;

interface BrokerSession { token: string }
interface BrokerState {
  leaseId: string | null;
  leaseUntil: number;
  state: 'pending' | 'done';
}

/** Guest-owned durable spool. Only the worker token can submit or read results. */
export class FleetModelBrokerQueue {
  constructor(private readonly root: string) { mkdirSync(root, { recursive: true, mode: 0o700 }); }

  register(sessionId: string): string {
    const directory = this.sessionDirectory(sessionId);
    mkdirSync(directory, { recursive: true, mode: 0o700 });
    const path = join(directory, 'session.json');
    if (existsSync(path)) return (JSON.parse(readFileSync(path, 'utf8')) as BrokerSession).token;
    const token = randomBytes(32).toString('base64url');
    writePrivate(path, { token });
    return token;
  }

  token(sessionId: string): string | null {
    const path = join(this.sessionDirectory(sessionId), 'session.json');
    return existsSync(path) ? (JSON.parse(readFileSync(path, 'utf8')) as BrokerSession).token : null;
  }

  submit(sessionId: string, token: string, request: ModelBrokerRequest): void {
    this.authorize(sessionId, token);
    if (!ID.test(request.id) || !ID.test(request.model.provider) || !ID.test(request.model.id)
      || !request.context || typeof request.context !== 'object') throw new Error('Invalid model broker request.');
    const serialized = JSON.stringify(request);
    if (Buffer.byteLength(serialized) > MAX_REQUEST_BYTES) throw new Error('Model broker request exceeds its size limit.');
    const path = this.requestPath(sessionId, request.id);
    if (existsSync(path)) {
      if (readFileSync(path, 'utf8') !== serialized) throw new Error('Model broker request ID was reused with different content.');
      return;
    }
    if (readdirSync(this.sessionDirectory(sessionId)).filter((name) => name.endsWith('.request.json')).length >= MAX_REQUESTS_PER_SESSION) {
      throw new Error('Model broker session request limit reached.');
    }
    writeFileSync(path, serialized, { flag: 'wx', mode: 0o600 });
    writePrivate(this.statePath(sessionId, request.id), { leaseId: null, leaseUntil: 0, state: 'pending' } satisfies BrokerState);
  }

  result(sessionId: string, token: string, requestId: string): unknown {
    this.authorize(sessionId, token);
    const state = this.readState(sessionId, requestId);
    if (state.state !== 'done') return { state: 'pending' };
    const resultPath = this.resultPath(sessionId, requestId);
    return JSON.parse(readFileSync(resultPath, 'utf8')) as unknown;
  }

  poll(sessionId: string): { request: ModelBrokerRequest; leaseId: string } | null {
    const directory = this.sessionDirectory(sessionId);
    if (!existsSync(join(directory, 'session.json'))) throw new Error('Model broker session is unavailable.');
    for (const name of readdirSync(directory).filter((entry) => entry.endsWith('.request.json')).sort()) {
      const requestId = name.slice(0, -'.request.json'.length);
      const state = this.readState(sessionId, requestId);
      if (state.state === 'done' || state.leaseUntil > Date.now()) continue;
      const leaseId = randomUUID();
      writePrivate(this.statePath(sessionId, requestId), { state: 'pending', leaseId, leaseUntil: Date.now() + LEASE_MS } satisfies BrokerState);
      return { request: JSON.parse(readFileSync(this.requestPath(sessionId, requestId), 'utf8')) as ModelBrokerRequest, leaseId };
    }
    return null;
  }

  renew(sessionId: string, requestId: string, leaseId: string): void {
    const state = this.requireLease(sessionId, requestId, leaseId);
    writePrivate(this.statePath(sessionId, requestId), { ...state, leaseUntil: Date.now() + LEASE_MS });
  }

  beginResult(sessionId: string, requestId: string, leaseId: string): void {
    this.requireLease(sessionId, requestId, leaseId);
    writeFileSync(this.partPath(sessionId, requestId), '', { mode: 0o600 });
  }

  appendResult(sessionId: string, requestId: string, leaseId: string, offset: number, data: string): void {
    this.requireLease(sessionId, requestId, leaseId);
    if (!Number.isSafeInteger(offset) || offset < 0 || data.length > 256 * 1024) throw new Error('Invalid broker result chunk.');
    const bytes = Buffer.from(data, 'base64');
    if (bytes.toString('base64') !== data || bytes.length > 192 * 1024) throw new Error('Invalid broker result encoding.');
    const path = this.partPath(sessionId, requestId);
    if (!existsSync(path) || statSync(path).size !== offset || offset + bytes.length > MAX_RESULT_BYTES) {
      throw new Error('Broker result chunk is out of sequence.');
    }
    writeFileSync(path, bytes, { flag: 'a', mode: 0o600 });
  }

  sealResult(sessionId: string, requestId: string, leaseId: string, sha256: string): void {
    this.requireLease(sessionId, requestId, leaseId);
    const bytes = readFileSync(this.partPath(sessionId, requestId));
    if (createHash('sha256').update(bytes).digest('hex') !== sha256) throw new Error('Broker result integrity check failed.');
    const result: unknown = JSON.parse(bytes.toString('utf8'));
    if (!result || typeof result !== 'object' || !('state' in result)
      || (result.state !== 'done' && result.state !== 'error')) throw new Error('Invalid broker result.');
    const resultPath = this.resultPath(sessionId, requestId);
    const temporary = `${resultPath}.${randomUUID()}.tmp`;
    writeFileSync(temporary, bytes, { mode: 0o600 });
    renameSync(temporary, resultPath);
    writePrivate(this.statePath(sessionId, requestId), { state: 'done', leaseId: null, leaseUntil: 0 } satisfies BrokerState);
  }

  remove(sessionId: string): void { rmSync(this.sessionDirectory(sessionId), { recursive: true, force: true }); }

  private authorize(sessionId: string, token: string): void {
    const expected = this.token(sessionId);
    if (!expected || token !== expected) throw new Error('Invalid model broker session token.');
  }

  private requireLease(sessionId: string, requestId: string, leaseId: string): BrokerState {
    const state = this.readState(sessionId, requestId);
    if (state.state !== 'pending' || !state.leaseId || state.leaseId !== leaseId || state.leaseUntil < Date.now()) {
      throw new Error('Model broker lease has expired.');
    }
    return state;
  }

  private readState(sessionId: string, requestId: string): BrokerState {
    const path = this.statePath(sessionId, requestId);
    if (!existsSync(path)) throw new Error('Unknown model broker request.');
    return JSON.parse(readFileSync(path, 'utf8')) as BrokerState;
  }

  private sessionDirectory(id: string): string { if (!ID.test(id)) throw new Error('Invalid broker session ID.'); return join(this.root, id); }
  private requestPath(s: string, r: string): string { return join(this.sessionDirectory(s), `${safeId(r)}.request.json`); }
  private statePath(s: string, r: string): string { return join(this.sessionDirectory(s), `${safeId(r)}.state.json`); }
  private partPath(s: string, r: string): string { return join(this.sessionDirectory(s), `${safeId(r)}.part`); }
  private resultPath(s: string, r: string): string { return join(this.sessionDirectory(s), `${safeId(r)}.result.json`); }
}

function safeId(id: string): string { if (!ID.test(id)) throw new Error('Invalid broker request ID.'); return id; }
function writePrivate(path: string, value: unknown): void {
  const temporary = `${path}.${randomUUID()}.tmp`;
  writeFileSync(temporary, JSON.stringify(value), { mode: 0o600 });
  renameSync(temporary, path);
}
