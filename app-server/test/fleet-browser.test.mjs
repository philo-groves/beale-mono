import assert from 'node:assert/strict';
import { EventEmitter, once } from 'node:events';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { WebSocket, WebSocketServer } from 'ws';
import { FleetBrowserService } from '../dist/fleetBrowser.js';

test('Fleet browser streams one guest page and accepts researcher input without a second navigation', async () => {
  const commands = [];
  let pageSockets = 0;
  const server = createServer((request, response) => {
    if (request.url === '/json/list') {
      response.setHeader('content-type', 'application/json');
      response.end(JSON.stringify([{ id: 'page-example', type: 'page', title: 'Example', url: 'about:blank',
        webSocketDebuggerUrl: `ws://127.0.0.1:${server.address().port}/devtools/page/page-example` }]));
    } else if (request.url === '/json/version') {
      response.end('{}');
    } else response.writeHead(404).end();
  });
  const sockets = new WebSocketServer({ noServer: true });
  const fleet = new FleetBrowserService(async () => `http://127.0.0.1:${server.address().port}`);
  server.on('upgrade', (request, socket, head) => {
    sockets.handleUpgrade(request, socket, head, (client) => {
      if (request.url === '/viewer') {
        void fleet.viewerSocket('session-example', client);
        return;
      }
      if (request.url === '/agent') {
        void fleet.agentSocket('session-example', 'page-example', client);
        return;
      }
      pageSockets += 1;
      client.on('message', (raw) => {
        const command = JSON.parse(String(raw));
        commands.push(command);
        client.send(JSON.stringify({ id: command.id, result: {} }));
        if (command.method === 'Page.startScreencast') {
          client.send(JSON.stringify({ method: 'Page.screencastFrame', params: {
            data: Buffer.from('fake-jpeg').toString('base64'), sessionId: 1,
            metadata: { deviceWidth: 1280, deviceHeight: 800 }
          } }));
        }
      });
    });
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const viewer = new WebSocket(`ws://127.0.0.1:${server.address().port}/viewer`);
  try {
    fleet.tokenFor('session-example');
    const framePromise = new Promise((resolve, reject) => {
      const timeout = setTimeout(() => reject(new Error('No Fleet browser frame arrived.')), 3000);
      viewer.on('message', (raw) => {
        const message = JSON.parse(String(raw));
        if (message.type === 'browser.frame') { clearTimeout(timeout); resolve(message); }
      });
    });
    await once(viewer, 'open');
    const frame = await framePromise;
    assert.equal(frame.data, Buffer.from('fake-jpeg').toString('base64'));
    const endpoint = await fleet.discovery('session-example', 'version', `http://127.0.0.1:${server.address().port}`);
    assert.match(endpoint.webSocketDebuggerUrl, /^ws:\/\/127\.0\.0\.1:[0-9]+\/v1\/fleet-browser\/session-example\/devtools\/page\/page-example\?token=/u);
    viewer.send(JSON.stringify({ id: 1, type: 'browser.navigate', url: 'https://example.test/' }));
    await new Promise((resolve) => setTimeout(resolve, 30));
    assert.equal(commands.filter((command) => command.method === 'Page.navigate').length, 1);
    assert.equal(pageSockets, 1);
    const agentClient = new WebSocket(`ws://127.0.0.1:${server.address().port}/agent`);
    await once(agentClient, 'open');
    agentClient.send(JSON.stringify({ id: 7, method: 'Input.dispatchMouseEvent', params: { type: 'mouseMoved', x: 10, y: 10 } }));
    const denied = await once(agentClient, 'message');
    assert.match(String(denied[0]), /researcher is controlling/u);
    assert.equal(commands.filter((command) => command.method === 'Input.dispatchMouseEvent').length, 0);
    agentClient.close();
  } finally {
    viewer.close();
    fleet.close();
    sockets.close();
    server.close();
  }
});

test('closeSession tears down one session process, sockets, pending commands, and cached frame', async () => {
  const rootDir = mkdtempSync(join(tmpdir(), 'beale-fleet-browser-close-example-'));
  const cdpPageSockets = [];
  let onCdpPageSocket;
  let holdNavigateCommand = false;
  let markNavigateReceived;
  const navigateReceived = new Promise((resolve) => { markNavigateReceived = resolve; });
  let sendFrameOnScreencast = true;
  const server = createServer((request, response) => {
    if (request.url === '/json/list') {
      response.setHeader('content-type', 'application/json');
      response.end(JSON.stringify([{ id: 'page-example', type: 'page', title: 'Example', url: 'about:blank',
        webSocketDebuggerUrl: `ws://127.0.0.1:${server.address().port}/devtools/page/page-example` }]));
    } else if (request.url === '/json/version') {
      response.end('{}');
    } else response.writeHead(404).end();
  });
  const sockets = new WebSocketServer({ noServer: true });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const cdpPort = server.address().port;

  const spawnedChildren = new Map();
  const fleet = new FleetBrowserService(undefined, undefined, {
    rootDir,
    resolveExecutable: async () => '/synthetic/chrome',
    spawnBrowser: (_executable, args) => {
      const child = new EventEmitter();
      child.exitCode = null;
      child.signalCode = null;
      child.killCalls = 0;
      child.kill = () => {
        child.killCalls += 1;
        child.exitCode = 0;
        child.emit('exit', 0);
        return true;
      };
      const profileArg = args.find((arg) => arg.startsWith('--user-data-dir='));
      const profilePath = profileArg.slice('--user-data-dir='.length);
      writeFileSync(join(profilePath, 'DevToolsActivePort'), String(cdpPort));
      const sessionId = profilePath.split(/[\\/]/u).at(-2);
      spawnedChildren.set(sessionId, child);
      return child;
    },
  });

  server.on('upgrade', (request, socket, head) => {
    sockets.handleUpgrade(request, socket, head, (client) => {
      const match = /^\/(viewer|agent)\/([^/]+)$/u.exec(request.url ?? '');
      if (match?.[1] === 'viewer') {
        void fleet.viewerSocket(match[2], client).catch(() => undefined);
        return;
      }
      if (match?.[1] === 'agent') {
        void fleet.agentSocket(match[2], 'page-example', client).catch(() => undefined);
        return;
      }
      cdpPageSockets.push(client);
      onCdpPageSocket?.(client);
      client.on('message', (raw) => {
        const command = JSON.parse(String(raw));
        if (command.method === 'Page.navigate' && holdNavigateCommand) {
          markNavigateReceived();
          return;
        }
        client.send(JSON.stringify({ id: command.id, result: {} }));
        if (command.method === 'Page.startScreencast' && sendFrameOnScreencast) {
          client.send(JSON.stringify({ method: 'Page.screencastFrame', params: {
            data: Buffer.from('stale-frame-example').toString('base64'), sessionId: 1,
            metadata: { deviceWidth: 1280, deviceHeight: 800 },
          } }));
        }
      });
    });
  });

  const waitForMessage = (ws, predicate) => new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error('Timed out waiting for browser message.')), 3000);
    const onMessage = (raw) => {
      const parsed = JSON.parse(String(raw));
      if (predicate(parsed)) {
        clearTimeout(timeout);
        ws.off('message', onMessage);
        resolve(parsed);
      }
    };
    ws.on('message', onMessage);
  });

  const closingToken = fleet.tokenFor('session-closing-example');
  const activeToken = fleet.tokenFor('session-active-example');
  const closingViewer = new WebSocket(`ws://127.0.0.1:${cdpPort}/viewer/session-closing-example`);
  let activeViewer;
  let agentClient;
  let reusedViewer;
  try {
    await waitForMessage(closingViewer, (msg) => msg.type === 'browser.frame');
    const closingCdpPage = cdpPageSockets[0];

    activeViewer = new WebSocket(`ws://127.0.0.1:${cdpPort}/viewer/session-active-example`);
    await waitForMessage(activeViewer, (msg) => msg.type === 'browser.frame');

    const agentUpstreamPromise = new Promise((resolve) => { onCdpPageSocket = resolve; });
    agentClient = new WebSocket(`ws://127.0.0.1:${cdpPort}/agent/session-closing-example`);
    await once(agentClient, 'open');
    const agentUpstream = await agentUpstreamPromise;
    onCdpPageSocket = undefined;

    // Leave one CDP command pending before tearing down the session.
    holdNavigateCommand = true;
    closingViewer.send(JSON.stringify({ id: 99, type: 'browser.navigate', url: 'https://example.test/pending' }));
    await navigateReceived;

    const closingViewerClosed = once(closingViewer, 'close');
    const agentClientClosed = once(agentClient, 'close');
    const agentUpstreamClosed = once(agentUpstream, 'close');
    const cdpPageClosed = once(closingCdpPage, 'close');

    fleet.closeSession('session-closing-example');

    assert.equal(fleet.hasSession('session-closing-example'), false);
    assert.equal(fleet.authorized('session-closing-example', closingToken), false);
    assert.equal(spawnedChildren.get('session-closing-example')?.killCalls, 1);

    await Promise.all([closingViewerClosed, agentClientClosed, agentUpstreamClosed, cdpPageClosed]);

    // Unrelated session stays alive and unaffected.
    assert.equal(fleet.hasSession('session-active-example'), true);
    assert.equal(fleet.authorized('session-active-example', activeToken), true);
    assert.equal(spawnedChildren.get('session-active-example')?.killCalls, 0);
    assert.equal(activeViewer.readyState, WebSocket.OPEN);

    // Re-allocating the same session ID must not replay the closed session's cached frame.
    sendFrameOnScreencast = false;
    fleet.tokenFor('session-closing-example');
    reusedViewer = new WebSocket(`ws://127.0.0.1:${cdpPort}/viewer/session-closing-example`);
    const reusedMessages = [];
    reusedViewer.on('message', (raw) => reusedMessages.push(JSON.parse(String(raw))));
    await waitForMessage(reusedViewer, (msg) => msg.type === 'browser.ready');
    await new Promise((resolve) => setTimeout(resolve, 30));
    assert.equal(reusedMessages.some((msg) => msg.type === 'browser.frame'), false);
  } finally {
    closingViewer.close();
    activeViewer?.close();
    agentClient?.close();
    reusedViewer?.close();
    fleet.close();
    sockets.close();
    server.close();
    rmSync(rootDir, { recursive: true, force: true });
  }
});

test('closeSession aborts an in-flight launch before or after spawning Chrome', async () => {
  const rootDir = mkdtempSync(join(tmpdir(), 'beale-fleet-browser-inflight-example-'));
  try {
    // Case 1: closed while executable resolution is still pending -> never spawns Chrome.
    let releaseExecutable;
    let spawnCountBefore = 0;
    const fleetBeforeSpawn = new FleetBrowserService(undefined, undefined, {
      rootDir,
      resolveExecutable: () => new Promise((resolve) => { releaseExecutable = resolve; }),
      spawnBrowser: () => {
        spawnCountBefore += 1;
        throw new Error('spawnBrowser should not be called after closeSession.');
      },
    });
    fleetBeforeSpawn.tokenFor('session-inflight-pre-spawn');
    const preSpawnDiscovery = fleetBeforeSpawn.discovery('session-inflight-pre-spawn', 'contexts', 'http://127.0.0.1:4000');
    await new Promise((resolve) => setTimeout(resolve, 10));
    fleetBeforeSpawn.closeSession('session-inflight-pre-spawn');
    releaseExecutable('/synthetic/chrome');
    await assert.rejects(preSpawnDiscovery, /closed/u);
    assert.equal(spawnCountBefore, 0);
    assert.equal(fleetBeforeSpawn.hasSession('session-inflight-pre-spawn'), false);

    // Case 2: closed after Chrome was spawned while waiting for DevToolsActivePort -> kills child and rejects.
    let spawnedChild;
    let markSpawned;
    const spawned = new Promise((resolve) => { markSpawned = resolve; });
    const fleetDuringBoot = new FleetBrowserService(undefined, undefined, {
      rootDir,
      resolveExecutable: async () => '/synthetic/chrome',
      spawnBrowser: () => {
        const child = new EventEmitter();
        child.exitCode = null;
        child.signalCode = null;
        child.killCalls = 0;
        child.kill = () => {
          child.killCalls += 1;
          child.exitCode = 0;
          child.emit('exit', 0);
          return true;
        };
        spawnedChild = child;
        markSpawned();
        return child;
      },
    });
    fleetDuringBoot.tokenFor('session-inflight-boot');
    const bootDiscovery = fleetDuringBoot.discovery('session-inflight-boot', 'contexts', 'http://127.0.0.1:4000');
    await spawned;
    fleetDuringBoot.closeSession('session-inflight-boot');
    await assert.rejects(bootDiscovery, /closed/u);
    assert.equal(spawnedChild.killCalls, 1);
    assert.equal(fleetDuringBoot.hasSession('session-inflight-boot'), false);
  } finally {
    rmSync(rootDir, { recursive: true, force: true });
  }
});
