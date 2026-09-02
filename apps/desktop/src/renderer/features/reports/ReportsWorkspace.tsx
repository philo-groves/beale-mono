import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { JSX } from 'react';
import { ArrowUp, CircleAlert, CircleDot, FileArchive, FileText, LoaderCircle, Pencil, Video } from 'lucide-react';
import type {
  AppServerReportDocument,
  AppServerReportSummary,
  AppServerReportTriageStatus,
  WorkspaceRegistryEntry
} from '@shared/types';
import { CenteredLoadingState } from '../../app/CenteredLoadingState';
import { traceLabel } from '../../lib/formatting';
import { scrollFadeClasses } from '../../lib/scrollFade';
import {
  replaceReportMarkdownBlock,
  reportChangeInstruction,
  reportMarkdownBlocks,
  reportTitleFromMarkdown
} from '../../view-models/reports';
import { renderTraceProseText } from '../traces/traceMarkup';

const REPORT_TRIAGE_STATUS_OPTIONS: ReadonlyArray<{ value: AppServerReportTriageStatus; label: string }> = [
  { value: 'editing', label: 'Editing' },
  { value: 'submitted', label: 'Submitted' },
  { value: 'reviewing', label: 'Reviewing' },
  { value: 'rejected', label: 'Rejected' },
  { value: 'accepted', label: 'Accepted' }
];

export function ReportsIndex({
  reports,
  workspaces,
  selectedWorkspaceId,
  loading,
  error,
  onScopeChange,
  onOpenReport
}: {
  reports: readonly AppServerReportSummary[];
  workspaces: readonly WorkspaceRegistryEntry[];
  selectedWorkspaceId: string | null;
  loading: boolean;
  error: string | null;
  onScopeChange: (workspaceId: string | null) => void;
  onOpenReport: (report: AppServerReportSummary) => void;
}): JSX.Element {
  const scopeTabs = [
    { id: null, key: 'all', label: 'All Reports' },
    ...workspaces
      .filter((workspace) => workspace.workspaceId.length > 0)
      .map((workspace) => ({ id: workspace.workspaceId, key: workspace.id, label: workspace.workspaceName }))
  ];
  const currentScopeName = selectedWorkspaceId
    ? workspaces.find((workspace) => workspace.workspaceId === selectedWorkspaceId)?.workspaceName ?? 'Workspace'
    : 'All';
  return (
    <section className="reports-index" aria-label="Reporting">
      <div className="reports-index-tabs research-side-view-tabs research-side-view-tabs-scrollable" role="tablist" aria-label="Report workspace scope">
        {scopeTabs.map((scope) => {
          const selected = selectedWorkspaceId === scope.id;
          return (
            <div className={`research-side-view-tab provider-settings-tab reports-index-tab ${selected ? 'active' : ''}`.trim()} key={scope.key}>
              <button
                type="button"
                className="research-side-view-tab-activate"
                role="tab"
                aria-selected={selected}
                aria-controls="reports-index-panel"
                onClick={() => onScopeChange(scope.id)}
              >
                <span>{scope.label}</span>
              </button>
            </div>
          );
        })}
      </div>
      <header className="resource-workspace-heading">
        <h1>{currentScopeName} Reporting</h1>
        <p>Review, edit, and prepare reports created during research sessions.</p>
      </header>
      <div id="reports-index-panel" role="tabpanel">
        {loading ? (
          <CenteredLoadingState label="Loading reports…" />
        ) : error ? (
          <div className="reports-index-empty is-error" role="alert">
            <CircleAlert size={20} aria-hidden="true" />
            <strong>Reports could not be loaded</strong>
            <span>{error}</span>
          </div>
        ) : reports.length === 0 ? (
          <div className="reports-index-empty">
            <FileText size={20} aria-hidden="true" />
            <strong>No reports yet</strong>
            <span>Reports created by agents during research sessions will appear here.</span>
          </div>
        ) : (
          <div className="reports-index-list">
            {reports.map((report) => (
              <div className="reports-index-row" key={`${report.workspaceId}:${report.id}`}>
                <span className="reports-index-row-copy">
                  <strong>{report.title}</strong>
                  <small>{traceLabel(report.triageStatus)}</small>
                </span>
                <button
                  type="button"
                  className="reports-index-edit-button"
                  onClick={() => onOpenReport(report)}
                >
                  <Pencil size={14} aria-hidden="true" />
                  <span>Edit</span>
                </button>
              </div>
            ))}
          </div>
        )}
      </div>
    </section>
  );
}

export function ReportSessionWorkspace({
  report,
  document,
  loading,
  error,
  onReportChange,
  onReportMarkdownChange,
  onStatusChange,
  onChooseSubmissionPacket,
  onChooseRecording
}: {
  report: AppServerReportSummary;
  document: AppServerReportDocument | null;
  loading: boolean;
  error: string | null;
  onReportChange: (instruction: string) => Promise<void>;
  onReportMarkdownChange: (content: string) => Promise<void>;
  onStatusChange: (status: AppServerReportTriageStatus) => Promise<void>;
  onChooseSubmissionPacket: () => Promise<void>;
  onChooseRecording: () => Promise<void>;
}): JSX.Element {
  return (
    <div className="report-session-grid">
      <EditableReport
        report={report}
        document={document}
        loading={loading}
        error={error}
        onChange={onReportChange}
        onMarkdownChange={onReportMarkdownChange}
      />
      <div className="report-session-sidenav-gutter" aria-hidden="true" />
      <ReportSummarySidebar
        report={report}
        onStatusChange={onStatusChange}
        onChooseSubmissionPacket={onChooseSubmissionPacket}
        onChooseRecording={onChooseRecording}
      />
    </div>
  );
}

export function EditableReport({
  report,
  document,
  loading,
  error,
  onChange,
  onMarkdownChange
}: {
  report: AppServerReportSummary;
  document: AppServerReportDocument | null;
  loading: boolean;
  error: string | null;
  onChange: (instruction: string) => Promise<void>;
  onMarkdownChange: (content: string) => Promise<void>;
}): JSX.Element {
  const blocks = useMemo(() => reportMarkdownBlocks(document?.content ?? ''), [document?.content]);
  const title = reportTitleFromMarkdown(document?.content ?? '', report.title);
  const [editingBlockId, setEditingBlockId] = useState<string | null>(null);
  const [markdownDraft, setMarkdownDraft] = useState('');
  const [markdownBaseline, setMarkdownBaseline] = useState('');
  const [markdownEditorHeight, setMarkdownEditorHeight] = useState(0);
  const [changeRequest, setChangeRequest] = useState('');
  const [changePending, setChangePending] = useState(false);
  const [markdownPending, setMarkdownPending] = useState(false);
  const [changeError, setChangeError] = useState<string | null>(null);
  const editorRef = useRef<HTMLTextAreaElement | null>(null);
  const scrollFrameRef = useRef<HTMLElement | null>(null);
  const documentScrollRef = useRef<HTMLDivElement | null>(null);

  const updateScrollEdges = useCallback((): void => {
    const frame = scrollFrameRef.current;
    const scroll = documentScrollRef.current;
    if (!frame || !scroll) return;
    const fadeClasses = scrollFadeClasses(scroll);
    frame.classList.toggle('has-top-fade', fadeClasses['has-top-fade']);
    frame.classList.toggle('has-bottom-fade', fadeClasses['has-bottom-fade']);
  }, []);

  useLayoutEffect(() => {
    const scroll = documentScrollRef.current;
    if (!scroll) return undefined;
    const animationFrame = window.requestAnimationFrame(updateScrollEdges);
    const resizeObserver = new ResizeObserver(updateScrollEdges);
    resizeObserver.observe(scroll);
    const content = scroll.firstElementChild;
    if (content) resizeObserver.observe(content);
    return () => {
      window.cancelAnimationFrame(animationFrame);
      resizeObserver.disconnect();
    };
  }, [document?.content, error, loading, updateScrollEdges]);

  useEffect(() => {
    if (editingBlockId) editorRef.current?.focus();
  }, [editingBlockId]);
  useEffect(() => {
    setEditingBlockId(null);
    setMarkdownDraft('');
    setMarkdownBaseline('');
    setMarkdownEditorHeight(0);
    setChangeRequest('');
    setChangePending(false);
    setMarkdownPending(false);
    setChangeError(null);
  }, [report.id]);

  const openBlock = (blockId: string, content: string, height: number): void => {
    setEditingBlockId(blockId);
    setMarkdownDraft(content);
    setMarkdownBaseline(content);
    setMarkdownEditorHeight(Math.ceil(height));
    setChangeRequest('');
    setChangeError(null);
  };

  const saveMarkdownDraft = async (
    block: (typeof blocks)[number]
  ): Promise<void> => {
    if (!document || markdownDraft === markdownBaseline) return;
    const nextContent = replaceReportMarkdownBlock(document.content, block, markdownDraft);
    setMarkdownPending(true);
    setChangeError(null);
    try {
      await onMarkdownChange(nextContent);
      setMarkdownBaseline(markdownDraft);
    } catch (caught: unknown) {
      setChangeError(caught instanceof Error ? caught.message : String(caught));
      throw caught;
    } finally {
      setMarkdownPending(false);
    }
  };

  const closeBlock = async (block: (typeof blocks)[number], save: boolean): Promise<void> => {
    try {
      if (save) await saveMarkdownDraft(block);
      setEditingBlockId((current) => current === block.id ? null : current);
    } catch {
      editorRef.current?.focus();
    }
  };

  const requestEdit = async (block: (typeof blocks)[number]): Promise<void> => {
    const instruction = reportChangeInstruction({
      ...block,
      content: markdownDraft,
      endLine: block.startLine + markdownDraft.replace(/\r\n?/g, '\n').split('\n').length - 1
    }, changeRequest);
    if (!instruction) return;
    setChangePending(true);
    setChangeError(null);
    try {
      await saveMarkdownDraft(block);
      await onChange(instruction);
      setEditingBlockId(null);
      setChangeRequest('');
    } catch (caught: unknown) {
      setChangeError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setChangePending(false);
    }
  };

  return (
    <section ref={scrollFrameRef} className="report-session-document" aria-label={`Report: ${title}`}>
      <div ref={documentScrollRef} className="report-session-document-scroll" onScroll={updateScrollEdges}>
        {loading && !document ? (
          <div className="report-session-state"><LoaderCircle className="runbook-view-spinner" size={18} /> Loading report.</div>
        ) : error ? (
          <div className="report-session-state is-error"><CircleAlert size={18} /> {error}</div>
        ) : document ? (
          <article className="report-session-content" aria-label="Editable report content">
            {blocks.map((block) => {
              const editing = editingBlockId === block.id;
              return (
                <section
                  className={`report-editable-block${editing ? ' is-editing' : ''}`}
                  key={block.id}
                  onBlur={(event) => {
                    if (!editing || changePending || markdownPending || event.currentTarget.contains(event.relatedTarget)) return;
                    void closeBlock(block, true);
                  }}
                >
                  {editing ? (
                    <>
                      <textarea
                        ref={editorRef}
                        className="report-markdown-editor"
                        value={markdownDraft}
                        aria-label={`Edit report lines ${block.startLine} through ${block.endLine}`}
                        spellCheck={false}
                        disabled={changePending || markdownPending}
                        style={{ height: markdownEditorHeight }}
                        onChange={(event) => setMarkdownDraft(event.target.value)}
                        onKeyDown={(event) => {
                          if (event.key === 'Escape') {
                            event.preventDefault();
                            setEditingBlockId(null);
                          }
                        }}
                      />
                      <form className="report-edit-request" onSubmit={(event) => {
                        event.preventDefault();
                        void requestEdit(block);
                      }}>
                        <input
                          value={changeRequest}
                          placeholder="Request an edit"
                          aria-label="Request an edit"
                          disabled={changePending || markdownPending}
                          onChange={(event) => setChangeRequest(event.target.value)}
                        />
                        <button
                          type="submit"
                          className="main-steer-send report-edit-request-send"
                          title="Send edit request"
                          aria-label="Send edit request"
                          disabled={changePending || markdownPending || !changeRequest.trim()}
                        >
                          <ArrowUp size={16} />
                        </button>
                      </form>
                      {changeError ? <p className="report-inline-change-error" role="alert">{changeError}</p> : null}
                    </>
                  ) : (
                    <div
                      className="report-editable-block-content"
                      role="button"
                      tabIndex={0}
                      aria-label={`Edit report lines ${block.startLine} through ${block.endLine}`}
                      onClick={(event) => openBlock(block.id, block.content, event.currentTarget.getBoundingClientRect().height)}
                      onKeyDown={(event) => {
                        if (event.key !== 'Enter' && event.key !== ' ') return;
                        event.preventDefault();
                        openBlock(block.id, block.content, event.currentTarget.getBoundingClientRect().height);
                      }}
                    >
                      {renderTraceProseText(block.content, 'agent_output')}
                    </div>
                  )}
                </section>
              );
            })}
          </article>
        ) : (
          <div className="report-session-state">This report has no content.</div>
        )}
      </div>
    </section>
  );
}

export function ReportSummarySidebar({
  report,
  onStatusChange,
  onChooseSubmissionPacket,
  onChooseRecording
}: {
  report: AppServerReportSummary;
  onStatusChange: (status: AppServerReportTriageStatus) => Promise<void>;
  onChooseSubmissionPacket: () => Promise<void>;
  onChooseRecording: () => Promise<void>;
}): JSX.Element {
  const [choosing, setChoosing] = useState<'packet' | 'recording' | null>(null);
  const [statusPending, setStatusPending] = useState(false);
  const [attachmentError, setAttachmentError] = useState<string | null>(null);

  const choose = async (kind: 'packet' | 'recording'): Promise<void> => {
    setChoosing(kind);
    setAttachmentError(null);
    try {
      await (kind === 'packet' ? onChooseSubmissionPacket() : onChooseRecording());
    } catch (caught: unknown) {
      setAttachmentError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setChoosing(null);
    }
  };

  const changeStatus = async (triageStatus: AppServerReportTriageStatus): Promise<void> => {
    if (triageStatus === report.triageStatus) return;
    setStatusPending(true);
    setAttachmentError(null);
    try {
      await onStatusChange(triageStatus);
    } catch (caught: unknown) {
      setAttachmentError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setStatusPending(false);
    }
  };

  return (
    <aside className="main-session-side session-summary-panel report-summary-panel" aria-label="Report summary">
      <section className="session-summary-card">
        <header className="session-summary-heading">
          <h2 className="session-summary-title">Report</h2>
        </header>
        <section className="session-summary-items" aria-label="Report summary details">
          <label className="session-summary-item report-summary-item report-summary-status">
            <CircleDot size={15} aria-hidden="true" />
            <span>Status</span>
            <select
              value={report.triageStatus}
              aria-label="Report status"
              disabled={statusPending || choosing !== null}
              onChange={(event) => void changeStatus(event.target.value as AppServerReportTriageStatus)}
            >
              {REPORT_TRIAGE_STATUS_OPTIONS.map((option) => (
                <option value={option.value} key={option.value}>{option.label}</option>
              ))}
            </select>
          </label>
          <button
            type="button"
            className="session-summary-item report-summary-item"
            disabled={choosing !== null || statusPending}
            title={report.submissionPacket?.filename ?? 'Choose File'}
            onClick={() => void choose('packet')}
          >
            <FileArchive size={15} aria-hidden="true" />
            <span>Packet</span>
            <span className="session-summary-meta">{choosing === 'packet' ? 'Choosing…' : report.submissionPacket?.filename ?? 'Choose File'}</span>
          </button>
          <button
            type="button"
            className="session-summary-item report-summary-item"
            disabled={choosing !== null || statusPending}
            title={report.recording?.filename ?? 'Choose File'}
            onClick={() => void choose('recording')}
          >
            <Video size={15} aria-hidden="true" />
            <span>Recording</span>
            <span className="session-summary-meta">{choosing === 'recording' ? 'Choosing…' : report.recording?.filename ?? 'Choose File'}</span>
          </button>
        </section>
        {attachmentError ? <p className="report-summary-error" role="alert">{attachmentError}</p> : null}
      </section>
    </aside>
  );
}
