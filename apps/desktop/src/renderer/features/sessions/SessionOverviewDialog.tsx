import { useEffect, useMemo, useState } from 'react';
import type { JSX } from 'react';
import type { AppServerMemorySummary, ResearchProfileMemoryType, RunRow } from '@shared/types';
import { displaySessionTitle } from '../../../shared/sessionTitle';
import { Modal } from '../../app/Modal';
import { buildSessionTimelineProjection, formatWorkspaceTimelineDuration } from '../../view-models/workspaceTimeline';
import type { SessionHeatPreferences } from '../../view-models/sessionHeat';
import { CampaignSessionProjection } from '../workspaces/CampaignGraphView';

export function SessionOverviewDialog({
  memory,
  memoryTypes,
  nowMs,
  onClose,
  profileId,
  run,
  sessionHeatPreferences
}: {
  memory: AppServerMemorySummary | null;
  memoryTypes: readonly ResearchProfileMemoryType[];
  nowMs?: number;
  onClose: () => void;
  profileId?: string;
  run: RunRow;
  sessionHeatPreferences: SessionHeatPreferences;
}): JSX.Element {
  const [clockNowMs, setClockNowMs] = useState(() => nowMs ?? Date.now());
  useEffect(() => {
    if (nowMs !== undefined) return undefined;
    const timer = window.setInterval(() => setClockNowMs(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, [nowMs]);
  const timelineNowMs = nowMs ?? clockNowMs;
  const projection = useMemo(() => buildSessionTimelineProjection(
    run,
    memory?.nodes ?? [],
    memory?.runbooks ?? [],
    memory?.reports ?? [],
    memoryTypes,
    timelineNowMs
  ), [memory?.nodes, memory?.reports, memory?.runbooks, memoryTypes, run, timelineNowMs]);
  const sessionTitle = displaySessionTitle(run.run.title, run.run.promptMarkdown);
  const durationLabel = projection.totalDurationMs > 0
    ? formatWorkspaceTimelineDuration(projection.totalDurationMs)
    : 'No activity recorded';

  return (
    <Modal className="session-overview-dialog" title="Session Overview" wide onClose={onClose}>
      <section className="session-overview-content" aria-label={`${sessionTitle} overview`}>
        <header className="session-overview-summary">
          <h3>{sessionTitle}</h3>
          <p>{durationLabel}</p>
        </header>
        <CampaignSessionProjection
          memoryTypes={memoryTypes}
          profileId={profileId}
          projection={projection}
          sessionHeatPreferences={sessionHeatPreferences}
          sessionTitle={sessionTitle}
        />
      </section>
    </Modal>
  );
}
