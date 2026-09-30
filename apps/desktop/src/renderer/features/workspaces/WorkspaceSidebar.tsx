import { memo, useEffect, useRef, useState } from 'react';
import type { JSX, PointerEvent as ReactPointerEvent } from 'react';
import { Archive, BookOpen, Folder, FolderInput, FolderPlus, LoaderCircle, Plus, RefreshCw, Search, SquarePen, X, Zap } from 'lucide-react';
import type { WorkspaceRegistryEntry, WorkspaceRegistryState, ResearchTopicSummary, ResearchSessionSummary, RunStatus, WorkspaceSnapshot } from '@shared/types';
import { MainSideScrollRegion } from '../../app/MainSideScrollRegion';
import { useDevRenderProbe } from '../../devInstrumentation';
import { canonicalResearchTopicName } from '../../view-models/researchTopics';
import { promptSessionTitle, researchSessionsForWorkspace, shortRelativeAge } from '../../view-models/workspaceDisplay';

const SIDEBAR_SESSION_LIMIT = 4;
const SIDEBAR_TOPIC_LIMIT = 4;

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
  pluginsActive = false,
  snapshot,
  topics = [],
  topicsLoading = false,
  selectedTopicId = null,
  onAddWorkspace,
  onImportWorkspace,
  onOpenWorkspace,
  onOpenResearchSession,
  onOpenTopic = () => undefined,
  onArchiveSession = async () => undefined,
  onArchiveTopic = async () => undefined,
  onCreateTopic = async () => undefined,
  onResizePointerDown,
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
  pluginsActive?: boolean;
  snapshot: WorkspaceSnapshot | null;
  topics?: ResearchTopicSummary[];
  topicsLoading?: boolean;
  selectedTopicId?: string | null;
  onAddWorkspace: () => void;
  onImportWorkspace: () => void;
  onOpenWorkspace: (workspace: WorkspaceRegistryEntry) => void;
  onOpenResearchSession: (workspace: WorkspaceRegistryEntry, session: ResearchSessionSummary) => void;
  onOpenTopic?: (topic: ResearchTopicSummary) => void;
  onArchiveSession?: (session: ResearchSessionSummary) => Promise<void>;
  onArchiveTopic?: (topic: ResearchTopicSummary) => Promise<void>;
  onCreateTopic?: (input: { name: string; title: string; topic: string }) => Promise<void>;
  onResizePointerDown: (event: ReactPointerEvent<HTMLDivElement>) => void;
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
  const [newResearchPickerOpen, setNewResearchPickerOpen] = useState(false);
  const [activeList, setActiveList] = useState<'workspaces' | 'topics'>(() => selectedTopicId ? 'topics' : 'workspaces');
  const [topicSearchQuery, setTopicSearchQuery] = useState('');
  const [topicSearchResults, setTopicSearchResults] = useState<{ workspaceId: string; query: string; topics: ResearchTopicSummary[] } | null>(null);
  const [topicCreateOpen, setTopicCreateOpen] = useState(false);
  const [topicName, setTopicName] = useState('');
  const [topicTopic, setTopicTopic] = useState('');
  const [topicCreating, setTopicCreating] = useState(false);
  const [topicsExpanded, setTopicsExpanded] = useState(false);
  const workspaceAddMenuRef = useRef<HTMLDivElement | null>(null);
  const newResearchPickerRef = useRef<HTMLDivElement | null>(null);
  const newResearchButtonRef = useRef<HTMLButtonElement | null>(null);
  const firstWorkspaceOptionRef = useRef<HTMLButtonElement | null>(null);
  const normalizedSessionSearchQuery = sessionSearchQuery.trim();
  const normalizedTopicSearchQuery = topicSearchQuery.trim().toLocaleLowerCase();
  const topicWorkspaceId = snapshot?.workspace?.workspaceId;
  useEffect(() => {
    if (!topicWorkspaceId || !normalizedTopicSearchQuery) {
      setTopicSearchResults(null);
      return undefined;
    }
    let cancelled = false;
    const timeout = window.setTimeout(() => {
      void window.beale.searchResearchTopics(topicWorkspaceId, normalizedTopicSearchQuery)
        .then((found) => {
          if (!cancelled) setTopicSearchResults({ workspaceId: topicWorkspaceId, query: normalizedTopicSearchQuery, topics: found });
        })
        .catch(() => {
          if (!cancelled) setTopicSearchResults(null);
        });
    }, 200);
    return () => { cancelled = true; window.clearTimeout(timeout); };
  }, [topicWorkspaceId, normalizedTopicSearchQuery]);
  const visibleTopics = normalizedTopicSearchQuery
    ? topicSearchResults !== null && topicSearchResults.workspaceId === topicWorkspaceId && topicSearchResults.query === normalizedTopicSearchQuery
      ? topicSearchResults.topics
      : topics.filter((topic) => [topic.name, topic.title, topic.topic, topic.overviewMarkdown]
        .join('\n').toLocaleLowerCase().includes(normalizedTopicSearchQuery))
    : topics;
  const filteringTopics = normalizedTopicSearchQuery.length > 0;
  const primaryTopics = filteringTopics ? visibleTopics : visibleTopics.slice(0, SIDEBAR_TOPIC_LIMIT);
  const hiddenTopics = filteringTopics ? [] : visibleTopics.slice(SIDEBAR_TOPIC_LIMIT);
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
    topics.length,
    topicsExpanded,
    visibleTopics.map((topic) => `${topic.id}:${topic.updatedAt}`).join(',')
  ].join(':');
  const closeSessionSearch = (): void => {
    setSessionSearchOpen(false);
    setSessionSearchQuery('');
    setTopicSearchQuery('');
  };
  const renderTopic = (topic: ResearchTopicSummary): JSX.Element => (
    <div className="sidebar-topic-row" key={topic.id}>
      <button
        type="button"
        className={`sidebar-topic-item${selectedTopicId === topic.id ? ' active' : ''}`}
        aria-current={selectedTopicId === topic.id ? 'page' : undefined}
        onClick={() => onOpenTopic(topic)}
      >
        <span className="sidebar-topic-indent" aria-hidden="true" />
        <span className="sidebar-topic-name">{topic.title}</span>
        <span className="workspace-session-age">{shortRelativeAge(topic.updatedAt)}</span>
      </button>
      <button
        type="button"
        className="sidebar-row-archive-button"
        title={`Archive topic ${topic.title}`}
        aria-label={`Archive topic ${topic.title}`}
        onClick={() => {
          if (!window.confirm(`Archive ${topic.title}? You can restore it from Agent Settings > Archive.`)) return;
          void onArchiveTopic(topic);
        }}
      >
        <Archive size={13} aria-hidden="true" />
      </button>
    </div>
  );

  useEffect(() => {
    if (selectedTopicId) setActiveList('topics');
  }, [selectedTopicId]);

  useEffect(() => {
    setTopicsExpanded(false);
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

  useEffect(() => {
    if (!newResearchPickerOpen) return undefined;
    firstWorkspaceOptionRef.current?.focus();
    const dismissOnOutsidePointer = (event: PointerEvent): void => {
      if (!newResearchPickerRef.current?.contains(event.target as Node)) setNewResearchPickerOpen(false);
    };
    const dismissOnEscape = (event: KeyboardEvent): void => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      setNewResearchPickerOpen(false);
      newResearchButtonRef.current?.focus();
    };
    document.addEventListener('pointerdown', dismissOnOutsidePointer);
    document.addEventListener('keydown', dismissOnEscape);
    return () => {
      document.removeEventListener('pointerdown', dismissOnOutsidePointer);
      document.removeEventListener('keydown', dismissOnEscape);
    };
  }, [newResearchPickerOpen, workspaces.length]);

  useEffect(() => {
    if (snapshot || collapsed) setNewResearchPickerOpen(false);
  }, [snapshot, collapsed]);

  return (
    <aside className="sidebar" aria-hidden={collapsed} inert={collapsed}>
      <div className="sidebar-primary-actions">
        <div className="sidebar-wordmark">Beale</div>
        <div className="sidebar-new-research-anchor" ref={newResearchPickerRef}>
          <button
            ref={newResearchButtonRef}
            type="button"
            className="sidebar-utility-button sidebar-new-research"
            title={`Start ${newResearchLabel.toLocaleLowerCase()}`}
            aria-haspopup={!snapshot ? 'menu' : undefined}
            aria-expanded={!snapshot ? newResearchPickerOpen : undefined}
            disabled={busy}
            onClick={() => {
              if (snapshot) {
                onStartNewResearch();
              } else {
                setWorkspaceAddMenuOpen(false);
                setNewResearchPickerOpen((open) => !open);
              }
            }}
          >
            <SquarePen size={15} />
            <span>{newResearchLabel}</span>
          </button>
          {newResearchPickerOpen && !snapshot ? (
            <div className="sidebar-new-research-menu" role="menu" aria-label="Choose workspace for new research">
              {workspaceRegistryLoading ? (
                <div className="sidebar-new-research-menu-empty" role="status">Loading workspaces…</div>
              ) : workspaces.length === 0 ? (
                <>
                  <div className="sidebar-new-research-menu-empty">No workspaces yet.</div>
                  <button ref={firstWorkspaceOptionRef} type="button" role="menuitem" onClick={() => {
                    setNewResearchPickerOpen(false);
                    onAddWorkspace();
                  }}>Create Workspace</button>
                </>
              ) : workspaces.map((workspace, index) => (
                <button
                  key={workspace.id}
                  ref={index === 0 ? firstWorkspaceOptionRef : undefined}
                  type="button"
                  role="menuitem"
                  title={workspace.workspacePath}
                  aria-label={`Start new research in ${workspace.workspaceName}`}
                  onClick={() => {
                    setNewResearchPickerOpen(false);
                    onStartNewResearchForWorkspace(workspace);
                  }}
                >{workspace.workspaceName}</button>
              ))}
            </div>
          ) : null}
        </div>
        <div className="sidebar-quick-actions">
          <button type="button" className="sidebar-utility-button sidebar-quick-chat" title="Open a quick chat" onClick={onOpenQuickChat}>
            <Zap size={15} />
            <span>Quick Chat</span>
          </button>
        </div>
      </div>
      <div className="sidebar-section workspace-list">
        <div className={`section-row workspace-list-header${sessionSearchOpen ? ' search-open' : ''}`}>
          {sessionSearchOpen ? (
            <div className="workspace-list-search" role="search">
              <Search className="workspace-list-search-icon" aria-hidden="true" size={13} />
              <input
                autoFocus
                value={activeList === 'workspaces' ? sessionSearchQuery : topicSearchQuery}
                aria-label={activeList === 'workspaces' ? 'Search sessions' : 'Search topics'}
                placeholder={activeList === 'workspaces' ? 'Search sessions' : 'Search topics'}
                onChange={(event) => activeList === 'workspaces'
                  ? setSessionSearchQuery(event.target.value)
                  : setTopicSearchQuery(event.target.value)}
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
                setTopicCreateOpen(false);
                setActiveList('workspaces');
              }}>Workspaces</button>
              <span className="sidebar-list-tab-divider" aria-hidden="true" />
              <button type="button" role="tab" aria-selected={activeList === 'topics'} className={activeList === 'topics' ? 'active' : ''} onClick={() => {
                closeSessionSearch();
                setWorkspaceAddMenuOpen(false);
                setActiveList('topics');
              }}>Topics</button>
              {(activeList === 'workspaces' ? workspaceRegistryLoading : topicsLoading) ? (
                <span className="workspace-list-title-loading" role="status" aria-label={activeList === 'workspaces' ? 'Loading workspaces' : 'Loading topics'}>
                  <LoaderCircle aria-hidden="true" size={13} />
                </span>
              ) : null}
            </div>
          )}
          <div className="workspace-list-header-actions">
            {!sessionSearchOpen ? (
              <button type="button" title={activeList === 'workspaces' ? 'Search sessions' : 'Search topics'} aria-label={activeList === 'workspaces' ? 'Search sessions' : 'Search topics'} onClick={() => {
                setWorkspaceAddMenuOpen(false);
                setTopicCreateOpen(false);
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
                className={`workspace-list-add-button${topicCreateOpen ? ' active' : ''}`}
                title="Create topic"
                aria-label="Create topic"
                aria-expanded={topicCreateOpen}
                disabled={busy || !snapshot}
                onClick={() => setTopicCreateOpen((current) => !current)}
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
            {activeList === 'topics' ? (
              <>
                {topicCreateOpen ? (
                  <form className="sidebar-topic-create" onSubmit={(event) => {
                    event.preventDefault();
                    const name = canonicalResearchTopicName(topicName);
                    const topic = topicTopic.trim();
                    if (!name || !topic || topicCreating) return;
                    setTopicCreating(true);
                    void onCreateTopic({ name, title: topicName.trim(), topic })
                      .then(() => {
                        setTopicName('');
                        setTopicTopic('');
                        setTopicCreateOpen(false);
                      })
                      .catch(() => undefined)
                      .finally(() => setTopicCreating(false));
                  }}>
                    <input
                      value={topicName}
                      placeholder="Topic title"
                      aria-label="Topic title"
                      title="Use a short descriptive title."
                      autoFocus
                      maxLength={64}
                      onChange={(event) => setTopicName(event.target.value)}
                    />
                    <textarea value={topicTopic} placeholder="What does this topic cover?" aria-label="Topic purpose" rows={2} onChange={(event) => setTopicTopic(event.target.value)} />
                    <div>
                      <button type="button" onClick={() => setTopicCreateOpen(false)}>Cancel</button>
                      <button type="submit" disabled={topicCreating || !topicName.trim() || !topicTopic.trim()}>{topicCreating ? 'Creating…' : 'Create'}</button>
                    </div>
                  </form>
                ) : null}
                <div className="sidebar-topic-group" role="group" aria-label="All Topics">
                  <div className="sidebar-topic-group-heading">
                    <BookOpen size={15} aria-hidden="true" />
                    <span>All Topics</span>
                  </div>
                  <div className="sidebar-topic-list">
                    {!snapshot ? <span className="workspace-session-empty">Open a workspace to view its topics.</span> : null}
                    {snapshot && !topicsLoading && topics.length === 0 ? <span className="workspace-session-empty">No Topics Yet...</span> : null}
                    {snapshot && !topicsLoading && topics.length > 0 && visibleTopics.length === 0 ? <span className="workspace-session-empty">No matching topics.</span> : null}
                    {primaryTopics.map(renderTopic)}
                    {hiddenTopics.length > 0 ? (
                      <>
                        <div
                          className={`workspace-session-overflow ${topicsExpanded ? 'expanded' : ''}`.trim()}
                          aria-hidden={!topicsExpanded}
                          inert={!topicsExpanded}
                        >
                          <div className="workspace-session-overflow-inner">
                            {hiddenTopics.map(renderTopic)}
                          </div>
                        </div>
                        <button
                          type="button"
                          className="session-memory-type-toggle"
                          aria-expanded={topicsExpanded}
                          onClick={() => setTopicsExpanded((expanded) => !expanded)}
                        >
                          {topicsExpanded ? 'Show less' : `Show ${hiddenTopics.length} more`}
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
              const dashboardActive = workspaceLoaded && selectedRunId === null && !selectedTopicId && !workspaceCreationActive && !newResearchActive && !automationsActive && !pluginsActive;
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
