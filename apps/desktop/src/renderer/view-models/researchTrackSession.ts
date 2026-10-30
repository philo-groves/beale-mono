import type { BealeWorkspaceResearchTrack } from '@beale/app-server-runtime/protocol';
import type { BealeApi, RunDetail } from '@shared/types';

export async function pauseResearchTrackSessionAction(input: {
  action: string;
  workspaceId: string;
  sessionId: string;
  serverId?: string;
  api: Pick<BealeApi, 'getResearchTrack' | 'controlResearchTrack'>;
}): Promise<boolean> {
  if ((input.action !== 'pause' && input.action !== 'stop') || !input.sessionId.startsWith('track_')) return false;
  const track = await input.api.getResearchTrack(input.workspaceId, input.serverId);
  if (track?.id !== input.sessionId) return true;
  if (track.status === 'running') await input.api.controlResearchTrack(input.workspaceId, 'pause', input.serverId);
  return true;
}

export function runningResearchTrackDetail(detail: RunDetail | null, running: boolean): RunDetail | null {
  if (!detail || !running || detail.run.status === 'active' || detail.run.status === 'failed') return detail;
  return { ...detail, run: { ...detail.run, status: 'active', endedAt: null } };
}

export function deferredResearchTrackDetail(latest: RunDetail | null, deferred: RunDetail | null): RunDetail | null {
  if (!latest || deferred?.run.id !== latest.run.id) return latest;
  // Epochs reuse a run ID. Keep the latest run state even when the larger
  // transcript projection is deferred behind a stream of updates.
  return deferred.run === latest.run ? deferred : { ...deferred, run: latest.run };
}

export async function waitForResearchTrackSession<T>(input: {
  trackId: string;
  initialSessionId: string | null;
  signal: AbortSignal;
  getTrack: () => Promise<BealeWorkspaceResearchTrack | null>;
  findSession: (sessionId: string) => Promise<T | null>;
  wait?: (signal: AbortSignal) => Promise<void>;
}): Promise<T | null> {
  let sessionId = input.initialSessionId;
  let consecutiveFailures = 0;
  let inactiveChecks = 0;
  while (!input.signal.aborted) {
    try {
      const track = await input.getTrack();
      if (input.signal.aborted || !track || track.id !== input.trackId) return null;
      sessionId = track.activeSessionId ?? sessionId;
      if (sessionId) {
        const session = await input.findSession(sessionId);
        if (input.signal.aborted) return null;
        if (session) return session;
      }
      if (track.status !== 'running') {
        if (!sessionId || ++inactiveChecks >= 5) return null;
      } else {
        inactiveChecks = 0;
      }
      consecutiveFailures = 0;
    } catch (error) {
      if (++consecutiveFailures >= 3) throw error;
    }
    await (input.wait ?? waitForNextCheck)(input.signal);
  }
  return null;
}

function waitForNextCheck(signal: AbortSignal): Promise<void> {
  if (signal.aborted) return Promise.resolve();
  return new Promise((resolve) => {
    const finish = (): void => {
      clearTimeout(timer);
      signal.removeEventListener('abort', finish);
      resolve();
    };
    const timer = setTimeout(finish, 1_500);
    signal.addEventListener('abort', finish, { once: true });
  });
}
