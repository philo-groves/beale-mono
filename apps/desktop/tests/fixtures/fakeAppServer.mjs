import { createServer } from 'node:http';
import { randomBytes } from 'node:crypto';
import { spawn } from 'node:child_process';
import { existsSync, rmSync, writeFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
import { WebSocket, WebSocketServer } from 'ws';
import {
  BEALE_APP_SERVER_CAPABILITIES,
  BEALE_APP_SERVER_CONTRACT_TIMESTAMP,
  BEALE_APP_SERVER_CONTROL_VERSION,
  BEALE_APP_SERVER_OPERATIONS_PATH,
  BEALE_APP_SERVER_SESSIONS_PATH,
  APP_SERVER_CONTRACT_VERSION,
  APP_SERVER_PROTOCOL_VERSION
} from '@beale/app-server-runtime/protocol';

const OPERATOR_TOKEN = 'fake-operator-token';
const stateFile = process.env.FAKE_APP_SERVER_STATE_FILE ?? '';
const childScript = process.env.FAKE_APP_SERVER_CHILD_SCRIPT ?? '';
const childArgs = JSON.parse(process.env.FAKE_APP_SERVER_CHILD_ARGS_JSON ?? '[]');
const sessionLaunchModule = process.env.FAKE_APP_SERVER_SESSION_LAUNCH_MODULE ?? '';
const researchAgentModule = process.env.FAKE_RESEARCH_AGENT_MODULE ?? '';
if (!stateFile || !childScript || !sessionLaunchModule || !researchAgentModule) {
  console.error('fake-app-server requires its state, child, session-launch, and research-agent module paths.');
  process.exit(1);
}
const {
  AppServerHostRegistry,
  AppServerHostService,
  invokeAppServerProtocol,
  prepareAppServerSessionLaunch
} = await import(pathToFileURL(sessionLaunchModule).href);
const { AppServerSessionStore } = await import(pathToFileURL(researchAgentModule).href);
const hostRegistry = new AppServerHostRegistry({
  registryDirectory: process.env.FAKE_APP_SERVER_REGISTRY_DIRECTORY,
  appServerDatabasePath: process.env.FAKE_APP_SERVER_DATABASE_PATH,
  appServerArtifactDirectory: process.env.FAKE_APP_SERVER_ARTIFACT_DIRECTORY
});
const hostService = new AppServerHostService({
  registry: hostRegistry,
  invokeProtocol: async (operation, options) => {
    if (operation === 'provider.complete') {
      const goals = [
        'Compare precipitation bias across the recorded regional datasets.',
        'Trace how boundary conditions alter the recorded temperature projections.',
        'Audit calibration assumptions against the collection’s observed outcomes.',
        'Reproduce the uncertainty analysis across the bounded model families.'
      ];
      return {
        text: JSON.stringify({
          candidates: goals.map((goal, index) => ({
            goal,
            groundingRefs: ['workspace:scope'],
            rationale: 'The comparison is grounded in the recorded collection.',
            noveltyAxis: `axis-${index + 1}`
          }))
        })
      };
    }
    return invokeAppServerProtocol(operation, options);
  }
});

const operatorToken = OPERATOR_TOKEN;
const sessions = new Map();

const server = createServer((request, response) => {
  void handle(request, response).catch((error) => {
    if (!response.headersSent) {
      response.writeHead(500, { 'content-type': 'application/json' });
      response.end(JSON.stringify({ error: `fake app-server failure: ${error instanceof Error ? error.message : String(error)}` }));
    }
  });
});

const wss = new WebSocketServer({ noServer: true, maxPayload: 1_048_576 });
server.on('upgrade', (request, socket, head) => {
  const url = new URL(request.url ?? '/', 'http://localhost');
  const match = /^\/v1\/sessions\/([^/]+)\/transport$/.exec(url.pathname);
  const entry = match ? sessions.get(decodeURIComponent(match[1] ?? '')) : null;
  if (!entry || request.headers.authorization !== `Bearer ${entry.token}`) {
    socket.write('HTTP/1.1 401 Unauthorized\r\nConnection: close\r\nContent-Length: 0\r\n\r\n');
    socket.destroy();
    return;
  }
  wss.handleUpgrade(request, socket, head, (client) => proxySessionTransport(entry, client));
});

server.listen(0, '127.0.0.1', () => {
  const address = server.address();
  const port = address && typeof address !== 'string' ? address.port : 0;
  writeFileSync(stateFile, `${JSON.stringify({
    version: 1,
    contractTimestamp: BEALE_APP_SERVER_CONTRACT_TIMESTAMP,
    pid: process.pid,
    host: '127.0.0.1',
    port,
    url: `http://127.0.0.1:${port}`,
    operatorToken,
    startedAt: new Date().toISOString()
  }, null, 2)}\n`, 'utf8');
});

async function handle(request, response) {
  const url = new URL(request.url ?? '/', 'http://localhost');
  if (request.method === 'GET' && url.pathname === '/health') {
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end(JSON.stringify({
      ok: true,
      controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
      contractTimestamp: BEALE_APP_SERVER_CONTRACT_TIMESTAMP,
      capabilities: BEALE_APP_SERVER_CAPABILITIES
    }));
    return;
  }

  const authorization = request.headers.authorization ?? '';
  if (authorization !== `Bearer ${operatorToken}`) {
    response.writeHead(401, { 'content-type': 'application/json' });
    response.end(JSON.stringify({ error: 'An operator bearer token is required.' }));
    return;
  }

  if (request.method === 'GET' && url.pathname === BEALE_APP_SERVER_SESSIONS_PATH) {
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end(JSON.stringify({
      controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
      sessions: [...sessions.values()].map(catalogEntry)
    }));
    return;
  }

  if (request.method === 'POST' && url.pathname === BEALE_APP_SERVER_OPERATIONS_PATH) {
    const body = await readJson(request);
    const result = await hostService.executeOperation({
      operation: body.operation,
      ...(Array.isArray(body.args) ? { args: body.args } : {}),
      ...(body.input !== undefined ? { input: body.input } : {}),
      ...(typeof body.profileId === 'string' ? { profileId: body.profileId } : {})
    });
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end(JSON.stringify({ controlVersion: BEALE_APP_SERVER_CONTROL_VERSION, result }));
    return;
  }

  const canonicalMatch = /^\/v1\/workspaces\/([^/]+)\/sessions\/([^/]+)\/(update|events|collaboration|captures|event-details)$/.exec(url.pathname);
  if (canonicalMatch) {
    const workspaceIdentifier = decodeURIComponent(canonicalMatch[1] ?? '');
    const sessionId = decodeURIComponent(canonicalMatch[2] ?? '');
    const operation = canonicalMatch[3];
    const workspace = hostRegistry.resolveWorkspace(workspaceIdentifier) ?? {
      id: workspaceIdentifier,
      workspaceId: workspaceIdentifier,
      name: 'Fixture workspace',
      researchProfileId: 'security-research',
      updatedAt: new Date().toISOString()
    };
    const storage = hostRegistry.storageForProfile(workspace.researchProfileId);
    const store = new AppServerSessionStore({
      databasePath: storage.databasePath,
      artifactDirectoryPath: storage.artifactDirectoryPath,
      readOnly: true
    });
    try {
      let result;
      if (request.method === 'GET' && operation === 'update') {
        result = store.getUpdate(sessionId, url.searchParams.get('afterEventId'), {
          tail: url.searchParams.get('tail') === 'true',
          limit: positiveQuery(url, 'limit', 200),
          maxBytes: positiveQuery(url, 'maxBytes', 1_000_000)
        });
      } else if (request.method === 'GET' && operation === 'events') {
        result = store.getEventPage(sessionId, {
          stream: ['transcript', 'trace'].includes(url.searchParams.get('stream')) ? url.searchParams.get('stream') : 'all',
          ...(url.searchParams.get('afterEventId') ? { afterEventId: url.searchParams.get('afterEventId') } : {}),
          tail: url.searchParams.get('tail') === 'true',
          limit: positiveQuery(url, 'limit', 200),
          maxBytes: positiveQuery(url, 'maxBytes', 1_000_000)
        });
      } else if (request.method === 'GET' && operation === 'collaboration') {
        result = store.getCollaborationState(sessionId, positiveQuery(url, 'messageLimit', 200));
      } else if (request.method === 'GET' && operation === 'captures') {
        result = store.listCaptureSummaries(sessionId);
      } else if (request.method === 'POST' && operation === 'event-details') {
        const body = await readJson(request);
        result = store.getEventDetails(sessionId, Array.isArray(body.eventIds) ? body.eventIds : []);
      } else {
        response.writeHead(405, { 'content-type': 'application/json' });
        response.end(JSON.stringify({ error: 'Method not allowed.' }));
        return;
      }
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify({
        controlVersion: 1,
        workspace: {
          id: workspace.id,
          workspaceId: workspace.workspaceId,
          name: workspace.name,
          researchProfileId: workspace.researchProfileId,
          updatedAt: workspace.updatedAt
        },
        result
      }));
      return;
    } finally {
      store.close();
    }
  }

  if (request.method === 'POST' && url.pathname === BEALE_APP_SERVER_SESSIONS_PATH) {
    const body = await readJson(request);
    const sessionId = typeof body.sessionId === 'string' ? body.sessionId : `session-${randomBytes(6).toString('hex')}`;
    if (sessions.has(sessionId) && !isTerminal(sessions.get(sessionId).state)) {
      response.writeHead(409, { 'content-type': 'application/json' });
      response.end(JSON.stringify({ error: `Session ${sessionId} already exists.` }));
      return;
    }
    const token = randomBytes(24).toString('base64url');
    const entry = {
      sessionId,
      token,
      state: 'starting',
      exitCode: null,
      endedAt: null,
      startedAt: new Date().toISOString(),
      clientConnected: false,
      diagnostic: null,
      clientSockets: new Set()
    };
    sessions.set(sessionId, entry);
    const resolved = await hostService.prepareSession(body, sessionId);
    const prepared = prepareAppServerSessionLaunch(resolved.launch, process.env);
    const child = spawn(process.execPath, [
      childScript,
      ...childArgs,
      '--hosted-session',
      '--session-id',
      sessionId,
      ...prepared.args
    ], {
      env: { ...prepared.env, APP_SERVER_TRANSPORT_TOKEN: token },
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true
    });
    entry.child = child;
    let stderr = '';
    child.stderr?.on('data', (chunk) => { stderr += chunk.toString('utf8'); });
    child.once('error', () => {
      entry.state = 'failed';
      entry.endedAt = new Date().toISOString();
    });
    child.once('close', (code) => {
      entry.exitCode = code;
      entry.endedAt = new Date().toISOString();
      if (entry.state !== 'failed') {
        entry.state = entry.stopRequested ? 'stopped' : code === 0 ? 'completed' : 'failed';
      }
      if (stderr.trim()) {
        entry.diagnostic = stderr.trim().slice(-1000);
      }
      for (const client of entry.clientSockets) {
        if (client.readyState === WebSocket.OPEN) client.close(1000, 'fixture child exited');
      }
    });

    entry.state = 'running';
    response.writeHead(201, { 'content-type': 'application/json' });
    response.end(JSON.stringify({
      controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
      session: catalogEntry(entry),
      attemptId: resolved.attemptId,
      transport: {
        path: `${BEALE_APP_SERVER_SESSIONS_PATH}/${encodeURIComponent(sessionId)}/transport`,
        protocolVersion: APP_SERVER_PROTOCOL_VERSION,
        authentication: 'bearer',
        token,
        reconnect: 'replay'
      }
    }));
    return;
  }

  const sessionMatch = /^\/v1\/sessions\/([^/]+)$/.exec(url.pathname);
  if (sessionMatch && request.method === 'GET') {
    const sessionId = decodeURIComponent(sessionMatch[1] ?? '');
    const entry = sessions.get(sessionId);
    if (!entry) {
      response.writeHead(404, { 'content-type': 'application/json' });
      response.end(JSON.stringify({ error: 'Unknown session.' }));
      return;
    }
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end(JSON.stringify({
      controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
      session: catalogEntry(entry)
    }));
    return;
  }

  const deleteMatch = request.method === 'DELETE' ? sessionMatch : null;
  if (deleteMatch) {
    const sessionId = decodeURIComponent(deleteMatch[1] ?? '');
    const entry = sessions.get(sessionId);
    if (!entry) {
      response.writeHead(404, { 'content-type': 'application/json' });
      response.end(JSON.stringify({ error: 'Unknown session.' }));
      return;
    }
    if (!isTerminal(entry.state)) {
      entry.stopRequested = true;
      entry.child?.kill();
      response.writeHead(202, { 'content-type': 'application/json' });
      response.end(JSON.stringify({
        controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
        stopped: true,
        sessionId
      }));
      return;
    }
    sessions.delete(sessionId);
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end(JSON.stringify({
      controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
      stopped: false,
      sessionId
    }));
    return;
  }

  response.writeHead(404, { 'content-type': 'application/json' });
  response.end(JSON.stringify({ error: 'Not found.' }));
}

function catalogEntry(entry) {
  return {
    sessionId: entry.sessionId,
    state: entry.state,
    startedAt: entry.startedAt,
    endedAt: entry.endedAt,
    exitCode: entry.exitCode,
    diagnostic: entry.diagnostic,
    clientConnected: entry.clientConnected,
    replay: { bufferedFrames: 0, bufferedBytes: 0, droppedFrames: 0 }
  };
}

function proxySessionTransport(entry, client) {
  entry.clientConnected = true;
  entry.clientSockets.add(client);
  client.on('close', () => {
    entry.clientSockets.delete(client);
    entry.clientConnected = entry.clientSockets.size > 0;
  });
  client.on('error', () => client.terminate());
  if (isTerminal(entry.state)) {
    queueMicrotask(() => client.close(1000, 'fixture child already exited'));
    return;
  }
  client.on('message', (data) => {
    const message = JSON.parse(data.toString('utf8'));
    if (message.type === 'client.hello') {
      client.send(JSON.stringify({
        protocolVersion: 1,
        type: 'server.hello',
        sessionId: entry.sessionId,
        server: { name: 'app-server', version: '0.1.0', buildId: 'fixture' },
        contractVersion: APP_SERVER_CONTRACT_VERSION,
        schemas: { protocol: 1, session: 1, memorySummary: 11, finding: 4, campaignGraph: 4, goalSuggestions: 1 },
        capabilities: ['session.events', 'session.controls']
      }));
    }
  });
}

function isTerminal(state) {
  return state === 'completed' || state === 'failed' || state === 'stopped';
}

function positiveQuery(url, name, fallback) {
  const value = Number(url.searchParams.get(name));
  return Number.isSafeInteger(value) && value > 0 ? value : fallback;
}

function readJson(request) {
  return new Promise((resolve, reject) => {
    let body = '';
    request.on('data', (chunk) => { body += chunk.toString('utf8'); });
    request.on('end', () => {
      try {
        resolve(body ? JSON.parse(body) : {});
      } catch {
        resolve({});
      }
    });
    request.on('error', reject);
  });
}

let shuttingDown = false;
const parentPid = Number.parseInt(process.env.BEALE_APP_SERVER_PARENT_PID ?? '', 10);
const lifetimeMonitor = Number.isInteger(parentPid) && parentPid > 0 && parentPid !== process.pid
  ? setInterval(() => {
      try {
        process.kill(parentPid, 0);
      } catch {
        shutdown('ephemeral parent exited');
      }
    }, 500)
  : null;
lifetimeMonitor?.unref();
for (const signal of ['SIGINT', 'SIGTERM']) {
  process.once(signal, () => shutdown(signal));
}

function shutdown(_reason) {
  if (shuttingDown) return;
  shuttingDown = true;
  if (lifetimeMonitor) clearInterval(lifetimeMonitor);
  for (const entry of sessions.values()) {
    entry.child?.kill();
  }
  if (stateFile && existsSync(stateFile)) {
    try { rmSync(stateFile); } catch { /* best effort */ }
  }
  const forcedExit = setTimeout(() => process.exit(1), 5_000);
  server.close(() => {
    clearTimeout(forcedExit);
    process.exit(0);
  });
}
