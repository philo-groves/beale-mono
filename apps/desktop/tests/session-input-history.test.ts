import { describe, expect, it } from 'vitest';
import { navigateSessionInputHistory, sessionInputHistory } from '../src/renderer/view-models/sessionInputHistory';

describe('session input history', () => {
  it('recalls the initial prompt and sent steering messages from the session transcript', () => {
    expect(sessionInputHistory('Inspect ExampleCo parser states.', [
      { role: 'assistant', source: 'model', contentMarkdown: 'I will inspect the parser.' },
      { role: 'user', source: 'user_steering', contentMarkdown: 'Check the length boundary.' },
      { role: 'user', source: 'system_context', contentMarkdown: 'Injected context.' },
      { role: 'user', source: 'user_steering', contentMarkdown: 'Compare the two branches.' }
    ])).toEqual([
      'Inspect ExampleCo parser states.',
      'Check the length boundary.',
      'Compare the two branches.'
    ]);
  });

  it('shows a just-sent message before refresh and removes its optimistic copy afterward', () => {
    const sent = { role: 'user' as const, source: 'user_steering', contentMarkdown: 'Check the length boundary.' };
    expect(sessionInputHistory('Initial prompt.', [], [sent.contentMarkdown])).toEqual([
      'Initial prompt.', sent.contentMarkdown
    ]);
    expect(sessionInputHistory('Initial prompt.', [sent], [sent.contentMarkdown])).toEqual([
      'Initial prompt.', sent.contentMarkdown
    ]);
  });

  it('walks backward and forward, then restores the unsent draft', () => {
    const entries = ['First prompt', 'Later steering'];
    const base = { value: 'Unsent draft', selectionStart: 11, selectionEnd: 11, entries, modified: false, composing: false };
    const latest = navigateSessionInputHistory({ ...base, key: 'ArrowUp', index: null, draft: '' });
    expect(latest).toEqual({ value: 'Later steering', index: 1, draft: 'Unsent draft', caret: 0 });
    const older = navigateSessionInputHistory({ ...base, key: 'ArrowUp', value: latest!.value, selectionStart: 0, selectionEnd: 0, index: latest!.index, draft: latest!.draft });
    expect(older).toEqual({ value: 'First prompt', index: 0, draft: 'Unsent draft', caret: 0 });
    const newer = navigateSessionInputHistory({ ...base, key: 'ArrowDown', value: older!.value, selectionStart: older!.value.length, selectionEnd: older!.value.length, index: older!.index, draft: older!.draft });
    expect(newer).toEqual({ value: 'Later steering', index: 1, draft: 'Unsent draft', caret: 14 });
    expect(navigateSessionInputHistory({ ...base, key: 'ArrowDown', value: newer!.value, selectionStart: newer!.value.length, selectionEnd: newer!.value.length, index: newer!.index, draft: newer!.draft }))
      .toEqual({ value: 'Unsent draft', index: null, draft: 'Unsent draft', caret: 12 });
  });

  it('leaves multiline cursor movement, selection, modifiers, and composition untouched', () => {
    const base = { key: 'ArrowUp', value: 'First line\nSecond line', selectionStart: 12, selectionEnd: 12, entries: ['Older message'], index: null, draft: '', modified: false, composing: false };
    expect(navigateSessionInputHistory(base)).toBeNull();
    expect(navigateSessionInputHistory({ ...base, key: 'ArrowDown', index: 0 })?.value).toBe('');
    expect(navigateSessionInputHistory({ ...base, selectionStart: 0, selectionEnd: 5 })).toBeNull();
    expect(navigateSessionInputHistory({ ...base, modified: true })).toBeNull();
    expect(navigateSessionInputHistory({ ...base, composing: true })).toBeNull();
    expect(navigateSessionInputHistory({ ...base, key: 'ArrowUp', selectionStart: 0, selectionEnd: 0 })?.value).toBe('Older message');
  });

  it('continues browsing after recalling a multiline message', () => {
    const entries = ['First line\nSecond line', 'Later steering'];
    const latest = navigateSessionInputHistory({
      key: 'ArrowUp', value: '', selectionStart: 0, selectionEnd: 0,
      entries, index: null, draft: '', modified: false, composing: false
    })!;
    const older = navigateSessionInputHistory({
      key: 'ArrowUp', value: latest.value, selectionStart: latest.caret, selectionEnd: latest.caret,
      entries, index: latest.index, draft: latest.draft, modified: false, composing: false
    })!;
    expect(older.value).toBe('First line\nSecond line');
    expect(navigateSessionInputHistory({
      key: 'ArrowDown', value: older.value, selectionStart: 0, selectionEnd: 0,
      entries, index: older.index, draft: older.draft, modified: false, composing: false
    })?.value).toBe('Later steering');
  });
});
