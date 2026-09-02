import { createServer, type Server } from 'node:http';
import { spawn } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  BEALE_APP_SERVER_CAPABILITIES,
  BEALE_APP_SERVER_CONTRACT_TIMESTAMP,
  BEALE_APP_SERVER_CONTROL_VERSION
} from '@beale/app-server-runtime/protocol';
import {
  attachAppServerSession,
  BealeDesktopRestartRequiredError,
  ensureBealeAppServerRunning,
  fetchExistingAppServerCanonicalResult,
  fetchAppServerSession,
  inspectAppServerCompatibility,
  probeAppServerHealth,
  probeAppServerCompatibility,
  readBealeAppServerDiscovery,
  readLiveBealeAppServerDiscovery,
  requireBuiltAppServerTrayEntry,
  setBealeDesktopRestartRequiredHandler,
  shouldLaunchAppServerTrayController,
  shouldReplaceAppServerWithTray,
  startAppServerSession,
  stopAppServerSession,
  writePrivateAppServerLaunchEnvironment,
  type BealeAppServerDiscovery
} from '../src/main/bealeAppServerClient';

const environmentKeys = [
  'BEALE_APP_SERVER_STATE_FILE',
  'BEALE_APP_SERVER_REMOTE_ACCESS_FILE',
  'BEALE_APP_SERVER_COMMAND',
  'BEALE_APP_SERVER_ARGS_JSON'
] as const;
let savedEnvironment: Record<string, string | undefined> = {};
let temporaryDirectory = '';
let server: Server | null = null;
let serverUrl = '';

beforeEach(() => {
  savedEnvironment = Object.fromEntries(environmentKeys.map((key) => [key, process.env[key]]));
  temporaryDirectory = mkdtempSync(join(tmpdir(), 'beale-app-server-client-'));
  process.env.BEALE_APP_SERVER_STATE_FILE = join(temporaryDirectory, 'app-server.json');
  process.env.BEALE_APP_SERVER_REMOTE_ACCESS_FILE = join(temporaryDirectory, 'remote-access.json');
  delete process.env.BEALE_APP_SERVER_COMMAND;
  delete process.env.BEALE_APP_SERVER_ARGS_JSON;
});

afterEach(async () => {
  for (const [key, value] of Object.entries(savedEnvironment)) {
    if (value === undefined) delete process.env[key];
    else process.env[key] = value;
  }
  await new Promise<void>((resolve) => {
    if (!server) {
      resolve();
      return;
    }
    server.close(() => resolve());
  });
  server = null;
  rmSync(temporaryDirectory, { recursive: true, force: true });
});

describe('beale app-server client', () => {
  it('writes a private one-shot environment envelope for LaunchServices startup', () => {
    const launchEnvironment = writePrivateAppServerLaunchEnvironment({
      BEALE_APP_SERVER_STATE_FILE: '/tmp/beale-state.json',
      PROVIDER_TOKEN: 'host-only-test-value'
    });
    try {
      expect(statSync(launchEnvironment.directory).mode & 0o777).toBe(0o700);
      expect(statSync(launchEnvironment.path).mode & 0o777).toBe(0o600);
      expect(JSON.parse(readFileSync(launchEnvironment.path, 'utf8'))).toEqual({
        BEALE_APP_SERVER_STATE_FILE: '/tmp/beale-state.json',
        PROVIDER_TOKEN: 'host-only-test-value'
      });
    } finally {
      rmSync(launchEnvironment.directory, { recursive: true, force: true });
    }
  });

  it('reads valid discovery records and rejects malformed files', () => {
    const stateFile = process.env.BEALE_APP_SERVER_STATE_FILE!;
    expect(readBealeAppServerDiscovery(stateFile)).toBeNull();

    writeFileSync(stateFile, '{not json', 'utf8');
    expect(readBealeAppServerDiscovery(stateFile)).toBeNull();

    writeFileSync(stateFile, JSON.stringify(discoveryRecord({ port: 1, url: 'http://127.0.0.1:1' })), 'utf8');
    const record = readBealeAppServerDiscovery(stateFile);
    expect(record?.pid).toBe(process.pid);
    expect(record?.operatorToken).toBe('operator-token');
  });

  it('upgrades every implicit non-tray host to a tray process on desktop platforms', () => {
    const legacy = discoveryRecord({ hostMode: undefined });
    expect(shouldReplaceAppServerWithTray(legacy, 'darwin', undefined)).toBe(true);
    expect(shouldReplaceAppServerWithTray(legacy, 'win32', undefined)).toBe(true);
    expect(shouldReplaceAppServerWithTray(legacy, 'linux', undefined)).toBe(false);
    expect(shouldReplaceAppServerWithTray(legacy, 'darwin', '/custom/server')).toBe(false);
    expect(shouldReplaceAppServerWithTray(discoveryRecord({ hostMode: 'tray' }), 'darwin', undefined)).toBe(false);
    expect(shouldReplaceAppServerWithTray(discoveryRecord({ hostMode: 'headless' }), 'darwin', undefined)).toBe(true);
    expect(shouldReplaceAppServerWithTray(discoveryRecord({ hostMode: 'headless' }), 'win32', undefined)).toBe(true);
    expect(shouldReplaceAppServerWithTray(discoveryRecord({ hostMode: 'headless' }), 'darwin', '/custom/server')).toBe(false);
  });

  it('launches a tray controller when an active session prevents replacing a headless macOS host', () => {
    const headless = discoveryRecord({ hostMode: 'headless' });
    expect(shouldLaunchAppServerTrayController(headless, 1, 'darwin', undefined)).toBe(true);
    expect(shouldLaunchAppServerTrayController(headless, 0, 'darwin', undefined)).toBe(false);
    expect(shouldLaunchAppServerTrayController(headless, 1, 'linux', undefined)).toBe(false);
    expect(shouldLaunchAppServerTrayController(headless, 1, 'darwin', '/custom/server')).toBe(false);
    expect(shouldLaunchAppServerTrayController(discoveryRecord({ hostMode: 'tray' }), 1, 'darwin', undefined)).toBe(false);
  });

  it('rejects a missing built tray entry before Electron is launched', () => {
    const appServerRoot = join(temporaryDirectory, 'app-server');
    const expectedEntry = join(appServerRoot, 'dist', 'trayBootstrap.js');

    expect(() => requireBuiltAppServerTrayEntry(appServerRoot, () => false))
      .toThrow(`The Beale app-server tray entry was not found at ${expectedEntry}. Build the workspace packages first.`);
    expect(requireBuiltAppServerTrayEntry(appServerRoot, (path) => path === expectedEntry)).toBe(expectedEntry);
  });

  it('reuses a healthy recorded instance without launching anything', async () => {
    await startStubServer((request, response) => {
      expect(request.url).toBe('/health');
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify({
        ok: true,
        controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
        contractTimestamp: BEALE_APP_SERVER_CONTRACT_TIMESTAMP,
        capabilities: BEALE_APP_SERVER_CAPABILITIES
      }));
    });
    const stateFile = process.env.BEALE_APP_SERVER_STATE_FILE!;
    writeFileSync(stateFile, JSON.stringify(discoveryRecord({ url: serverUrl })), 'utf8');

    const record = await ensureBealeAppServerRunning({ readyTimeoutMs: 1_000 });
    expect(record.url).toBe(serverUrl);
    expect(record.operatorToken).toBe('operator-token');
  });

  it('coalesces concurrent ensure requests for the same discovery file', async () => {
    let healthRequests = 0;
    await startStubServer((_request, response) => {
      healthRequests += 1;
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify({
        ok: true,
        controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
        contractTimestamp: BEALE_APP_SERVER_CONTRACT_TIMESTAMP,
        capabilities: BEALE_APP_SERVER_CAPABILITIES
      }));
    });
    const stateFile = process.env.BEALE_APP_SERVER_STATE_FILE!;
    writeFileSync(stateFile, JSON.stringify(discoveryRecord({ url: serverUrl })), 'utf8');

    const [first, second] = await Promise.all([
      ensureBealeAppServerRunning({ readyTimeoutMs: 1_000 }),
      ensureBealeAppServerRunning({ readyTimeoutMs: 1_000 })
    ]);

    expect(first).toEqual(second);
    expect(healthRequests).toBe(1);
  });

  it('reuses a validated process without health-checking every operation', async () => {
    let healthRequests = 0;
    await startStubServer((_request, response) => {
      healthRequests += 1;
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify({
        ok: true,
        controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
        contractTimestamp: BEALE_APP_SERVER_CONTRACT_TIMESTAMP,
        capabilities: BEALE_APP_SERVER_CAPABILITIES
      }));
    });
    const stateFile = process.env.BEALE_APP_SERVER_STATE_FILE!;
    writeFileSync(stateFile, JSON.stringify(discoveryRecord({ url: serverUrl })), 'utf8');

    const first = await ensureBealeAppServerRunning({ readyTimeoutMs: 1_000 });
    const second = await ensureBealeAppServerRunning({ readyTimeoutMs: 1_000 });

    expect(second).toEqual(first);
    expect(readLiveBealeAppServerDiscovery(stateFile)).toEqual(first);
    expect(healthRequests).toBe(1);
  });

  it('reattaches existing-session reads after a socket reset without entering launch recovery', async () => {
    let canonicalRequests = 0;
    await startStubServer((request, response) => {
      if (request.url !== '/v1/workspaces/workspace-1/sessions/session-1/update') {
        response.writeHead(404).end();
        return;
      }
      canonicalRequests += 1;
      if (canonicalRequests === 1) {
        request.socket.destroy();
        return;
      }
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify({
        controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
        workspace: { workspaceId: 'workspace-1' },
        result: { revision: 7 }
      }));
    });
    const stateFile = process.env.BEALE_APP_SERVER_STATE_FILE!;
    const record = discoveryRecord({ url: serverUrl });
    writeFileSync(stateFile, JSON.stringify(record), 'utf8');
    process.env.BEALE_APP_SERVER_COMMAND = 'definitely-not-a-real-beale-launcher';

    await expect(fetchExistingAppServerCanonicalResult<{ revision: number }>(
      record,
      '/v1/workspaces/workspace-1/sessions/session-1/update'
    )).resolves.toEqual({ revision: 7 });
    expect(canonicalRequests).toBe(2);
  });

  it('uses the loopback discovery URL for Desktop health checks when a public URL is advertised', async () => {
    await startStubServer((_request, response) => {
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify({ ok: true }));
    });
    const record = discoveryRecord({
      url: 'https://beale-mac.example.ts.net:47174',
      localUrl: serverUrl
    });
    await expect(probeAppServerHealth(record, 1_000)).resolves.toBe(true);
  });

  it('rejects a live but capability-incompatible background server', async () => {
    await startStubServer((_request, response) => {
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify({ ok: true }));
    });
    const record = discoveryRecord({ url: serverUrl });

    await expect(probeAppServerCompatibility(record, 1_000)).resolves.toBe(false);
  });

  it('compares contract timestamps directionally', async () => {
    let timestamp = offsetContractTimestamp(-1);
    await startStubServer((_request, response) => {
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify({
        ok: true,
        controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
        contractTimestamp: timestamp,
        capabilities: BEALE_APP_SERVER_CAPABILITIES
      }));
    });
    const record = discoveryRecord({ url: serverUrl });
    await expect(inspectAppServerCompatibility(record, 1_000)).resolves.toMatchObject({ status: 'server_older' });

    timestamp = BEALE_APP_SERVER_CONTRACT_TIMESTAMP;
    await expect(inspectAppServerCompatibility(record, 1_000)).resolves.toEqual({
      status: 'compatible',
      serverContractTimestamp: BEALE_APP_SERVER_CONTRACT_TIMESTAMP
    });

    timestamp = offsetContractTimestamp(1);
    const newer = await inspectAppServerCompatibility(record, 1_000);
    expect(newer).toMatchObject({ status: 'desktop_older', serverContractTimestamp: timestamp });
    expect(new BealeDesktopRestartRequiredError(timestamp).message).toMatch(/Restart Beale/);
  });

  it('requests a Desktop restart instead of replacing a newer app-server', async () => {
    const newerTimestamp = offsetContractTimestamp(1);
    const restartErrors: BealeDesktopRestartRequiredError[] = [];
    setBealeDesktopRestartRequiredHandler((error) => { restartErrors.push(error); });
    try {
      await startStubServer((_request, response) => {
        response.writeHead(200, { 'content-type': 'application/json' });
        response.end(JSON.stringify({
          ok: true,
          controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
          contractTimestamp: newerTimestamp,
          capabilities: BEALE_APP_SERVER_CAPABILITIES
        }));
      });
      const stateFile = process.env.BEALE_APP_SERVER_STATE_FILE!;
      writeFileSync(stateFile, JSON.stringify(discoveryRecord({ url: serverUrl })), 'utf8');

      await expect(ensureBealeAppServerRunning({ readyTimeoutMs: 1_000 }))
        .rejects.toBeInstanceOf(BealeDesktopRestartRequiredError);
      expect(restartErrors[0]?.serverContractTimestamp).toBe(newerTimestamp);
    } finally {
      setBealeDesktopRestartRequiredHandler(null);
    }
  });

  it('stops an older app-server before attempting to launch its replacement', async () => {
    const oldProcess = spawn(process.execPath, ['-e', 'setInterval(() => undefined, 1000)'], {
      stdio: 'ignore',
      windowsHide: true
    });
    const oldProcessExited = new Promise<void>((resolve) => oldProcess.once('exit', () => resolve()));
    try {
      await startStubServer((request, response) => {
        if (request.url === '/v1/sessions') {
          response.writeHead(200, { 'content-type': 'application/json' });
          response.end(JSON.stringify({
            controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
            sessions: []
          }));
          return;
        }
        if (request.url === '/v1/server/shutdown') {
          response.writeHead(404).end();
          return;
        }
        if (request.url === '/shutdown') {
          oldProcess.kill();
          response.writeHead(202, { 'content-type': 'application/json' });
          response.end(JSON.stringify({ shuttingDown: true }));
          return;
        }
        response.writeHead(200, { 'content-type': 'application/json' });
        response.end(JSON.stringify({
          ok: true,
          controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
          contractTimestamp: offsetContractTimestamp(-1),
          capabilities: BEALE_APP_SERVER_CAPABILITIES
        }));
      });
      const stateFile = process.env.BEALE_APP_SERVER_STATE_FILE!;
      writeFileSync(stateFile, JSON.stringify(discoveryRecord({
        pid: oldProcess.pid!,
        url: serverUrl
      })), 'utf8');
      process.env.BEALE_APP_SERVER_COMMAND = 'definitely-not-a-real-beale-launcher';

      await expect(ensureBealeAppServerRunning({ readyTimeoutMs: 500 }))
        .rejects.toThrow(/did not become ready/);
      await oldProcessExited;
    } finally {
      if (oldProcess.exitCode === null) oldProcess.kill();
    }
  });

  it('refuses replacement when session activity cannot be checked', async () => {
    let shutdownRequests = 0;
    await startStubServer((request, response) => {
      if (request.url === '/v1/sessions') {
        response.writeHead(503).end();
        return;
      }
      if (request.url === '/v1/server/shutdown' || request.url === '/shutdown') {
        shutdownRequests += 1;
        response.writeHead(202).end();
        return;
      }
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify({
        ok: true,
        controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
        contractTimestamp: offsetContractTimestamp(-1),
        capabilities: BEALE_APP_SERVER_CAPABILITIES
      }));
    });
    const stateFile = process.env.BEALE_APP_SERVER_STATE_FILE!;
    writeFileSync(stateFile, JSON.stringify(discoveryRecord({ url: serverUrl })), 'utf8');

    await expect(ensureBealeAppServerRunning({ readyTimeoutMs: 500 }))
      .rejects.toThrow(/did not confirm that no research sessions are active/);
    expect(shutdownRequests).toBe(0);
  });

  it('replaces an unresponsive app-server only when its discovery lock verifies the process owner', async () => {
    const oldProcess = spawn(process.execPath, ['-e', [
      ...(process.platform === 'win32' ? [] : ["process.on('SIGTERM', () => undefined);"]),
      "process.send?.('ready');",
      'setInterval(() => undefined, 1000);'
    ].join('')], {
      stdio: ['ignore', 'ignore', 'ignore', 'ipc'],
      windowsHide: true
    });
    try {
      await new Promise<void>((resolve) => oldProcess.once('message', () => resolve()));
      const stateFile = process.env.BEALE_APP_SERVER_STATE_FILE!;
      writeFileSync(stateFile, JSON.stringify(discoveryRecord({
        pid: oldProcess.pid!,
        url: 'http://127.0.0.1:9'
      })), 'utf8');
      writeFileSync(`${stateFile}.lock`, JSON.stringify({ pid: oldProcess.pid }), 'utf8');
      process.env.BEALE_APP_SERVER_COMMAND = 'definitely-not-a-real-beale-launcher';

      await expect(ensureBealeAppServerRunning({
        healthTimeoutMs: 100,
        readyTimeoutMs: 500
      })).rejects.toThrow(/did not become ready/);
      await new Promise<void>((resolve) => {
        if (oldProcess.exitCode !== null || oldProcess.signalCode !== null) resolve();
        else oldProcess.once('exit', () => resolve());
      });
      expect(oldProcess.exitCode !== null || oldProcess.signalCode !== null).toBe(true);
    } finally {
      if (oldProcess.exitCode === null && oldProcess.signalCode === null) oldProcess.kill();
    }
  });

  it('preserves an unresponsive process when discovery-lock ownership cannot be verified', async () => {
    const oldProcess = spawn(process.execPath, ['-e', 'setInterval(() => undefined, 1000)'], {
      stdio: 'ignore',
      windowsHide: true
    });
    try {
      const stateFile = process.env.BEALE_APP_SERVER_STATE_FILE!;
      writeFileSync(stateFile, JSON.stringify(discoveryRecord({
        pid: oldProcess.pid!,
        url: 'http://127.0.0.1:9'
      })), 'utf8');
      writeFileSync(`${stateFile}.lock`, JSON.stringify({ pid: process.pid }), 'utf8');

      await expect(ensureBealeAppServerRunning({ healthTimeoutMs: 100 }))
        .rejects.toThrow(/does not own its discovery lock/);
      expect(oldProcess.exitCode).toBeNull();
    } finally {
      if (oldProcess.exitCode === null && oldProcess.signalCode === null) {
        oldProcess.kill();
        await new Promise<void>((resolve) => oldProcess.once('exit', () => resolve()));
      }
    }
  });

  it('defers replacement of an older app-server while a research session is active', async () => {
    const oldProcess = spawn(process.execPath, ['-e', 'setInterval(() => undefined, 1000)'], {
      stdio: 'ignore',
      windowsHide: true
    });
    try {
      let shutdownRequests = 0;
      await startStubServer((request, response) => {
        if (request.url === '/v1/sessions') {
          response.writeHead(200, { 'content-type': 'application/json' });
          response.end(JSON.stringify({
            controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
            sessions: [appServerSessionEntry('session-active', 'running')]
          }));
          return;
        }
        if (request.url === '/v1/server/shutdown' || request.url === '/shutdown') {
          shutdownRequests += 1;
          response.writeHead(202, { 'content-type': 'application/json' });
          response.end(JSON.stringify({ shuttingDown: true }));
          return;
        }
        response.writeHead(200, { 'content-type': 'application/json' });
        response.end(JSON.stringify({
          ok: true,
          controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
          contractTimestamp: offsetContractTimestamp(-1),
          capabilities: BEALE_APP_SERVER_CAPABILITIES
        }));
      });
      const stateFile = process.env.BEALE_APP_SERVER_STATE_FILE!;
      writeFileSync(stateFile, JSON.stringify(discoveryRecord({
        pid: oldProcess.pid!,
        url: serverUrl
      })), 'utf8');
      process.env.BEALE_APP_SERVER_COMMAND = 'definitely-not-a-real-beale-launcher';

      await expect(ensureBealeAppServerRunning({ readyTimeoutMs: 500 }))
        .rejects.toThrow(/cannot restart while 1 research session is active/);
      expect(shutdownRequests).toBe(0);
      expect(oldProcess.exitCode).toBeNull();
    } finally {
      if (oldProcess.exitCode === null) oldProcess.kill();
      await new Promise<void>((resolve) => oldProcess.once('exit', () => resolve()));
    }
  });

  it('fails with the launcher diagnostic when no instance can be started', async () => {
    process.env.BEALE_APP_SERVER_COMMAND = 'definitely-not-a-real-beale-launcher';
    await expect(
      ensureBealeAppServerRunning({ readyTimeoutMs: 1_500 })
    ).rejects.toThrow(/did not become ready/);
  });

  it('starts and stops sessions through the operator control plane', async () => {
    const seenAuth: string[] = [];
    const seenBody: unknown[] = [];
    await startStubServer((request, response) => {
      if (request.url === '/health') {
        response.writeHead(200, { 'content-type': 'application/json' });
        response.end(JSON.stringify({
          ok: true,
          controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
          contractTimestamp: BEALE_APP_SERVER_CONTRACT_TIMESTAMP,
          capabilities: BEALE_APP_SERVER_CAPABILITIES
        }));
        return;
      }
      if (request.url === '/v1/sessions' && request.method === 'POST') {
        seenAuth.push(request.headers.authorization ?? '');
        let body = '';
        request.on('data', (chunk: Buffer) => { body += chunk.toString('utf8'); });
        request.on('end', () => {
          seenBody.push(JSON.parse(body));
          response.writeHead(201, { 'content-type': 'application/json' });
          response.end(JSON.stringify({
            controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
            session: appServerSessionEntry('session-1', 'running'),
            attemptId: 'attempt-1',
            transport: {
              path: '/v1/sessions/session-1/transport',
              protocolVersion: 1,
              authentication: 'bearer',
              token: 'session-token',
              reconnect: 'replay'
            }
          }));
        });
        return;
      }
      if (request.url === '/v1/sessions/session-1' && request.method === 'GET') {
        response.writeHead(200, { 'content-type': 'application/json' });
        response.end(JSON.stringify({
          controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
          session: appServerSessionEntry('session-1', 'completed')
        }));
        return;
      }
      if (request.url === '/v1/sessions/session-1/attachments' && request.method === 'POST') {
        seenAuth.push(request.headers.authorization ?? '');
        response.writeHead(201, { 'content-type': 'application/json' });
        response.end(JSON.stringify({
          controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
          session: appServerSessionEntry('session-1', 'running'),
          transport: {
            path: '/v1/sessions/session-1/transport',
            protocolVersion: 1,
            authentication: 'bearer',
            token: 'attachment-token',
            reconnect: 'replay'
          }
        }));
        return;
      }
      if (request.url === '/v1/sessions/session-failed' && request.method === 'GET') {
        response.writeHead(200, { 'content-type': 'application/json' });
        response.end(JSON.stringify({
          controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
          session: appServerSessionEntry('session-failed', 'failed', 'Provider authentication failed.')
        }));
        return;
      }
      if (request.url === '/v1/sessions/session-1' && request.method === 'DELETE') {
        response.writeHead(202, { 'content-type': 'application/json' });
        response.end(JSON.stringify({
          controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
          stopped: true,
          sessionId: 'session-1'
        }));
        return;
      }
      response.writeHead(404, { 'content-type': 'application/json' });
      response.end(JSON.stringify({ error: 'Not found.' }));
    });
    const stateFile = process.env.BEALE_APP_SERVER_STATE_FILE!;
    writeFileSync(stateFile, JSON.stringify(discoveryRecord({ url: serverUrl })), 'utf8');
    const record = readBealeAppServerDiscovery(stateFile)!;

    const request = sessionLaunchRequest('session-1');
    const started = await startAppServerSession(record, request);
    expect(started.sessionId).toBe('session-1');
    expect(started.attemptId).toBe('attempt-1');
    expect(started.token).toBe('session-token');
    expect(started.url).toBe(`${serverUrl.replace('http', 'ws')}/v1/sessions/session-1/transport`);
    expect(seenAuth).toEqual(['Bearer operator-token']);
    expect(seenBody[0]).toEqual(request);

    await expect(attachAppServerSession(record, 'session-1')).resolves.toEqual({
      sessionId: 'session-1',
      url: `${serverUrl.replace('http', 'ws')}/v1/sessions/session-1/transport`,
      token: 'attachment-token'
    });
    expect(seenAuth).toEqual(['Bearer operator-token', 'Bearer operator-token']);

    const entry = await fetchAppServerSession(record, 'session-1');
    expect(entry?.state).toBe('completed');
    expect(entry?.diagnostic).toBeNull();
    expect((await fetchAppServerSession(record, 'session-failed'))?.diagnostic).toBe('Provider authentication failed.');
    expect(await fetchAppServerSession(record, 'missing')).toBeNull();

    await expect(stopAppServerSession(record, 'session-1')).resolves.toBeUndefined();
  });
});

function offsetContractTimestamp(milliseconds: number): string {
  return new Date(Date.parse(BEALE_APP_SERVER_CONTRACT_TIMESTAMP) + milliseconds).toISOString();
}

let stubHandler: ((request: import('node:http').IncomingMessage, response: import('node:http').ServerResponse) => void) | null = null;

async function startStubServer(
  handler: (request: import('node:http').IncomingMessage, response: import('node:http').ServerResponse) => void
): Promise<void> {
  stubHandler = handler;
  server = createServer((request, response) => stubHandler?.(request, response));
  await new Promise<void>((resolve) => {
    server!.listen(0, '127.0.0.1', () => resolve());
  });
  const address = server.address();
  serverUrl = `http://127.0.0.1:${address && typeof address !== 'string' ? address.port : 0}`;
}

function discoveryRecord(overrides: Partial<BealeAppServerDiscovery>): BealeAppServerDiscovery {
  return {
    version: 1,
    hostMode: 'tray',
    pid: process.pid,
    host: '127.0.0.1',
    port: 0,
    url: '',
    operatorToken: 'operator-token',
    startedAt: '2026-08-21T00:00:00.000Z',
    ...overrides
  };
}

function sessionLaunchRequest(sessionId: string) {
  return {
    launchVersion: 2 as const,
    sessionId,
    launch: {
      workspaceId: 'workspace-test',
      promptMarkdown: 'Test the typed app-server launch contract.',
      provider: {
        id: 'openai-codex'
      },
      shellSafetyMode: 'manual_approval'
    }
  };
}

function appServerSessionEntry(
  sessionId: string,
  state: 'running' | 'completed' | 'failed',
  diagnostic: string | null = null
) {
  return {
    sessionId,
    state,
    startedAt: '2026-08-21T00:00:00.000Z',
    endedAt: state === 'running' ? null : '2026-08-21T00:01:00.000Z',
    exitCode: state === 'running' ? null : state === 'completed' ? 0 : 1,
    clientConnected: state === 'running',
    diagnostic,
    replay: { bufferedFrames: 0, bufferedBytes: 0, droppedFrames: 0 }
  };
}
