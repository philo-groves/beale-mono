import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { CSSProperties, FormEvent, JSX } from 'react';
import { ArrowRight, BookOpen, Brain, ChevronRight, FileText, Hash, LoaderCircle, RefreshCw, Trash2, Users } from 'lucide-react';
import type {
  ResearchChannelDetail,
  ResearchChannelMemberRecord,
  ResearchChannelMemberStatus,
  ResearchChannelSharedResourceKind,
  ResearchChannelSharedResourceRecord,
  ResearchProviderModelCatalog,
  RunStatus
} from '@shared/types';
import { ProviderIcon } from '../../app/ProviderIcon';
import { renderTraceProseText } from '../traces/traceMarkup';
import { ResearchSideNestedHeader, SubagentDetailPanel, subagentModelDisplayName } from '../research/MemorySidePanel';
import { shortRelativeAge } from '../../view-models/workspaceDisplay';
import { buildTraceDisplayEvents } from '../../view-models/traceDisplay';
import { traceEventsForSubagent } from '../../view-models/subagents';
import {
  MIN_RESEARCH_SIDE_PANEL_WIDTH,
  useResizableResearchSidePanel
} from '../../hooks/useResizableResearchSidePanel';
import { useRunDetailPolling } from '../../hooks/useRunDetailPolling';

interface ChannelTranscriptScrollEdges {
  top: boolean;
  bottom: boolean;
}

export function channelTranscriptScrollEdges({
  scrollHeight,
  clientHeight,
  scrollTop
}: {
  scrollHeight: number;
  clientHeight: number;
  scrollTop: number;
}): ChannelTranscriptScrollEdges {
  const maximumScrollTop = Math.max(0, scrollHeight - clientHeight);
  return {
    top: scrollTop > 1,
    bottom: maximumScrollTop - scrollTop > 1
  };
}

export function ChannelWorkspace({
  detail,
  loading,
  error,
  posting,
  providerModelCatalog,
  summaryExpanded,
  onSummaryExpandedChange = () => undefined,
  onRefresh,
  onPost,
  onDelete
}: {
  detail: ResearchChannelDetail | null;
  loading: boolean;
  error: string | null;
  posting: boolean;
  providerModelCatalog: ResearchProviderModelCatalog[];
  summaryExpanded?: boolean;
  onSummaryExpandedChange?: (expanded: boolean) => void;
  onRefresh: () => void;
  onPost: (contentMarkdown: string) => Promise<void>;
  onDelete: () => Promise<void>;
}): JSX.Element {
  const [draft, setDraft] = useState('');
  const [scrollEdges, setScrollEdges] = useState<ChannelTranscriptScrollEdges>({ top: false, bottom: false });
  const [selectedMemberId, setSelectedMemberId] = useState<string | null>(null);
  const [selectedSharedResourceId, setSelectedSharedResourceId] = useState<string | null>(null);
  const [subagentDetailError, setSubagentDetailError] = useState<string | null>(null);
  const transcriptRef = useRef<HTMLDivElement | null>(null);
  const composerTextareaRef = useRef<HTMLTextAreaElement | null>(null);
  const {
    containerRef,
    panelWidth,
    maximumPanelWidth,
    beginResize,
    handleResizeKeyDown
  } = useResizableResearchSidePanel(Boolean(detail));
  const selectedMember = detail?.members.find((member) => member.id === selectedMemberId) ?? null;
  const selectedSharedResource = detail?.sharedResources.find((resource) => resource.id === selectedSharedResourceId) ?? null;
  const selectedMemberRunId = selectedMember?.sessionId ?? null;
  const selectedMemberRunStatus = selectedMember ? channelMemberRunStatus(selectedMember.status) : null;
  const { runDetail: selectedMemberRunDetail } = useRunDetailPolling({
    selectedRunId: selectedMemberRunId,
    selectedRunState: selectedMemberRunStatus,
    projection: 'full',
    refreshKey: selectedMember?.lastSeenAt ?? null,
    onError: setSubagentDetailError
  });
  const resolvedSelectedMemberRunDetail = selectedMemberRunDetail?.run.id === selectedMemberRunId
    ? selectedMemberRunDetail
    : null;
  const selectedMemberEvents = useMemo(() => resolvedSelectedMemberRunDetail && selectedMember
    ? traceEventsForSubagent(buildTraceDisplayEvents(resolvedSelectedMemberRunDetail), selectedMember.agentPath)
    : [], [resolvedSelectedMemberRunDetail, selectedMember]);
  const sharedResourceByMessageId = useMemo(() => new Map(
    detail?.sharedResources.map((resource) => [resource.messageId, resource]) ?? []
  ), [detail?.sharedResources]);
  const memberById = useMemo(() => new Map(
    detail?.members.map((member) => [member.id, member]) ?? []
  ), [detail?.members]);
  const memberByMessageIdentity = useMemo(() => new Map(
    detail?.members.map((member) => [channelMessageMemberKey(member.sessionId, member.agentPath), member]) ?? []
  ), [detail?.members]);

  const updateScrollEdges = useCallback((): void => {
    const transcript = transcriptRef.current;
    if (!transcript) return;
    const nextEdges = channelTranscriptScrollEdges(transcript);
    setScrollEdges((current) => current.top === nextEdges.top && current.bottom === nextEdges.bottom
      ? current
      : nextEdges);
  }, []);

  useEffect(() => {
    const transcript = transcriptRef.current;
    if (!transcript) return undefined;
    transcript.scrollTop = transcript.scrollHeight;
    updateScrollEdges();
    const frame = window.requestAnimationFrame(updateScrollEdges);
    return () => window.cancelAnimationFrame(frame);
  }, [detail?.messages.length, updateScrollEdges]);

  useEffect(() => {
    const transcript = transcriptRef.current;
    if (!transcript || typeof ResizeObserver === 'undefined') return undefined;
    const observer = new ResizeObserver(updateScrollEdges);
    observer.observe(transcript);
    return () => observer.disconnect();
  }, [detail, updateScrollEdges]);

  useLayoutEffect(() => {
    const textarea = composerTextareaRef.current;
    if (!textarea) return;
    textarea.style.height = '0px';
    const style = window.getComputedStyle(textarea);
    const lineHeight = Number.parseFloat(style.lineHeight) || 18;
    const padding = (Number.parseFloat(style.paddingTop) || 0) + (Number.parseFloat(style.paddingBottom) || 0);
    const minimumHeight = Number.parseFloat(style.minHeight) || 54;
    const maximumHeight = lineHeight * 7 + padding;
    textarea.style.height = `${Math.max(minimumHeight, Math.min(textarea.scrollHeight, maximumHeight))}px`;
    textarea.style.overflowY = textarea.scrollHeight > maximumHeight ? 'auto' : 'hidden';
  }, [draft]);

  useEffect(() => {
    setSelectedMemberId(null);
    setSelectedSharedResourceId(null);
    setSubagentDetailError(null);
  }, [detail?.channel.id]);

  useEffect(() => {
    if (summaryExpanded) return;
    setSelectedMemberId(null);
    setSelectedSharedResourceId(null);
    setSubagentDetailError(null);
  }, [summaryExpanded]);

  useEffect(() => {
    if (resolvedSelectedMemberRunDetail) setSubagentDetailError(null);
  }, [resolvedSelectedMemberRunDetail]);

  const submit = (event: FormEvent): void => {
    event.preventDefault();
    const content = draft.trim();
    if (!content || posting) return;
    void onPost(content).then(() => setDraft('')).catch(() => undefined);
  };

  if (!detail && loading) {
    return <div className="channel-workspace channel-workspace-state"><RefreshCw className="channel-spinner" size={18} /><span>Loading channel…</span></div>;
  }
  if (!detail) {
    return <div className="channel-workspace channel-workspace-state"><span>{error ?? 'Channel unavailable.'}</span></div>;
  }

  return (
    <div
      ref={containerRef}
      className={`main-session-grid channel-workspace-layout${summaryExpanded ? ' research-details-open' : ''}`}
      style={{ '--research-side-panel-width': `${panelWidth}px` } as CSSProperties}
    >
    <section className="channel-workspace" aria-label={`Channel ${detail.channel.name}`}>
      <header className="channel-workspace-header">
        <div className="channel-workspace-heading">
          <span className="channel-workspace-icon"><Hash size={17} aria-hidden="true" /></span>
          <div>
            <h1>{detail.channel.name}</h1>
          </div>
        </div>
        <div className="channel-workspace-actions">
          <span className="channel-member-count" title="Participants"><Users size={14} aria-hidden="true" />{detail.members.length}</span>
          <button type="button" title="Refresh channel" aria-label="Refresh channel" onClick={onRefresh}><RefreshCw size={14} /></button>
          <button
            type="button"
            className="channel-delete-button"
            title="Delete channel"
            aria-label="Delete channel"
            onClick={() => {
              if (window.confirm(`Delete #${detail.channel.name} and its transcript? This cannot be undone.`)) {
                void onDelete().catch(() => undefined);
              }
            }}
          ><Trash2 size={14} /></button>
        </div>
      </header>

      {error ? <div className="channel-inline-error">{error}</div> : null}
      <div className={`channel-transcript-scroll${scrollEdges.top ? ' has-top-shadow' : ''}${scrollEdges.bottom ? ' has-bottom-shadow' : ''}`}>
        <div className="channel-transcript" ref={transcriptRef} onScroll={updateScrollEdges}>
        {detail.messages.length === 0 ? (
          <div className="channel-empty-transcript">
            <Hash size={22} aria-hidden="true" />
            <strong>Start the research record</strong>
            <span>Messages and future agent work stay with this workspace until the channel is deleted.</span>
          </div>
        ) : detail.messages.map((message) => {
          const sharedResource = sharedResourceByMessageId.get(message.id) ?? null;
          const messageMember = (message.memberId ? memberById.get(message.memberId) : undefined)
            ?? memberByMessageIdentity.get(channelMessageMemberKey(message.sessionId, message.senderAgentPath))
            ?? null;
          const hideGeneratedShareBody = sharedResource
            && message.metadata.source === 'channel_share'
            && message.contentMarkdown === `Shared a ${sharedResource.kind}.`;
          return (
            <article className={`channel-message channel-message-${message.kind}`} key={message.id}>
              <div className="channel-message-avatar" aria-hidden="true">{channelSenderInitial(message.senderAgentPath)}</div>
              <div className="channel-message-content">
              <header>
                <div className="channel-message-author">
                  <div className="channel-message-author-heading">
                    <span className="channel-message-sender">{channelSubagentName(message.senderAgentPath)}</span>
                    <span className="channel-message-kind">{channelMessageKindLabel(message.kind)}</span>
                  </div>
                  {messageMember?.model ? (
                    <div className="channel-message-model">
                      <ProviderIcon
                        className="channel-message-provider-icon"
                        provider={messageMember.provider ?? messageMember.model}
                        size={13}
                        aria-hidden="true"
                      />
                      <span>{subagentModelDisplayName(messageMember.provider, messageMember.model, providerModelCatalog)}</span>
                    </div>
                  ) : null}
                </div>
                <time dateTime={message.createdAt} title={formatChannelTimestamp(message.createdAt)}>
                  {formatChannelMessageTime(message.createdAt)}
                </time>
              </header>
              {!hideGeneratedShareBody ? <div className="channel-message-body">{renderTraceProseText(message.contentMarkdown, 'agent_output')}</div> : null}
              {message.evidenceRefs.length > 0 ? <div className="channel-message-evidence">Evidence: {message.evidenceRefs.join(', ')}</div> : null}
              {sharedResource ? (
                <button
                  type="button"
                  className="channel-message-shared-resource"
                  title={`Open ${sharedResource.title}`}
                  onClick={() => {
                    setSelectedMemberId(null);
                    setSelectedSharedResourceId(sharedResource.id);
                    onSummaryExpandedChange(true);
                  }}
                >
                  <ChannelSharedResourceIcon kind={sharedResource.kind} />
                  <span>{channelSharedMessageLabel(sharedResource.kind)}</span>
                  <ChevronRight size={14} aria-hidden="true" />
                </button>
              ) : null}
              </div>
            </article>
          );
        })}
        </div>
      </div>

      <form className="channel-composer" onSubmit={submit}>
        <textarea
          ref={composerTextareaRef}
          value={draft}
          rows={1}
          placeholder={`Message #${detail.channel.name}`}
          aria-label={`Message channel ${detail.channel.name}`}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && !event.shiftKey) {
              event.preventDefault();
              submit(event);
            }
          }}
        />
        <span className="channel-composer-destination" aria-hidden="true">
          <Hash size={13} />
          <span>{detail.channel.name}</span>
        </span>
        <button type="submit" className="main-steer-send channel-composer-send" title="Post message" aria-label="Post message" disabled={posting || !draft.trim()}>
          <ArrowRight size={16} aria-hidden="true" />
        </button>
      </form>
    </section>
    <div
      className="research-side-resize-handle"
      role="separator"
      aria-label="Resize channel summary sidebar"
      aria-orientation="vertical"
      aria-valuemin={MIN_RESEARCH_SIDE_PANEL_WIDTH}
      aria-valuemax={maximumPanelWidth}
      aria-valuenow={panelWidth}
      aria-hidden={summaryExpanded}
      tabIndex={summaryExpanded ? -1 : 0}
      onKeyDown={summaryExpanded ? undefined : handleResizeKeyDown}
      onPointerDown={summaryExpanded ? undefined : beginResize}
    />
    <div className="research-side-column">
      {selectedMember ? (
        <SubagentDetailPanel
          detail={resolvedSelectedMemberRunDetail}
          error={subagentDetailError}
          events={selectedMemberEvents}
          providerModelCatalog={providerModelCatalog}
          runId={selectedMemberRunId ?? 'unavailable'}
          subagent={{
            path: selectedMember.agentPath,
            name: channelSubagentName(selectedMember.agentPath),
            provider: selectedMember.provider,
            model: selectedMember.model,
            status: selectedMember.status
          }}
          backLabel="Channel"
          onBack={() => {
            setSelectedMemberId(null);
            setSubagentDetailError(null);
            onSummaryExpandedChange(false);
          }}
        />
      ) : selectedSharedResource ? (
        <ChannelSharedResourcePanel
          resource={selectedSharedResource}
          onBack={() => {
            setSelectedSharedResourceId(null);
            onSummaryExpandedChange(false);
          }}
        />
      ) : (
        <ChannelSummaryPanel
          members={detail.members}
          messages={detail.messages}
          sharedResources={detail.sharedResources}
          onSelectMember={(member) => {
            setSubagentDetailError(member.sessionId ? null : 'This subagent session is unavailable.');
            setSelectedSharedResourceId(null);
            setSelectedMemberId(member.id);
            onSummaryExpandedChange(true);
          }}
        />
      )}
    </div>
    </div>
  );
}

function ChannelSummaryPanel({
  members,
  messages,
  sharedResources,
  onSelectMember
}: Pick<ResearchChannelDetail, 'members' | 'messages' | 'sharedResources'> & {
  onSelectMember: (member: ResearchChannelMemberRecord) => void;
}): JSX.Element {
  const subagents = members
    .filter((member) => member.agentPath !== '/root' && member.agentPath !== '/human')
    .sort(compareChannelMembers);

  return (
    <aside className="main-session-side session-summary-panel channel-summary-panel" aria-label="Channel summary">
      <section className="session-summary-card channel-summary-section">
        <header className="session-summary-heading">
          <h2 className="session-summary-title">Members</h2>
        </header>
        <div className="channel-summary-members" aria-label="Channel subagents">
          {subagents.length === 0 ? (
            <p className="channel-summary-empty">No subagents yet.</p>
          ) : subagents.map((member) => (
            <button
              type="button"
              className={`channel-summary-member${channelMemberActive(member.status) ? '' : ' is-inactive'}`}
              title={`Open ${channelSubagentName(member.agentPath)} details`}
              key={member.id}
              onClick={() => onSelectMember(member)}
            >
              <ChannelMemberStatus status={member.status} />
              <span className="channel-summary-member-name">{channelSubagentName(member.agentPath)}</span>
              <ChannelMemberMessageAge
                member={member}
                messages={messages}
              />
              <ChevronRight className="channel-summary-member-chevron" size={15} aria-hidden="true" />
            </button>
          ))}
        </div>
      </section>
      <section className="session-summary-card channel-summary-section">
        <header className="session-summary-heading">
          <h2 className="session-summary-title">Shared</h2>
        </header>
        <div className="channel-summary-shared" aria-label="Shared channel resources">
          {(['file', 'runbook', 'memory'] as const).map((kind) => (
            <div className="channel-summary-shared-row" key={kind}>
              <ChannelSharedResourceIcon kind={kind} />
              <span>{channelSharedCountLabel(kind)}</span>
              <strong>{sharedResources.filter((resource) => resource.kind === kind).length}</strong>
            </div>
          ))}
        </div>
      </section>
    </aside>
  );
}

function ChannelSharedResourcePanel({
  resource,
  onBack
}: {
  resource: ResearchChannelSharedResourceRecord;
  onBack: () => void;
}): JSX.Element {
  return (
    <aside className="main-session-side memory-catalog has-nested-view channel-shared-detail-panel" aria-label={`Shared ${resource.kind} details`}>
      <ResearchSideNestedHeader
        label="Shared"
        leading={<ChannelSharedResourceIcon kind={resource.kind} />}
        name={resource.title}
        onBack={onBack}
      />
      <div className="channel-shared-detail">
        <span className="channel-shared-detail-icon"><ChannelSharedResourceIcon kind={resource.kind} /></span>
        <span className="channel-shared-detail-kind">{channelSharedSingularLabel(resource.kind)}</span>
        <h2>{resource.title}</h2>
        <code>{resource.resourceId}</code>
        <p>Agents can read or update this resource with the corresponding {resource.kind} tools.</p>
        <dl>
          <div><dt>Shared by</dt><dd>{channelSenderLabel(resource.senderAgentPath)}</dd></div>
          <div><dt>Updated</dt><dd>{formatChannelTimestamp(resource.updatedAt)}</dd></div>
        </dl>
      </div>
    </aside>
  );
}

function ChannelSharedResourceIcon({ kind }: { kind: ResearchChannelSharedResourceKind }): JSX.Element {
  if (kind === 'runbook') return <BookOpen size={15} aria-hidden="true" />;
  if (kind === 'memory') return <Brain size={15} aria-hidden="true" />;
  return <FileText size={15} aria-hidden="true" />;
}

function channelSharedMessageLabel(kind: ResearchChannelSharedResourceKind): string {
  return `Shared a ${kind}`;
}

function channelSharedCountLabel(kind: ResearchChannelSharedResourceKind): string {
  if (kind === 'memory') return 'Memories';
  return `${kind.slice(0, 1).toLocaleUpperCase()}${kind.slice(1)}s`;
}

function channelSharedSingularLabel(kind: ResearchChannelSharedResourceKind): string {
  return `${kind.slice(0, 1).toLocaleUpperCase()}${kind.slice(1)}`;
}

function ChannelMemberStatus({ status }: { status: ResearchChannelMemberStatus }): JSX.Element {
  const label = channelMemberStatusLabel(status);
  if (channelMemberActive(status)) {
    return (
      <LoaderCircle
        className="channel-summary-member-spinner"
        size={13}
        aria-label={`Status: ${label}`}
      />
    );
  }
  return (
    <span
      className={`channel-summary-member-status is-${status}`}
      aria-label={`Status: ${label}`}
      title={label}
    />
  );
}

function ChannelMemberMessageAge({
  member,
  messages
}: {
  member: ResearchChannelMemberRecord;
  messages: ResearchChannelDetail['messages'];
}): JSX.Element {
  const latestMessage = messages.findLast((message) => message.memberId === member.id)
    ?? messages.findLast((message) => (
      message.memberId === null
      && message.senderAgentPath === member.agentPath
      && message.sessionId === member.sessionId
    ));
  if (!latestMessage) {
    return <span className="channel-summary-member-age" title="No messages yet">--</span>;
  }
  return (
    <time
      className="channel-summary-member-age"
      dateTime={latestMessage.createdAt}
      title={formatChannelTimestamp(latestMessage.createdAt)}
    >
      {shortRelativeAge(latestMessage.createdAt)}
    </time>
  );
}

function compareChannelMembers(left: ResearchChannelMemberRecord, right: ResearchChannelMemberRecord): number {
  const leftActive = channelMemberActive(left.status);
  const rightActive = channelMemberActive(right.status);
  if (leftActive !== rightActive) return leftActive ? -1 : 1;
  return right.lastSeenAt.localeCompare(left.lastSeenAt) || left.agentPath.localeCompare(right.agentPath);
}

function channelMemberActive(status: ResearchChannelMemberStatus): boolean {
  return status === 'pending' || status === 'running';
}

function channelMessageMemberKey(sessionId: string | null, agentPath: string): string {
  return `${sessionId ?? ''}\0${agentPath}`;
}

function channelMemberRunStatus(status: ResearchChannelMemberStatus): RunStatus {
  if (status === 'pending') return 'queued';
  if (status === 'running') return 'active';
  if (status === 'errored') return 'failed';
  if (status === 'interrupted') return 'stopped';
  return 'completed';
}

function channelMemberStatusLabel(status: ResearchChannelMemberStatus): string {
  if (status === 'errored') return 'Error';
  return `${status.slice(0, 1).toLocaleUpperCase()}${status.slice(1)}`;
}

export function channelSubagentName(path: string): string {
  return channelSenderLabel(path).replace(/(^|\s)(\p{L})/gu, (_match, prefix: string, letter: string) => (
    `${prefix}${letter.toLocaleUpperCase()}`
  ));
}

function channelMessageKindLabel(kind: ResearchChannelDetail['messages'][number]['kind']): string {
  return `${kind.slice(0, 1).toLocaleUpperCase()}${kind.slice(1)}`;
}

function channelSenderLabel(path: string): string {
  if (path === '/human') return 'You';
  if (path === '/root') return 'Lead agent';
  const name = path.split('/').filter(Boolean).at(-1) ?? path;
  return name.replaceAll('_', ' ');
}

function channelSenderInitial(path: string): string {
  return channelSenderLabel(path).slice(0, 1).toLocaleUpperCase();
}

function formatChannelTimestamp(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : new Intl.DateTimeFormat(undefined, {
    month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit'
  }).format(date);
}

export function formatChannelMessageTime(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  const parts = new Intl.DateTimeFormat('en-US', {
    hour: 'numeric', minute: '2-digit', hour12: true
  }).formatToParts(date);
  const hour = parts.find((part) => part.type === 'hour')?.value ?? '';
  const minute = parts.find((part) => part.type === 'minute')?.value ?? '';
  const dayPeriod = parts.find((part) => part.type === 'dayPeriod')?.value.slice(0, 1).toLocaleLowerCase() ?? '';
  return `${hour}:${minute}${dayPeriod}`;
}
