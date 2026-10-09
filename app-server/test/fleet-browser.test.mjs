import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { once } from 'node:events';
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
