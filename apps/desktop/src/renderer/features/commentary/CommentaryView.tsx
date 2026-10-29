import { memo, useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { JSX, ReactNode } from 'react';
import { ArrowLeft, ArrowRight, BookOpen, Bot, Brain, ChevronRight, CircleAlert, Database, FileText, Mail, Terminal, Wrench } from 'lucide-react';
import type {
  ApprovalRecord,
  PolicyReviewDecision,
  ProviderModelDefaults,
  ResearchCollaborationPreferences,
  ResearchModelProviderId,
  ResearchModelSelection,
  ResearchProviderModelCatalog,
  RunDetail,
  ShellSafetyMode,
  SteeringAction,
  WorkspaceScopeVersion
} from '@shared/types';
import { renderSearchHighlightedText, searchHighlightTerms } from '../search/searchHighlight';
import { renderTraceProseText } from '../traces/traceMarkup';
import { MainSteerArea, SessionLoadingState } from '../sessions/SessionComposer';
import { SurfaceThreatDiagram } from '../workspaces/SurfaceThreatDiagram';
import { HarnessFeedbackViewer } from './HarnessFeedbackViewer';
import { devInstrumentation, useDevRenderProbe } from '../../devInstrumentation';
import {
  commentaryMessagesForSession,
  commentaryRepositoryMetadataForScope,
  hydrateCommentaryToolCall,
  surfaceDiagramToolLine,
  type CommentaryToolCall,
  type CommentaryMessage
} from '../../view-models/commentary';
import type { TraceDisplayEvent } from '../../view-models/traceDisplay';
import { formatDurationHms } from '../../lib/formatting';
import { errorMessage } from '../../lib/errors';
import { scrollFadeClasses } from '../../lib/scrollFade';
import { INSET_SCROLLBAR_ACTIVE_MS } from '../../hooks/useInsetScrollbarActivation';
import {
  advanceCommentaryStreaming,
  commentaryStreamingModeForUserAgent,
  initialCommentaryStreamingState,
  reconcileCommentaryStreaming,
  visibleCommentaryMessages
} from './commentaryStreaming';

interface CommentaryScrollAnchor {
  messageId: string;
  offsetTop: number;
}

interface CommentaryScrollAnchorOptions {
  canUseMessageId?: (messageId: string) => boolean;
}

export const COMMENTARY_RENDER_WINDOW_SIZE = 120;
const COMMENTARY_ESTIMATED_MESSAGE_HEIGHT = 56;
const COMMENTARY_AUTO_FOLLOW_THRESHOLD = 112;
const COMMENTARY_FOLLOW_SCROLL_EASE_MS = 70;
const COMMENTARY_WINDOW_SLIDE_STEP = 30;
const COMMENTARY_WINDOW_EDGE_BUFFER = 280;
const returnToolCallWithoutLoading = async (toolCall: CommentaryToolCall): Promise<CommentaryToolCall> => toolCall;
const INLINE_SINGULAR_TOOL_NAMES = new Set(['file.read', 'shell.run']);

export type CommentaryViewState = 'new-research' | 'session';

export const CommentaryView = memo(function CommentaryView({
  busy,
  state = 'session',
  detail,
  sessionSetupPending = false,
  events,
  onLoadOlderHistory,
  activeScope = null,
  providerModelCatalog,
  providerModelDefaults,
  selectedRunId,
  showBackToMain,
  viewedAgentPath = '/root',
  showBackButton = showBackToMain,
  scrollScopeKey = selectedRunId,
  searchHighlightQuery,
  shellApproval = null,
  shellApprovalBusy = false,
  emptyContent,
  initialModelSelection,
  collaboration,
  initialSafetyMode,
  inputPlaceholder,
  instructionLocked = false,
  ariaLabel,
  showCollaboration = true,
  initialInstruction,
  preComposerContent,
  postComposerContent,
  responseSuggestionsEnabled = true,
  onBackToMain,
  onCancel,
  onCollaborationChange,
  onInitialInstruction,
  onShellApprovalDecision = () => undefined,
  onSessionAction,
  onSteerInstruction,
  onRequestToolCallDetail
}: {
  busy: boolean;
  state?: CommentaryViewState;
  detail: RunDetail | null;
  sessionSetupPending?: boolean;
  events: TraceDisplayEvent[];
  onLoadOlderHistory?: () => Promise<boolean>;
  activeScope?: WorkspaceScopeVersion | null;
  providerModelCatalog: ResearchProviderModelCatalog[];
  providerModelDefaults?: Partial<Record<ResearchModelProviderId, ProviderModelDefaults>>;
  selectedRunId: string | null;
  showBackToMain: boolean;
  viewedAgentPath?: string;
  showBackButton?: boolean;
  scrollScopeKey?: string | null;
  searchHighlightQuery: string;
  shellApproval?: ApprovalRecord | null;
  shellApprovalBusy?: boolean;
  emptyContent?: ReactNode;
  initialModelSelection?: ResearchModelSelection;
  collaboration?: ResearchCollaborationPreferences;
  initialSafetyMode?: ShellSafetyMode;
  inputPlaceholder?: string;
  instructionLocked?: boolean;
  ariaLabel?: string;
  showCollaboration?: boolean;
  initialInstruction?: string;
  preComposerContent?: ReactNode;
  postComposerContent?: ReactNode;
  responseSuggestionsEnabled?: boolean;
  onBackToMain: () => void;
  onCancel?: () => void;
  onCollaborationChange?: (collaboration: ResearchCollaborationPreferences) => void;
  onInitialInstruction?: (
    instruction: string,
    modelSelection: ResearchModelSelection,
    shellSafetyMode: ShellSafetyMode
  ) => void;
  onShellApprovalDecision?: (decision: PolicyReviewDecision) => void;
  onSessionAction: (action: SteeringAction) => void;
  onSteerInstruction: (runId: string, instruction: string, modelSelection: ResearchModelSelection) => void;
  onRequestToolCallDetail?: (toolCall: CommentaryToolCall) => Promise<CommentaryToolCall>;
}): JSX.Element | null {
  const repositoryMetadata = useMemo(() => commentaryRepositoryMetadataForScope(activeScope), [activeScope]);
  const messages = useMemo(() => devInstrumentation.time('commentary.buildMessages', () => {
    const projected = commentaryMessagesForSession(detail, events, {
      includeInitialPrompt: !showBackToMain,
      repositoryMetadata
    });
    if (!sessionSetupPending || !detail) return projected;
    return [...projected, {
      id: `session-setup:${detail.run.id}`,
      traceEventId: null,
      kind: 'progress',
      contentMarkdown: 'The session is in a setup phase. Please wait…',
      reasoningTraceLines: ['The session is in a setup phase. Please wait…'],
      createdAt: detail.run.startedAt ?? detail.run.createdAt
    } satisfies CommentaryMessage];
  }, {
    traceEvents: detail?.traceEvents.length ?? 0,
    displayEvents: events.length
  }), [detail, events, repositoryMetadata, sessionSetupPending, showBackToMain]);
  const streamingScopeKey = detail ? `${selectedRunId ?? detail.run.id}:${detail.run.id}:${showBackToMain}` : null;
  const streamingMode = commentaryStreamingModeForUserAgent(typeof navigator === 'undefined' ? '' : navigator.userAgent);
  const [reducedMotion, setReducedMotion] = useState(() => typeof window !== 'undefined'
    && typeof window.matchMedia === 'function'
    && window.matchMedia('(prefers-reduced-motion: reduce)').matches);
  const [streamingState, setStreamingState] = useState(() => initialCommentaryStreamingState(streamingScopeKey, messages));
  const iconAnimationTimersRef = useRef<Map<string, number>>(new Map());
  const streamEnabled = Boolean(detail && !reducedMotion && !searchHighlightQuery
    && typeof document !== 'undefined' && !document.hidden);
  useEffect(() => {
    if (typeof window.matchMedia !== 'function') return undefined;
    const preference = window.matchMedia('(prefers-reduced-motion: reduce)');
    const onChange = (): void => setReducedMotion(preference.matches);
    preference.addEventListener('change', onChange);
    return () => preference.removeEventListener('change', onChange);
  }, []);
  useLayoutEffect(() => {
    setStreamingState((current) => reconcileCommentaryStreaming(
      current, streamingScopeKey, messages, streamEnabled, Boolean(detail && !isRunWorkingStatus(detail.run.status)), streamingMode
    ));
  }, [detail?.run.status, messages, streamEnabled, streamingMode, streamingScopeKey]);
  useEffect(() => {
    if (!streamingState.pending.length) return undefined;
    const timer = window.setTimeout(() => setStreamingState(advanceCommentaryStreaming), 12);
    return () => window.clearTimeout(timer);
  }, [streamingState]);
  useEffect(() => {
    const activeId = streamingState.pending[0]?.id;
    if (!activeId || !streamingState.animatedIds.has(activeId) || iconAnimationTimersRef.current.has(activeId)) return;
    const timer = window.setTimeout(() => {
      iconAnimationTimersRef.current.delete(activeId);
      setStreamingState((current) => {
        if (!current.animatedIds.has(activeId)) return current;
        const animatedIds = new Set(current.animatedIds);
        animatedIds.delete(activeId);
        return { ...current, animatedIds };
      });
    }, 400);
    iconAnimationTimersRef.current.set(activeId, timer);
  }, [streamingState.pending, streamingState.animatedIds]);
  useEffect(() => () => {
    for (const timer of iconAnimationTimersRef.current.values()) window.clearTimeout(timer);
    iconAnimationTimersRef.current.clear();
  }, []);
  useEffect(() => {
    const onVisibilityChange = (): void => {
      if (document.hidden) setStreamingState((current) => current.pending.length
        ? initialCommentaryStreamingState(streamingScopeKey, messages) : current);
    };
    document.addEventListener('visibilitychange', onVisibilityChange);
    return () => document.removeEventListener('visibilitychange', onVisibilityChange);
  }, [messages, streamingScopeKey]);
  const visibleMessages = streamingState.scopeKey === streamingScopeKey
    ? visibleCommentaryMessages(streamingState, messages)
    : messages;
  const messageSections = useMemo(
    () => devInstrumentation.time(
      'commentary.buildSections',
      () => commentaryMessageSections(visibleMessages, Boolean(detail && !isRunWorkingStatus(detail.run.status)), !showBackToMain),
      { messages: visibleMessages.length }
    ),
    [detail, visibleMessages, showBackToMain]
  );
  const activityMessages = messageSections.activity;
  const messageUpdateKey = devInstrumentation.time(
    'commentary.buildUpdateKey',
    () => commentaryMessageUpdateKey(visibleMessages, events),
    { messages: visibleMessages.length, displayEvents: events.length }
  );
  const messageIndexById = useMemo(
    () => devInstrumentation.time('commentary.buildMessageIndex', () => commentaryMessageIndex(activityMessages), { messages: activityMessages.length }),
    [activityMessages]
  );
  const maxWindowStart = Math.max(0, activityMessages.length - COMMENTARY_RENDER_WINDOW_SIZE);
  const [windowStart, setWindowStart] = useState(maxWindowStart);
  const normalizedWindowStart = Math.min(windowStart, maxWindowStart);
  const renderedMessages = activityMessages.slice(normalizedWindowStart, normalizedWindowStart + COMMENTARY_RENDER_WINDOW_SIZE);
  const measuredHeightsRef = useRef<{ scopeKey: string | null; heights: Map<string, number> }>({ scopeKey: scrollScopeKey, heights: new Map() });
  if (measuredHeightsRef.current.scopeKey !== scrollScopeKey) {
    measuredHeightsRef.current = { scopeKey: scrollScopeKey, heights: new Map() };
  }
  const [heightRevision, setHeightRevision] = useState(0);
  const [itemGap, setItemGap] = useState(14);
  const messageOffsets = useMemo(() => commentaryMessageOffsets(
    activityMessages,
    measuredHeightsRef.current.heights,
    commentaryEstimatedMessageHeight(measuredHeightsRef.current.heights),
    itemGap
  ), [activityMessages, heightRevision, itemGap, scrollScopeKey]);
  const topSpacerHeight = commentarySpacerHeight(messageOffsets, 0, normalizedWindowStart, itemGap);
  const bottomSpacerHeight = commentarySpacerHeight(
    messageOffsets, normalizedWindowStart + renderedMessages.length, activityMessages.length, itemGap
  );
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);
  const topSpacerRef = useRef<HTMLDivElement | null>(null);
  const bottomSpacerRef = useRef<HTMLDivElement | null>(null);
  const followLatestRef = useRef(true);
  const followScrollFrameRef = useRef<number | null>(null);
  const followScrollTimeRef = useRef<number | null>(null);
  const followScrollReadyRef = useRef(false);
  const restoringAnchorRef = useRef(false);
  const pendingScrollAnchorRef = useRef<CommentaryScrollAnchor | null>(null);
  const pendingScrollTopRef = useRef<number | null>(null);
  const pendingScrollWindowStartRef = useRef<number | null>(null);
  const pendingMeasuredRevisionRef = useRef<number | null>(null);
  const pendingItemGapRef = useRef<number | null>(null);
  const historyAnchorRef = useRef<CommentaryScrollAnchor | null>(null);
  const historyCursorRef = useRef(detail?.historyCursor?.beforeEventId ?? null);
  const requestedHistoryCursorRef = useRef<string | null>(null);
  const userScrollIntentRef = useRef(false);
  const userScrollIntentTimerRef = useRef<number | null>(null);
  const scrollbarHideTimerRef = useRef<number | null>(null);
  const scrollbarDragRef = useRef(false);
  const scrollScopeKeyRef = useRef(scrollScopeKey);
  const detailRef = useRef(detail);
  detailRef.current = detail;
  const loading = Boolean(selectedRunId && !detail);
  const newResearch = state === 'new-research';

  const requestToolCallDetail = useCallback(async (toolCall: CommentaryToolCall): Promise<CommentaryToolCall> => {
    if (onRequestToolCallDetail) return onRequestToolCallDetail(toolCall);
    const currentDetail = detailRef.current;
    if (!selectedRunId || !currentDetail) throw new Error('Session detail is unavailable.');
    const traceEventIds = [toolCall.requestTraceEventId, toolCall.observationTraceEventId]
      .filter((eventId): eventId is string => Boolean(eventId));
    const result = await window.beale.getRunMessageDetail({ runId: selectedRunId, traceEventIds });
    if (result.runId !== selectedRunId) throw new Error('Received tool detail for a different session.');
    return hydrateCommentaryToolCall(toolCall, result.traceEvents, currentDetail);
  }, [onRequestToolCallDetail, selectedRunId]);

  useDevRenderProbe('commentary.list', () => ({
    messages: messages.length,
    rendered: renderedMessages.length,
    windowStart: normalizedWindowStart,
    following: followLatestRef.current
  }));

  const updateScrollEdges = useCallback((): void => {
    const scroll = scrollRef.current;
    const list = listRef.current;
    if (!scroll) return;
    if (!list) {
      scroll.classList.remove('has-top-fade', 'has-bottom-fade');
      return;
    }
    const fadeClasses = commentaryScrollFadeClasses({
      scrollHeight: list.scrollHeight,
      clientHeight: list.clientHeight,
      scrollTop: list.scrollTop
    });
    scroll.classList.toggle('has-top-fade', fadeClasses['has-top-fade']);
    scroll.classList.toggle('has-bottom-fade', fadeClasses['has-bottom-fade']);
  }, []);

  const cancelFollowScroll = useCallback((): void => {
    if (followScrollFrameRef.current !== null) window.cancelAnimationFrame(followScrollFrameRef.current);
    followScrollFrameRef.current = null;
    followScrollTimeRef.current = null;
  }, []);

  const clearUserScrollbar = useCallback((): void => {
    if (scrollbarHideTimerRef.current !== null) window.clearTimeout(scrollbarHideTimerRef.current);
    scrollbarHideTimerRef.current = null;
    listRef.current?.classList.remove('scrollbar-active');
  }, []);

  const activateUserScrollbar = useCallback((): void => {
    const list = listRef.current;
    if (!list || list.scrollHeight <= list.clientHeight) return;
    list.classList.add('scrollbar-active');
    if (scrollbarHideTimerRef.current !== null) window.clearTimeout(scrollbarHideTimerRef.current);
    scrollbarHideTimerRef.current = window.setTimeout(() => {
      list.classList.remove('scrollbar-active');
      scrollbarHideTimerRef.current = null;
    }, INSET_SCROLLBAR_ACTIVE_MS);
  }, []);

  const scrollToLatest = useCallback((): void => {
    const list = listRef.current;
    if (!list) return;
    if (userScrollIntentRef.current || scrollbarDragRef.current) return;
    if (!streamEnabled || !followScrollReadyRef.current) {
      cancelFollowScroll();
      list.scrollTop = Math.max(0, list.scrollHeight - list.clientHeight);
      followScrollReadyRef.current = true;
      updateScrollEdges();
      return;
    }
    if (Math.abs(Math.max(0, list.scrollHeight - list.clientHeight) - list.scrollTop) <= 0.75) return;
    if (followScrollFrameRef.current !== null) return;
    const animate = (timestamp: number): void => {
      followScrollFrameRef.current = null;
      const currentList = listRef.current;
      if (!currentList || !followLatestRef.current || userScrollIntentRef.current || scrollbarDragRef.current) {
        followScrollTimeRef.current = null;
        return;
      }
      const target = Math.max(0, currentList.scrollHeight - currentList.clientHeight);
      const remaining = target - currentList.scrollTop;
      if (Math.abs(remaining) <= 0.75) {
        currentList.scrollTop = target;
        followScrollTimeRef.current = null;
        updateScrollEdges();
        return;
      }
      const elapsed = Math.min(64, Math.max(0, timestamp - (followScrollTimeRef.current ?? timestamp - 16)));
      followScrollTimeRef.current = timestamp;
      const offset = remaining * (1 - Math.exp(-elapsed / COMMENTARY_FOLLOW_SCROLL_EASE_MS));
      currentList.scrollTop += Math.sign(remaining) * Math.min(Math.abs(remaining), Math.max(1, Math.abs(offset)));
      updateScrollEdges();
      followScrollFrameRef.current = window.requestAnimationFrame(animate);
    };
    followScrollFrameRef.current = window.requestAnimationFrame(animate);
  }, [cancelFollowScroll, streamEnabled, updateScrollEdges]);

  const syncScrollState = useCallback((): void => {
    if (followLatestRef.current) {
      scrollToLatest();
      return;
    }
    updateScrollEdges();
  }, [scrollToLatest, updateScrollEdges]);

  const markUserScrollIntent = useCallback((): void => {
    cancelFollowScroll();
    userScrollIntentRef.current = true;
    if (userScrollIntentTimerRef.current !== null) {
      window.clearTimeout(userScrollIntentTimerRef.current);
    }
    userScrollIntentTimerRef.current = window.setTimeout(() => {
      userScrollIntentRef.current = false;
      userScrollIntentTimerRef.current = null;
    }, 200);
  }, [cancelFollowScroll]);

  const loadOlderHistory = useCallback((): void => {
    const cursor = detail?.historyCursor;
    if (!onLoadOlderHistory || !cursor?.hasEarlier || !cursor.beforeEventId
      || requestedHistoryCursorRef.current === cursor.beforeEventId) return;
    const beforeEventId = cursor.beforeEventId;
    requestedHistoryCursorRef.current = beforeEventId;
    historyAnchorRef.current = listRef.current ? captureCommentaryScrollAnchor(listRef.current) : null;
    void onLoadOlderHistory().then((loaded) => {
      if (!loaded && requestedHistoryCursorRef.current === beforeEventId) {
        requestedHistoryCursorRef.current = null;
        historyAnchorRef.current = null;
      }
    });
  }, [detail?.historyCursor, onLoadOlderHistory]);

  useLayoutEffect(() => {
    const list = listRef.current;
    if (!list) return undefined;
    const nodes = commentaryMessageNodes(list);
    const measure = (): void => {
      const gap = Number.parseFloat(window.getComputedStyle(list).getPropertyValue('--main-commentary-item-gap'));
      if (Number.isFinite(gap) && gap !== itemGap) {
        if (!followLatestRef.current && pendingScrollAnchorRef.current === null && pendingScrollTopRef.current === null) {
          pendingScrollAnchorRef.current = captureCommentaryScrollAnchor(list);
        }
        pendingItemGapRef.current = gap;
        setItemGap(gap);
      }
      let changed = false;
      for (const node of nodes) {
        const id = node.dataset.commentaryEventId;
        if (!id) continue;
        const height = Math.ceil(node.getBoundingClientRect().height);
        if (height <= 0 || measuredHeightsRef.current.heights.get(id) === height) continue;
        measuredHeightsRef.current.heights.set(id, height);
        changed = true;
      }
      if (!changed) return;
      if (!followLatestRef.current && pendingScrollAnchorRef.current === null && pendingScrollTopRef.current === null) {
        pendingScrollAnchorRef.current = captureCommentaryScrollAnchor(list);
      }
      pendingMeasuredRevisionRef.current = heightRevision + 1;
      setHeightRevision((revision) => revision + 1);
    };
    measure();
    if (typeof ResizeObserver === 'undefined') return undefined;
    const observer = new ResizeObserver(measure);
    for (const node of nodes) observer.observe(node);
    return () => observer.disconnect();
  }, [heightRevision, itemGap, messageUpdateKey, normalizedWindowStart, renderedMessages.length, scrollScopeKey]);

  useLayoutEffect(() => {
    const cursor = detail?.historyCursor?.beforeEventId ?? null;
    if (historyCursorRef.current === cursor) return;
    historyCursorRef.current = cursor;
    requestedHistoryCursorRef.current = null;
    const anchor = historyAnchorRef.current;
    historyAnchorRef.current = null;
    if (!anchor) return;
    const index = messageIndexById.get(anchor.messageId)
      ?? (anchor.messageId.startsWith('tool:') ? messageIndexById.get(anchor.messageId.slice(5)) : undefined);
    if (index === undefined) return;
    const nextStart = commentaryWindowStartForIndex(activityMessages.length, index);
    pendingScrollAnchorRef.current = { ...anchor, messageId: activityMessages[index].id };
    pendingScrollWindowStartRef.current = nextStart;
    if (nextStart !== normalizedWindowStart) setWindowStart(nextStart);
  }, [activityMessages.length, detail?.historyCursor?.beforeEventId, messageIndexById, normalizedWindowStart]);

  useLayoutEffect(() => {
    if (pendingMeasuredRevisionRef.current !== null && heightRevision < pendingMeasuredRevisionRef.current) return undefined;
    if (pendingItemGapRef.current !== null && itemGap !== pendingItemGapRef.current) return undefined;
    pendingMeasuredRevisionRef.current = null;
    pendingItemGapRef.current = null;
    if (pendingScrollWindowStartRef.current !== null && pendingScrollWindowStartRef.current !== normalizedWindowStart) return undefined;
    const targetScrollTop = pendingScrollTopRef.current;
    const anchor = pendingScrollAnchorRef.current;
    if (targetScrollTop === null && !anchor) return undefined;
    const list = listRef.current;
    if (!list) {
      pendingScrollAnchorRef.current = null;
      pendingScrollTopRef.current = null;
      pendingScrollWindowStartRef.current = null;
      pendingMeasuredRevisionRef.current = null;
      pendingItemGapRef.current = null;
      return undefined;
    }
    const anchorNode = anchor
      ? commentaryMessageNodes(list).find((node) => node.dataset.commentaryEventId === anchor.messageId)
      : null;
    pendingScrollAnchorRef.current = null;
    pendingScrollTopRef.current = null;
    pendingScrollWindowStartRef.current = null;
    if (targetScrollTop === null && !anchorNode) return undefined;

    restoringAnchorRef.current = true;
    if (targetScrollTop !== null) {
      list.scrollTop = Math.max(0, targetScrollTop);
    } else {
      const currentTop = anchorNode!.getBoundingClientRect().top - list.getBoundingClientRect().top;
      list.scrollTop = Math.max(0, list.scrollTop + currentTop - anchor!.offsetTop);
    }
    updateScrollEdges();
    const frame = window.requestAnimationFrame(() => {
      restoringAnchorRef.current = false;
      updateScrollEdges();
    });
    return () => {
      window.cancelAnimationFrame(frame);
      restoringAnchorRef.current = false;
    };
  }, [detail?.historyCursor?.beforeEventId, heightRevision, itemGap, normalizedWindowStart, renderedMessages.length, updateScrollEdges]);

  useLayoutEffect(() => {
    if (scrollScopeKeyRef.current !== scrollScopeKey) {
      cancelFollowScroll();
      clearUserScrollbar();
      followScrollReadyRef.current = false;
      scrollScopeKeyRef.current = scrollScopeKey;
      followLatestRef.current = true;
      userScrollIntentRef.current = false;
      scrollbarDragRef.current = false;
      pendingScrollAnchorRef.current = null;
      pendingScrollTopRef.current = null;
      pendingScrollWindowStartRef.current = null;
      pendingMeasuredRevisionRef.current = null;
      pendingItemGapRef.current = null;
      historyAnchorRef.current = null;
      requestedHistoryCursorRef.current = null;
      historyCursorRef.current = detail?.historyCursor?.beforeEventId ?? null;
      if (userScrollIntentTimerRef.current !== null) {
        window.clearTimeout(userScrollIntentTimerRef.current);
        userScrollIntentTimerRef.current = null;
      }
    }
    if (!followLatestRef.current) {
      const frame = window.requestAnimationFrame(updateScrollEdges);
      return () => window.cancelAnimationFrame(frame);
    }
    if (normalizedWindowStart !== maxWindowStart) {
      cancelFollowScroll();
      setWindowStart(maxWindowStart);
      return undefined;
    }
    scrollToLatest();
    return undefined;
  }, [cancelFollowScroll, clearUserScrollbar, heightRevision, itemGap, maxWindowStart, messageUpdateKey, normalizedWindowStart, scrollScopeKey, scrollToLatest, updateScrollEdges]);

  useEffect(() => {
    setWindowStart((current) => Math.min(current, maxWindowStart));
  }, [maxWindowStart]);

  useEffect(() => {
    if (!followLatestRef.current) return undefined;
    const frame = window.requestAnimationFrame(syncScrollState);
    return () => window.cancelAnimationFrame(frame);
  }, [messageUpdateKey, normalizedWindowStart, renderedMessages.length, scrollScopeKey, syncScrollState]);

  useEffect(() => {
    const endScrollbarDrag = (): void => {
      scrollbarDragRef.current = false;
    };
    window.addEventListener('pointerup', endScrollbarDrag);
    window.addEventListener('pointercancel', endScrollbarDrag);
    return () => {
      cancelFollowScroll();
      clearUserScrollbar();
      window.removeEventListener('pointerup', endScrollbarDrag);
      window.removeEventListener('pointercancel', endScrollbarDrag);
      if (userScrollIntentTimerRef.current !== null) {
        window.clearTimeout(userScrollIntentTimerRef.current);
      }
    };
  }, [cancelFollowScroll, clearUserScrollbar]);

  const handleScroll = useCallback((): void => {
    const list = listRef.current;
    if (!list) return;
    updateScrollEdges();
    if (restoringAnchorRef.current && !userScrollIntentRef.current && !scrollbarDragRef.current) return;
    const distanceFromBottom = list.scrollHeight - list.clientHeight - list.scrollTop;
    if (scrollbarDragRef.current) activateUserScrollbar();
    followLatestRef.current = commentaryFollowLatestAfterScroll({
      wasFollowingLatest: followLatestRef.current,
      distanceFromBottom,
      userInitiated: userScrollIntentRef.current || scrollbarDragRef.current
    });
    if (!followLatestRef.current) {
      cancelFollowScroll();
      setStreamingState((current) => current.pending.length
        ? initialCommentaryStreamingState(streamingScopeKey, messages) : current);
    }
    userScrollIntentRef.current = false;
    if (userScrollIntentTimerRef.current !== null) {
      window.clearTimeout(userScrollIntentTimerRef.current);
      userScrollIntentTimerRef.current = null;
    }
    const messageNodes = commentaryMessageNodes(list);
    const viewportTop = list.scrollTop;
    if (requestedHistoryCursorRef.current) {
      historyAnchorRef.current = captureCommentaryScrollAnchor(list) ?? historyAnchorRef.current;
    }
    if (normalizedWindowStart === 0 && messageNodes[0]
      && viewportTop <= commentaryNodeTop(list, messageNodes[0]) + list.clientHeight * 0.75) {
      loadOlderHistory();
    }
    if (activityMessages.length <= COMMENTARY_RENDER_WINDOW_SIZE) return;

    const visibleAnchor = captureCommentaryScrollAnchor(list);
    const viewportBottom = viewportTop + list.clientHeight;
    let nextStart = normalizedWindowStart;
    let preserveAnchor = true;
    const topSpacer = topSpacerRef.current;
    const listRect = list.getBoundingClientRect();
    const topSpacerRect = topSpacer?.getBoundingClientRect();
    const bottomSpacerRect = bottomSpacerRef.current?.getBoundingClientRect();

    if (topSpacerRect && topSpacerRect.bottom > listRect.top && topSpacerRect.top < listRect.bottom) {
      const spacerTop = viewportTop + topSpacerRect.top - listRect.top;
      nextStart = commentaryWindowStartForOffset(messageOffsets, viewportTop - spacerTop, maxWindowStart);
      preserveAnchor = false;
    } else if (bottomSpacerRect && bottomSpacerRect.top <= listRect.top && bottomSpacerRect.bottom > listRect.top) {
      const spacerTop = viewportTop + bottomSpacerRect.top - listRect.top;
      const firstIndex = normalizedWindowStart + renderedMessages.length;
      nextStart = commentaryWindowStartForOffset(
        messageOffsets, messageOffsets[firstIndex] + viewportTop - spacerTop, maxWindowStart
      );
      preserveAnchor = false;
    } else if (distanceFromBottom <= COMMENTARY_AUTO_FOLLOW_THRESHOLD) {
      nextStart = maxWindowStart;
    } else if (messageNodes.length === 0 || !visibleAnchor) {
      const firstNodeTop = messageNodes[0] ? commentaryNodeTop(list, messageNodes[0]) : 0;
      nextStart = commentaryWindowStartForOffset(
        messageOffsets, messageOffsets[normalizedWindowStart] + viewportTop - firstNodeTop, maxWindowStart
      );
      preserveAnchor = false;
    } else {
      const firstRenderedTop = messageNodes[0] ? commentaryNodeTop(list, messageNodes[0]) : 0;
      const lastNode = messageNodes.at(-1);
      const lastRenderedBottom = lastNode ? commentaryNodeTop(list, lastNode) + lastNode.offsetHeight : firstRenderedTop;
      const edgeBuffer = Math.max(COMMENTARY_WINDOW_EDGE_BUFFER, list.clientHeight * 0.35);
      const viewportMissedWindow = viewportBottom < firstRenderedTop - edgeBuffer || viewportTop > lastRenderedBottom + edgeBuffer;

      if (viewportMissedWindow) {
        nextStart = commentaryWindowStartForOffset(
          messageOffsets, messageOffsets[normalizedWindowStart] + viewportTop - firstRenderedTop, maxWindowStart
        );
        preserveAnchor = false;
      } else if (viewportTop < firstRenderedTop + edgeBuffer && normalizedWindowStart > 0) {
        nextStart = normalizedWindowStart - COMMENTARY_WINDOW_SLIDE_STEP;
      } else if (viewportBottom > lastRenderedBottom - edgeBuffer && normalizedWindowStart < maxWindowStart) {
        nextStart = normalizedWindowStart + COMMENTARY_WINDOW_SLIDE_STEP;
      }
    }

    nextStart = Math.max(0, Math.min(maxWindowStart, nextStart));
    if (nextStart !== normalizedWindowStart) {
      const anchor = preserveAnchor ? captureCommentaryScrollAnchor(list, {
        canUseMessageId: (messageId) => {
          const index = messageIndexById.get(messageId);
          return index !== undefined && index >= nextStart && index < nextStart + COMMENTARY_RENDER_WINDOW_SIZE;
        }
      }) : null;
      pendingScrollAnchorRef.current = anchor;
      pendingScrollTopRef.current = anchor ? null : viewportTop;
      pendingScrollWindowStartRef.current = nextStart;
      setWindowStart(nextStart);
    }
  }, [activateUserScrollbar, activityMessages.length, cancelFollowScroll, loadOlderHistory, maxWindowStart, messageIndexById, messageOffsets, messages, normalizedWindowStart, renderedMessages.length, streamingScopeKey, updateScrollEdges]);

  if (!selectedRunId && !onInitialInstruction) return null;

  return (
    <section
      className={`main-trace-view main-commentary-view${newResearch ? ' new-research-session-view' : ''}${showBackToMain ? ' is-subagent-trace' : ''}${loading ? ' is-loading' : ''}`}
      aria-label={newResearch ? 'New Research' : showBackToMain ? 'Subagent commentary' : 'Lead commentary'}
      data-commentary-state={state}
    >
      {showBackButton ? (
        <button
          type="button"
          className="back-to-main-button trace-back-to-main-button"
          title="Return to Lead commentary"
          onClick={onBackToMain}
        >
          <ArrowLeft size={14} />
          <span>Back to Lead</span>
        </button>
      ) : null}
      {loading ? <SessionLoadingState label="Loading session" /> : null}
      {!detail && onInitialInstruction ? (
        <div className="main-commentary-scroll" aria-label={newResearch ? 'New research commentary' : 'No report review started yet'}>
          <div className="main-commentary-list">{emptyContent}</div>
        </div>
      ) : null}
      {detail && showBackToMain && messages.length === 0 ? <div className="main-trace-empty">No commentary recorded yet.</div> : null}
      {detail && (!showBackToMain || messages.length > 0) ? (
        <div className="main-commentary-scroll" ref={scrollRef}>
          <div
            className="main-commentary-list"
            ref={listRef}
            onScroll={handleScroll}
            onWheel={(event) => {
              if (event.deltaX !== 0 || event.deltaY !== 0) activateUserScrollbar();
              if (event.deltaY < 0) markUserScrollIntent();
            }}
            onTouchMove={() => {
              activateUserScrollbar();
              markUserScrollIntent();
            }}
            onKeyDown={(event) => {
              if (!['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'Home', 'End', ' '].includes(event.key)) return;
              if (event.target instanceof HTMLElement
                && event.target.closest('button, input, textarea, select, [contenteditable="true"]')) return;
              activateUserScrollbar();
              if (['ArrowUp', 'PageUp', 'Home'].includes(event.key) || (event.key === ' ' && event.shiftKey)) {
                markUserScrollIntent();
              }
            }}
            onPointerDown={(event) => {
              const list = listRef.current;
              if (!list || event.pointerType === 'touch') return;
              const bounds = list.getBoundingClientRect();
              const scrollbarWidth = Math.max(12, list.offsetWidth - list.clientWidth + 4);
              if (event.clientX < bounds.right - scrollbarWidth) return;
              scrollbarDragRef.current = true;
              activateUserScrollbar();
              markUserScrollIntent();
            }}
          >
            {messageSections.leading.map((message) => (
              <CommentaryMessageRow
                key={message.id}
                message={message}
                viewedAgentPath={viewedAgentPath}
                animateIcon={streamingState.animatedIds.has(message.id)}
                searchHighlightQuery={searchHighlightQuery}
                selected={false}
                onRequestToolCallDetail={requestToolCallDetail}
              />
            ))}
            {!showBackToMain ? (
              <RunWorkDisclosure
                detail={detail}
                onExpand={() => {
                  followLatestRef.current = false;
                  pendingScrollAnchorRef.current = null;
                  if (normalizedWindowStart > 0) {
                    pendingScrollTopRef.current = listRef.current?.scrollTop ?? null;
                    pendingScrollWindowStartRef.current = 0;
                    setWindowStart(0);
                  }
                  loadOlderHistory();
                }}
              >
                {() => (
                  <>
                    {topSpacerHeight > 0 ? <div className="main-commentary-spacer" ref={topSpacerRef} style={{ height: topSpacerHeight }} aria-hidden="true" /> : null}
                    {renderedMessages.map((message) => (
                      <CommentaryMessageRow
                        key={message.id}
                        message={message}
                        viewedAgentPath={viewedAgentPath}
                        animateIcon={streamingState.animatedIds.has(message.id)}
                        windowed
                        searchHighlightQuery={searchHighlightQuery}
                        selected={false}
                        onRequestToolCallDetail={requestToolCallDetail}
                      />
                    ))}
                    {bottomSpacerHeight > 0 ? <div className="main-commentary-spacer" ref={bottomSpacerRef} style={{ height: bottomSpacerHeight }} aria-hidden="true" /> : null}
                  </>
                )}
              </RunWorkDisclosure>
            ) : (
              <>
                {topSpacerHeight > 0 ? <div className="main-commentary-spacer" ref={topSpacerRef} style={{ height: topSpacerHeight }} aria-hidden="true" /> : null}
                {renderedMessages.map((message) => (
                  <CommentaryMessageRow
                    key={message.id}
                    message={message}
                    viewedAgentPath={viewedAgentPath}
                    animateIcon={streamingState.animatedIds.has(message.id)}
                    windowed
                    searchHighlightQuery={searchHighlightQuery}
                    selected={false}
                    onRequestToolCallDetail={requestToolCallDetail}
                  />
                ))}
                {bottomSpacerHeight > 0 ? <div className="main-commentary-spacer" ref={bottomSpacerRef} style={{ height: bottomSpacerHeight }} aria-hidden="true" /> : null}
              </>
            )}
            {messageSections.trailing.map((message) => (
              <CommentaryMessageRow
                key={message.id}
                message={message}
                viewedAgentPath={viewedAgentPath}
                animateIcon={streamingState.animatedIds.has(message.id)}
                searchHighlightQuery={searchHighlightQuery}
                selected={false}
                onRequestToolCallDetail={requestToolCallDetail}
              />
            ))}
          </div>
        </div>
      ) : null}
      {!showBackToMain && !loading ? (
        <MainSteerArea
          busy={busy}
          detail={detail}
          providerModelCatalog={providerModelCatalog}
          providerModelDefaults={providerModelDefaults}
          initialModelSelection={initialModelSelection}
          collaboration={collaboration}
          initialSafetyMode={initialSafetyMode}
          inputPlaceholder={inputPlaceholder}
          instructionLocked={instructionLocked}
          ariaLabel={ariaLabel}
          showCollaboration={showCollaboration}
          runId={detail?.run.id ?? selectedRunId}
          initialInstruction={initialInstruction}
          preComposerContent={preComposerContent}
          postComposerContent={postComposerContent}
          contextUsageAccessory={import.meta.env.DEV && detail && !isRunWorkingStatus(detail.run.status)
            ? <HarnessFeedbackViewer key={detail.run.id} runId={detail.run.id} /> : null}
          responseSuggestionsEnabled={responseSuggestionsEnabled}
          shellApproval={shellApproval}
          shellApprovalBusy={shellApprovalBusy}
          onCancel={onCancel}
          onCollaborationChange={onCollaborationChange}
          onInitialInstruction={onInitialInstruction}
          onShellApprovalDecision={onShellApprovalDecision}
          onSessionAction={onSessionAction}
          onSteerInstruction={onSteerInstruction}
        />
      ) : null}
    </section>
  );
});

interface CommentaryMessageSections {
  leading: CommentaryMessage[];
  activity: CommentaryMessage[];
  trailing: CommentaryMessage[];
}

export function commentaryMessageSections(
  messages: readonly CommentaryMessage[],
  terminal: boolean,
  enabled = true
): CommentaryMessageSections {
  if (!enabled) return { leading: [], activity: [...messages], trailing: [] };

  let leadingEnd = 0;
  while (messages[leadingEnd]?.kind === 'user') leadingEnd += 1;

  let trailingStart = messages.length;
  if (terminal) {
    while (trailingStart > leadingEnd) {
      const kind = messages[trailingStart - 1]?.kind;
      if (kind !== 'final_answer' && kind !== 'error') break;
      trailingStart -= 1;
    }
  }

  return {
    leading: messages.slice(0, leadingEnd),
    activity: messages.slice(leadingEnd, trailingStart),
    trailing: messages.slice(trailingStart)
  };
}

function RunWorkDisclosure({ detail, children, onExpand }: { detail: RunDetail; children: ReactNode | (() => ReactNode); onExpand?: () => void }): JSX.Element {
  const working = isRunWorkingStatus(detail.run.status);
  const [expanded, setExpanded] = useState(() => working);
  const [nowMs, setNowMs] = useState(() => Date.now());

  useLayoutEffect(() => {
    setExpanded(working);
  }, [detail.run.id, working]);

  useEffect(() => {
    if (!working) return undefined;
    setNowMs(Date.now());
    const interval = window.setInterval(() => setNowMs(Date.now()), 1000);
    return () => window.clearInterval(interval);
  }, [detail.run.id, working]);

  const durationEndMs = working ? nowMs : stoppedWorkEndMs(detail, nowMs);
  const durationLabel = formatDurationHms(runWorkingDurationMs(detail, durationEndMs));
  const label = `${working ? 'Working' : 'Worked'} for ${durationLabel}`;

  return (
    <section className={`run-work-disclosure${working ? ' is-working' : ' is-terminal'}${expanded ? ' is-expanded' : ''}`}>
      <div className="run-work-header">
        {!working ? (
          <button
            type="button"
            className="run-work-toggle"
            aria-expanded={expanded}
            onClick={() => {
              if (!expanded) onExpand?.();
              setExpanded((current) => !current);
            }}
          >
            <span>{label}</span>
            <ChevronRight className="run-work-chevron" size={14} aria-hidden="true" />
          </button>
        ) : (
          <span className="run-work-label">{label}</span>
        )}
      </div>
      {expanded ? <div className="run-work-history">{typeof children === 'function' ? children() : children}</div> : null}
    </section>
  );
}

export function runWorkingDurationMs(detail: RunDetail, nowMs: number): number {
  const attempts = detail.attempts ?? [];
  const attemptDuration = attempts.reduce((total, attempt) => {
    const startMs = Date.parse(attempt.startedAt);
    if (!Number.isFinite(startMs)) return total;
    const parsedEndMs = attempt.endedAt ? Date.parse(attempt.endedAt) : Number.NaN;
    const endMs = Number.isFinite(parsedEndMs) ? parsedEndMs : nowMs;
    return total + Math.max(0, endMs - startMs);
  }, 0);
  if (attempts.length > 0) return attemptDuration;

  const startMs = detail.run.startedAt ? Date.parse(detail.run.startedAt) : Number.NaN;
  if (!Number.isFinite(startMs)) return 0;
  return Math.max(0, nowMs - startMs);
}

function stoppedWorkEndMs(detail: RunDetail, fallbackMs: number): number {
  const runEndMs = detail.run.endedAt ? Date.parse(detail.run.endedAt) : Number.NaN;
  if (Number.isFinite(runEndMs)) return runEndMs;
  const timestamps = [
    ...(detail.attempts ?? []).map((attempt) => attempt.endedAt),
    ...(detail.traceEvents ?? []).map((event) => event.createdAt),
    ...(detail.transcriptMessages ?? []).map((message) => message.createdAt)
  ];
  return timestamps.reduce((latest, value) => {
    if (!value) return latest;
    const timestamp = Date.parse(value);
    return Number.isFinite(timestamp) ? Math.max(latest, timestamp) : latest;
  }, 0) || fallbackMs;
}

export function isRunWorkingStatus(status: RunDetail['run']['status']): boolean {
  return status === 'active';
}

export const CommentaryMessageRow = memo(function CommentaryMessageRow({
  message,
  viewedAgentPath = '/root',
  animateIcon = false,
  windowed = false,
  searchHighlightQuery,
  selected,
  onRequestToolCallDetail = returnToolCallWithoutLoading
}: {
  message: CommentaryMessage;
  viewedAgentPath?: string;
  animateIcon?: boolean;
  windowed?: boolean;
  searchHighlightQuery: string;
  selected: boolean;
  onRequestToolCallDetail?: (toolCall: CommentaryToolCall) => Promise<CommentaryToolCall>;
}): JSX.Element {
  const hasSearchHighlight = searchHighlightTerms(searchHighlightQuery).length > 0;
  const label = commentaryMessageLabel(message.kind, message.taskAction);
  const icon = commentaryMessageIcon(message.kind, message.toolName);
  const hasAgentHeader = message.kind === 'task' && message.taskAction !== 'spawn';
  const isAgentMessage = message.kind === 'task' && (message.taskAction === 'message' || message.taskAction === 'room_message');
  const originIsCurrentAgent = message.taskAuthorAgentPath === viewedAgentPath
    || !message.taskAuthorAgentPath && viewedAgentPath === '/root' && message.taskOrigin === 'Lead';
  const destinationIsCurrentAgent = message.taskRecipientAgentPath === viewedAgentPath;
  const communicationClass = message.kind === 'task'
    ? hasAgentHeader
      ? originIsCurrentAgent ? 'agent-communication-outbound' : 'agent-communication-inbound'
      : (message.taskAuthorAgentPath && message.taskAuthorAgentPath !== '/root') || message.taskRecipientAgentPath === '/root'
        ? 'agent-communication-inbound'
        : 'agent-communication-outbound'
    : '';
  const reasoningTraceLines = message.kind === 'progress'
    ? message.reasoningTraceLines?.length ? message.reasoningTraceLines : [message.contentMarkdown]
    : [];
  const messageContent = message.kind === 'marker' || message.kind === 'progress' ? null : (
    <div className="main-commentary-message-content">
      {message.kind === 'diagram' && message.surfaceDiagram ? (
        <>
          <div className="main-commentary-diagram-tool-line">
            <Wrench size={16} aria-hidden="true" />
            <span>{message.contentMarkdown}</span>
          </div>
          {message.contentMarkdown === surfaceDiagramToolLine(message.surfaceDiagram) ? (
            <SurfaceThreatDiagram source={message.surfaceDiagram.source}
              surfaceName={message.surfaceDiagram.surfaceName} />
          ) : null}
        </>
      ) : message.kind === 'tool' ? (
        <CommentaryToolMessageContent
          message={message}
          hasSearchHighlight={hasSearchHighlight}
          searchHighlightQuery={searchHighlightQuery}
          onRequestToolCallDetail={onRequestToolCallDetail}
        />
      ) : isAgentMessage ? (
        <CollapsibleAgentMessage contentMarkdown={message.contentMarkdown}
          searchHighlightQuery={searchHighlightQuery} hasSearchHighlight={hasSearchHighlight} />
      ) : hasSearchHighlight ? (
        renderSearchHighlightedText(message.contentMarkdown, searchHighlightQuery)
      ) : (
        renderTraceProseText(message.contentMarkdown, message.kind === 'commentary' ? 'reasoning' : 'agent_output')
      )}
      {message.abortNotice ? <p className="main-commentary-abort-notice">{message.abortNotice}</p> : null}
    </div>
  );
  return (
    <article
      className={`main-commentary-message kind-${message.kind} ${communicationClass} ${hasAgentHeader ? 'has-agent-header' : ''} ${selected ? 'selected' : ''}${animateIcon ? ' is-streamed' : ''}`}
      data-commentary-event-id={message.id}
      data-commentary-window-item={windowed ? '' : undefined}
      data-commentary-trace-id={message.traceEventId ?? undefined}
    >
      {message.kind === 'marker' ? (
        <div className={`main-commentary-session-marker is-${message.markerType ?? 'compaction'}`}>
          <span>{message.contentMarkdown}</span>
        </div>
      ) : message.kind === 'progress' ? (
        <div className="main-commentary-reasoning-lines">
          {reasoningTraceLines.map((line, index) => (
            <div className="main-commentary-reasoning-line" key={index}>
              <span className="main-commentary-message-icon" aria-hidden="true">
                <Brain size={16} />
              </span>
              <div className="main-commentary-message-content">
                {hasSearchHighlight
                  ? renderSearchHighlightedText(line, searchHighlightQuery)
                  : renderTraceProseText(line, 'reasoning')}
              </div>
            </div>
          ))}
        </div>
      ) : hasAgentHeader ? (
        <div className="main-commentary-agent-card">
          <div className="main-commentary-agent-header">
            <div className="main-commentary-agent-route">
              <Mail size={14} aria-hidden="true" />
              <span className={`main-commentary-agent-name${originIsCurrentAgent ? ' is-current-agent' : ''}`}
                title={message.taskOrigin}>{message.taskOrigin ?? 'Lead'}</span>
              <ArrowRight size={14} aria-hidden="true" />
              <span className={`main-commentary-agent-name${destinationIsCurrentAgent ? ' is-current-agent' : ''}`}
                title={message.taskDestination}>{message.taskDestination ?? 'Unknown recipient'}</span>
            </div>
            <span className="main-commentary-agent-type">{label}</span>
          </div>
          {messageContent}
        </div>
      ) : (
        <>
          {icon ? <span className="main-commentary-message-icon" aria-hidden="true">{icon}</span> : null}
          {label ? <span className="main-commentary-message-label">{label}{message.taskRoute ? ` · ${message.taskRoute}` : ''}</span> : null}
          {messageContent}
        </>
      )}
    </article>
  );
}, commentaryMessageRowPropsEqual);

function CollapsibleAgentMessage({ contentMarkdown, searchHighlightQuery, hasSearchHighlight }: {
  contentMarkdown: string;
  searchHighlightQuery: string;
  hasSearchHighlight: boolean;
}): JSX.Element {
  const [expanded, setExpanded] = useState(false);
  const [overflows, setOverflows] = useState(false);
  const bodyRef = useRef<HTMLDivElement>(null);
  const bodyId = useId();

  useLayoutEffect(() => {
    const body = bodyRef.current;
    if (!body) return;
    const measure = (): void => {
      const lineHeight = Number.parseFloat(window.getComputedStyle(body).lineHeight);
      setOverflows(Number.isFinite(lineHeight) && body.scrollHeight > lineHeight * 4 + 1);
    };
    measure();
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(measure);
    observer.observe(body);
    if (body.firstElementChild instanceof HTMLElement) observer.observe(body.firstElementChild);
    return () => observer.disconnect();
  }, [contentMarkdown, searchHighlightQuery]);

  return (
    <>
      <div id={bodyId} ref={bodyRef} className={`main-commentary-agent-message-body${expanded ? ' is-expanded' : ''}`}>
        {hasSearchHighlight
          ? renderSearchHighlightedText(contentMarkdown, searchHighlightQuery)
          : renderTraceProseText(contentMarkdown, 'agent_output')}
      </div>
      {overflows ? (
        <button type="button" className="main-commentary-agent-message-toggle"
          aria-controls={bodyId} aria-expanded={expanded} onClick={() => setExpanded((value) => !value)}>
          {expanded ? 'Show less' : 'Show more'}
        </button>
      ) : null}
    </>
  );
}

function CommentaryToolMessageContent({
  message,
  hasSearchHighlight,
  searchHighlightQuery,
  onRequestToolCallDetail
}: {
  message: CommentaryMessage;
  hasSearchHighlight: boolean;
  searchHighlightQuery: string;
  onRequestToolCallDetail: (toolCall: CommentaryToolCall) => Promise<CommentaryToolCall>;
}): JSX.Element {
  const [expanded, setExpanded] = useState(false);
  const [expandedCallIds, setExpandedCallIds] = useState<Set<string>>(() => new Set());
  const [resolvedToolCalls, setResolvedToolCalls] = useState<Map<string, CommentaryToolCall>>(() => new Map());
  const [loadingCallIds, setLoadingCallIds] = useState<Set<string>>(() => new Set());
  const [callErrors, setCallErrors] = useState<Map<string, string>>(() => new Map());
  const toolCalls = message.toolCalls ?? [];

  const toggleToolCall = (toolCall: CommentaryToolCall): void => {
    const expanding = !expandedCallIds.has(toolCall.id);
    const callLoading = loadingCallIds.has(toolCall.id);
    setExpandedCallIds((current) => toggledSetValue(current, toolCall.id));
    if (!expanding || !toolCall.detailsDeferred || resolvedToolCalls.has(toolCall.id) || callLoading) return;
    setLoadingCallIds((current) => addedSetValue(current, toolCall.id));
    setCallErrors((current) => mapWithoutKey(current, toolCall.id));
    void onRequestToolCallDetail(toolCall)
      .then((resolved) => {
        setResolvedToolCalls((current) => new Map(current).set(toolCall.id, resolved));
      })
      .catch((caught: unknown) => {
        setCallErrors((current) => new Map(current).set(toolCall.id, errorMessage(caught)));
      })
      .finally(() => setLoadingCallIds((current) => setWithoutValue(current, toolCall.id)));
  };

  if (INLINE_SINGULAR_TOOL_NAMES.has(message.toolName ?? '') && (message.toolCount ?? 1) === 1 && toolCalls.length === 1) {
    const toolCall = toolCalls[0];
    const callExpanded = expandedCallIds.has(toolCall.id);
    const resolvedToolCall = resolvedToolCalls.get(toolCall.id) ?? toolCall;
    const callLoading = loadingCallIds.has(toolCall.id);
    const callError = callErrors.get(toolCall.id) ?? null;
    return (
      <div className={`main-commentary-tool-call main-commentary-single-tool-disclosure${callExpanded ? ' expanded' : ''}`}>
        <button
          aria-expanded={callExpanded}
          className="main-commentary-tool-call-summary main-commentary-single-tool"
          onClick={() => toggleToolCall(toolCall)}
          title={message.contentMarkdown}
          type="button"
        >
          <span className="main-commentary-single-tool-label">
            {hasSearchHighlight
              ? renderSearchHighlightedText(message.contentMarkdown, searchHighlightQuery)
              : message.contentMarkdown}
          </span>
          <ChevronRight className="main-commentary-single-tool-chevron" size={14} aria-hidden="true" />
        </button>
        {callExpanded ? (
          <div className="main-commentary-tool-call-details">
            {callLoading ? <div className="main-commentary-tool-call-loading">Loading details…</div> : null}
            {callError ? <div className="main-commentary-tool-call-loading">{callError}</div> : null}
            {!callLoading && !callError ? (
              <>
                <ToolCallValue label="Input" value={resolvedToolCall.input} />
                <ToolCallValue label="Output" value={resolvedToolCall.output} />
              </>
            ) : null}
          </div>
        ) : null}
      </div>
    );
  }

  return (
    <div className={`main-commentary-tool-disclosure${expanded ? ' expanded' : ''}`}>
      <button
        type="button"
        className="main-commentary-tool-summary"
        aria-expanded={expanded}
        onClick={() => setExpanded((current) => !current)}
      >
        <span>
          {hasSearchHighlight
            ? renderSearchHighlightedText(message.contentMarkdown, searchHighlightQuery)
            : message.contentMarkdown}
        </span>
        <ChevronRight className="main-commentary-tool-summary-chevron" size={16} aria-hidden="true" />
      </button>
      {expanded ? (
        <div className="main-commentary-tool-call-list" role="list">
          {toolCalls.map((toolCall) => {
            const callExpanded = expandedCallIds.has(toolCall.id);
            const resolvedToolCall = resolvedToolCalls.get(toolCall.id) ?? toolCall;
            const callLoading = loadingCallIds.has(toolCall.id);
            const callError = callErrors.get(toolCall.id) ?? null;
            return (
              <div className={`main-commentary-tool-call${callExpanded ? ' expanded' : ''}`} role="listitem" key={toolCall.id}>
                <button
                  type="button"
                  className="main-commentary-tool-call-summary"
                  aria-expanded={callExpanded}
                  onClick={() => toggleToolCall(toolCall)}
                >
                  <code title={toolCall.label}>
                    {hasSearchHighlight
                      ? renderSearchHighlightedText(toolCall.label, searchHighlightQuery)
                      : toolCall.label}
                  </code>
                  <ChevronRight size={14} aria-hidden="true" />
                </button>
                {callExpanded ? (
                  <div className="main-commentary-tool-call-details">
                    {callLoading ? <div className="main-commentary-tool-call-loading">Loading details…</div> : null}
                    {callError ? <div className="main-commentary-tool-call-loading">{callError}</div> : null}
                    {!callLoading && !callError ? (
                      <>
                        <ToolCallValue label="Input" value={resolvedToolCall.input} />
                        <ToolCallValue label="Output" value={resolvedToolCall.output} />
                      </>
                    ) : null}
                  </div>
                ) : null}
              </div>
            );
          })}
        </div>
      ) : null}
    </div>
  );
}

function commentaryMessageUpdateKey(
  messages: readonly CommentaryMessage[],
  events: readonly TraceDisplayEvent[]
): string {
  const latestMessage = messages.at(-1);
  const latestEvent = events.at(-1);
  return [
    messages.length,
    latestMessage?.id ?? '',
    latestMessage?.contentMarkdown.length ?? 0,
    latestMessage?.toolCalls?.length ?? 0,
    events.length,
    latestEvent?.id ?? '',
    latestEvent?.summary.length ?? 0
  ].join(':');
}

function commentaryMessageIndex(messages: readonly CommentaryMessage[]): Map<string, number> {
  const index = new Map<string, number>();
  messages.forEach((message, messageIndex) => {
    index.set(message.id, messageIndex);
    if (message.traceEventId) index.set(message.traceEventId, messageIndex);
    message.toolCalls?.forEach((toolCall) => {
      index.set(toolCall.id, messageIndex);
      index.set(toolCall.traceEventId, messageIndex);
    });
  });
  return index;
}

export function commentaryWindowStartForIndex(messageCount: number, messageIndex: number): number {
  const maxWindowStart = Math.max(0, messageCount - COMMENTARY_RENDER_WINDOW_SIZE);
  return Math.max(0, Math.min(maxWindowStart, messageIndex - Math.floor(COMMENTARY_RENDER_WINDOW_SIZE / 3)));
}

export function commentaryMessageOffsets(
  messages: readonly Pick<CommentaryMessage, 'id'>[],
  measuredHeights: ReadonlyMap<string, number>,
  estimatedHeight: number,
  gap: number
): number[] {
  const offsets = [0];
  for (const message of messages) {
    offsets.push(offsets[offsets.length - 1] + (measuredHeights.get(message.id) ?? estimatedHeight) + gap);
  }
  return offsets;
}

export function commentarySpacerHeight(offsets: readonly number[], start: number, end: number, gap: number): number {
  return end <= start ? 0 : offsets[end] - offsets[start] - gap;
}

export function commentaryWindowStartForOffset(offsets: readonly number[], offset: number, maxWindowStart: number): number {
  let low = 0;
  let high = offsets.length - 1;
  while (low < high) {
    const middle = Math.floor((low + high + 1) / 2);
    if (offsets[middle] <= offset) low = middle;
    else high = middle - 1;
  }
  return Math.max(0, Math.min(maxWindowStart, low));
}

function commentaryEstimatedMessageHeight(measuredHeights: ReadonlyMap<string, number>): number {
  if (measuredHeights.size < 4) return COMMENTARY_ESTIMATED_MESSAGE_HEIGHT;
  const heights = [...measuredHeights.values()].sort((left, right) => left - right);
  return Math.max(28, Math.min(160, heights[Math.floor(heights.length / 2)]));
}

function commentaryMessageNodes(list: HTMLDivElement): HTMLElement[] {
  return Array.from(list.querySelectorAll<HTMLElement>('[data-commentary-window-item]'));
}

function commentaryNodeTop(list: HTMLDivElement, node: HTMLElement): number {
  return list.scrollTop + node.getBoundingClientRect().top - list.getBoundingClientRect().top;
}

function captureCommentaryScrollAnchor(
  list: HTMLDivElement,
  options: CommentaryScrollAnchorOptions = {}
): CommentaryScrollAnchor | null {
  const viewport = list.getBoundingClientRect();
  for (const node of commentaryMessageNodes(list)) {
    const messageId = node.dataset.commentaryEventId;
    if (!messageId || (options.canUseMessageId && !options.canUseMessageId(messageId))) continue;
    const rect = node.getBoundingClientRect();
    if (rect.bottom < viewport.top) continue;
    if (rect.top > viewport.bottom) break;
    return { messageId, offsetTop: rect.top - viewport.top };
  }
  return null;
}

function commentaryMessageRowPropsEqual(
  left: { message: CommentaryMessage; viewedAgentPath?: string; animateIcon?: boolean; windowed?: boolean; searchHighlightQuery: string; selected: boolean; onRequestToolCallDetail?: (toolCall: CommentaryToolCall) => Promise<CommentaryToolCall> },
  right: { message: CommentaryMessage; viewedAgentPath?: string; animateIcon?: boolean; windowed?: boolean; searchHighlightQuery: string; selected: boolean; onRequestToolCallDetail?: (toolCall: CommentaryToolCall) => Promise<CommentaryToolCall> }
): boolean {
  if (
    left.viewedAgentPath !== right.viewedAgentPath ||
    left.animateIcon !== right.animateIcon ||
    left.windowed !== right.windowed ||
    left.searchHighlightQuery !== right.searchHighlightQuery ||
    left.selected !== right.selected ||
    left.onRequestToolCallDetail !== right.onRequestToolCallDetail
  ) return false;
  return commentaryMessagesRenderEqual(left.message, right.message);
}

function commentaryMessagesRenderEqual(left: CommentaryMessage, right: CommentaryMessage): boolean {
  if (
    left.id !== right.id ||
    left.traceEventId !== right.traceEventId ||
    left.kind !== right.kind ||
    left.markerType !== right.markerType ||
    left.taskAction !== right.taskAction ||
    left.taskAuthorAgentPath !== right.taskAuthorAgentPath ||
    left.taskRecipientAgentPath !== right.taskRecipientAgentPath ||
    left.taskOrigin !== right.taskOrigin ||
    left.taskDestination !== right.taskDestination ||
    left.taskRoute !== right.taskRoute ||
    left.toolName !== right.toolName ||
    left.toolCount !== right.toolCount ||
    left.contentMarkdown !== right.contentMarkdown ||
    left.abortNotice !== right.abortNotice ||
    left.surfaceDiagram?.source !== right.surfaceDiagram?.source ||
    left.surfaceDiagram?.surfaceId !== right.surfaceDiagram?.surfaceId ||
    left.surfaceDiagram?.surfaceName !== right.surfaceDiagram?.surfaceName ||
    left.surfaceDiagram?.changeKind !== right.surfaceDiagram?.changeKind
  ) return false;
  if (!stringArraysEqual(left.reasoningTraceLines, right.reasoningTraceLines)) return false;
  const leftCalls = left.toolCalls ?? [];
  const rightCalls = right.toolCalls ?? [];
  if (leftCalls.length !== rightCalls.length) return false;
  return leftCalls.every((call, index) => {
    const candidate = rightCalls[index];
    return candidate !== undefined &&
      call.id === candidate.id &&
      call.traceEventId === candidate.traceEventId &&
      call.label === candidate.label &&
      call.requestTraceEventId === candidate.requestTraceEventId &&
      call.observationTraceEventId === candidate.observationTraceEventId &&
      call.detailsDeferred === candidate.detailsDeferred &&
      repositorySearchDetailsEqual(call.repositorySearch, candidate.repositorySearch) &&
      call.input === candidate.input &&
      call.output === candidate.output;
  });
}

function repositorySearchDetailsEqual(
  left: CommentaryToolCall['repositorySearch'],
  right: CommentaryToolCall['repositorySearch']
): boolean {
  if (left === right) return true;
  return Boolean(
    left
    && right
    && left.query === right.query
    && stringArraysEqual(left.repositories, right.repositories)
    && stringArraysEqual(left.repositoryNames, right.repositoryNames)
  );
}

function stringArraysEqual(left: readonly string[] | undefined, right: readonly string[] | undefined): boolean {
  if (left === right) return true;
  if (!left || !right || left.length !== right.length) return false;
  return left.every((value, index) => value === right[index]);
}

export const commentaryScrollFadeClasses = scrollFadeClasses;

export function commentaryFollowLatestAfterScroll({
  wasFollowingLatest,
  distanceFromBottom,
  userInitiated
}: {
  wasFollowingLatest: boolean;
  distanceFromBottom: number;
  userInitiated: boolean;
}): boolean {
  if (distanceFromBottom <= 24) return true;
  if (userInitiated) return false;
  return wasFollowingLatest;
}

function ToolCallValue({ label, value }: { label: string; value: unknown }): JSX.Element {
  return (
    <div className="main-commentary-tool-call-value">
      <span>{label}</span>
      <pre>{commentaryToolValueText(value)}</pre>
    </div>
  );
}

function toggledSetValue(values: ReadonlySet<string>, value: string): Set<string> {
  const next = new Set(values);
  if (next.has(value)) next.delete(value);
  else next.add(value);
  return next;
}

function addedSetValue(values: ReadonlySet<string>, value: string): Set<string> {
  return new Set(values).add(value);
}

function setWithoutValue(values: ReadonlySet<string>, value: string): Set<string> {
  const next = new Set(values);
  next.delete(value);
  return next;
}

function mapWithoutKey<T>(values: ReadonlyMap<string, T>, key: string): Map<string, T> {
  const next = new Map(values);
  next.delete(key);
  return next;
}

export function commentaryToolValueText(value: unknown): string {
  if (typeof value === 'string') return value || '""';
  try {
    return JSON.stringify(value, null, 2) ?? String(value);
  } catch {
    return String(value);
  }
}

const SUBAGENT_TOOL_NAMES = new Set([
  'followup_task',
  'interrupt_agent',
  'list_agents',
  'send_message',
  'spawn_agent',
  'wait_agent'
]);

export function commentaryMessageIcon(
  kind: CommentaryMessage['kind'],
  toolName?: string
): JSX.Element | null {
  if (kind === 'progress') return <Brain size={16} />;
  if (kind === 'error') return <CircleAlert size={16} />;
  if (kind !== 'tool') return null;
  const normalizedToolName = toolName?.trim().toLowerCase() ?? '';
  if (SUBAGENT_TOOL_NAMES.has(normalizedToolName)) return <Bot size={16} />;
  if (normalizedToolName.startsWith('runbook.') || normalizedToolName.startsWith('runbook_')) {
    return <BookOpen size={16} />;
  }
  if (normalizedToolName.startsWith('report.') || normalizedToolName.startsWith('report_')) {
    return <FileText size={16} />;
  }
  if (
    normalizedToolName.startsWith('memory.')
    || normalizedToolName.startsWith('memory_')
    || normalizedToolName === 'history.search'
    || normalizedToolName === 'history_search'
  ) {
    return <Database size={16} />;
  }
  if (
    normalizedToolName === 'experiment.run'
    || normalizedToolName === 'experiment_run'
    || normalizedToolName === 'shell.run'
    || normalizedToolName === 'shell_run'
  ) {
    return <Terminal size={16} />;
  }
  return <Wrench size={16} />;
}

export function commentaryMessageLabel(
  kind: CommentaryMessage['kind'],
  taskAction?: CommentaryMessage['taskAction']
): string | null {
  if (kind === 'task') {
    if (taskAction === 'spawn') return 'New subagent task';
    if (taskAction === 'followup') return 'Follow-up task';
    if (taskAction === 'room_message') return 'Room message';
    return 'Inter-Agent Message';
  }
  return null;
}
