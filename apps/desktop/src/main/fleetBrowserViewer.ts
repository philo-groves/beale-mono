import WebSocket from 'ws';
import { attachAppServerSession, ensureBealeAppServerRunning, invokeAppServerOperation } from './bealeAppServerClient';
import type { FleetBrowserInput, FleetBrowserUpdate } from '../shared/fleetBrowser';

interface Viewer {
  socket: WebSocket | null;
  stopped: boolean;
  nextId: number;
  target: { kind: 'vm'; machineId: string } | { kind: 'remote'; serverId: string };
}

/** Keeps browser pixels out of the canonical session event stream. */
export class FleetBrowserViewer {
  private readonly viewers = new Map<string, Viewer>();

  public constructor(private readonly onUpdate: (update: FleetBrowserUpdate) => void) {}

  public connect(runId: string, machineId: string): void {
    if (!runId || !machineId || machineId === 'local') throw new Error('A Fleet session is required for guest browser viewing.');
    const existing = this.viewers.get(runId);
    if (existing && !existing.stopped && existing.target.kind === 'vm' && existing.target.machineId === machineId) return;
    if (existing) this.disconnect(runId);
    const viewer: Viewer = { socket: null, stopped: false, nextId: 1, target: { kind: 'vm', machineId } };
    this.viewers.set(runId, viewer);
    void this.run(runId, viewer);
  }

  public connectRemote(runId: string, serverId: string): void {
    if (!runId || !serverId || serverId === 'local') throw new Error('A connected remote app server is required.');
    const existing = this.viewers.get(runId);
    if (existing && !existing.stopped && existing.target.kind === 'remote' && existing.target.serverId === serverId) return;
    if (existing) this.disconnect(runId);
    const viewer: Viewer = { socket: null, stopped: false, nextId: 1, target: { kind: 'remote', serverId } };
    this.viewers.set(runId, viewer);
    void this.run(runId, viewer);
  }

  public input(runId: string, input: FleetBrowserInput): void {
    const viewer = this.viewers.get(runId);
    if (!viewer || viewer.socket?.readyState !== WebSocket.OPEN) throw new Error('Fleet browser is not connected.');
    viewer.socket.send(JSON.stringify({ ...input, id: viewer.nextId++ }));
  }

  public disconnect(runId: string): void {
    const viewer = this.viewers.get(runId);
    if (!viewer) return;
    viewer.stopped = true;
    if (viewer.socket?.readyState === WebSocket.OPEN) viewer.socket.send(JSON.stringify({ type: 'browser.release', id: viewer.nextId++ }));
    viewer.socket?.close();
    this.viewers.delete(runId);
  }

  public close(): void {
    for (const runId of [...this.viewers.keys()]) this.disconnect(runId);
  }

  private async run(runId: string, viewer: Viewer): Promise<void> {
    let attempts = 0;
    while (!viewer.stopped) {
      try {
        let url: string;
        let token: string;
        if (viewer.target.kind === 'remote') {
          const attachment = await invokeAppServerOperation<{ url: string; token: string }>({
            operation: 'fleet.remote_browser_attach', input: { serverId: viewer.target.serverId, sessionId: runId }
          });
          url = attachment.url;
          token = attachment.token;
        } else {
          const local = await ensureBealeAppServerRunning();
          if (viewer.stopped) break;
          await invokeAppServerOperation({ operation: 'fleet.connect', input: { runId, machineId: viewer.target.machineId, proxy: true } });
          if (viewer.stopped) break;
          const attachment = await attachAppServerSession(local, runId);
          url = attachment.url.replace(/\/transport$/u, '/browser');
          if (url === attachment.url) throw new Error('Fleet browser session transport is unavailable.');
          token = attachment.token;
        }
        if (viewer.stopped) break;
        const socket = new WebSocket(url, { headers: { authorization: `Bearer ${token}` }, maxPayload: 1_048_576 });
        viewer.socket = socket;
        if (viewer.stopped) { socket.close(); break; }
        await new Promise<void>((resolveClosed, rejectOpen) => {
          let opened = false;
          socket.once('open', () => { opened = true; attempts = 0; if (viewer.stopped) socket.close(); });
          socket.on('message', (raw) => {
            if (viewer.stopped || this.viewers.get(runId) !== viewer) return;
            let value: unknown;
            try { value = JSON.parse(String(raw)); } catch { return; }
            if (!value || typeof value !== 'object' || Array.isArray(value) || !('type' in value)) return;
            this.onUpdate({ ...(value as Omit<FleetBrowserUpdate, 'runId'>), runId } as FleetBrowserUpdate);
          });
          socket.once('error', (error) => { if (!opened) rejectOpen(error); });
          socket.once('close', () => { if (opened) resolveClosed(); else rejectOpen(new Error('Fleet browser connection closed.')); });
        });
      } catch (error) {
        if (!viewer.stopped) this.onUpdate({ runId, type: 'browser.error', message: error instanceof Error ? error.message : String(error) });
      } finally {
        viewer.socket = null;
      }
      if (viewer.stopped) break;
      this.onUpdate({ runId, type: 'browser.disconnected' });
      attempts += 1;
      await new Promise((resolveDelay) => setTimeout(resolveDelay, Math.min(10_000, 500 * 2 ** Math.min(attempts, 5))));
    }
  }
}
