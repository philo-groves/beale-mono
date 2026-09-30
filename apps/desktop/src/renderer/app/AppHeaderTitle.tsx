import { memo } from 'react';
import type { JSX } from 'react';
import { Archive, CalendarClock, FileText, Folder, Palette, Plug, ServerCog, Settings, SlidersHorizontal, Ticket, UserRoundCog, Wifi } from 'lucide-react';
import type { RunRecord } from '@shared/types';
import { displaySessionTitle } from '../../shared/sessionTitle';
import { useDevRenderProbe } from '../devInstrumentation';
import { displayTopicTitle, displayWorkspaceHeaderName } from '../view-models/appHeader';

export type AppHeaderViewIcon =
  | 'automations'
  | 'plugins'
  | 'reporting'
  | 'settings'
  | 'settings-archive'
  | 'settings-appearance'
  | 'settings-optional-features'
  | 'settings-profiles'
  | 'settings-providers'
  | 'settings-remote'
  | 'settings-ticketing';

export interface AppHeaderRun {
  run: Pick<RunRecord, 'id' | 'title' | 'promptMarkdown'>;
}

export const AppHeaderTitle = memo(function AppHeaderTitle({
  workspaceName,
  workspaceViewTitle,
  detail,
  topicTitle,
  onOpenSessionOverview
}: {
  workspaceName: string;
  workspaceViewTitle?: string | null;
  detail: AppHeaderRun | null;
  topicTitle?: string | null;
  onOpenSessionOverview?: () => void;
}): JSX.Element {
  const workspaceLabel = displayWorkspaceHeaderName(workspaceName);
  const workspaceViewLabel = workspaceViewTitle?.trim() || null;
  const sessionTitle = !workspaceViewLabel && detail
    ? displaySessionTitle(detail.run.title, detail.run.promptMarkdown)
    : null;
  const topicLabel = !workspaceViewLabel && topicTitle
    ? displayTopicTitle(topicTitle)
    : null;
  const headerSegments = [
    workspaceLabel,
    ...(workspaceViewLabel ? [workspaceViewLabel] : []),
    ...(sessionTitle ? [sessionTitle] : []),
    ...(topicLabel ? [topicLabel] : [])
  ];
  useDevRenderProbe('appHeaderTitle', () => ({
    workspace: workspaceLabel,
    run: detail?.run.id ?? 'none',
    topic: topicLabel ?? 'none'
  }));

  return (
    <div className="app-header-title" aria-label={headerSegments.join(', ')}>
      <div className="app-header-identity">
        <span className="app-header-divider" aria-hidden="true" />
        <span className="app-header-workspace-title app-header-static-title" title={workspaceLabel}>
          <Folder className="app-header-view-icon" size={15} aria-hidden="true" />
          <span>{workspaceLabel}</span>
        </span>
        {workspaceViewLabel ? (
          <span className="app-header-session-title app-header-static-title" title={workspaceViewLabel}>
            <span>{workspaceViewLabel}</span>
          </span>
        ) : null}
        {detail && sessionTitle ? (
          <>
            {onOpenSessionOverview ? (
              <button
                aria-label={`Open Session Overview for ${sessionTitle}`}
                className="app-header-session-title app-header-session-overview-button"
                onClick={onOpenSessionOverview}
                title={sessionTitle}
                type="button"
              >
                <span>{sessionTitle}</span>
              </button>
            ) : (
              <span className="app-header-session-title app-header-static-title" title={sessionTitle}>
                <span>{sessionTitle}</span>
              </span>
            )}
          </>
        ) : null}
        {topicLabel ? (
          <span className="app-header-topic-title app-header-static-title" title={topicLabel}>
            <span>{topicLabel}</span>
          </span>
        ) : null}
      </div>
    </div>
  );
});

export const StaticAppHeaderTitle = memo(function StaticAppHeaderTitle({
  primaryTitle,
  secondaryTitle,
  icon
}: {
  primaryTitle: string;
  secondaryTitle: string;
  icon: AppHeaderViewIcon;
}): JSX.Element {
  const HeaderIcon = {
    automations: CalendarClock,
    plugins: Plug,
    reporting: FileText,
    settings: Settings,
    'settings-archive': Archive,
    'settings-appearance': Palette,
    'settings-optional-features': SlidersHorizontal,
    'settings-profiles': UserRoundCog,
    'settings-providers': ServerCog,
    'settings-remote': Wifi,
    'settings-ticketing': Ticket
  }[icon];

  return (
    <div className="app-header-title" aria-label={`${primaryTitle}, ${secondaryTitle}`}>
      <div className="app-header-identity">
        <span className="app-header-divider" aria-hidden="true" />
        <span className="app-header-workspace-title app-header-static-title">
          <HeaderIcon className="app-header-view-icon" size={15} aria-hidden="true" />
          <span>{primaryTitle}</span>
        </span>
        <span className="app-header-session-title app-header-static-title">
          <span>{secondaryTitle}</span>
        </span>
      </div>
    </div>
  );
});
