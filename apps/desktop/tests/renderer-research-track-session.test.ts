import { describe, expect, it, vi } from 'vitest';
import type { BealeWorkspaceResearchTrack } from '@beale/app-server-runtime/protocol';
import type { BealeApi, RunDetail } from '../src/shared/types';
import { deferredResearchTrackDetail, pauseResearchTrackSessionAction, runningResearchTrackDetail, waitForResearchTrackSession } from '../src/renderer/view-models/researchTrackSession';

function track(activeSessionId: string | null): BealeWorkspaceResearchTrack {
  return {
    id: 'track_example', workspaceId: 'workspace_example', status: 'running',
    activeSessionId
  } as BealeWorkspaceResearchTrack;
}

describe('research track session handoff', () => {
  it('pauses a local or remote track from the session Stop control', async () => {
    const getResearchTrack = vi.fn(async () => track('session_example'));
    const controlResearchTrack = vi.fn(async () => track('session_example'));
    const api = { getResearchTrack, controlResearchTrack } as unknown as Pick<BealeApi, 'getResearchTrack' | 'controlResearchTrack'>;

    expect(await pauseResearchTrackSessionAction({ action: 'stop', workspaceId: 'workspace_example',
      sessionId: 'track_example', api })).toBe(true);
    expect(controlResearchTrack).toHaveBeenCalledWith('workspace_example', 'pause', undefined);
    expect(getResearchTrack).toHaveBeenCalledWith('workspace_example', undefined);

    expect(await pauseResearchTrackSessionAction({ action: 'stop', workspaceId: 'workspace_example',
      sessionId: 'track_example', serverId: 'server_example', api })).toBe(true);
    expect(getResearchTrack).toHaveBeenCalledWith('workspace_example', 'server_example');
    expect(controlResearchTrack).toHaveBeenLastCalledWith('workspace_example', 'pause', 'server_example');
  });

  it('does not stop an already paused track or intercept an unrelated session', async () => {
    const getResearchTrack = vi.fn(async () => ({ ...track(null), status: 'paused' as const }));
    const controlResearchTrack = vi.fn();
    const api = { getResearchTrack, controlResearchTrack } as unknown as Pick<BealeApi, 'getResearchTrack' | 'controlResearchTrack'>;

    expect(await pauseResearchTrackSessionAction({ action: 'stop', workspaceId: 'workspace_example',
      sessionId: 'track_example', api })).toBe(true);
    expect(controlResearchTrack).not.toHaveBeenCalled();
    expect(await pauseResearchTrackSessionAction({ action: 'stop', workspaceId: 'workspace_example',
      sessionId: 'session_example', api })).toBe(false);
    expect(await pauseResearchTrackSessionAction({ action: 'stop', workspaceId: 'workspace_example',
      sessionId: 'track_previous', api })).toBe(true);
    expect(controlResearchTrack).not.toHaveBeenCalled();
  });

  it('presents a terminal epoch as ongoing while the track is running', () => {
    const detail = { run: { id: 'track_example', status: 'blocked', endedAt: '2026-08-20T12:00:00.000Z' } } as RunDetail;
    const presented = runningResearchTrackDetail(detail, true);

    expect(presented?.run.status).toBe('active');
    expect(presented?.run.endedAt).toBeNull();
    expect(detail.run.status).toBe('blocked');
    expect(runningResearchTrackDetail(detail, false)).toBe(detail);
    expect(runningResearchTrackDetail({ ...detail, run: { ...detail.run, status: 'failed' } }, true)?.run.status)
      .toBe('failed');
  });

  it('keeps the latest epoch status while transcript updates are deferred', () => {
    const deferred = { run: { id: 'track_example', status: 'completed', endedAt: '2026-08-20T12:00:00.000Z' },
      transcriptMessages: [{ id: 'message_previous' }] } as RunDetail;
    const latest = runningResearchTrackDetail({ ...deferred,
      run: { ...deferred.run }, transcriptMessages: [{ id: 'message_current' }] } as RunDetail, true)!;
    const presented = deferredResearchTrackDetail(latest, deferred);

    expect(presented?.run.status).toBe('active');
    expect(presented?.run.endedAt).toBeNull();
    expect(presented?.transcriptMessages).toBe(deferred.transcriptMessages);
    expect(deferredResearchTrackDetail(latest, { ...deferred, run: { ...deferred.run, id: 'other_run' } })).toBe(latest);
  });

  it('waits for a started track to create and expose its session', async () => {
    let checks = 0;
    let resolutions = 0;
    const session = await waitForResearchTrackSession({
      trackId: 'track_example', initialSessionId: null,
      signal: new AbortController().signal,
      getTrack: async () => track(++checks < 2 ? null : 'session_example'),
      findSession: async (sessionId) => ++resolutions < 2 ? null : sessionId,
      wait: async () => undefined
    });

    expect(session).toBe('session_example');
    expect(checks).toBe(3);
  });

  it('opens the existing session on resume and stops if the track is replaced', async () => {
    const resumed = await waitForResearchTrackSession({
      trackId: 'track_example', initialSessionId: 'session_example',
      signal: new AbortController().signal,
      getTrack: async () => track(null),
      findSession: async (sessionId) => sessionId,
      wait: async () => undefined
    });
    const replaced = await waitForResearchTrackSession({
      trackId: 'track_example', initialSessionId: null,
      signal: new AbortController().signal,
      getTrack: async () => ({ ...track(null), id: 'track_replacement' }),
      findSession: async () => 'wrong session',
      wait: async () => undefined
    });

    expect(resumed).toBe('session_example');
    expect(replaced).toBeNull();
  });
});
