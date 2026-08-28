import { memo, useEffect, useMemo, useRef, useState } from 'react';
import type {
  JSX,
  KeyboardEvent as ReactKeyboardEvent,
  PointerEvent as ReactPointerEvent
} from 'react';
import { ChevronDown, ChevronUp, FileText, MessageSquare, X } from 'lucide-react';
import type {
  ApprovalRecord,
  PolicyReviewDecision,
  ResearchModelSelection,
  ResearchProviderModelCatalog,
  RunRecord,
  RunStatus,
  ShellSafetyMode,
  SteeringAction
} from '@shared/types';
import { CommentaryView } from '../commentary/CommentaryView';
import { BealeWelcomeIcon } from '../../app/BealeWelcomeIcon';
import { useRunDetailPolling } from '../../hooks/useRunDetailPolling';
import { buildTraceDisplayEvents } from '../../view-models/traceDisplay';
import { errorMessage } from '../../lib/errors';

const QUICK_CHAT_MIN_WIDTH = 280;
const QUICK_CHAT_MIN_HEIGHT = 260;
const QUICK_CHAT_HORIZONTAL_INSET = 28;
const QUICK_CHAT_VERTICAL_INSET = 86;
const QUICK_CHAT_KEYBOARD_RESIZE_STEP = 16;
const QUICK_CHAT_EXIT_ANIMATION_MS = 180;

interface QuickChatSize {
  width: number;
  height: number;
}

export interface QuickChatDescriptor {
  id: string;
  runId?: string;
  runState?: RunStatus;
  kind?: 'quick-chat' | 'report-edit';
  title?: string;
}

interface PresentQuickChat {
  chat: QuickChatDescriptor;
  exiting: boolean;
}

export const QuickChatDock = memo(function QuickChatDock({
  chats,
  providerModelCatalog,
  initialModelSelection,
  onClose,
  onRunStarted,
  reportShellApproval = null,
  reportShellApprovalBusy = false,
  reportBusy = false,
  reportResponseSuggestionsEnabled = true,
  dangerModeEnabled = false,
  onReportInitialInstruction,
  onReportSessionAction,
  onReportShellApprovalDecision
}: {
  chats: QuickChatDescriptor[];
  providerModelCatalog: ResearchProviderModelCatalog[];
  initialModelSelection?: ResearchModelSelection;
  onClose: (id: string) => void;
  onRunStarted: (chatId: string, run: RunRecord) => void;
  reportShellApproval?: ApprovalRecord | null;
  reportShellApprovalBusy?: boolean;
  reportBusy?: boolean;
  reportResponseSuggestionsEnabled?: boolean;
  dangerModeEnabled?: boolean;
  onReportInitialInstruction?: (
    instruction: string,
    modelSelection: ResearchModelSelection,
    shellSafetyMode: ShellSafetyMode
  ) => Promise<RunRecord>;
  onReportSessionAction?: (action: SteeringAction) => Promise<RunRecord | null>;
  onReportShellApprovalDecision?: (decision: PolicyReviewDecision) => void;
}): JSX.Element | null {
  const [presentChats, setPresentChats] = useState<PresentQuickChat[]>(() => (
    chats.map((chat) => ({ chat, exiting: false }))
  ));

  useEffect(() => {
    setPresentChats((current) => {
      const incoming = new Map(chats.map((chat) => [chat.id, chat]));
      const currentIds = new Set(current.map(({ chat }) => chat.id));
      return [
        ...current.map((entry) => {
          const nextChat = incoming.get(entry.chat.id);
          return nextChat ? { chat: nextChat, exiting: false } : { ...entry, exiting: true };
        }),
        ...chats
          .filter((chat) => !currentIds.has(chat.id))
          .map((chat) => ({ chat, exiting: false }))
      ];
    });
  }, [chats]);

  useEffect(() => {
    const exitingIds = presentChats.filter((entry) => entry.exiting).map((entry) => entry.chat.id);
    if (exitingIds.length === 0) return undefined;
    const timeout = window.setTimeout(() => {
      setPresentChats((current) => current.filter((entry) => !entry.exiting));
    }, QUICK_CHAT_EXIT_ANIMATION_MS);
    return () => window.clearTimeout(timeout);
  }, [presentChats]);

  if (presentChats.length === 0) return null;
  return (
    <aside className="quick-chat-dock" aria-label="Quick chats">
      {presentChats.map(({ chat, exiting }) => (
        <QuickChatCard
          key={chat.id}
          chat={chat}
          exiting={exiting}
          providerModelCatalog={providerModelCatalog}
          initialModelSelection={initialModelSelection}
          onClose={onClose}
          onRunStarted={onRunStarted}
          reportShellApproval={reportShellApproval}
          reportShellApprovalBusy={reportShellApprovalBusy}
          reportBusy={reportBusy}
          reportResponseSuggestionsEnabled={reportResponseSuggestionsEnabled}
          dangerModeEnabled={dangerModeEnabled}
          onReportInitialInstruction={onReportInitialInstruction}
          onReportSessionAction={onReportSessionAction}
          onReportShellApprovalDecision={onReportShellApprovalDecision}
        />
      ))}
    </aside>
  );
});

function QuickChatCard({
  chat,
  exiting,
  providerModelCatalog,
  initialModelSelection,
  onClose,
  onRunStarted,
  reportShellApproval,
  reportShellApprovalBusy,
  reportBusy,
  reportResponseSuggestionsEnabled,
  dangerModeEnabled,
  onReportInitialInstruction,
  onReportSessionAction,
  onReportShellApprovalDecision
}: {
  chat: QuickChatDescriptor;
  exiting: boolean;
  providerModelCatalog: ResearchProviderModelCatalog[];
  initialModelSelection?: ResearchModelSelection;
  onClose: (id: string) => void;
  onRunStarted: (chatId: string, run: RunRecord) => void;
  reportShellApproval: ApprovalRecord | null;
  reportShellApprovalBusy: boolean;
  reportBusy: boolean;
  reportResponseSuggestionsEnabled: boolean;
  dangerModeEnabled: boolean;
  onReportInitialInstruction?: (
    instruction: string,
    modelSelection: ResearchModelSelection,
    shellSafetyMode: ShellSafetyMode
  ) => Promise<RunRecord>;
  onReportSessionAction?: (action: SteeringAction) => Promise<RunRecord | null>;
  onReportShellApprovalDecision?: (decision: PolicyReviewDecision) => void;
}): JSX.Element {
  const reportEditing = chat.kind === 'report-edit';
  const [collapsed, setCollapsed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [runId, setRunId] = useState<string | null>(chat.runId ?? null);
  const [runState, setRunState] = useState<RunStatus | null>(chat.runState ?? null);
  const [size, setSize] = useState<QuickChatSize | null>(null);
  const cardRef = useRef<HTMLElement | null>(null);
  const closedRef = useRef(false);
  const activeResizeCleanupRef = useRef<(() => void) | null>(null);
  const { runDetail, primeRunDetail } = useRunDetailPolling({
    selectedRunId: runId,
    selectedRunState: runState,
    projection: 'commentary',
    refreshKey: chat.id,
    onError: setError
  });
  const events = useMemo(() => runDetail ? buildTraceDisplayEvents(runDetail) : [], [runDetail]);

  useEffect(() => {
    if (runDetail?.run.status) setRunState(runDetail.run.status);
  }, [runDetail?.run.status]);

  useEffect(() => {
    if (!chat.runId) return;
    setRunId(chat.runId);
    setRunState(chat.runState ?? null);
  }, [chat.runId, chat.runState]);

  useEffect(
    () => () => {
      activeResizeCleanupRef.current?.();
      document.body.classList.remove('is-resizing-quick-chat');
    },
    []
  );

  const resizeTo = (width: number, height: number): void => {
    setSize({
      width: clampQuickChatDimension(width, QUICK_CHAT_MIN_WIDTH, window.innerWidth - QUICK_CHAT_HORIZONTAL_INSET),
      height: clampQuickChatDimension(height, QUICK_CHAT_MIN_HEIGHT, window.innerHeight - QUICK_CHAT_VERTICAL_INSET)
    });
  };

  const beginResize = (event: ReactPointerEvent<HTMLButtonElement>): void => {
    if (event.button !== 0) return;
    const bounds = cardRef.current?.getBoundingClientRect();
    if (!bounds) return;

    event.preventDefault();
    activeResizeCleanupRef.current?.();
    const pointerId = event.pointerId;
    const startX = event.clientX;
    const startY = event.clientY;
    const target = event.currentTarget;
    target.setPointerCapture(pointerId);
    document.body.classList.add('is-resizing-quick-chat');

    const handlePointerMove = (moveEvent: PointerEvent): void => {
      resizeTo(
        bounds.width + startX - moveEvent.clientX,
        bounds.height + startY - moveEvent.clientY
      );
    };
    const cleanup = (): void => {
      document.body.classList.remove('is-resizing-quick-chat');
      if (target.hasPointerCapture(pointerId)) target.releasePointerCapture(pointerId);
      window.removeEventListener('pointermove', handlePointerMove);
      window.removeEventListener('pointerup', cleanup);
      window.removeEventListener('pointercancel', cleanup);
      activeResizeCleanupRef.current = null;
    };

    activeResizeCleanupRef.current = cleanup;
    window.addEventListener('pointermove', handlePointerMove);
    window.addEventListener('pointerup', cleanup);
    window.addEventListener('pointercancel', cleanup);
  };

  const resizeWithKeyboard = (event: ReactKeyboardEvent<HTMLButtonElement>): void => {
    const bounds = cardRef.current?.getBoundingClientRect();
    if (!bounds) return;
    let width = bounds.width;
    let height = bounds.height;
    if (event.key === 'ArrowLeft') width += QUICK_CHAT_KEYBOARD_RESIZE_STEP;
    else if (event.key === 'ArrowRight') width -= QUICK_CHAT_KEYBOARD_RESIZE_STEP;
    else if (event.key === 'ArrowUp') height += QUICK_CHAT_KEYBOARD_RESIZE_STEP;
    else if (event.key === 'ArrowDown') height -= QUICK_CHAT_KEYBOARD_RESIZE_STEP;
    else return;

    event.preventDefault();
    resizeTo(width, height);
  };

  const applyAction = async (action: SteeringAction): Promise<void> => {
    setBusy(true);
    setError(null);
    try {
      const nextRun = reportEditing && onReportSessionAction
        ? await onReportSessionAction(action)
        : (await window.beale.steerRun(action)).runs.find((row) => row.run.id === action.runId)?.run ?? null;
      if (nextRun && !closedRef.current) setRunState(nextRun.status);
    } catch (caught: unknown) {
      if (!closedRef.current) setError(errorMessage(caught));
    } finally {
      if (!closedRef.current) setBusy(false);
    }
  };

  const close = (): void => {
    closedRef.current = true;
    if (runId && runState && ['queued', 'active', 'paused'].includes(runState)) {
      void window.beale.steerRun({ type: 'stop', runId, note: 'Quick chat closed.' }).catch(() => undefined);
    }
    onClose(chat.id);
  };

  const start = async (
    instruction: string,
    modelSelection: ResearchModelSelection
  ): Promise<void> => {
    setBusy(true);
    setError(null);
    try {
      const run = reportEditing && onReportInitialInstruction
        ? await onReportInitialInstruction(instruction, modelSelection, 'auto_review')
        : (await window.beale.startQuickChat({ promptMarkdown: instruction, modelSelection })).run;
      if (closedRef.current) {
        if (!reportEditing) void window.beale.steerRun({ type: 'stop', runId: run.id, note: 'Quick chat closed.' }).catch(() => undefined);
        return;
      }
      setRunId(run.id);
      setRunState(run.status);
      if (!reportEditing) onRunStarted(chat.id, run);
      primeRunDetail(run);
    } catch (caught: unknown) {
      if (!closedRef.current) setError(errorMessage(caught));
    } finally {
      if (!closedRef.current) setBusy(false);
    }
  };

  const title = reportEditing ? chat.title ?? 'Report' : quickChatTitle(runDetail?.run ?? null);
  return (
    <section
      ref={cardRef}
      className={`quick-chat-card${reportEditing ? ' is-report-edit' : ''}${collapsed ? ' is-collapsed' : ''}${exiting ? ' is-exiting' : ''}`}
      aria-label={title}
      style={!collapsed && size ? size : undefined}
    >
      {!collapsed ? (
        <button
          type="button"
          className="quick-chat-resize-anchor"
          title="Resize quick chat"
          aria-label="Resize quick chat"
          onPointerDown={beginResize}
          onKeyDown={resizeWithKeyboard}
        />
      ) : null}
      <header className="quick-chat-header">
        {reportEditing ? <FileText size={14} aria-hidden="true" /> : <MessageSquare size={14} aria-hidden="true" />}
        <span title={title}>{title}</span>
        <button type="button" title={collapsed ? 'Expand quick chat' : 'Collapse quick chat'} aria-label={collapsed ? 'Expand quick chat' : 'Collapse quick chat'} onClick={() => setCollapsed((value) => !value)}>
          {collapsed ? <ChevronUp size={14} /> : <ChevronDown size={14} />}
        </button>
        {!reportEditing ? (
          <button type="button" title="Close quick chat" aria-label="Close quick chat" onClick={close}>
            <X size={14} />
          </button>
        ) : null}
      </header>
      {!collapsed ? (
        <div className="quick-chat-body">
          {error ? <div className="quick-chat-error" role="alert">{error}</div> : null}
          <CommentaryView
            busy={busy || (reportEditing && reportBusy)}
            dangerModeEnabled={reportEditing && dangerModeEnabled}
            detail={runDetail}
            events={events}
            providerModelCatalog={providerModelCatalog}
            selectedRunId={runId}
            showBackToMain={false}
            searchHighlightQuery=""
            initialModelSelection={initialModelSelection}
            initialSafetyMode="auto_review"
            initialSuggestion={reportEditing && !runId ? 'Review and improve this report.' : undefined}
            emptyContent={(
              <div className="quick-chat-empty-state">
                <BealeWelcomeIcon />
                <p>This chat will be automatically archived.</p>
              </div>
            )}
            inputPlaceholder={reportEditing ? 'Describe a change to this report' : 'Any topic, any workspace'}
            ariaLabel={reportEditing ? 'Send a report editing message' : 'Send a quick chat message'}
            shellApproval={reportEditing ? reportShellApproval : null}
            shellApprovalBusy={reportEditing && reportShellApprovalBusy}
            showCollaboration={false}
            showSafetyMode={false}
            responseSuggestionsEnabled={reportEditing && reportResponseSuggestionsEnabled}
            onBackToMain={() => undefined}
            onInitialInstruction={(instruction, modelSelection) => {
              void start(instruction, modelSelection);
            }}
            onShellApprovalDecision={reportEditing ? onReportShellApprovalDecision : undefined}
            onSessionAction={(action) => {
              void applyAction(action);
            }}
            onSteerInstruction={(selectedRunId, instruction, modelSelection) => {
              void applyAction({ type: 'steer', runId: selectedRunId, instruction, modelSelection });
            }}
          />
        </div>
      ) : null}
    </section>
  );
}

function quickChatTitle(run: RunRecord | null): string {
  const title = run?.title.trim();
  return title && title !== 'Untitled research run' ? title : 'Quick Chat';
}

function clampQuickChatDimension(value: number, minimum: number, maximum: number): number {
  const boundedMaximum = Math.max(0, maximum);
  return Math.min(Math.max(Math.min(minimum, boundedMaximum), value), boundedMaximum);
}
