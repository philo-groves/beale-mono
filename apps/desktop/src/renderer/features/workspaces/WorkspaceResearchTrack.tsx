import { useEffect, useState, type JSX } from 'react';
import { createPortal } from 'react-dom';
import { ExternalLink, Gauge, LoaderCircle, Pause, Play, Route, Trash2, TriangleAlert } from 'lucide-react';
import type { BealeWorkspaceResearchTrack } from '@beale/app-server-runtime/protocol';
import type { BealeApi, ResearchModelProviderId } from '../../../shared/types';
import { Modal } from '../../app/Modal';

const MODEL_PROVIDERS = new Set<ResearchModelProviderId>(['openai-codex', 'anthropic', 'xai', 'zai', 'openrouter']);

export function researchTrackProviderIds(track: Pick<BealeWorkspaceResearchTrack, 'provider' | 'collaboration'>): ResearchModelProviderId[] {
  const providerIds = new Set<ResearchModelProviderId>();
  if (isResearchModelProviderId(track.provider.id)) providerIds.add(track.provider.id);
  const collaborators: unknown = track.collaboration?.providers;
  for (const collaborator of Array.isArray(collaborators) ? collaborators as unknown[] : []) {
    if (!isRecord(collaborator)) continue;
    if (collaborator.enabled !== false && isResearchModelProviderId(collaborator.provider)) {
      providerIds.add(collaborator.provider);
    }
  }
  return [...providerIds];
}

function isResearchModelProviderId(value: unknown): value is ResearchModelProviderId {
  return typeof value === 'string' && MODEL_PROVIDERS.has(value as ResearchModelProviderId);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export async function resumeResearchTrack(
  track: BealeWorkspaceResearchTrack,
  serverId?: string,
  api: Pick<BealeApi, 'getProviderCredentialAccessRequest' | 'unlockProviderApiKeys' | 'controlResearchTrack'> = window.beale
): Promise<BealeWorkspaceResearchTrack> {
  if (!serverId) {
    const access = await api.getProviderCredentialAccessRequest(researchTrackProviderIds(track));
    if (access.providerIds.length > 0) await api.unlockProviderApiKeys(access.providerIds);
  }
  return api.controlResearchTrack(track.workspaceId, 'resume', serverId);
}

export function WorkspaceResearchTrack({ workspaceId, serverId, enabled, onOpenSession, onStartTrack, onTrackResumed }: {
  workspaceId: string;
  serverId?: string;
  enabled: boolean;
  onOpenSession?: (sessionId: string) => void;
  onStartTrack?: () => void;
  onTrackResumed?: (track: BealeWorkspaceResearchTrack) => void;
}): JSX.Element {
  const [track, setTrack] = useState<BealeWorkspaceResearchTrack | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [progressOpen, setProgressOpen] = useState(false);
  const [confirmStopOpen, setConfirmStopOpen] = useState(false);
  const [observedSessionId, setObservedSessionId] = useState<string | null>(null);
  useEffect(() => {
    if (!workspaceId) { setLoading(false); return undefined; }
    let active = true;
    const refresh = (): void => {
      void window.beale.getResearchTrack(workspaceId, serverId)
        .then((value) => { if (active) { setTrack(value);
          if (value?.activeSessionId && value.activeSessionPhase === 'researching') setObservedSessionId(value.activeSessionId);
          setLoading(false); setError(null); } })
        .catch((caught: unknown) => { if (active) { setLoading(false); setError(message(caught)); } });
    };
    setTrack(null);
    setObservedSessionId(null);
    setLoading(true);
    refresh();
    const timer = window.setInterval(refresh, 5_000);
    return () => { active = false; window.clearInterval(timer); };
  }, [workspaceId, serverId, enabled]);
  const control = async (action: 'start' | 'pause' | 'resume' | 'stop'): Promise<void> => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const value = action === 'resume' && track
        ? await resumeResearchTrack(track, serverId)
        : await window.beale.controlResearchTrack(workspaceId, action, serverId);
      setTrack(value);
      if (action === 'stop') setConfirmStopOpen(false);
      if (action === 'resume') onTrackResumed?.(value);
      if (value.activeSessionId && value.activeSessionPhase === 'researching') setObservedSessionId(value.activeSessionId); }
    catch (caught) { setError(message(caught)); }
    finally { setBusy(false); }
  };
  const status = track?.status;
  const total = track?.surfaces.filter((surface) => surface.status !== 'excluded').length ?? 0;
  const complete = track?.surfaces.filter((surface) => surface.status === 'complete').length ?? 0;
  const investigated = track?.surfaces.filter((surface) => surface.status !== 'excluded'
    && (surface.epochs > 0 || surface.status === 'complete')).length ?? 0;
  const active = track?.surfaces.find((surface) => surface.surfaceId === track.activeSurfaceId);
  const scheduledRevisit = track?.surfaces.some((surface) => surface.revisitAfter && Date.parse(surface.revisitAfter) > Date.now());
  const action = !enabled ? null : !track || status === 'complete' || status === 'stopped' ? 'start'
    : status === 'paused' || status === 'waiting' ? 'resume' : null;
  const label = action === 'start' ? 'Start Track' : 'Resume Track';
  const phase = track?.activeSessionPhase ?? (active ? 'researching' : track?.epochCount ? 'switching' : 'selecting');
  const progress = status === 'running'
    ? phase === 'preparing' ? `Preparing ${active?.name ?? 'research session'}`
      : phase === 'switching' ? 'Preparing next surface'
        : phase === 'selecting' ? scheduledRevisit ? 'Second look scheduled' : 'Selecting next surface'
          : `Researching ${active?.name ?? 'assigned surface'}`
    : null;
  const statusTitle = track ? `${progress ?? status ?? 'Research track'} · ${investigated} of ${total} investigated · ${complete} complete` : undefined;
  const visibleError = error ?? (status === 'stopped' ? null : track?.lastError);
  const progressLabel = loading ? 'Loading track progress' : statusTitle ?? 'Track not started';
  return <div className="workspace-research-track" role="group" aria-label="Research track controls">
    <span className="workspace-research-track-label">
      <Route size={14} aria-hidden="true" /><span>Track{status === 'paused' ? ' · Paused' : status === 'waiting' ? ' · Waiting' : ''}</span>
      {visibleError ? <span className="workspace-research-track-warning" role="alert" aria-label={`Track error: ${visibleError}`} tabIndex={0}>
        <TriangleAlert size={11} aria-hidden="true" />
        <span className="workspace-research-track-error" role="tooltip">{visibleError}</span>
      </span> : null}
    </span>
    <div className="workspace-research-track-detail"
      onMouseEnter={() => setProgressOpen(true)}
      onMouseLeave={() => setProgressOpen(false)}
      onFocus={() => setProgressOpen(true)}
      onBlur={(event) => { if (!event.currentTarget.contains(event.relatedTarget)) setProgressOpen(false); }}
      onKeyDown={(event) => { if (event.key === 'Escape') setProgressOpen(false); }}>
      <button className="workspace-research-track-icon-button" type="button" aria-label={`Progress: ${progressLabel}`}
        aria-expanded={progressOpen} aria-controls={progressOpen ? 'workspace-research-track-progress' : undefined} onClick={() => setProgressOpen(true)}>
        <Gauge size={16} aria-hidden="true" />
      </button>
      {progressOpen ? <div className="workspace-research-track-surface-list" id="workspace-research-track-progress" role="region" aria-label="Track progress" tabIndex={0}>
        <strong>Progress</strong>
        <span>{progressLabel}</span>
        {status === 'paused' && track?.lastError ? <span>Pause reason: {track.lastError}</span> : null}
        {status === 'paused' && !track?.lastError ? <span>Paused with track controls. Resume Track to continue research.</span> : null}
        {track ? <>
          <span>Pass {track.pass} · {investigated} of {total} investigated · {complete} complete</span>
          <small>Discovery favors recorded high-value signals and strong impact with reachable proof. Productive engagements continue; two evidence-free checkpoints cool a component.</small>
          {track.surfaces.filter((surface) => surface.status !== 'excluded').map((surface) => <div key={surface.surfaceId} className="workspace-research-track-surface">
            <strong>{surface.name}</strong><span>{surface.status.replaceAll('_', ' ')} · prior impact {surface.impact} · {surface.epochs} epochs · {surface.noProgressEpochs} evidence-free{surface.validation ? ` · validation ${surface.validation.environment}` : ''}</span>
            {surface.revisitAfter ? <small>Cooling until {new Date(surface.revisitAfter).toLocaleString()}</small> : null}
            {surface.angles.length > 0 ? <small>Attacker paths: {surface.angles.map((angle) => `${angle.name} (${angle.status.replaceAll('_', ' ')})`).join(' · ')}</small> : null}
            {surface.nextAction ? <small>{surface.nextAction}</small> : null}
          </div>)}
        </> : null}
      </div> : null}
    </div>
    {action ? <button className="workspace-research-track-icon-button" type="button" disabled={loading || busy}
      aria-label={busy ? 'Working on track' : label} title={busy ? 'Working…' : label}
      onClick={() => action === 'start' && onStartTrack ? onStartTrack() : void control(action)}>
      {busy ? <LoaderCircle className="workspace-research-track-spinner" size={16} aria-hidden="true" /> : <Play size={16} aria-hidden="true" />}
    </button> : null}
    {status === 'running' ? <button className="workspace-research-track-icon-button" type="button" disabled={busy}
      aria-label="Pause Track" title="Pause Track" onClick={() => void control('pause')}><Pause size={16} aria-hidden="true" /></button> : null}
    {track && status !== 'complete' && status !== 'stopped' ? <button className="workspace-research-track-icon-button" type="button"
      disabled={busy} aria-label="Cancel Track" title="Cancel Track" onClick={() => setConfirmStopOpen(true)}><Trash2 size={16} aria-hidden="true" /></button> : null}
    {status !== 'running' && onOpenSession && observedSessionId ? <button className="workspace-research-track-icon-button" type="button"
      aria-label="View Track session" title="View Track session" onClick={() => onOpenSession(observedSessionId)}><ExternalLink size={16} aria-hidden="true" /></button> : null}
    {confirmStopOpen ? createPortal(<Modal title="Cancel research track?" closeDisabled={busy}
      onClose={() => setConfirmStopOpen(false)} footer={<>
        <button className="secondary-button" type="button" disabled={busy} onClick={() => setConfirmStopOpen(false)}>Keep Track</button>
        <button className="primary-button workspace-research-track-stop-confirm" type="button" disabled={busy}
          onClick={() => void control('stop')}>Cancel Track</button>
      </>}><p>Canceling ends this track. You cannot resume it, and unfinished progress in the current research session may be lost. Pause the track to continue later.</p>
      {error ? <p role="alert" className="form-error">{error}</p> : null}</Modal>, document.body) : null}
  </div>;
}

function message(error: unknown): string { return error instanceof Error ? error.message : String(error); }
