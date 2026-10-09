import { useEffect, useState } from 'react';
import type { JSX, PointerEvent as ReactPointerEvent, ReactNode } from 'react';
import type { FleetRemoteCatalog } from '@beale/app-server-runtime/protocol';
import { MainSideScrollRegion } from '../../app/MainSideScrollRegion';
import { FleetBrowserSideView } from '../research/FleetBrowserSideView';

export function RemoteFleetSidebar({ catalog, serverName, selectedWorkspaceId, selectedSessionId, automations, collapsed, serverSelector, onSelect, onResizePointerDown }: {
  catalog: FleetRemoteCatalog | null;
  serverName: string;
  selectedWorkspaceId: string | null;
  selectedSessionId: string | null;
  automations: boolean;
  collapsed: boolean;
  serverSelector: ReactNode;
  onSelect: (workspaceId: string, sessionId: string | null) => void;
  onResizePointerDown: (event: ReactPointerEvent<HTMLDivElement>) => void;
}): JSX.Element {
  const sessions = catalog?.sessions.filter((session) => !automations || session.automation) ?? [];
  return <aside className="sidebar collection-sidebar" aria-label={`${serverName} ${automations ? 'automations' : 'workspaces'} sidebar`} aria-hidden={collapsed} inert={collapsed}>
    <div className="sidebar-section collection-sidebar-section">
      <div className="sidebar-server-heading"><div className="sidebar-wordmark">{automations ? 'Automations' : 'Beale'}</div>{serverSelector}</div>
      <MainSideScrollRegion className="sidebar-list-scroll-region" listClassName="sidebar-list-scroll" updateKey={`${catalog?.serverId ?? ''}:${selectedWorkspaceId ?? ''}:${selectedSessionId ?? ''}:${automations}`}>
        <nav className="collection-sidebar-list sidebar-list-scroll-content" aria-label="Remote research">
          {!catalog ? <p className="collection-sidebar-empty">Loading remote app server…</p> : null}
          {!automations ? catalog?.workspaces.map((workspace) => <button type="button" className={`workspace-item collection-sidebar-item${selectedWorkspaceId === workspace.workspaceId && !selectedSessionId ? ' active' : ''}`} key={workspace.workspaceId} onClick={() => onSelect(workspace.workspaceId, null)}>{workspace.name}</button>) : null}
          <div className="collection-sidebar-list-heading">{automations ? 'Scheduled research' : 'Sessions'}</div>
          {sessions.length === 0 && catalog ? <p className="collection-sidebar-empty">No {automations ? 'automations' : 'sessions'} found.</p> : null}
          {sessions.map((session) => <button type="button" className={`workspace-item collection-sidebar-item${selectedSessionId === session.id ? ' active' : ''}`} key={`${session.workspaceId}:${session.id}`} onClick={() => onSelect(session.workspaceId, session.id)} title={session.title}>
            <span className="collection-sidebar-item-copy"><span>{session.title}</span><small>{session.status} · {catalog?.workspaces.find((workspace) => workspace.workspaceId === session.workspaceId)?.name ?? session.workspaceId}</small></span>
          </button>)}
        </nav>
      </MainSideScrollRegion>
    </div>
    <div className="sidebar-resize-handle" role="separator" aria-label="Resize sidebar" aria-orientation="vertical" onPointerDown={onResizePointerDown} />
  </aside>;
}

export function RemoteFleetWorkspace({ serverId, serverName, catalog, workspaceId, sessionId, automations, error, onLaunched }: {
  serverId: string;
  serverName: string;
  catalog: FleetRemoteCatalog | null;
  workspaceId: string | null;
  sessionId: string | null;
  automations: boolean;
  error: string | null;
  onLaunched: (workspaceId: string, sessionId: string) => void;
}): JSX.Element {
  const [detail, setDetail] = useState<unknown>(null);
  const [detailError, setDetailError] = useState<string | null>(null);
  const [prompt, setPrompt] = useState('');
  const [machineId, setMachineId] = useState('local');
  const [starting, setStarting] = useState(false);
  const [launchError, setLaunchError] = useState<string | null>(null);
  const [steering, setSteering] = useState('');
  const [controlBusy, setControlBusy] = useState(false);
  const [controlError, setControlError] = useState<string | null>(null);
  const [browserSelection, setBrowserSelection] = useState<string | null>(null);
  const browserOpen = browserSelection === `${serverId}:${sessionId ?? ''}`;
  useEffect(() => { setBrowserSelection(null); }, [serverId, sessionId]);
  useEffect(() => {
    if (!workspaceId || !sessionId) { setDetail(null); return; }
    let active = true;
    const refresh = (): void => {
      void window.beale.getFleetRemoteSession(serverId, workspaceId, sessionId)
        .then((value) => { if (active) { setDetail(value); setDetailError(null); } })
        .catch((caught: unknown) => { if (active) setDetailError(message(caught)); });
    };
    refresh();
    const interval = window.setInterval(refresh, 3_000);
    return () => { active = false; window.clearInterval(interval); };
  }, [serverId, workspaceId, sessionId]);
  const workspace = catalog?.workspaces.find((item) => item.workspaceId === workspaceId);
  const session = catalog?.sessions.find((item) => item.id === sessionId && item.workspaceId === workspaceId);
  const browserAvailable = Boolean(session && !['completed', 'failed', 'stopped'].includes(session.status));
  const required = Boolean(workspaceId && catalog && (catalog.requiredWorkspaceIds.includes(workspaceId)
    || !catalog.optionalWorkspaceIds.includes(workspaceId) && catalog.hasBaseVm));
  const runnable = catalog?.machines.filter((machine) => !machine.owner) ?? [];
  useEffect(() => { setMachineId(required ? runnable[0]?.id ?? '' : 'local'); }, [serverId, workspaceId, required]);
  const eventTexts = sessionEventTexts(detail);
  const start = async (): Promise<void> => {
    if (!workspaceId || !prompt.trim() || starting) return;
    setStarting(true);
    setLaunchError(null);
    try {
      const result = await window.beale.startFleetRemoteSession(serverId, workspaceId, prompt, machineId);
      setPrompt('');
      onLaunched(workspaceId, result.sessionId);
    } catch (caught) { setLaunchError(message(caught)); }
    finally { setStarting(false); }
  };
  const control = async (type: 'pause' | 'resume' | 'stop' | 'steer'): Promise<void> => {
    if (!sessionId || controlBusy) return;
    setControlBusy(true);
    setControlError(null);
    try {
      await window.beale.controlFleetRemoteSession(serverId, sessionId, type, type === 'steer' ? steering : undefined);
      if (type === 'steer') setSteering('');
    } catch (caught) { setControlError(message(caught)); }
    finally { setControlBusy(false); }
  };
  return <section className="remote-fleet-workspace" aria-label={`${serverName} research`}>
    <div className="remote-fleet-content">
      <p className="remote-fleet-eyebrow">{serverName}</p>
      <h1>{session?.title ?? workspace?.name ?? (automations ? 'Automations' : 'Research')}</h1>
      {error ? <p className="settings-form-error" role="alert">{error}</p> : null}
      {session ? <><p>{session.status} · {workspace?.name}</p>{detailError ? <p className="settings-form-error" role="alert">{detailError}</p> : null}
        {session.status !== 'completed' && session.status !== 'failed' && session.status !== 'stopped' ? <div className="remote-fleet-controls">
          <button type="button" className="secondary-button" disabled={controlBusy} onClick={() => void control('pause')}>Pause</button>
          <button type="button" className="secondary-button" disabled={controlBusy} onClick={() => void control('resume')}>Resume</button>
          <button type="button" className="secondary-button" disabled={controlBusy} onClick={() => void control('stop')}>Stop</button>
          <input aria-label="Steering instruction" value={steering} onChange={(event) => setSteering(event.currentTarget.value)} placeholder="Steer this session" />
          <button type="button" className="secondary-button" disabled={controlBusy || !steering.trim()} onClick={() => void control('steer')}>Send</button>
        </div> : null}
        {controlError ? <p className="settings-form-error" role="alert">{controlError}</p> : null}
        {browserAvailable ? <div className="remote-fleet-view-switch" aria-label="Remote session view">
          <button type="button" className="secondary-button" aria-pressed={!browserOpen} onClick={() => setBrowserSelection(null)}>Commentary</button>
          <button type="button" className="secondary-button" aria-pressed={browserOpen} onClick={() => setBrowserSelection(`${serverId}:${sessionId ?? ''}`)}>Browser</button>
        </div> : null}
        {browserAvailable && browserOpen && sessionId ? <div className="remote-fleet-browser">
          <FleetBrowserSideView runId={sessionId} remoteServerId={serverId} visible />
        </div> : eventTexts.length ? <div className="remote-fleet-events">{eventTexts.map((value, index) => <article key={`${index}:${value.slice(0, 20)}`}><pre>{value}</pre></article>)}</div> : <p>{detail ? 'No commentary yet.' : 'Loading session…'}</p>}
      </> : workspace && !automations ? <><p>{workspace.runCount} sessions on this app server.</p>
        <label className="fleet-vm-dialog-field"><span>New Research prompt</span><textarea value={prompt} onChange={(event) => setPrompt(event.currentTarget.value)} rows={7} placeholder="Describe the authorized research task." /></label>
        <label className="fleet-vm-dialog-field"><span>Machine</span><select value={machineId} onChange={(event) => setMachineId(event.currentTarget.value)}>
          {!required ? <option value="local">Local</option> : null}
          {required && runnable.length === 0 ? <option value="">No configured base VM</option> : null}
          {runnable.map((machine) => <option key={machine.id} value={machine.id}>{machine.name} ({machine.sshConfigured ? 'clone for session' : 'set SSH user in Fleet'})</option>)}
        </select></label>
        {launchError ? <p className="settings-form-error" role="alert">{launchError}</p> : null}
        <button type="button" className="primary-button" disabled={starting || !prompt.trim() || !machineId} onClick={() => void start()}>{starting ? 'Starting…' : 'Start Research'}</button>
      </> : <p>{catalog ? (automations ? 'Choose an automation.' : 'Choose a workspace or session.') : 'Loading remote app server…'}</p>}
    </div>
  </section>;
}

function sessionEventTexts(value: unknown): string[] {
  if (!record(value) || !record(value.result) || !Array.isArray(value.result.events)) return [];
  return value.result.events.flatMap((event: unknown) => {
    if (!record(event) || !record(event.payload) || !record(event.payload.record)) return [];
    const text = event.payload.record.contentMarkdown;
    return typeof text === 'string' && text.trim() ? [text] : [];
  });
}
function record(value: unknown): value is Record<string, unknown> { return typeof value === 'object' && value !== null && !Array.isArray(value); }
function message(error: unknown): string { return error instanceof Error ? error.message : String(error); }
