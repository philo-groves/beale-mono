import { useEffect, useState } from 'react';
import type { JSX } from 'react';
import type { BealeWorkspaceResearchTrack } from '@beale/app-server-runtime/protocol';
import type { RunStatus, TraceEventRecord } from '@shared/types';
import { ChevronRight } from 'lucide-react';
import { Modal } from '../../app/Modal';

type Worker = NonNullable<BealeWorkspaceResearchTrack['workers']>[number];
type WorkerGroup = { role: Worker['role']; workers: Worker[]; queued: Worker['queued'] };

const ROLE_LABELS: Record<Worker['role'], string> = {
  investigator: 'Investigator',
  chain_analyst: 'Chain Analyst',
  triager: 'Triager',
  reporter: 'Reporter'
};

function workerActivity(event: TraceEventRecord): Record<string, unknown> | null {
  const direct = event.payload;
  if (direct.type === 'subagent.activity') return direct;
  const nested = direct.payload;
  return nested && typeof nested === 'object' && !Array.isArray(nested)
    && (nested as Record<string, unknown>).type === 'subagent.activity'
    ? nested as Record<string, unknown> : null;
}

function workerRole(value: unknown): Worker['role'] | null {
  return typeof value === 'string' && Object.prototype.hasOwnProperty.call(ROLE_LABELS, value)
    ? value as Worker['role'] : null;
}

/** Live lifecycle events keep cards visible while the track status projection catches up. */
export function activeTrackWorkersFromEvents(
  events: readonly TraceEventRecord[],
  findings: readonly { id: string; title: string }[] = [],
  activeAttemptId?: string | null
): Worker[] {
  const titles = new Map(findings.map((finding) => [finding.id, finding.title]));
  const latest = new Map<string, { sequence: number; role: Worker['role']; claimId: string | null;
    title: string; assignedAt: string; status: string }>();
  for (const event of events) {
    if (activeAttemptId && event.attemptId !== activeAttemptId) continue;
    const activity = workerActivity(event);
    if (!activity) continue;
    const agentPath = typeof activity.agentPath === 'string' ? activity.agentPath : null;
    if (!agentPath) continue;
    const prior = latest.get(agentPath);
    const role = workerRole(activity.role) ?? prior?.role ?? null;
    if (!role) continue;
    if (prior && event.sequence < prior.sequence) continue;
    const claimId = typeof activity.trackClaimId === 'string' ? activity.trackClaimId : prior?.claimId ?? null;
    const action = typeof activity.action === 'string' ? activity.action : '';
    const status = ['completed', 'interrupted', 'errored'].includes(action) ? action
      : typeof activity.status === 'string' ? activity.status : 'running';
    const message = typeof activity.message === 'string' ? activity.message.split('\n')[0]?.trim() : '';
    latest.set(agentPath, {
      sequence: event.sequence, role, claimId,
      title: claimId ? titles.get(claimId) ?? 'Finding under review' : message || `${ROLE_LABELS[role]} in progress`,
      assignedAt: prior?.assignedAt ?? event.createdAt,
      status
    });
  }
  return [...latest.entries()].flatMap(([agentPath, activity]) =>
    activity.status === 'pending' || activity.status === 'running'
      ? [{ role: activity.role, claimId: activity.claimId ?? `agent:${agentPath}`, title: activity.title,
        assignedAt: activity.assignedAt, queued: [] }] : []);
}

export function activeTrackWorkerCards(workers: readonly Worker[], events: readonly TraceEventRecord[]): Worker[] {
  return workers.filter((worker) => {
    const assignedAt = Date.parse(worker.assignedAt);
    let latestSequence = -Infinity;
    let latestAction: unknown = null;
    for (const event of events) {
      const activity = workerActivity(event);
      if (activity?.type !== 'subagent.activity' || activity.trackClaimId !== worker.claimId
        || activity.role !== worker.role
        || (Number.isFinite(assignedAt) && Date.parse(String(activity.timestamp ?? event.createdAt)) < assignedAt)) continue;
      if (event.sequence > latestSequence) {
        latestSequence = event.sequence;
        latestAction = activity.action;
      }
    }
    return latestAction !== 'completed' && latestAction !== 'interrupted'
      && latestAction !== 'errored';
  });
}

export function groupTrackWorkers(workers: readonly Worker[]): WorkerGroup[] {
  const groups = new Map<Worker['role'], WorkerGroup>();
  for (const worker of workers) {
    let group = groups.get(worker.role);
    if (!group) {
      group = { role: worker.role, workers: [], queued: [] };
      groups.set(worker.role, group);
    }
    group.workers.push(worker);
    for (const claim of worker.queued) {
      if (!group.queued.some((queued) => queued.claimId === claim.claimId)) group.queued.push(claim);
    }
  }
  return [...groups.values()];
}

export function TrackWorkerCards({ workspaceId, sessionId, serverId, events, findings = [], activeAttemptId, runStatus }: {
  workspaceId: string; sessionId: string; serverId?: string;
  events: readonly TraceEventRecord[]; findings?: readonly { id: string; title: string }[];
  activeAttemptId?: string | null; runStatus?: RunStatus | null
}): JSX.Element | null {
  const [workers, setWorkers] = useState<Worker[]>([]);
  const [queuedRole, setQueuedRole] = useState<Worker['role'] | null>(null);

  useEffect(() => {
    let active = true;
    let timer: number | null = null;
    setWorkers([]);
    setQueuedRole(null);
    const refresh = async (): Promise<void> => {
      try {
        const track = await window.beale.getResearchTrack(workspaceId, serverId);
        if (active) setWorkers(track?.id === sessionId ? track.workers ?? [] : []);
      } catch {
        // Retain the last confirmed assignment through a transient status failure.
      } finally {
        if (active) timer = window.setTimeout(() => void refresh(), 5_000);
      }
    };
    void refresh();
    return () => {
      active = false;
      if (timer !== null) window.clearTimeout(timer);
    };
  }, [workspaceId, sessionId, serverId]);

  const activeWorkersByKey = new Map<string, Worker>();
  const sessionCanHaveActiveWorkers = runStatus === 'queued' || runStatus === 'active';
  for (const worker of sessionCanHaveActiveWorkers
    ? activeTrackWorkersFromEvents(events, findings, activeAttemptId) : []) {
    activeWorkersByKey.set(`${worker.role}:${worker.claimId}`, worker);
  }
  for (const worker of sessionCanHaveActiveWorkers ? activeTrackWorkerCards(workers, events) : []) {
    activeWorkersByKey.set(`${worker.role}:${worker.claimId}`, worker);
  }
  const activeWorkers = [...activeWorkersByKey.values()];
  const groups = groupTrackWorkers(activeWorkers);
  const selected = groups.find((group) => group.role === queuedRole) ?? null;
  if (groups.length === 0) return null;

  return (
    <>
      <section className="track-worker-cards" aria-label="Active track workers" aria-live="polite">
        {groups.map((group) => <TrackWorkerCard key={group.role}
          group={group} onShowQueue={() => setQueuedRole(group.role)} />)}
      </section>
      {selected ? <Modal title={`${ROLE_LABELS[selected.role]} queued findings`} onClose={() => setQueuedRole(null)}>
        {selected.queued.length > 0 ? <ol className="track-worker-queue-list">
          {selected.queued.map((claim) => <li key={claim.claimId}>{claim.title}</li>)}
        </ol> : <p>No findings are queued for this role.</p>}
      </Modal> : null}
    </>
  );
}

export function TrackWorkerCard({ group, onShowQueue }: { group: WorkerGroup; onShowQueue: () => void }): JSX.Element {
  const workerLabel = `${group.workers.length} ${ROLE_LABELS[group.role]}${group.workers.length === 1 ? '' : 's'}`;
  return <article className="session-summary-card track-worker-card">
    <header className="session-summary-heading">
      <h2 className="session-summary-title">{workerLabel}</h2>
      <button type="button" className="track-worker-queued" onClick={onShowQueue}
        aria-label={`Show ${group.queued.length} queued findings for ${workerLabel}`}>
        <span>{group.queued.length} Queued</span><ChevronRight size={15} aria-hidden="true" />
      </button>
    </header>
    <ul className="track-worker-findings">
      {group.workers.map((worker) => <li key={`${worker.claimId}:${worker.assignedAt}`}>
        <p className="track-worker-finding" title={worker.title}>{worker.title}</p>
      </li>)}
    </ul>
  </article>;
}
