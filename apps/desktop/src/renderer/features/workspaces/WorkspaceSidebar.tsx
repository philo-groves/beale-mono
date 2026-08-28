import { memo, useEffect, useRef, useState } from 'react';
import type { JSX, PointerEvent as ReactPointerEvent } from 'react';
import { Archive, CalendarClock, FileText, Folder, FolderInput, FolderPlus, Hash, LoaderCircle, Plus, Plug, RefreshCw, Search, SquarePen, X, Zap } from 'lucide-react';
import type { WorkspaceRegistryEntry, WorkspaceRegistryState, ResearchChannelSummary, ResearchSessionSummary, RunStatus, WorkspaceSnapshot } from '@shared/types';
import { MainSideScrollRegion } from '../../app/MainSideScrollRegion';
import { useDevRenderProbe } from '../../devInstrumentation';
import { canonicalResearchChannelName, normalizeResearchChannelNameDraft } from '../../view-models/researchChannels';
import { promptSessionTitle, researchSessionsForWorkspace, shortRelativeAge } from '../../view-models/workspaceDisplay';

const SIDEBAR_SESSION_LIMIT = 4;
const SIDEBAR_CHANNEL_LIMIT = 4;

export const WorkspaceSidebar = memo(function WorkspaceSidebar({
  busy,
  collapsed,
  error,
  workspaceRegistry,
  workspaceRegistryLoading = false,
  selectedRunId,
  workspaceCreationActive = false,
  newResearchActive = false,
  automationsActive = false,
  reportsActive = false,
  pluginsActive = false,
  snapshot,
  channels = [],
  channelsLoading = false,
  selectedChannelId = null,
  onAddWorkspace,
  onImportWorkspace,
  onOpenWorkspace,
  onOpenResearchSession,
  onOpenChannel = () => undefined,
  onArchiveSession = async () => undefined,
  onArchiveChannel = async () => undefined,
  onCreateChannel = async () => undefined,
  onResizePointerDown,
  onOpenAutomations = () => undefined,
  onOpenReports = () => undefined,
  onOpenPlugins = () => undefined,
  onStartNewResearch,
  onOpenQuickChat = () => undefined,
  onStartNewResearchForWorkspace
}: {
  busy: boolean;
  collapsed: boolean;
  error: string | null;
  workspaceRegistry: WorkspaceRegistryState | null;
  workspaceRegistryLoading?: boolean;
  selectedRunId: string | null;
  workspaceCreationActive?: boolean;
  newResearchActive?: boolean;
  automationsActive?: boolean;
  reportsActive?: boolean;
  pluginsActive?: boolean;
  snapshot: WorkspaceSnapshot | null;
  channels?: ResearchChannelSummary[];
  channelsLoading?: boolean;
  selectedChannelId?: string | null;
  onAddWorkspace: () => void;
  onImportWorkspace: () => void;
  onOpenWorkspace: (workspace: WorkspaceRegistryEntry) => void;
  onOpenResearchSession: (workspace: WorkspaceRegistryEntry, session: ResearchSessionSummary) => void;
  onOpenChannel?: (channel: ResearchChannelSummary) => void;
  onArchiveSession?: (session: ResearchSessionSummary) => Promise<void>;
  onArchiveChannel?: (channel: ResearchChannelSummary) => Promise<void>;
  onCreateChannel?: (input: { name: string; topic: string }) => Promise<void>;
  onResizePointerDown: (event: ReactPointerEvent<HTMLDivElement>) => void;
  onOpenAutomations?: () => void;
  onOpenReports?: () => void;
  onOpenPlugins?: () => void;
  onStartNewResearch: () => void;
  onOpenQuickChat?: () => void;
  onStartNewResearchForWorkspace: (workspace: WorkspaceRegistryEntry) => void;
}): JSX.Element {
  useDevRenderProbe('sidebar.workspaces', () => ({
    collapsed,
    workspaces: workspaceRegistry?.workspaces.length ?? 0,
    sessions: workspaceRegistry?.researchSessions.length ?? 0
  }));
  const workspaces = workspaceRegistry?.workspaces ?? [];
  const presentation = snapshot?.researchProfile?.profile.presentation;
  const newResearchLabel = presentation?.newResearchLabel ?? 'New Research';
  const sessionLabel = presentation?.sessionLabel ?? 'Session';
  const workspaceNoun = snapshot?.researchProfile?.profile.workspace.workspaceNoun ?? 'Research Workspace';
  const [expandedWorkspaceIds, setExpandedWorkspaceIds] = useState<Set<string>>(() => new Set());
  const [sessionSearchOpen, setSessionSearchOpen] = useState(false);
  const [sessionSearchQuery, setSessionSearchQuery] = useState('');
  const [workspaceAddMenuOpen, setWorkspaceAddMenuOpen] = useState(false);
  const [activeList, setActiveList] = useState<'workspaces' | 'channels'>(() => selectedChannelId ? 'channels' : 'workspaces');
  const [channelSearchQuery, setChannelSearchQuery] = useState('');
  const [channelCreateOpen, setChannelCreateOpen] = useState(false);
  const [channelName, setChannelName] = useState('');
  const [channelTopic, setChannelTopic] = useState('');
  const [channelCreating, setChannelCreating] = useState(false);
  const [channelsExpanded, setChannelsExpanded] = useState(false);
  const workspaceAddMenuRef = useRef<HTMLDivElement | null>(null);
  const normalizedSessionSearchQuery = sessionSearchQuery.trim();
  const normalizedChannelSearchQuery = channelSearchQuery.trim().toLocaleLowerCase();
  const visibleChannels = normalizedChannelSearchQuery
    ? channels.filter((channel) => [channel.name, channel.title, channel.topic, channel.latestMessagePreview ?? '']
      .join('\n').toLocaleLowerCase().includes(normalizedChannelSearchQuery))
    : channels;
  const filteringChannels = normalizedChannelSearchQuery.length > 0;
  const primaryChannels = filteringChannels ? visibleChannels : visibleChannels.slice(0, SIDEBAR_CHANNEL_LIMIT);
  const hiddenChannels = filteringChannels ? [] : visibleChannels.slice(SIDEBAR_CHANNEL_LIMIT);
  const filteringSessions = normalizedSessionSearchQuery.length > 0;
  const workspaceRows = workspaces
    .map((workspace) => {
      const sessions = workspaceRegistry ? researchSessionsForWorkspace(workspaceRegistry, workspace) : [];
      return {
        workspace,
        sessions: filteringSessions
          ? sessions.filter((session) => sessionMatchesSidebarSearch(session, normalizedSessionSearchQuery))
          : sessions
      };
    })
    .filter(({ workspace, sessions }) => (
      !filteringSessions
      || sessions.length > 0
      || (newResearchActive && snapshot?.workspace.workspacePath === workspace.workspacePath)
    ));
  const listUpdateKey = [
    workspaceRegistryLoading,
    workspaces.length,
    workspaceRegistry?.researchSessions.length ?? 0,
    [...expandedWorkspaceIds].sort().join(','),
    normalizedSessionSearchQuery,
    activeList,
    channels.length,
    channelsExpanded,
    visibleChannels.map((channel) => `${channel.id}:${channel.updatedAt}`).join(',')
  ].join(':');
  const closeSessionSearch = (): void => {
    setSessionSearchOpen(false);
    setSessionSearchQuery('');
    setChannelSearchQuery('');
  };
  const renderChannel = (channel: ResearchChannelSummary): JSX.Element => (
    <div className="sidebar-channel-row" key={channel.id}>
      <button
        type="button"
        className={`sidebar-channel-item${selectedChannelId === channel.id ? ' active' : ''}`}
        aria-current={selectedChannelId === channel.id ? 'page' : undefined}
        onClick={() => onOpenChannel(channel)}
      >
        <span className="sidebar-channel-indent" aria-hidden="true" />
        <span className="sidebar-channel-name">{channel.name}</span>
        <span className="workspace-session-age">{shortRelativeAge(channel.updatedAt)}</span>
      </button>
      <button
        type="button"
        className="sidebar-row-archive-button"
        title={`Archive channel ${channel.name}`}
        aria-label={`Archive channel ${channel.name}`}
        onClick={() => {
          if (!window.confirm(`Archive #${channel.name}? You can restore it from Agent Settings > Archive.`)) return;
          void onArchiveChannel(channel);
        }}
      >
        <Archive size={13} aria-hidden="true" />
      </button>
    </div>
  );

  useEffect(() => {
    if (selectedChannelId) setActiveList('channels');
  }, [selectedChannelId]);

  useEffect(() => {
    setChannelsExpanded(false);
  }, [snapshot?.workspace?.workspaceId]);

  useEffect(() => {
    if (!workspaceAddMenuOpen) return undefined;

    const dismissOnOutsidePointer = (event: PointerEvent): void => {
      if (workspaceAddMenuRef.current?.contains(event.target as Node)) return;
      setWorkspaceAddMenuOpen(false);
    };
    const dismissOnEscape = (event: KeyboardEvent): void => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      setWorkspaceAddMenuOpen(false);
    };

    document.addEventListener('pointerdown', dismissOnOutsidePointer);
    document.addEventListener('keydown', dismissOnEscape);
    return () => {
      document.removeEventListener('pointerdown', dismissOnOutsidePointer);
      document.removeEventListener('keydown', dismissOnEscape);
    };
  }, [workspaceAddMenuOpen]);

  return (
    <aside className="sidebar" aria-hidden={collapsed} inert={collapsed}>
      <button type="button" className="sidebar-new-research" title={`Start ${newResearchLabel.toLocaleLowerCase()}`} disabled={busy || !snapshot} onClick={onStartNewResearch}>
        <SquarePen size={15} />
        <span>{newResearchLabel}</span>
      </button>
      <div className="sidebar-quick-actions">
        <button type="button" className="sidebar-utility-button sidebar-quick-chat" title="Open a quick chat" onClick={onOpenQuickChat}>
          <Zap size={15} />
          <span>New Quick Chat</span>
        </button>
        <button type="button" className={`sidebar-utility-button${automationsActive && !workspaceCreationActive ? ' active' : ''}`} title="Automations" aria-current={automationsActive && !workspaceCreationActive ? 'page' : undefined} onClick={onOpenAutomations}>
          <CalendarClock size={15} />
          <span>Automations</span>
        </button>
        <button type="button" className={`sidebar-utility-button${reportsActive && !workspaceCreationActive ? ' active' : ''}`} title="Reporting" aria-current={reportsActive && !workspaceCreationActive ? 'page' : undefined} onClick={onOpenReports}>
          <FileText size={15} />
          <span>Reporting</span>
        </button>
        <button type="button" className={`sidebar-utility-button${pluginsActive && !workspaceCreationActive ? ' active' : ''}`} title="Plugins" aria-current={pluginsActive && !workspaceCreationActive ? 'page' : undefined} onClick={onOpenPlugins}>
          <Plug size={15} />
          <span>Plugins</span>
        </button>
      </div>
      <div className="sidebar-section workspace-list">
        <div className={`section-row workspace-list-header${sessionSearchOpen ? ' search-open' : ''}`}>
          {sessionSearchOpen ? (
            <div className="workspace-list-search" role="search">
              <Search className="workspace-list-search-icon" aria-hidden="true" size={13} />
              <input
                autoFocus
                value={activeList === 'workspaces' ? sessionSearchQuery : channelSearchQuery}
                aria-label={activeList === 'workspaces' ? 'Search sessions' : 'Search channels'}
                placeholder={activeList === 'workspaces' ? 'Search sessions' : 'Search channels'}
                onChange={(event) => activeList === 'workspaces'
                  ? setSessionSearchQuery(event.target.value)
                  : setChannelSearchQuery(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === 'Escape') closeSessionSearch();
                }}
              />
              <button type="button" className="workspace-list-search-close" title="Close search" aria-label="Close search" onClick={closeSessionSearch}>
                <X size={13} />
              </button>
            </div>
          ) : (
            <div className="workspace-list-title sidebar-list-tabs" role="tablist" aria-label="Sidebar list">
              <button type="button" role="tab" aria-selected={activeList === 'workspaces'} className={activeList === 'workspaces' ? 'active' : ''} onClick={() => {
                closeSessionSearch();
                setChannelCreateOpen(false);
                setActiveList('workspaces');
              }}>Workspaces</button>
              <span className="sidebar-list-tab-divider" aria-hidden="true" />
              <button type="button" role="tab" aria-selected={activeList === 'channels'} className={activeList === 'channels' ? 'active' : ''} onClick={() => {
                closeSessionSearch();
                setWorkspaceAddMenuOpen(false);
                setActiveList('channels');
              }}>Channels</button>
              {(activeList === 'workspaces' ? workspaceRegistryLoading : channelsLoading) ? (
                <span className="workspace-list-title-loading" role="status" aria-label={activeList === 'workspaces' ? 'Loading workspaces' : 'Loading channels'}>
                  <LoaderCircle aria-hidden="true" size={13} />
                </span>
              ) : null}
            </div>
          )}
          <div className="workspace-list-header-actions">
            {!sessionSearchOpen ? (
              <button type="button" title={activeList === 'workspaces' ? 'Search sessions' : 'Search channels'} aria-label={activeList === 'workspaces' ? 'Search sessions' : 'Search channels'} onClick={() => {
                setWorkspaceAddMenuOpen(false);
                setChannelCreateOpen(false);
                setSessionSearchOpen(true);
              }}>
                <Search size={15} />
              </button>
            ) : null}
            {activeList === 'workspaces' ? <div className="workspace-list-add-menu-anchor" ref={workspaceAddMenuRef}>
              <button
                type="button"
                className={`workspace-list-add-button${workspaceCreationActive ? ' active' : ''}`}
                title={`Add ${workspaceNoun.toLocaleLowerCase()}`}
                aria-label={`Add ${workspaceNoun.toLocaleLowerCase()}`}
                aria-current={workspaceCreationActive ? 'page' : undefined}
                aria-haspopup="menu"
                aria-expanded={workspaceAddMenuOpen}
                disabled={busy || workspaceRegistryLoading}
                onClick={() => setWorkspaceAddMenuOpen((current) => !current)}
              >
                <FolderPlus size={15} />
              </button>
              {workspaceAddMenuOpen ? (
                <div className="workspace-list-add-menu" role="menu" aria-label="Add workspace">
                  <button type="button" role="menuitem" onClick={() => {
                    setWorkspaceAddMenuOpen(false);
                    onAddWorkspace();
                  }}>
                    <FolderPlus size={15} aria-hidden="true" />
                    <span>Create Workspace</span>
                  </button>
                  <button type="button" role="menuitem" onClick={() => {
                    setWorkspaceAddMenuOpen(false);
                    onImportWorkspace();
                  }}>
                    <FolderInput size={15} aria-hidden="true" />
                    <span>Import Workspace</span>
                  </button>
                </div>
              ) : null}
            </div> : (
              <button
                type="button"
                className={`workspace-list-add-button${channelCreateOpen ? ' active' : ''}`}
                title="Create channel"
                aria-label="Create channel"
                aria-expanded={channelCreateOpen}
                disabled={busy || !snapshot}
                onClick={() => setChannelCreateOpen((current) => !current)}
              ><Plus size={15} /></button>
            )}
          </div>
        </div>
        <MainSideScrollRegion
          className="sidebar-list-scroll-region"
          listClassName="sidebar-list-scroll workspace-list-items"
          updateKey={listUpdateKey}
        >
          <div className="sidebar-list-scroll-content">
            {activeList === 'channels' ? (
              <>
                {channelCreateOpen ? (
                  <form className="sidebar-channel-create" onSubmit={(event) => {
                    event.preventDefault();
                    const name = canonicalResearchChannelName(channelName);
                    const topic = channelTopic.trim();
                    if (!name || !topic || channelCreating) return;
                    setChannelCreating(true);
                    void onCreateChannel({ name, topic })
                      .then(() => {
                        setChannelName('');
                        setChannelTopic('');
                        setChannelCreateOpen(false);
                      })
                      .catch(() => undefined)
                      .finally(() => setChannelCreating(false));
                  }}>
                    <input
                      value={channelName}
                      placeholder="channel-name"
                      aria-label="Channel name"
                      title="Use up to three words; names are lowercase and dash-separated."
                      autoFocus
                      maxLength={64}
                      onChange={(event) => setChannelName(normalizeResearchChannelNameDraft(event.target.value))}
                    />
                    <textarea value={channelTopic} placeholder="What research belongs here?" aria-label="Channel topic" rows={2} onChange={(event) => setChannelTopic(event.target.value)} />
                    <div>
                      <button type="button" onClick={() => setChannelCreateOpen(false)}>Cancel</button>
                      <button type="submit" disabled={channelCreating || !channelName.trim() || !channelTopic.trim()}>{channelCreating ? 'Creating…' : 'Create'}</button>
                    </div>
                  </form>
                ) : null}
                <div className="sidebar-channel-group" role="group" aria-label="All Channels">
                  <div className="sidebar-channel-group-heading">
                    <Hash size={15} aria-hidden="true" />
                    <span>All Channels</span>
                  </div>
                  <div className="sidebar-channel-list">
                    {!snapshot ? <span className="workspace-session-empty">Open a workspace to view its channels.</span> : null}
                    {snapshot && !channelsLoading && channels.length === 0 ? <span className="workspace-session-empty">No Channels Yet...</span> : null}
                    {snapshot && !channelsLoading && channels.length > 0 && visibleChannels.length === 0 ? <span className="workspace-session-empty">No matching channels.</span> : null}
                    {primaryChannels.map(renderChannel)}
                    {hiddenChannels.length > 0 ? (
                      <>
                        <div
                          className={`workspace-session-overflow ${channelsExpanded ? 'expanded' : ''}`.trim()}
                          aria-hidden={!channelsExpanded}
                          inert={!channelsExpanded}
                        >
                          <div className="workspace-session-overflow-inner">
                            {hiddenChannels.map(renderChannel)}
                          </div>
                        </div>
                        <button
                          type="button"
                          className="session-memory-type-toggle"
                          aria-expanded={channelsExpanded}
                          onClick={() => setChannelsExpanded((expanded) => !expanded)}
                        >
                          {channelsExpanded ? 'Show less' : `Show ${hiddenChannels.length} more`}
                        </button>
                      </>
                    ) : null}
                  </div>
                </div>
              </>
            ) : (
              <>
            {!workspaceRegistryLoading && workspaces.length === 0 ? (
              <span className="workspace-session-empty">No Workspaces Yet...</span>
            ) : null}
            {!workspaceRegistryLoading && workspaces.length > 0 && filteringSessions && workspaceRows.length === 0 ? (
              <span className="workspace-session-empty">No matching sessions.</span>
            ) : null}
            {workspaceRows.map(({ workspace, sessions }) => {
              const workspaceLoaded = snapshot?.workspace.workspacePath === workspace.workspacePath;
              const newResearchSessionActive = workspaceLoaded && newResearchActive && !workspaceCreationActive;
              const dashboardActive = workspaceLoaded && selectedRunId === null && !selectedChannelId && !workspaceCreationActive && !newResearchActive && !automationsActive && !reportsActive && !pluginsActive;
              const sessionsExpanded = expandedWorkspaceIds.has(workspace.id);
              const visibleSessions = filteringSessions ? sessions : sessions.slice(0, SIDEBAR_SESSION_LIMIT);
              const hiddenSessions = filteringSessions ? [] : sessions.slice(SIDEBAR_SESSION_LIMIT);
              const renderSession = (session: ResearchSessionSummary): JSX.Element => {
                return (
                  <div className="workspace-session-row" key={session.id}>
                    <button
                      type="button"
                      className={`workspace-session-item ${!workspaceCreationActive && !newResearchActive && selectedRunId === session.runId ? 'active' : ''}`}
                      title={promptSessionTitle(session)}
                      onClick={() => onOpenResearchSession(workspace, session)}
                    >
                      <SessionLeadingIndicator session={session} />
                      <span className="workspace-session-title">{promptSessionTitle(session)}</span>
                      <span className="workspace-session-age">{shortRelativeAge(session.updatedAt)}</span>
                    </button>
                    <button
                      type="button"
                      className="sidebar-row-archive-button"
                      title={`Archive session ${promptSessionTitle(session)}`}
                      aria-label={`Archive session ${promptSessionTitle(session)}`}
                      onClick={() => {
                        if (!window.confirm(`Archive “${promptSessionTitle(session)}”? You can restore it from Agent Settings > Archive.`)) return;
                        void onArchiveSession(session);
                      }}
                    >
                      <Archive size={13} aria-hidden="true" />
                    </button>
                  </div>
                );
              };
              return (
                <div className="workspace-group" key={workspace.id}>
                  <div className={`workspace-item-row ${dashboardActive ? 'active' : ''}`}>
                    <button type="button" className="workspace-item" title={workspace.workspacePath} onClick={() => onOpenWorkspace(workspace)}>
                      <Folder size={15} aria-hidden="true" />
                      <span>{workspace.workspaceName}</span>
                    </button>
                    <button
                      type="button"
                      className="workspace-new-research-button"
                      title={`Start new research in ${workspace.workspaceName}`}
                      aria-label={`Start new research in ${workspace.workspaceName}`}
                      disabled={busy}
                      onClick={(event) => {
                        event.stopPropagation();
                        onStartNewResearchForWorkspace(workspace);
                      }}
                    >
                      <SquarePen size={14} aria-hidden="true" />
                    </button>
                  </div>
                  <div className="workspace-session-list">
                    {newResearchSessionActive ? (
                      <div className="workspace-session-row workspace-new-research-session-row">
                        <button
                          type="button"
                          className="workspace-session-item workspace-new-research-session-item active"
                          aria-current="page"
                          onClick={onStartNewResearch}
                        >
                          <span className="workspace-new-research-session-indent" aria-hidden="true" />
                          <span className="workspace-session-title">{newResearchLabel}</span>
                        </button>
                      </div>
                    ) : null}
                    {visibleSessions.length > 0 ? (
                      visibleSessions.map(renderSession)
                    ) : !newResearchSessionActive ? (
                      <span className="workspace-session-empty">No {sessionLabel} Yet...</span>
                    ) : null}
                    {hiddenSessions.length > 0 ? (
                      <>
                        <div
                          className={`workspace-session-overflow ${sessionsExpanded ? 'expanded' : ''}`.trim()}
                          aria-hidden={!sessionsExpanded}
                          inert={!sessionsExpanded}
                        >
                          <div className="workspace-session-overflow-inner">
                            {hiddenSessions.map(renderSession)}
                          </div>
                        </div>
                        <button
                          type="button"
                          className="session-memory-type-toggle"
                          aria-expanded={sessionsExpanded}
                          onClick={() => setExpandedWorkspaceIds((current) => {
                            const next = new Set(current);
                            if (next.has(workspace.id)) next.delete(workspace.id);
                            else next.add(workspace.id);
                            return next;
                          })}
                        >
                          {sessionsExpanded ? 'Show less' : `Show ${hiddenSessions.length} more`}
                        </button>
                      </>
                    ) : null}
                  </div>
                </div>
              );
            })}
              </>
            )}
          </div>
        </MainSideScrollRegion>
      </div>
      {error ? <div className="error-box">{error}</div> : null}
      <div className="sidebar-resize-handle" role="separator" aria-label="Resize sidebar" aria-orientation="vertical" onPointerDown={onResizePointerDown} />
    </aside>
  );
});

export function sessionMatchesSidebarSearch(session: ResearchSessionSummary, query: string): boolean {
  const terms = query.trim().toLocaleLowerCase().split(/\s+/u).filter(Boolean);
  if (terms.length === 0) return true;
  const searchableText = [
    promptSessionTitle(session),
    session.promptMarkdown,
    session.summary,
    session.model,
    session.reasoningEffort,
    session.status
  ].join('\n').toLocaleLowerCase();
  return terms.every((term) => searchableText.includes(term));
}

function SessionLeadingIndicator({ session }: { session: ResearchSessionSummary }): JSX.Element {
  if (session.status === 'active') {
    return (
      <span className="workspace-session-leading-status" title="Active" aria-label="Session status: Active">
        <RefreshCw size={10} aria-hidden="true" />
      </span>
    );
  }
  if (isEndedResearchRunStatus(session.status) && session.resultViewedAt === null) {
    return (
      <span className="workspace-session-leading-status" title="Unviewed result" aria-label="Session result not viewed">
        <span className="workspace-session-unviewed-dot" aria-hidden="true" />
      </span>
    );
  }
  return (
    <span className="workspace-session-leading-status" aria-hidden="true" />
  );
}

function isEndedResearchRunStatus(status: RunStatus): boolean {
  return status === 'blocked' || status === 'completed' || status === 'failed' || status === 'stopped';
}
