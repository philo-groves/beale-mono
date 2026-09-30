import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { JSX } from 'react';
import { ArrowLeft, ArrowUp, CircleAlert, CircleDot, FileArchive, FolderOpen, LoaderCircle, Pencil, RefreshCw, Video } from 'lucide-react';
import type {
  AppServerReportDocument,
  AppServerReportSummary,
  AppServerReportTriageStatus
} from '@shared/types';
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

export function ReportSessionWorkspace({
  report,
  submissionPacketPath,
  document,
  loading,
  error,
  onReportChange,
  onReportMarkdownChange,
  onBackToReports,
  onStatusChange,
  onChooseSubmissionPacket,
  onRevealSubmissionPacket,
  onChooseRecording
}: {
  report: AppServerReportSummary;
  submissionPacketPath: string | null;
  document: AppServerReportDocument | null;
  loading: boolean;
  error: string | null;
  onReportChange: (instruction: string) => Promise<void>;
  onReportMarkdownChange: (content: string) => Promise<void>;
  onBackToReports?: () => void;
  onStatusChange: (status: AppServerReportTriageStatus) => Promise<void>;
  onChooseSubmissionPacket: () => Promise<void>;
  onRevealSubmissionPacket: () => Promise<void>;
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
        onBackToReports={onBackToReports}
      />
      <div className="report-session-sidenav-gutter" aria-hidden="true" />
      <ReportSummarySidebar
        report={report}
        submissionPacketPath={submissionPacketPath}
        onStatusChange={onStatusChange}
        onChooseSubmissionPacket={onChooseSubmissionPacket}
        onRevealSubmissionPacket={onRevealSubmissionPacket}
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
  onMarkdownChange,
  onBackToReports
}: {
  report: AppServerReportSummary;
  document: AppServerReportDocument | null;
  loading: boolean;
  error: string | null;
  onChange: (instruction: string) => Promise<void>;
  onMarkdownChange: (content: string) => Promise<void>;
  onBackToReports?: () => void;
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
        {onBackToReports ? <button className="report-session-back-button" onClick={onBackToReports} type="button"><ArrowLeft size={16} aria-hidden="true" /> Back to Reports</button> : null}
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
  submissionPacketPath = null,
  onStatusChange,
  onChooseSubmissionPacket,
  onRevealSubmissionPacket = async () => undefined,
  onChooseRecording
}: {
  report: AppServerReportSummary;
  submissionPacketPath?: string | null;
  onStatusChange: (status: AppServerReportTriageStatus) => Promise<void>;
  onChooseSubmissionPacket: () => Promise<void>;
  onRevealSubmissionPacket?: () => Promise<void>;
  onChooseRecording: () => Promise<void>;
}): JSX.Element {
  const [choosing, setChoosing] = useState<'packet' | 'recording' | null>(null);
  const [revealingPacket, setRevealingPacket] = useState(false);
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

  const revealPacket = async (): Promise<void> => {
    if (!report.submissionPacket) return;
    setRevealingPacket(true);
    setAttachmentError(null);
    try {
      await onRevealSubmissionPacket();
    } catch (caught: unknown) {
      setAttachmentError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setRevealingPacket(false);
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
          <div className="session-summary-item report-summary-item report-summary-attachment">
            <FileArchive size={15} aria-hidden="true" />
            <span>Packet</span>
            <div
              className={`report-summary-file-tooltip${submissionPacketPath ? ' has-path-tooltip' : ''}`}
              data-tooltip={submissionPacketPath ?? undefined}
              aria-label={submissionPacketPath
                ? `${report.submissionPacket?.filename ?? 'Submission packet'}: ${submissionPacketPath}`
                : undefined}
            >
              <span className="session-summary-meta report-summary-file-name">
                {choosing === 'packet' ? 'Choosing…' : report.submissionPacket?.filename ?? 'No file'}
              </span>
            </div>
            <div className="report-summary-attachment-actions">
              <button
                type="button"
                className="report-summary-attachment-action"
                disabled={choosing !== null || revealingPacket || statusPending}
                title={report.submissionPacket ? `Replace ${report.submissionPacket.filename}` : 'Choose submission packet'}
                aria-label={report.submissionPacket ? `Replace ${report.submissionPacket.filename}` : 'Choose submission packet'}
                onClick={() => void choose('packet')}
              >
                <RefreshCw size={14} aria-hidden="true" />
              </button>
              <button
                type="button"
                className="report-summary-attachment-action"
                disabled={!report.submissionPacket || choosing !== null || revealingPacket || statusPending}
                title={report.submissionPacket ? `Show ${report.submissionPacket.filename} in file explorer` : 'No submission packet to show'}
                aria-label={report.submissionPacket ? `Show ${report.submissionPacket.filename} in file explorer` : 'No submission packet to show'}
                onClick={() => void revealPacket()}
              >
                <FolderOpen size={14} aria-hidden="true" />
              </button>
            </div>
          </div>
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
