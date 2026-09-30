import { useEffect, useState } from 'react';
import type { FormEvent, JSX } from 'react';
import { Archive, ArrowLeft, FilePlus2, Link2, RefreshCw, Save, Trash2 } from 'lucide-react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import type { ResearchTopicDetail, ResearchTopicLinkKind, ResearchTopicLinkRecord, ResearchTopicSummary } from '@shared/types';

const LINK_KINDS: readonly ResearchTopicLinkKind[] = ['claim', 'memory', 'runbook', 'file', 'session', 'topic'];

export function TopicWorkspace({
  detail,
  loading,
  error,
  saving,
  onRefresh,
  onUpdateOverview,
  onSavePage,
  onDeletePage,
  onLink,
  onUnlink,
  onArchive,
  availableTopics,
  onMerge,
  onUnmerge,
  onOpenTopic
}: {
  detail: ResearchTopicDetail | null;
  loading: boolean;
  error: string | null;
  saving: boolean;
  onRefresh: () => void;
  onUpdateOverview: (content: string, expectedUpdatedAt: string) => Promise<void>;
  onSavePage: (input: { id?: string; title: string; contentMarkdown: string; expectedUpdatedAt?: string }) => Promise<void>;
  onDeletePage: (pageId: string) => Promise<void>;
  onLink: (input: { kind: ResearchTopicLinkKind; resourceId: string; title: string }) => Promise<void>;
  onUnlink: (linkId: string) => Promise<void>;
  onArchive: () => Promise<void>;
  availableTopics: ResearchTopicSummary[];
  onMerge: (targetTopicId: string) => Promise<void>;
  onUnmerge: (sourceTopicId: string) => Promise<void>;
  onOpenTopic: (topicId: string) => void;
}): JSX.Element {
  const [activeSection, setActiveSection] = useState<'overview' | 'pages' | 'links' | 'history'>('overview');
  const [editingOverview, setEditingOverview] = useState(false);
  const [overviewDraft, setOverviewDraft] = useState('');
  const [selectedPageId, setSelectedPageId] = useState<string | null>(null);
  const [editingPage, setEditingPage] = useState(false);
  const [pageTitle, setPageTitle] = useState('');
  const [pageBody, setPageBody] = useState('');
  const [linkKind, setLinkKind] = useState<ResearchTopicLinkKind>('claim');
  const [linkId, setLinkId] = useState('');
  const [linkTitle, setLinkTitle] = useState('');
  const [localError, setLocalError] = useState<string | null>(null);
  const [mergeOpen, setMergeOpen] = useState(false);
  const [mergeTargetId, setMergeTargetId] = useState('');
  const topicRecord = detail?.topic;
  const selectedPage = detail?.pages.find((page) => page.id === selectedPageId) ?? null;

  useEffect(() => {
    setActiveSection('overview');
    setEditingOverview(false);
    setSelectedPageId(null);
    setEditingPage(false);
    setLocalError(null);
    setMergeOpen(false);
    setMergeTargetId('');
  }, [topicRecord?.id]);
  useEffect(() => {
    if (!editingOverview) setOverviewDraft(topicRecord?.overviewMarkdown ?? '');
  }, [editingOverview, topicRecord?.overviewMarkdown]);
  useEffect(() => {
    if (editingPage) return;
    setPageTitle(selectedPage?.title ?? '');
    setPageBody(selectedPage?.contentMarkdown ?? '');
  }, [editingPage, selectedPage?.id, selectedPage?.updatedAt]);

  const run = async (action: () => Promise<void>, after?: () => void): Promise<void> => {
    setLocalError(null);
    try {
      await action();
      after?.();
    } catch (caught) {
      setLocalError(caught instanceof Error ? caught.message : String(caught));
    }
  };

  if (!detail) return <div className="topic-workspace topic-workspace-state">{loading ? 'Loading topic…' : error ?? 'Topic unavailable.'}</div>;
  const topic = detail.topic;
  const mergeTargets = availableTopics.filter((candidate) => candidate.id !== topic.id && !candidate.archivedAt && !candidate.mergedIntoTopicId);

  const submitOverview = (event: FormEvent): void => {
    event.preventDefault();
    void run(() => onUpdateOverview(overviewDraft, detail.topic.updatedAt), () => setEditingOverview(false));
  };
  const submitPage = (event: FormEvent): void => {
    event.preventDefault();
    if (!pageTitle.trim()) return;
    void run(() => onSavePage({
      ...(selectedPage ? { id: selectedPage.id, expectedUpdatedAt: selectedPage.updatedAt } : {}),
      title: pageTitle.trim(), contentMarkdown: pageBody
    }), () => setEditingPage(false));
  };
  const submitLink = (event: FormEvent): void => {
    event.preventDefault();
    if (!linkId.trim() || !linkTitle.trim()) return;
    void run(() => onLink({ kind: linkKind, resourceId: linkId.trim(), title: linkTitle.trim() }), () => {
      setLinkId(''); setLinkTitle('');
    });
  };

  return <section className="topic-workspace" aria-label={`Topic ${topic?.title}`}>
    <header className="topic-workspace-header">
      <div>
        <span className="topic-workspace-eyebrow">Research topic</span>
        <h1>{topic.title}</h1>
        <p>{topic.topic}</p>
      </div>
      <div className="topic-workspace-header-actions">
        <button type="button" title="Refresh topic" aria-label="Refresh topic" onClick={onRefresh}><RefreshCw size={16} /></button>
        {!topic.archivedAt && !detail.mergedTopics.length && mergeTargets.length ? <button type="button" title="Merge topic" aria-label="Merge topic" onClick={() => setMergeOpen((current) => !current)}>Merge</button> : null}
        {!topic.archivedAt && !detail.mergedTopics.length ? <button type="button" title="Archive topic" aria-label="Archive topic" onClick={() => {
          if (window.confirm(`Archive ${topic.title}? You can restore it in Agent Settings.`)) void run(onArchive);
        }}><Archive size={16} /></button> : null}
      </div>
    </header>
    {topic.mergedIntoTopicId ? <div className="topic-workspace-merged-source">
      <span>This topic was merged into another topic. Its pages and references are preserved.</span>
      <button type="button" onClick={() => onOpenTopic(topic.mergedIntoTopicId!)}>Open target</button>
      <button type="button" disabled={saving} onClick={() => void run(() => onUnmerge(topic.id))}>Undo merge</button>
    </div> : null}
    {mergeOpen ? <div className="topic-workspace-merge">
      <label htmlFor="topic-merge-target">Merge this topic into</label>
      <select id="topic-merge-target" value={mergeTargetId} onChange={(event) => setMergeTargetId(event.target.value)}>
        <option value="">Choose a topic</option>
        {mergeTargets.map((candidate) => <option key={candidate.id} value={candidate.id}>{candidate.title}</option>)}
      </select>
      <button type="button" disabled={!mergeTargetId || saving} onClick={() => {
        const target = mergeTargets.find((candidate) => candidate.id === mergeTargetId);
        if (target && window.confirm(`Merge ${topic.title} into ${target.title}? You can undo this from the target topic or Archive settings.`)) {
          void run(() => onMerge(target.id));
        }
      }}>Merge topic</button>
      <button type="button" onClick={() => setMergeOpen(false)}>Cancel</button>
    </div> : null}
    {detail.mergedTopics.length ? <div className="topic-workspace-merged">
      <strong>Merged topics</strong>
      {detail.mergedTopics.map((source) => <div key={source.id}>
        <span>{source.title}</span>
        <button type="button" onClick={() => onOpenTopic(source.id)}>View source</button>
        <button type="button" disabled={saving} onClick={() => void run(() => onUnmerge(source.id))}>Undo merge</button>
      </div>)}
    </div> : null}
    {(error || localError) ? <div className="topic-workspace-error" role="alert">{localError ?? error}</div> : null}
    <nav className="topic-workspace-tabs" aria-label="Topic sections">
      {(['overview', 'pages', 'links', 'history'] as const).map((section) => <button
        key={section} type="button" className={activeSection === section ? 'active' : ''}
        aria-current={activeSection === section ? 'page' : undefined}
        onClick={() => { setActiveSection(section); setEditingOverview(false); setEditingPage(false); }}
      >{section === 'links' ? `References (${detail.links.length})` : section === 'pages' ? `Pages (${detail.pages.length})` : section === 'history' ? 'Activity history' : 'Overview'}</button>)}
    </nav>
    <div className="topic-workspace-body">
      {activeSection === 'overview' ? <article className="topic-workspace-document">
        {editingOverview ? <form onSubmit={submitOverview}>
          <textarea aria-label="Topic overview" autoFocus value={overviewDraft} onChange={(event) => setOverviewDraft(event.target.value)} />
          <div className="topic-workspace-form-actions"><button type="button" onClick={() => setEditingOverview(false)}>Cancel</button><button type="submit" disabled={saving}><Save size={14} /> Save overview</button></div>
        </form> : <>
          <div className="topic-workspace-document-actions"><button type="button" onClick={() => setEditingOverview(true)}>Edit overview</button></div>
          {topic.overviewMarkdown ? <ReactMarkdown remarkPlugins={[remarkGfm]}>{topic.overviewMarkdown}</ReactMarkdown> : <p className="topic-workspace-empty">Add the current understanding, open questions, and links to supporting records.</p>}
        </>}
      </article> : null}
      {activeSection === 'pages' ? <div className="topic-workspace-pages">
        <div className="topic-workspace-page-list">
          <button type="button" onClick={() => { setSelectedPageId(null); setPageTitle(''); setPageBody(''); setEditingPage(true); }}><FilePlus2 size={15} /> New page</button>
          {detail.pages.map((page) => <button key={page.id} type="button" className={selectedPageId === page.id ? 'active' : ''} onClick={() => { setSelectedPageId(page.id); setEditingPage(false); }}>{page.title}</button>)}
        </div>
        <article className="topic-workspace-document">
          {editingPage ? <form onSubmit={submitPage}>
            <input autoFocus aria-label="Page title" placeholder="Page title" value={pageTitle} onChange={(event) => setPageTitle(event.target.value)} />
            <textarea aria-label="Page content" value={pageBody} onChange={(event) => setPageBody(event.target.value)} />
            <div className="topic-workspace-form-actions"><button type="button" onClick={() => setEditingPage(false)}>Cancel</button><button type="submit" disabled={saving || !pageTitle.trim()}><Save size={14} /> Save page</button></div>
          </form> : selectedPage ? <>
            <div className="topic-workspace-document-actions">
              <button type="button" onClick={() => setEditingPage(true)}>Edit</button>
              <button type="button" aria-label={`Delete ${selectedPage.title}`} onClick={() => {
                if (window.confirm(`Delete page ${selectedPage.title}?`)) void run(() => onDeletePage(selectedPage.id), () => setSelectedPageId(null));
              }}><Trash2 size={14} /></button>
            </div>
            <h2>{selectedPage.title}</h2>
            <ReactMarkdown remarkPlugins={[remarkGfm]}>{selectedPage.contentMarkdown}</ReactMarkdown>
          </> : <p className="topic-workspace-empty">Choose a page or create one for material that would make the overview too long.</p>}
        </article>
      </div> : null}
      {activeSection === 'links' ? <div className="topic-workspace-references">
        <p>These references point to existing research records. Their content and status remain in the original record.</p>
        <form className="topic-workspace-link-form" onSubmit={submitLink}>
          <select aria-label="Reference type" value={linkKind} onChange={(event) => setLinkKind(event.target.value as ResearchTopicLinkKind)}>{LINK_KINDS.map((kind) => <option key={kind} value={kind}>{kind}</option>)}</select>
          <input aria-label="Reference ID or workspace path" placeholder="Record ID or workspace path" value={linkId} onChange={(event) => setLinkId(event.target.value)} />
          <input aria-label="Reference title" placeholder="Display title" value={linkTitle} onChange={(event) => setLinkTitle(event.target.value)} />
          <button type="submit" disabled={saving || !linkId.trim() || !linkTitle.trim()}><Link2 size={14} /> Link</button>
        </form>
        <div className="topic-workspace-link-list">{detail.links.length ? detail.links.map((link: ResearchTopicLinkRecord) => <div key={link.id} className="topic-workspace-link">
          <span className="topic-workspace-link-kind">{link.kind}</span>
          {link.kind === 'topic' ? <button type="button" className="topic-workspace-linked-topic" onClick={() => onOpenTopic(link.resourceId)}>{link.title}</button> : <strong>{link.title}</strong>}
          <code>{link.resourceId}</code>
          <button type="button" title={`Remove ${link.title}`} aria-label={`Remove ${link.title}`} onClick={() => void run(() => onUnlink(link.id))}><Trash2 size={14} /></button>
        </div>) : <p className="topic-workspace-empty">No references yet.</p>}</div>
      </div> : null}
      {activeSection === 'history' ? <div className="topic-workspace-history">
        <p>Historical activity is preserved for context. Verify factual conclusions in canonical claims, memories, and runbooks.</p>
        {detail.messages.length ? detail.messages.map((message) => <article key={message.id} className="topic-workspace-activity">
          <header><span>{message.senderAgentPath}</span><time dateTime={message.createdAt}>{new Date(message.createdAt).toLocaleString()}</time></header>
          <ReactMarkdown remarkPlugins={[remarkGfm]}>{message.contentMarkdown}</ReactMarkdown>
        </article>) : <p className="topic-workspace-empty">No historical activity.</p>}
        <button type="button" className="topic-workspace-back" onClick={() => setActiveSection('overview')}><ArrowLeft size={14} /> Back to overview</button>
      </div> : null}
    </div>
  </section>;
}
