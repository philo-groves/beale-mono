import { spawn } from 'node:child_process';
import { timingSafeEqual } from 'node:crypto';
import { createServer, type Server } from 'node:http';
import { BEALE_APP_SERVER_SUPERVISOR_RESTART_PATH } from '@beale/app-server-runtime/protocol';
import { discoveryLockPath, isProcessAlive, readDiscoveryLockPid, readDiscoveryRecord } from './discovery.js';

export interface RestartSupervisorConfig {
  discoveryFile: string;
  command: string;
  args: string[];
  hostMode: 'tray' | 'headless';
  operatorToken: string;
  port: number;
}

const STOP_TIMEOUT_MS = 5_000;
const START_TIMEOUT_MS = 45_000;

function authorized(header: string | undefined, token: string): boolean {
  if (!header?.startsWith('Bearer ')) return false;
  const supplied = Buffer.from(header.slice(7));
  const expected = Buffer.from(token);
  return supplied.length === expected.length && timingSafeEqual(supplied, expected);
}

function currentToken(config: RestartSupervisorConfig): string {
  return readDiscoveryRecord(config.discoveryFile)?.operatorToken || config.operatorToken;
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export async function restartSupervisedAppServer(config: RestartSupervisorConfig): Promise<void> {
  const prior = readDiscoveryRecord(config.discoveryFile);
  const lockOwner = readDiscoveryLockPid(discoveryLockPath(config.discoveryFile));
  if (!prior && lockOwner && isProcessAlive(lockOwner)) {
    throw new Error('Another process owns the app-server discovery lock. Refusing to replace it.');
  }
  if (prior && isProcessAlive(prior.pid)) {
    if (prior.pid === process.pid || lockOwner !== prior.pid) {
      throw new Error('The app-server process does not own its discovery lock. Refusing to terminate it.');
    }
    process.kill(prior.pid, 'SIGTERM');
    const gracefulDeadline = Date.now() + STOP_TIMEOUT_MS;
    while (isProcessAlive(prior.pid) && Date.now() < gracefulDeadline) await delay(100);
    if (isProcessAlive(prior.pid)) {
      const current = readDiscoveryRecord(config.discoveryFile);
      if (!current || current.pid !== prior.pid || readDiscoveryLockPid(discoveryLockPath(config.discoveryFile)) !== prior.pid) {
        throw new Error('The app-server process changed during restart. Refusing to force-stop it.');
      }
      process.kill(prior.pid, 'SIGKILL');
      const forcedDeadline = Date.now() + STOP_TIMEOUT_MS;
      while (isProcessAlive(prior.pid) && Date.now() < forcedDeadline) await delay(100);
      if (isProcessAlive(prior.pid)) throw new Error('The app-server process did not stop.');
    }
  }

  const environment = { ...process.env };
  delete environment.BEALE_APP_SERVER_SUPERVISOR_CONFIG;
  if (config.hostMode === 'tray') delete environment.ELECTRON_RUN_AS_NODE;
  const child = spawn(config.command, config.args, {
    detached: true, stdio: 'ignore', windowsHide: true, env: environment
  });
  await new Promise<void>((resolve, reject) => {
    child.once('spawn', () => resolve());
    child.once('error', reject);
  });
  child.unref();
  const deadline = Date.now() + START_TIMEOUT_MS;
  while (Date.now() < deadline) {
    const record = readDiscoveryRecord(config.discoveryFile);
    if (record && record.pid !== prior?.pid && isProcessAlive(record.pid)) {
      try {
        const response = await fetch(`${record.localUrl?.trim() || `http://127.0.0.1:${record.port}`}/health`, {
          signal: AbortSignal.timeout(1_000)
        });
        if (response.ok) return;
      } catch { /* Wait for the replacement host to finish startup. */ }
    }
    await delay(250);
  }
  throw new Error('The replacement app-server did not become ready.');
}

export async function startRestartSupervisor(config: RestartSupervisorConfig): Promise<Server> {
  let restarting = false;
  const server = createServer((request, response) => {
    if (request.method === 'GET' && request.url === '/health') {
      response.writeHead(200, { 'content-type': 'application/json' }).end('{"ok":true}');
      return;
    }
    if (request.method !== 'POST' || request.url !== BEALE_APP_SERVER_SUPERVISOR_RESTART_PATH) {
      response.writeHead(404).end();
      return;
    }
    if (!authorized(request.headers.authorization, currentToken(config))) {
      response.writeHead(401).end();
      return;
    }
    if (restarting) {
      response.writeHead(409, { 'content-type': 'application/json' }).end('{"error":"Restart already in progress."}');
      return;
    }
    restarting = true;
    void restartSupervisedAppServer(config).then(
      () => response.writeHead(200, { 'content-type': 'application/json' }).end('{"restarted":true}'),
      (error: unknown) => response.writeHead(500, { 'content-type': 'application/json' }).end(JSON.stringify({ error: error instanceof Error ? error.message : String(error) }))
    ).finally(() => { restarting = false; });
  });
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(config.port, '127.0.0.1', resolve);
  });
  return server;
}
