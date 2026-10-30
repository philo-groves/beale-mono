import type { CommentaryMessage } from '../../view-models/commentary';

interface MessageSnapshot {
  id: string;
  text: string;
}

interface PendingMessage {
  id: string;
  units: string[];
  shown: number;
}

export type CommentaryStreamingMode = 'character' | 'word';

export interface CommentaryStreamingState {
  scopeKey: string | null;
  source: MessageSnapshot[];
  pending: PendingMessage[];
  animatedIds: ReadonlySet<string>;
}

const segmenter = new Intl.Segmenter(undefined, { granularity: 'grapheme' });
const charactersOf = (text: string): string[] => Array.from(segmenter.segment(text), ({ segment }) => segment);
const wordsOf = (text: string): string[] => text.match(/\s*\S+\s*|\s+/gu) ?? [];
const unitsOf = (text: string, mode: CommentaryStreamingMode): string[] => mode === 'word' ? wordsOf(text) : charactersOf(text);
export const commentaryStreamingModeForUserAgent = (userAgent: string): CommentaryStreamingMode =>
  /\bWindows\b/iu.test(userAgent) ? 'word' : 'character';
const messageText = (message: CommentaryMessage): string => message.kind === 'progress'
  ? (message.reasoningTraceLines ?? [message.contentMarkdown]).join('\n')
  : message.contentMarkdown;
const isInstantMessage = (message: CommentaryMessage): boolean => message.kind === 'diagram'
  || message.kind === 'task' && (message.taskAction === 'message' || message.taskAction === 'room_message');

function visiblePartialReasoningLine(line: string): string {
  if (line === '*') return '';
  if (!line.startsWith('**') || line.indexOf('**', 2) !== -1) return line;
  const content = line.slice(2, line.endsWith('*') ? -1 : undefined);
  const trailingWhitespace = content.match(/\s*$/)?.[0] ?? '';
  const emphasized = content.slice(0, content.length - trailingWhitespace.length);
  return emphasized ? `**${emphasized}**${trailingWhitespace}` : trailingWhitespace;
}

export function initialCommentaryStreamingState(
  scopeKey: string | null,
  messages: readonly CommentaryMessage[]
): CommentaryStreamingState {
  return {
    scopeKey,
    source: messages.map((message) => ({ id: message.id, text: messageText(message) })),
    pending: [],
    animatedIds: new Set()
  };
}

export function reconcileCommentaryStreaming(
  state: CommentaryStreamingState,
  scopeKey: string | null,
  messages: readonly CommentaryMessage[],
  animate: boolean,
  terminal = false,
  mode: CommentaryStreamingMode = 'character'
): CommentaryStreamingState {
  if (state.scopeKey !== scopeKey) return initialCommentaryStreamingState(scopeKey, messages);
  const source = messages.map((message) => ({ id: message.id, text: messageText(message) }));
  const unchanged = source.length === state.source.length && source.every((entry, index) =>
    entry.id === state.source[index]?.id && entry.text === state.source[index]?.text
  );
  const kindById = new Map(messages.map((message) => [message.id, message.kind]));
  const instantMessageIds = new Set(messages.filter(isInstantMessage).map((message) => message.id));
  const hasHiddenTerminalActivity = terminal && state.pending.some((entry) => {
    const kind = kindById.get(entry.id);
    return kind !== 'final_answer' && kind !== 'error';
  });
  if (!animate) return unchanged && state.pending.length === 0 ? state : initialCommentaryStreamingState(scopeKey, messages);
  if (unchanged && !hasHiddenTerminalActivity) return state;

  const previousById = new Map(state.source.map((entry) => [entry.id, entry.text]));
  const currentById = new Map(source.map((entry) => [entry.id, entry.text]));
  const pending = state.pending.flatMap((entry) => {
    if (instantMessageIds.has(entry.id)) return [];
    const text = currentById.get(entry.id);
    if (text === undefined) return [];
    const kind = kindById.get(entry.id);
    if (terminal && kind !== 'final_answer' && kind !== 'error') return [];
    const previousText = previousById.get(entry.id) ?? '';
    if (!text.startsWith(previousText)) return [];
    return [{ ...entry, units: unitsOf(text, mode) }];
  });
  const queuedIds = new Set(pending.map((entry) => entry.id));
  const lastPrevious = state.source.at(-1);
  if (lastPrevious && !queuedIds.has(lastPrevious.id) && !instantMessageIds.has(lastPrevious.id)
    && (!terminal || kindById.get(lastPrevious.id) === 'final_answer' || kindById.get(lastPrevious.id) === 'error')) {
    const updatedText = currentById.get(lastPrevious.id);
    if (updatedText?.startsWith(lastPrevious.text) && updatedText.length > lastPrevious.text.length) {
      pending.push({ id: lastPrevious.id, units: unitsOf(updatedText, mode), shown: unitsOf(lastPrevious.text, mode).length });
      queuedIds.add(lastPrevious.id);
    }
  }

  let lastKnownIndex = -1;
  source.forEach((entry, index) => {
    if (previousById.has(entry.id)) lastKnownIndex = index;
  });
  const animatedIds = new Set(state.animatedIds);
  for (const message of messages.slice(lastKnownIndex + 1)) {
    if (previousById.has(message.id) || queuedIds.has(message.id)) continue;
    if (terminal && message.kind !== 'final_answer' && message.kind !== 'error') continue;
    if (isInstantMessage(message)) {
      animatedIds.add(message.id);
      continue;
    }
    const units = unitsOf(messageText(message), mode);
    pending.push({
      id: message.id,
      units,
      shown: message.kind === 'user' || message.kind === 'marker' ? units.length : 0
    });
    animatedIds.add(message.id);
  }
  return { scopeKey, source, pending, animatedIds };
}

export function advanceCommentaryStreaming(state: CommentaryStreamingState): CommentaryStreamingState {
  const first = state.pending[0];
  if (!first) return state;
  if (first.shown >= first.units.length) return { ...state, pending: state.pending.slice(1) };
  return {
    ...state,
    pending: [{ ...first, shown: first.shown + 1 }, ...state.pending.slice(1)]
  };
}

export function visibleCommentaryMessages(
  state: CommentaryStreamingState,
  messages: readonly CommentaryMessage[]
): readonly CommentaryMessage[] {
  const first = state.pending[0];
  if (!first) return messages;
  const activeIndex = messages.findIndex((message) => message.id === first.id);
  if (activeIndex < 0) return messages;
  const visible = messages.slice(0, activeIndex + 1);
  const message = visible[activeIndex];
  const text = first.units.slice(0, first.shown).join('');
  const reasoningTraceLines = text ? text.split('\n') : [];
  if (reasoningTraceLines.length > 0) {
    const lastIndex = reasoningTraceLines.length - 1;
    reasoningTraceLines[lastIndex] = visiblePartialReasoningLine(reasoningTraceLines[lastIndex] ?? '');
  }
  const visibleReasoningTraceLines = reasoningTraceLines.filter((line, index, lines) => line || index < lines.length - 1);
  if (visibleReasoningTraceLines.length === 0) visibleReasoningTraceLines.push('');
  visible[activeIndex] = message.kind === 'progress'
    ? { ...message, contentMarkdown: text, reasoningTraceLines: visibleReasoningTraceLines }
    : { ...message, contentMarkdown: text };
  return visible;
}
