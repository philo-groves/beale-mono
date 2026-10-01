import { useEffect, useRef, useState } from 'react';
import type { JSX, PointerEvent as ReactPointerEvent } from 'react';
import { ArrowLeft, BookOpen, Plus } from 'lucide-react';
import type { ResearchTopicSummary, WorkspaceRegistryEntry } from '@shared/types';
import { CollectionSidebar } from '../../app/CollectionSidebar';
import { canonicalResearchTopicName } from '../../view-models/researchTopics';

export function topicsForScope(topics: readonly ResearchTopicSummary[], workspaceId: string | null): ResearchTopicSummary[] {
  return topics
    .filter((topic) => !workspaceId || topic.workspaceId === workspaceId)
    .sort((left, right) => right.updatedAt.localeCompare(left.updatedAt) || left.title.localeCompare(right.title));
}

export function TopicsSidebar({
  topics,
  workspaces,
  workspaceRegistryLoading,
  selectedWorkspaceId,
  activeWorkspaceId,
  selectedTopicId,
  collapsed,
  loading,
  error,
  onOpenExplorer,
  onOpenTopic,
  onCreateTopic,
  onAddWorkspace,
  onResizePointerDown
}: {
  topics: readonly ResearchTopicSummary[];
  workspaces: readonly WorkspaceRegistryEntry[];
  workspaceRegistryLoading: boolean;
  selectedWorkspaceId: string | null;
  activeWorkspaceId: string | null;
  selectedTopicId: string | null;
  collapsed: boolean;
  loading: boolean;
  error: string | null;
  onOpenExplorer: () => void;
  onOpenTopic: (topic: ResearchTopicSummary) => void;
  onCreateTopic: (workspaceId: string, input: { name: string; title: string; topic: string }) => Promise<void>;
  onAddWorkspace: () => void;
  onResizePointerDown: (event: ReactPointerEvent<HTMLDivElement>) => void;
}): JSX.Element {
  const [createOpen, setCreateOpen] = useState(false);
  const [creating, setCreating] = useState(false);
  const [topicName, setTopicName] = useState('');
  const [topicPurpose, setTopicPurpose] = useState('');
  const [createWorkspaceId, setCreateWorkspaceId] = useState<string | null>(null);
  const [workspacePickerOpen, setWorkspacePickerOpen] = useState(false);
  const workspacePickerRef = useRef<HTMLDivElement | null>(null);
  const createButtonRef = useRef<HTMLButtonElement | null>(null);
  const firstWorkspaceOptionRef = useRef<HTMLButtonElement | null>(null);
  const targetWorkspaceId = createWorkspaceId ?? selectedWorkspaceId ?? activeWorkspaceId;
  const recent = topicsForScope(topics, selectedWorkspaceId).slice(0, 12);
  const workspaceNames = new Map(workspaces.map((workspace) => [workspace.workspaceId, workspace.workspaceName]));
  useEffect(() => {
    if (activeWorkspaceId || collapsed) setWorkspacePickerOpen(false);
  }, [activeWorkspaceId, collapsed]);
  useEffect(() => {
    if (workspacePickerOpen) firstWorkspaceOptionRef.current?.focus();
  }, [workspacePickerOpen, workspaceRegistryLoading, workspaces.length]);
  useEffect(() => {
    if (!workspacePickerOpen) return undefined;
    const dismissOnOutsidePointer = (event: PointerEvent): void => {
      if (!workspacePickerRef.current?.contains(event.target as Node)) setWorkspacePickerOpen(false);
    };
    const dismissOnEscape = (event: KeyboardEvent): void => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      setWorkspacePickerOpen(false);
      createButtonRef.current?.focus();
    };
    document.addEventListener('pointerdown', dismissOnOutsidePointer);
    document.addEventListener('keydown', dismissOnEscape);
    return () => {
      document.removeEventListener('pointerdown', dismissOnOutsidePointer);
      document.removeEventListener('keydown', dismissOnEscape);
    };
  }, [workspacePickerOpen]);

  return (
    <CollectionSidebar
      title="Topics"
      label="Topics sidebar"
      collapsed={collapsed}
      error={error}
      updateKey={`${selectedWorkspaceId ?? 'all'}:${recent.map((topic) => `${topic.id}:${topic.updatedAt}`).join(',')}`}
      onResizePointerDown={onResizePointerDown}
      primaryAction={
        <div className="sidebar-new-research-anchor" ref={workspacePickerRef}>
          <button
            ref={createButtonRef}
            type="button"
            className="sidebar-utility-button"
            title={activeWorkspaceId ? 'Create topic' : 'Choose a workspace to create a topic'}
            disabled={creating}
            aria-haspopup={!activeWorkspaceId ? 'menu' : undefined}
            aria-expanded={!activeWorkspaceId ? workspacePickerOpen : createOpen}
            onClick={() => {
              if (createOpen) {
                setCreateOpen(false);
                setCreateWorkspaceId(null);
              } else if (activeWorkspaceId) {
                setCreateWorkspaceId(null);
                setCreateOpen(true);
              } else {
                setWorkspacePickerOpen((open) => !open);
              }
            }}
          >
            <Plus size={15} aria-hidden="true" />
            <span>New Topic</span>
          </button>
          {workspacePickerOpen && !activeWorkspaceId ? (
            <div className="sidebar-new-research-menu" role="menu" aria-label="Choose workspace for new topic">
              {workspaceRegistryLoading ? (
                <div className="sidebar-new-research-menu-empty" role="status">Loading workspaces…</div>
              ) : workspaces.length === 0 ? (
                <>
                  <div className="sidebar-new-research-menu-empty">No workspaces yet.</div>
                  <button ref={firstWorkspaceOptionRef} type="button" role="menuitem" onClick={() => {
                    setWorkspacePickerOpen(false);
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
                  aria-label={`Create topic in ${workspace.workspaceName}`}
                  onClick={() => {
                    setWorkspacePickerOpen(false);
                    setCreateWorkspaceId(workspace.workspaceId);
                    setCreateOpen(true);
                  }}
                >{workspace.workspaceName}</button>
              ))}
            </div>
          ) : null}
        </div>
      }
    >
      <nav className="collection-sidebar-list sidebar-list-scroll-content" aria-label="Recent topics">
        {selectedTopicId ? (
          <button type="button" className="workspace-item topics-sidebar-back" onClick={onOpenExplorer}>
            <ArrowLeft size={15} aria-hidden="true" />
            <span>All Topics</span>
          </button>
        ) : null}
        {createOpen && targetWorkspaceId ? (
          <form className="sidebar-topic-create" onSubmit={(event) => {
            event.preventDefault();
            const name = canonicalResearchTopicName(topicName);
            const topic = topicPurpose.trim();
            if (!name || !topic || creating) return;
            setCreating(true);
            void onCreateTopic(targetWorkspaceId, { name, title: topicName.trim(), topic })
              .then(() => {
                setTopicName('');
                setTopicPurpose('');
                setCreateOpen(false);
                setCreateWorkspaceId(null);
              })
              .catch(() => undefined)
              .finally(() => setCreating(false));
          }}>
            <span className="sidebar-topic-create-workspace">{workspaceNames.get(targetWorkspaceId) ?? 'Workspace'}</span>
            <input autoFocus value={topicName} placeholder="Topic title" aria-label="Topic title" maxLength={64} onChange={(event) => setTopicName(event.target.value)} />
            <textarea value={topicPurpose} placeholder="What does this topic cover?" aria-label="Topic purpose" rows={2} onChange={(event) => setTopicPurpose(event.target.value)} />
            <div>
              <button type="button" onClick={() => { setCreateOpen(false); setCreateWorkspaceId(null); }}>Cancel</button>
              <button type="submit" disabled={creating || !topicName.trim() || !topicPurpose.trim()}>{creating ? 'Creating…' : 'Create'}</button>
            </div>
          </form>
        ) : null}
        <div className="collection-sidebar-list-heading">Recent</div>
        {loading && recent.length === 0 ? <p className="collection-sidebar-empty">Loading topics…</p> : recent.length === 0 ? (
          <p className="collection-sidebar-empty">No topics yet</p>
        ) : recent.map((topic) => {
          const selected = topic.id === selectedTopicId;
          return (
            <div className={`workspace-item-row no-menu ${selected ? 'active' : ''}`.trim()} key={`${topic.workspaceId}:${topic.id}`}>
              <button type="button" className="workspace-item collection-sidebar-item" title={topic.title} aria-current={selected ? 'page' : undefined} onClick={() => onOpenTopic(topic)}>
                <BookOpen size={15} aria-hidden="true" />
                <span className="collection-sidebar-item-copy">
                  <span>{topic.title}</span>
                  <small>{workspaceNames.get(topic.workspaceId) ?? 'Workspace'}</small>
                </span>
              </button>
            </div>
          );
        })}
      </nav>
    </CollectionSidebar>
  );
}

export function TopicsExplorer({
  topics,
  workspaces,
  selectedWorkspaceId,
  loading,
  error,
  onScopeChange,
  onOpenTopic
}: {
  topics: readonly ResearchTopicSummary[];
  workspaces: readonly WorkspaceRegistryEntry[];
  selectedWorkspaceId: string | null;
  loading: boolean;
  error: string | null;
  onScopeChange: (workspaceId: string | null) => void;
  onOpenTopic: (topic: ResearchTopicSummary) => void;
}): JSX.Element {
  const scoped = topicsForScope(topics, selectedWorkspaceId);
  const workspaceNames = new Map(workspaces.map((workspace) => [workspace.workspaceId, workspace.workspaceName]));
  const scopeTabs = [
    { id: null, key: 'all', label: 'All Topics' },
    ...workspaces.filter((workspace) => workspace.workspaceId).map((workspace) => ({
      id: workspace.workspaceId, key: workspace.id, label: workspace.workspaceName
    }))
  ];
  const scopeName = selectedWorkspaceId ? workspaceNames.get(selectedWorkspaceId) ?? 'Workspace' : 'All';

  return (
    <section className="topics-explorer-workspace" aria-label="Topics" aria-busy={loading}>
      <div className="wide-content-container">
        <div className="research-side-view-tabs research-side-view-tabs-scrollable pill-view-tabs" role="tablist" aria-label="Topic workspace scope">
          {scopeTabs.map((scope) => {
            const selected = selectedWorkspaceId === scope.id;
            return (
              <div className={`research-side-view-tab provider-settings-tab ${selected ? 'active' : ''}`.trim()} key={scope.key}>
                <button type="button" className="research-side-view-tab-activate" role="tab" aria-selected={selected} aria-controls="topics-explorer-panel" onClick={() => onScopeChange(scope.id)}>
                  <span>{scope.label}</span>
                </button>
              </div>
            );
          })}
        </div>
        <header className="resource-workspace-heading">
          <h1>{scopeName} Topics</h1>
          <p>Browse research topics across your workspaces.</p>
        </header>
        <div className="topics-explorer-list" id="topics-explorer-panel" role="tabpanel">
          {error ? <p className="topic-workspace-error" role="alert">{error}</p> : null}
          {loading && scoped.length === 0 ? <p className="topic-workspace-empty">Loading topics…</p> : scoped.length === 0 ? (
            <p className="topic-workspace-empty">No topics in this view.</p>
          ) : scoped.map((topic) => (
            <button type="button" className="topics-explorer-row" key={`${topic.workspaceId}:${topic.id}`} onClick={() => onOpenTopic(topic)}>
              <span className="topics-explorer-row-copy">
                <strong>{topic.title}</strong>
                <small>{topic.topic}</small>
              </span>
              <span className="topics-explorer-row-meta">{workspaceNames.get(topic.workspaceId) ?? 'Workspace'} · {topic.memberCount} members · {topic.messageCount} messages</span>
            </button>
          ))}
        </div>
      </div>
    </section>
  );
}
