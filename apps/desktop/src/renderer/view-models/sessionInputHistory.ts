import type { TranscriptMessageRecord } from '@shared/types';

type SentMessage = Pick<TranscriptMessageRecord, 'role' | 'source' | 'contentMarkdown'>;

export function sessionInputHistory(
  promptMarkdown: string,
  transcriptMessages: readonly SentMessage[],
  optimisticMessages: readonly string[] = []
): string[] {
  const steeringEntries = transcriptMessages
    .filter((message) => message.role === 'user' && message.source === 'user_steering')
    .map((message) => message.contentMarkdown.trim())
    .filter(Boolean);
  const entries = [promptMarkdown.trim(), ...steeringEntries].filter(Boolean);
  const optimistic = optimisticMessages.map((message) => message.trim()).filter(Boolean);
  let overlap = Math.min(steeringEntries.length, optimistic.length);
  while (overlap > 0 && !steeringEntries.slice(-overlap).every((entry, index) => entry === optimistic[index])) {
    overlap -= 1;
  }
  return [...entries, ...optimistic.slice(overlap)];
}

export interface SessionInputHistoryNavigation {
  value: string;
  index: number | null;
  draft: string;
  caret: number;
}

export function navigateSessionInputHistory(input: {
  key: string;
  value: string;
  selectionStart: number;
  selectionEnd: number;
  entries: readonly string[];
  index: number | null;
  draft: string;
  modified: boolean;
  composing: boolean;
}): SessionInputHistoryNavigation | null {
  const { key, value, selectionStart, selectionEnd, entries, index, draft, modified, composing } = input;
  if (modified || composing || selectionStart !== selectionEnd || entries.length === 0) return null;
  if (key === 'ArrowUp') {
    if (index === null && value.includes('\n') && selectionStart !== 0) return null;
    const nextIndex = index === null ? entries.length - 1 : Math.max(0, Math.min(index, entries.length - 1) - 1);
    return { value: entries[nextIndex]!, index: nextIndex, draft: index === null ? value : draft, caret: 0 };
  }
  if (key === 'ArrowDown' && index !== null) {
    const nextIndex = Math.min(index + 1, entries.length);
    const nextValue = nextIndex === entries.length ? draft : entries[nextIndex]!;
    return { value: nextValue, index: nextIndex === entries.length ? null : nextIndex, draft, caret: nextValue.length };
  }
  return null;
}
