import { createServer } from 'node:http';
import { once } from 'node:events';
import { WebSocketServer } from 'ws';
import { afterEach, expect, test, vi } from 'vitest';

const appServer = vi.hoisted(() => ({
  invoke: vi.fn(),
  ensure: vi.fn(),
  attach: vi.fn(),
}));
vi.mock('../src/main/bealeAppServerClient', () => ({
  invokeAppServerOperation: appServer.invoke,
  ensureBealeAppServerRunning: appServer.ensure,
  attachAppServerSession: appServer.attach,
}));

import { FleetBrowserViewer } from '../src/main/fleetBrowserViewer';

afterEach(() => { vi.clearAllMocks(); });

test('a remote primary browser streams and accepts input through its session attachment', async () => {
  const httpServer = createServer();
  const sockets = new WebSocketServer({ server: httpServer });
  httpServer.listen(0, '127.0.0.1');
  await once(httpServer, 'listening');
  const address = httpServer.address();
  if (!address || typeof address === 'string') throw new Error('Missing test browser port.');
  appServer.invoke.mockResolvedValue({ url: `ws://127.0.0.1:${address.port}/browser`, token: 'synthetic-session-token' });
  const input = new Promise<unknown>((resolveInput) => {
    sockets.on('connection', (socket, request) => {
      expect(request.headers.authorization).toBe('Bearer synthetic-session-token');
      socket.send(JSON.stringify({ type: 'browser.ready', url: 'https://example.test/', width: 1280, height: 800 }));
      socket.once('message', (raw) => resolveInput(JSON.parse(String(raw))));
    });
  });
  const updates: Array<{ type: string; runId: string }> = [];
  const viewer = new FleetBrowserViewer((update) => { updates.push(update); });
  try {
    viewer.connectRemote('session-example', 'server-example');
    const deadline = Date.now() + 3_000;
    while (!updates.some((update) => update.type === 'browser.ready') && Date.now() < deadline) {
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 10));
    }
    expect(updates).toContainEqual(expect.objectContaining({ type: 'browser.ready', runId: 'session-example' }));
    viewer.input('session-example', { type: 'browser.text', text: 'example input' });
    expect(await input).toEqual({ type: 'browser.text', text: 'example input', id: 1 });
    expect(appServer.invoke).toHaveBeenCalledWith({ operation: 'fleet.remote_browser_attach',
      input: { serverId: 'server-example', sessionId: 'session-example' } });
    expect(appServer.ensure).not.toHaveBeenCalled();
  } finally {
    viewer.close();
    sockets.close();
    await new Promise<void>((resolveClose) => httpServer.close(() => resolveClose()));
  }
});
