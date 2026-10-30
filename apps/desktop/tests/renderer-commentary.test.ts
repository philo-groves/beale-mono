import { describe, expect, it } from 'vitest';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { readFileSync } from 'node:fs';
import type { RunDetail, TraceEventRecord, WorkspaceScopeVersion } from '@shared/types';
import {
  COMMENTARY_RENDER_WINDOW_SIZE,
  CommentaryMessageRow,
  CommentaryView,
  commentaryFollowLatestAfterScroll,
  commentaryMessageIcon,
  commentaryMessageLabel,
  commentaryMessageOffsets,
  commentaryMessageSections,
  commentaryScrollFadeClasses,
  commentarySpacerHeight,
  commentaryStreamingScopeKey,
  commentaryToolValueText,
  commentaryWindowStartForIndex,
  commentaryWindowStartForOffset,
  isRunWorkingStatus,
  runWorkingDurationMs
} from '../src/renderer/features/commentary/CommentaryView';
import {
  commentaryMessagesForSession,
  commentaryRepositoryMetadataForScope,
  commentaryToolUsageText,
  type CommentaryMessage
} from '../src/renderer/view-models/commentary';
import {
  advanceCommentaryStreaming,
  commentaryStreamingModeForUserAgent,
  initialCommentaryStreamingState,
  reconcileCommentaryStreaming,
  visibleCommentaryMessages
} from '../src/renderer/features/commentary/commentaryStreaming';
import type { TraceDisplayEvent } from '../src/renderer/view-models/traceDisplay';
import { buildTraceDisplayEventsForAgentPath } from '../src/renderer/view-models/traceDisplay';
import { deferredResearchTrackDetail, runningResearchTrackDetail } from '../src/renderer/view-models/researchTrackSession';

describe('commentary streaming', () => {
  const message = (id: string, contentMarkdown: string, kind: CommentaryMessage['kind'] = 'commentary'): CommentaryMessage => ({
    id, traceEventId: id, kind, contentMarkdown, createdAt: '2026-01-01T00:00:00.000Z'
  });

  it('shows another subagent’s stored transcript immediately and animates only later messages', () => {
    const detail = runDetail('Inspect a synthetic parser.');
    const firstScope = commentaryStreamingScopeKey(detail, detail.run.id, true, '/root/example_reviewer_one');
    const secondScope = commentaryStreamingScopeKey(detail, detail.run.id, true, '/root/example_reviewer_two');
    const firstHistory = message('first-history', 'Earlier reviewer history.');
    const secondHistory = message('second-history', 'Another reviewer’s stored history.');
    const next = message('new-message', 'A new observation.');
    expect(secondScope).not.toBe(firstScope);
    let state = initialCommentaryStreamingState(firstScope, [firstHistory]);
    state = reconcileCommentaryStreaming(state, secondScope, [secondHistory], true);
    expect(state.pending).toHaveLength(0);
    expect(visibleCommentaryMessages(state, [secondHistory])).toEqual([secondHistory]);
    state = reconcileCommentaryStreaming(state, secondScope, [secondHistory, next], true);
    expect(state.pending.map((entry) => entry.id)).toEqual(['new-message']);
    expect(visibleCommentaryMessages(state, [secondHistory, next]).map((entry) => entry.contentMarkdown))
      .toEqual(['Another reviewer’s stored history.', '']);
  });

  it('shows existing history immediately, then reveals new messages in order by character', () => {
    const history = message('history', 'Already recorded.');
    const tool = message('tool', 'Read file', 'tool');
    const commentary = message('commentary', 'Done.');
    let state = initialCommentaryStreamingState('run-example', [history]);
    state = reconcileCommentaryStreaming(state, 'run-example', [history, tool, commentary], true);

    expect(visibleCommentaryMessages(state, [history, tool, commentary]).map((entry) => entry.contentMarkdown))
      .toEqual(['Already recorded.', '']);
    state = advanceCommentaryStreaming(state);
    expect(visibleCommentaryMessages(state, [history, tool, commentary]).map((entry) => entry.contentMarkdown))
      .toEqual(['Already recorded.', 'R']);
    for (let index = 0; index < 9; index += 1) state = advanceCommentaryStreaming(state);
    expect(visibleCommentaryMessages(state, [history, tool, commentary]).map((entry) => entry.contentMarkdown))
      .toEqual(['Already recorded.', 'Read file', '']);
    state = advanceCommentaryStreaming(state);
    expect(visibleCommentaryMessages(state, [history, tool, commentary]).at(-1)?.contentMarkdown).toBe('D');
  });

  it('shows inter-agent messages immediately so long messages do not delay later output', () => {
    const history = message('history', 'Earlier update.');
    const agentMessage: CommentaryMessage = { ...message('agent-message', 'A long synthetic handoff.\n'.repeat(20), 'task'),
      taskAction: 'message' };
    const commentary = message('commentary', 'Next step.');
    const state = reconcileCommentaryStreaming(initialCommentaryStreamingState('run-example', [history]),
      'run-example', [history, agentMessage, commentary], true);
    expect(state.pending.map((entry) => entry.id)).toEqual(['commentary']);
    expect(visibleCommentaryMessages(state, [history, agentMessage, commentary]).map((entry) => entry.contentMarkdown))
      .toEqual([history.contentMarkdown, agentMessage.contentMarkdown, '']);
  });

  it('shows a threat model diagram immediately without a synthetic tool line', () => {
    const existing = message('existing', 'Earlier update.');
    const diagram: CommentaryMessage = { ...message('diagram', 'Example parser', 'diagram'),
      surfaceDiagram: { source: 'flowchart LR\n  input["Example input"] --> parser["Parser"]',
        surfaceId: 'surface-example', surfaceName: 'Example parser', changeKind: 'created' } };
    const next = message('next', 'Next step.');
    const state = reconcileCommentaryStreaming(initialCommentaryStreamingState('run-example', [existing]),
      'run-example', [existing, diagram, next], true);
    expect(state.pending.map((entry) => entry.id)).toEqual(['next']);
    expect(visibleCommentaryMessages(state, [existing, diagram, next]).map((entry) => entry.contentMarkdown))
      .toEqual(['Earlier update.', 'Example parser', '']);
    const html = renderToStaticMarkup(createElement(CommentaryMessageRow, {
      message: diagram, searchHighlightQuery: '', selected: false
    }));
    expect(html).toMatch(/workspace-surface-threat-diagram-card is-session[\s\S]*workspace-surface-threat-diagram-header[\s\S]*workspace-surface-threat-diagram-icon[\s\S]*lucide-network[\s\S]*Example Parser[\s\S]*Threat Model[\s\S]*workspace-surface-threat-diagram/);
    expect(html).not.toContain('workspace-surface-threat-diagram-divider');
    expect((html.match(/lucide-network/gu) ?? []).length).toBe(1);
    expect(html).toContain('title="Example Parser">Example Parser</span>');
    expect(html).toContain('aria-label="Expand threat model diagram for Example parser"');
    expect(html).not.toContain('New Threat Model');
    expect(html).not.toContain('main-commentary-diagram-tool-line');
    expect(html).not.toContain('Creating threat model diagram');
    const updatedHtml = renderToStaticMarkup(createElement(CommentaryMessageRow, {
      message: { ...diagram, surfaceDiagram: { ...diagram.surfaceDiagram!, changeKind: 'updated' } },
      searchHighlightQuery: '', selected: false
    }));
    expect(updatedHtml).toContain('aria-label="Expand threat model diagram for Example parser"');
    expect(updatedHtml).toMatch(/workspace-surface-threat-diagram-type[^>]*>Threat Model<\/span>/);
    expect(updatedHtml).not.toContain('Updated Threat Model');
    expect(updatedHtml).not.toContain('New Threat Model');
    expect(updatedHtml).not.toContain('Updating threat model diagram');
    const acronymHtml = renderToStaticMarkup(createElement(CommentaryMessageRow, {
      message: { ...diagram, surfaceDiagram: { ...diagram.surfaceDiagram!, surfaceName: 'example API gateway' } },
      searchHighlightQuery: '', selected: false
    }));
    expect(acronymHtml).toContain('title="Example API Gateway">Example API Gateway</span>');
  });

  it('reveals whole words on Windows and keeps character streaming on macOS', () => {
    expect(commentaryStreamingModeForUserAgent('Electron (Windows NT 10.0; Win64; x64)')).toBe('word');
    expect(commentaryStreamingModeForUserAgent('Electron (Macintosh; Intel Mac OS X)')).toBe('character');
    const history = message('history', 'Earlier.');
    const next = message('next', 'Read file, then\ncheck 🔒 state.');
    let state = reconcileCommentaryStreaming(
      initialCommentaryStreamingState('run-example', [history]), 'run-example', [history, next], true, false, 'word'
    );
    const frames: string[] = [];
    while (state.pending.length > 0) {
      state = advanceCommentaryStreaming(state);
      frames.push(visibleCommentaryMessages(state, [history, next]).at(-1)?.contentMarkdown ?? '');
    }
    expect(frames).toEqual([
      'Read ', 'Read file, ', 'Read file, then\n',
      'Read file, then\ncheck ', 'Read file, then\ncheck 🔒 ',
      'Read file, then\ncheck 🔒 state.', 'Read file, then\ncheck 🔒 state.'
    ]);
  });

  it('keeps an already shown partial word visible when more text arrives', () => {
    const history = message('history', 'Earlier.');
    let current = message('current', 'Inspect par');
    let state = reconcileCommentaryStreaming(
      initialCommentaryStreamingState('run-example', [history]), 'run-example', [history, current], true, false, 'word'
    );
    state = advanceCommentaryStreaming(advanceCommentaryStreaming(state));
    expect(visibleCommentaryMessages(state, [history, current]).at(-1)?.contentMarkdown).toBe('Inspect par');
    current = message('current', 'Inspect parser now');
    state = reconcileCommentaryStreaming(state, 'run-example', [history, current], true, false, 'word');
    expect(visibleCommentaryMessages(state, [history, current]).at(-1)?.contentMarkdown).toBe('Inspect parser ');
    state = advanceCommentaryStreaming(state);
    expect(visibleCommentaryMessages(state, [history, current]).at(-1)?.contentMarkdown).toBe('Inspect parser now');
  });

  it('extends a streamed message and keeps earlier paginated history visible', () => {
    const initial = message('current', 'Hi');
    const older = message('older', 'Earlier.');
    let state = initialCommentaryStreamingState('run-example', [initial]);
    state = reconcileCommentaryStreaming(state, 'run-example', [older, message('current', 'Hi 👋')], true);
    expect(visibleCommentaryMessages(state, [older, message('current', 'Hi 👋')]).map((entry) => entry.contentMarkdown))
      .toEqual(['Earlier.', 'Hi']);
    state = advanceCommentaryStreaming(state);
    expect(visibleCommentaryMessages(state, [older, message('current', 'Hi 👋')]).at(-1)?.contentMarkdown).toBe('Hi ');
    state = advanceCommentaryStreaming(state);
    expect(visibleCommentaryMessages(state, [older, message('current', 'Hi 👋')]).at(-1)?.contentMarkdown).toBe('Hi 👋');
  });

  it('keeps opening reasoning emphasis out of every streamed frame', () => {
    const history = message('history', 'Earlier.');
    const lines = ['**Inspecting the parser**', '**Checking bounds**'];
    const progress = {
      ...message('progress', lines.join('\n'), 'progress'),
      reasoningTraceLines: lines
    };
    let state = reconcileCommentaryStreaming(
      initialCommentaryStreamingState('run-example', [history]),
      'run-example',
      [history, progress],
      true
    );
    let sawFirstEmphasis = false;
    let sawSecondEmphasis = false;
    while (state.pending.length > 0) {
      const visible = visibleCommentaryMessages(state, [history, progress]).at(-1);
      expect(visible?.kind).toBe('progress');
      const html = renderToStaticMarkup(createElement(CommentaryMessageRow, {
        message: visible!, searchHighlightQuery: '', selected: false
      }));
      expect(html).not.toContain('**');
      sawFirstEmphasis ||= html.includes('<strong>Inspect');
      sawSecondEmphasis ||= html.includes('<strong>Check');
      state = advanceCommentaryStreaming(state);
    }
    expect(sawFirstEmphasis).toBe(true);
    expect(sawSecondEmphasis).toBe(true);
  });

  it('flushes the queue for reduced motion or search and resets when the session changes', () => {
    const first = message('first', 'First');
    const next = message('next', 'Second');
    let state = reconcileCommentaryStreaming(initialCommentaryStreamingState('run-one', [first]), 'run-one', [first, next], true);
    state = reconcileCommentaryStreaming(state, 'run-one', [first, next], false);
    expect(state.pending).toHaveLength(0);
    expect(visibleCommentaryMessages(state, [first, next])).toEqual([first, next]);
    state = reconcileCommentaryStreaming(state, 'run-two', [message('other', 'Existing')], true);
    expect(state.pending).toHaveLength(0);
    expect(state.animatedIds.size).toBe(0);
  });

  it('lets a terminal answer stream without waiting for collapsed activity', () => {
    const history = message('history', 'Earlier.');
    const tool = message('tool', 'Long tool update', 'tool');
    const answer = message('answer', 'Complete.', 'final_answer');
    let state = reconcileCommentaryStreaming(
      initialCommentaryStreamingState('run-example', [history]), 'run-example', [history, tool], true
    );
    state = reconcileCommentaryStreaming(state, 'run-example', [history, tool, answer], true, true);
    expect(state.pending.map((entry) => entry.id)).toEqual(['answer']);
    expect(visibleCommentaryMessages(state, [history, tool, answer]).map((entry) => entry.contentMarkdown))
      .toEqual(['Earlier.', 'Long tool update', '']);
  });
});

describe('renderer commentary projection', () => {
  it('shows the stored failure reason when a session ends before producing transcript events', () => {
    const detail = runDetail('Inspect the example parser.');
    detail.run.status = 'failed';
    detail.run.summary = 'The workspace checkpoint rejected an unvalidated topic snapshot.';
    detail.run.endedAt = '2026-08-03T10:00:00.000Z';
    const messages = commentaryMessagesForSession(detail, []);
    expect(messages.map((message) => message.kind)).toEqual(['user', 'error']);
    expect(messages[1]?.contentMarkdown).toBe(detail.run.summary);
  });

  it('renders the commentary session loading state with a spinner and no composer', () => {
    const html = renderToStaticMarkup(
      createElement(CommentaryView, {
        busy: true,
        detail: null,
        events: [],
        providerModelCatalog: [],
        selectedRunId: 'run_loading',
        showBackToMain: false,
        searchHighlightQuery: '',
        onBackToMain: () => undefined,
        onSessionAction: () => undefined,
        onSteerInstruction: () => undefined
      })
    );

    expect(html).toContain('main-trace-view main-commentary-view is-loading');
    expect(html).toContain('data-commentary-state="session"');
    expect(html).toContain('class="centered-loading-state main-session-loading"');
    expect(html).toContain('class="centered-loading-state-spinner"');
    expect(html).not.toContain('lucide-loader-circle');
    expect(html).toContain('Loading session');
    expect(html).not.toContain('Loading session.');
    expect(html).not.toContain('class="main-trace-footer"');
  });

  it('shows a new session prompt and setup phase immediately', () => {
    const detail = runDetail('Inspect the parser boundary.');
    detail.run.status = 'active';
    detail.run.startedAt = '2026-08-03T09:59:00.000Z';
    detail.run.budget = {};
    detail.run.model = 'gpt-5.6-sol';
    detail.run.reasoningEffort = 'high';
    detail.run.shellSafetyMode = 'auto_review';
    const html = renderToStaticMarkup(
      createElement(CommentaryView, {
        busy: true,
        detail,
        sessionSetupPending: true,
        events: [],
        providerModelCatalog: [],
        selectedRunId: detail.run.id,
        showBackToMain: false,
        searchHighlightQuery: '',
        onBackToMain: () => undefined,
        onSessionAction: () => undefined,
        onSteerInstruction: () => undefined
      })
    );

    expect(html).toContain('Inspect the parser boundary.');
    expect(html).toContain('The session is in a setup phase. Please wait…');
    expect(html).not.toContain('Loading session');
  });

  it('renders feedback below the new research composer', () => {
    const html = renderToStaticMarkup(
      createElement(CommentaryView, {
        busy: false,
        state: 'new-research',
        detail: null,
        events: [],
        providerModelCatalog: [],
        selectedRunId: null,
        showBackToMain: false,
        searchHighlightQuery: '',
        postComposerContent: createElement('div', { role: 'alert' }, 'Could not add context.'),
        onBackToMain: () => undefined,
        onInitialInstruction: () => undefined,
        onSessionAction: () => undefined,
        onSteerInstruction: () => undefined
      })
    );

    expect(html).toContain('main-steer-post-composer-content');
    expect(html).toContain('Could not add context.');
  });

  it('keeps the working disclosure aligned to the commentary text width', () => {
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    const messageStyles = styles.match(/\.main-commentary-message\s*\{([^}]*)\}/)?.[1] ?? '';
    const disclosureStyles = styles.match(/\.run-work-disclosure\s*\{([^}]*)\}/)?.[1] ?? '';

    expect(messageStyles).toContain('max-width: var(--trace-content-max-width)');
    expect(disclosureStyles).toContain('max-width: var(--trace-content-max-width)');
    expect(disclosureStyles).toContain('margin-inline: auto');
    expect(styles).toContain('padding: 34px var(--trace-footer-composer-inset) 56px');
  });

  it('labels subagent assignments by lifecycle action', () => {
    expect(commentaryMessageLabel('user')).toBeNull();
    expect(commentaryMessageLabel('commentary')).toBeNull();
    expect(commentaryMessageLabel('progress')).toBeNull();
    expect(commentaryMessageLabel('tool')).toBeNull();
    expect(commentaryMessageLabel('task', 'spawn')).toBe('New subagent task');
    expect(commentaryMessageLabel('task', 'followup')).toBe('Follow-Up');
    expect(commentaryMessageLabel('task', 'message')).toBe('Collaboration');
    expect(commentaryMessageLabel('task', 'room_message')).toBe('Room message');
    expect(commentaryMessageLabel('final_answer')).toBeNull();
    expect(commentaryMessageLabel('error')).toBeNull();
  });

  it('uses distinct icons for reasoning and each tool family', () => {
    const progressIcon = commentaryMessageIcon('progress');
    expect(progressIcon).not.toBeNull();
    if (progressIcon) expect(renderToStaticMarkup(progressIcon)).toContain('lucide-brain');
    expect(renderToStaticMarkup(commentaryMessageIcon('tool', 'list_agents')!)).toContain('lucide-bot');
    expect(renderToStaticMarkup(commentaryMessageIcon('tool', 'wait_agent')!)).toContain('lucide-bot');
    expect(renderToStaticMarkup(commentaryMessageIcon('tool', 'runbook.get')!)).toContain('lucide-book-open');
    expect(renderToStaticMarkup(commentaryMessageIcon('tool', 'memory.search')!)).toContain('lucide-database');
    expect(renderToStaticMarkup(commentaryMessageIcon('tool', 'history.search')!)).toContain('lucide-database');
    expect(renderToStaticMarkup(commentaryMessageIcon('tool', 'shell.run')!)).toContain('lucide-terminal');
    expect(renderToStaticMarkup(commentaryMessageIcon('tool', 'experiment.run')!)).toContain('lucide-terminal');
    expect(renderToStaticMarkup(commentaryMessageIcon('tool', 'file.read')!)).toContain('lucide-wrench');
    expect(renderToStaticMarkup(commentaryMessageIcon('error')!)).toContain('lucide-circle-alert');
    expect(commentaryMessageIcon('commentary')).toBeNull();
    expect(commentaryMessageIcon('final_answer')).toBeNull();
  });

  it('uses concise sentence-case singular and plural copy for known and fallback tools', () => {
    expect(commentaryToolUsageText('list_agents', 1)).toBe('Checking on subagents');
    expect(commentaryToolUsageText('list_agents', 2)).toBe('Checking on subagents 2 times');
    expect(commentaryToolUsageText('file.read', 1)).toBe('Reading a file');
    expect(commentaryToolUsageText('file.read', 1, 'src/Parser.ts')).toBe('Reading src/Parser.ts');
    expect(commentaryToolUsageText('file.read', 1, 'C:\\Users\\alice\\repo\\src\\Parser.ts'))
      .toBe('Reading ~/repo/src/Parser.ts');
    expect(commentaryToolUsageText('file.read', 1, '/home/alice/repo/src/Parser.ts'))
      .toBe('Reading ~/repo/src/Parser.ts');
    expect(commentaryToolUsageText('file.read', 1, '/Users/alice/repo/src/Parser.ts'))
      .toBe('Reading ~/repo/src/Parser.ts');
    expect(commentaryToolUsageText('file.read', 1, '/root/repo/src/Parser.ts'))
      .toBe('Reading ~/repo/src/Parser.ts');
    expect(commentaryToolUsageText('file.read', 1, 'C:\\Users\\Public\\Parser.ts'))
      .toBe('Reading C:\\Users\\Public\\Parser.ts');
    expect(commentaryToolUsageText('file.read', 3)).toBe('Reading 3 files');
    expect(commentaryToolUsageText('file.read', 3, undefined, true)).toBe('Reading 3 files');
    expect(commentaryToolUsageText('file.edit', 1)).toBe('Editing a file');
    expect(commentaryToolUsageText('file.edit', 1, 'src/Parser.ts')).toBe('Editing src/Parser.ts');
    expect(commentaryToolUsageText('file.edit', 1, '/Users/alice/repo/src/Parser.ts'))
      .toBe('Editing ~/repo/src/Parser.ts');
    expect(commentaryToolUsageText('file.edit', 2)).toBe('Editing 2 files');
    expect(commentaryToolUsageText('memory.get', 3)).toBe('Reading 3 memories');
    expect(commentaryToolUsageText('memory.search', 3)).toBe('Searching memory with 3 queries');
    expect(commentaryToolUsageText('history.search', 3)).toBe('Searching workspace history with 3 queries');
    expect(commentaryToolUsageText('shell.run', 1)).toBe('Running a command');
    expect(commentaryToolUsageText('shell.run', 1, 'npm run Check')).toBe('Running npm run Check');
    expect(commentaryToolUsageText('shell.run', 4)).toBe('Running 4 commands');
    expect(commentaryToolUsageText('shell.run', 1, 'npm test', true)).toBe('Ran npm test');
    expect(commentaryToolUsageText('surface.get', 1)).toBe('Inspecting a surface');
    expect(commentaryToolUsageText('surface.views', 1)).toBe('Inspecting surface views');
    expect(commentaryToolUsageText('surface.list', 1)).toBe('Listing surfaces');
    expect(commentaryToolUsageText('surface.focus', 1)).toBe('Focusing on a surface');
    expect(commentaryToolUsageText('finding.revise', 1)).toBe('Revising a finding');
    expect(commentaryToolUsageText('finding.revise', 2)).toBe('Revising 2 findings');
    expect(commentaryToolUsageText('track.checkpoint', 1)).toBe('Checkpointing the research track');
    expect(commentaryToolUsageText('track.checkpoint', 2)).toBe('Checkpointing the research track 2 times');
    expect(commentaryToolUsageText('features.load', 1)).toBe('Loading features');
    expect(commentaryToolUsageText('plugins.load', 1)).toBe('Loading plugins');
    expect(commentaryToolUsageText('status', 1)).toBe('Checking status');
    expect(commentaryToolUsageText('iphones', 1)).toBe('Listing iPhones');
    expect(commentaryToolUsageText('mcp.browser.snapshot', 1)).toBe('Capturing a browser snapshot');
    expect(commentaryToolUsageText('mcp.example.status', 1)).toBe('Checking status');
    expect(commentaryToolUsageText('mcp.example.iphones', 1)).toBe('Listing iPhones');
    expect(commentaryToolUsageText('custom.scan', 2)).toBe('Scanning with custom scan 2 times');
    expect(commentaryToolUsageText('custom.search', 1)).toBe('Searching with custom search');
    expect(commentaryToolUsageText('custom.unknown', 1)).toBe('Calling custom unknown');
  });

  it('shows one activity row per tool call, coalesces repeats, and suppresses paired results', () => {
    const messages = commentaryMessagesForSession(runDetail('Inspect the parser.'), [
      toolEvent('agents-request', 'tool.requested', 'list_agents', 'agents', {}),
      toolEvent('agents-result', 'tool.observed', 'list_agents', 'agents', {}, { agents: [{ path: '/root/parser' }] }),
      toolEvent('read-one-request', 'tool.requested', 'file.read', 'read-one', { path: 'src/parser.ts' }),
      toolEvent('read-one-result', 'tool.observed', 'file.read', 'read-one', { path: 'src/parser.ts' }, { text: 'first file' }),
      toolEvent('read-two-request', 'tool.requested', 'file.read', 'read-two', { path: 'src/token.ts' }),
      toolEvent('read-two-result', 'tool.observed', 'file.read', 'read-two', { path: 'src/token.ts' }, { text: 'second file' }),
      toolEvent('shell-request', 'tool.requested', 'shell.run', 'shell', { utility: 'npm', args: ['test'] }),
      toolEvent('repository-result', 'tool.observed', 'repository.search', 'repository-only', { query: 'decodeToken', path: '/work/parser' }, { matches: 2 }),
      toolEvent('spawn-request', 'tool.requested', 'spawn_agent', 'spawn'),
      displayEvent('spawn', {
        type: 'subagent.activity',
        action: 'spawned',
        agentPath: '/root/parser_review',
        message: 'Inspect the parser boundary.'
      }, { source: 'system' })
    ]);

    expect(messages.map(({ kind, toolName, toolCount, contentMarkdown }) => [
      kind,
      toolName,
      toolCount,
      contentMarkdown
    ])).toEqual([
      ['user', undefined, undefined, 'Inspect the parser.'],
      ['tool', 'list_agents', 1, 'Checking on subagents'],
      ['tool', 'file.read', 2, 'Reading 2 files'],
      ['tool', 'shell.run', 1, 'Running npm test'],
      ['tool', 'repository.search', 1, 'Querying parser for "decodeToken"'],
      ['task', undefined, undefined, 'Inspect the parser boundary.']
    ]);

    expect(messages.find((message) => message.toolName === 'file.read')?.toolCalls).toEqual([
      {
        id: 'read-one-request',
        traceEventId: 'read-one-result',
        label: 'src/parser.ts',
        input: { path: 'src/parser.ts' },
        output: { text: 'first file' }
      },
      {
        id: 'read-two-request',
        traceEventId: 'read-two-result',
        label: 'src/token.ts',
        input: { path: 'src/token.ts' },
        output: { text: 'second file' }
      }
    ]);
    expect(messages.find((message) => message.toolName === 'shell.run')?.toolCalls).toEqual([
      {
        id: 'shell-request',
        traceEventId: 'shell-request',
        label: 'npm test',
        input: { utility: 'npm', args: ['test'] },
        output: 'Waiting for output.'
      }
    ]);
    expect(messages.find((message) => message.toolName === 'repository.search')?.toolCalls).toEqual([
      {
        id: 'repository-result',
        traceEventId: 'repository-result',
        label: 'Querying parser for "decodeToken"',
        repositorySearch: {
          repositories: ['/work/parser'],
          repositoryNames: ['parser'],
          query: 'decodeToken'
        },
        input: { query: 'decodeToken', path: '/work/parser' },
        output: { matches: 2 }
      }
    ]);
  });

  it('keeps completed tool labels in active tense while the run is active', () => {
    const detail = runDetail('Inspect and test the parser.');
    detail.run.status = 'active';
    const messages = commentaryMessagesForSession(detail, [
      toolEvent('read-request', 'tool.requested', 'file.read', 'read', { path: 'src/parser.ts' }),
      toolEvent('read-result', 'tool.observed', 'file.read', 'read', { path: 'src/parser.ts' }, { text: 'source' }),
      toolEvent('shell-request', 'tool.requested', 'shell.run', 'shell', { command: 'npm test' }),
      toolEvent('shell-result', 'tool.observed', 'shell.run', 'shell', { command: 'npm test' }, { exitCode: 0 })
    ]);

    expect(messages.find((message) => message.toolName === 'file.read')?.contentMarkdown)
      .toBe('Reading src/parser.ts');
    expect(messages.find((message) => message.toolName === 'shell.run')?.contentMarkdown)
      .toBe('Running npm test');
  });

  it('names the edited file in a session tool row', () => {
    const detail = runDetail('Update the parser.');
    const messages = commentaryMessagesForSession(detail, [
      toolEvent('edit-request', 'tool.requested', 'file.edit', 'edit', { path: 'src/parser.ts', oldText: 'old', newText: 'new' }),
      toolEvent('edit-result', 'tool.observed', 'file.edit', 'edit', { path: 'src/parser.ts' }, { path: 'src/parser.ts' })
    ]);
    const observationOnly = commentaryMessagesForSession(detail, [
      toolEvent('edit-only-result', 'tool.observed', 'file.edit', 'edit-only', { path: 'src/token.ts' }, { path: 'src/token.ts' })
    ]);

    expect(messages.find((message) => message.toolName === 'file.edit')?.contentMarkdown)
      .toBe('Editing src/parser.ts');
    expect(observationOnly.find((message) => message.toolName === 'file.edit')?.contentMarkdown)
      .toBe('Editing src/token.ts');
  });

  it('formats tool input and output values for expanded details', () => {
    expect(commentaryToolValueText({ path: 'src/parser.ts', lines: [1, 2] })).toBe(
      '{\n  "path": "src/parser.ts",\n  "lines": [\n    1,\n    2\n  ]\n}'
    );
    expect(commentaryToolValueText('Waiting for output.')).toBe('Waiting for output.');
    expect(commentaryToolValueText(null)).toBe('null');
  });

  it('shows the actual one-line shell command before its details are expanded', () => {
    const messages = commentaryMessagesForSession(runDetail('Run the checks.'), [
      toolEvent('shell-command-request', 'tool.requested', 'shell.run', 'shell-command', {
        command: 'npm test\n  -- --runInBand'
      })
    ]);

    expect(messages.find((message) => message.toolName === 'shell.run')?.toolCalls?.[0]?.label)
      .toBe('npm test -- --runInBand');
    expect(messages.find((message) => message.toolName === 'shell.run')?.contentMarkdown)
      .toBe('Running npm test -- --runInBand');
  });

  it('labels singular and grouped repository searches from repositories and query calls', () => {
    const detail = runDetail('Search the repositories.');
    const repositoryMetadata = commentaryRepositoryMetadataForScope({
      assets: [
        {
          id: 'repository-source',
          direction: 'in_scope',
          kind: 'repo',
          value: 'https://github.com/example/parser.git',
          attributes: {
            displayName: 'Parser Metadata Name',
            repositoryUrl: 'https://github.com/example/parser.git'
          }
        },
        {
          id: 'repository-checkout',
          direction: 'in_scope',
          kind: 'repo',
          value: 'C:\\Users\\alice\\work\\parser.git',
          attributes: {
            repositoryUrl: 'https://github.com/example/parser.git',
            sourceAssetId: 'repository-source'
          }
        },
        {
          id: 'runtime-checkout',
          direction: 'in_scope',
          kind: 'repo',
          value: 'C:\\Users\\alice\\.beale\\repositories\\github.com_example_runtime\\default',
          attributes: {
            repositoryUrl: 'https://github.com/example/runtime.git'
          }
        }
      ]
    } as unknown as WorkspaceScopeVersion);
    const singular = commentaryMessagesForSession(detail, [
      toolEvent('repo-one-request', 'tool.requested', 'repository.search', 'repo-one', {
        query: 'decodeToken'
      }),
      toolEvent('repo-one-result', 'tool.observed', 'repository.search', 'repo-one', {
        query: 'decodeToken'
      }, {
        roots: ['C:\\Users\\alice\\work\\parser.git'],
        query: 'decodeToken',
        matches: []
      })
    ], { repositoryMetadata }).find((message) => message.toolName === 'repository.search');
    expect(singular?.contentMarkdown).toBe('Querying Parser Metadata Name for "decodeToken"');
    expect(singular?.toolCalls?.[0]?.label).toBe('Querying Parser Metadata Name for "decodeToken"');

    const urlNamed = commentaryMessagesForSession(detail, [
      toolEvent('repo-url-request', 'tool.requested', 'repository.search', 'repo-url', {
        query: 'loadConfig'
      }),
      toolEvent('repo-url-result', 'tool.observed', 'repository.search', 'repo-url', {
        query: 'loadConfig'
      }, {
        roots: ['C:\\Users\\alice\\.beale\\repositories\\github.com_example_runtime\\default\\app'],
        query: 'loadConfig',
        matches: []
      })
    ], { repositoryMetadata }).find((message) => message.toolName === 'repository.search');
    expect(urlNamed?.contentMarkdown).toBe('Querying runtime for "loadConfig"');
    expect(urlNamed?.toolCalls?.[0]?.label).toBe('Querying runtime for "loadConfig"');

    const grouped = commentaryMessagesForSession(detail, [
      toolEvent('repo-parser-one-request', 'tool.requested', 'repository.search', 'repo-parser-one', {
        query: 'decodeToken'
      }),
      toolEvent('repo-parser-one-result', 'tool.observed', 'repository.search', 'repo-parser-one', {
        query: 'decodeToken'
      }, { roots: ['/work/parser'], query: 'decodeToken', matches: [] }),
      toolEvent('repo-parser-two-request', 'tool.requested', 'repository.search', 'repo-parser-two', {
        query: 'encodeToken'
      }),
      toolEvent('repo-parser-two-result', 'tool.observed', 'repository.search', 'repo-parser-two', {
        query: 'encodeToken'
      }, { roots: ['/work/parser/'], query: 'encodeToken', matches: [] }),
      toolEvent('repo-runtime-request', 'tool.requested', 'repository.search', 'repo-runtime', {
        query: 'token boundary'
      }),
      toolEvent('repo-runtime-result', 'tool.observed', 'repository.search', 'repo-runtime', {
        query: 'token boundary'
      }, { roots: ['/work/runtime'], query: 'token boundary', matches: [] })
    ]).find((message) => message.toolName === 'repository.search');
    expect(grouped).toMatchObject({
      toolCount: 3,
      contentMarkdown: 'Searching 2 repositories with 3 queries'
    });
    expect(grouped?.toolCalls).toHaveLength(3);
    expect(grouped?.toolCalls?.map((toolCall) => toolCall.label)).toEqual([
      'Querying parser for "decodeToken"',
      'Querying parser for "encodeToken"',
      'Querying runtime for "token boundary"'
    ]);

    const sameRepository = commentaryMessagesForSession(detail, [
      toolEvent('repo-same-one', 'tool.requested', 'repository.search', 'repo-same-one', {
        path: '/work/parser',
        query: 'decodeToken'
      }),
      toolEvent('repo-same-two', 'tool.requested', 'repository.search', 'repo-same-two', {
        path: '/work/parser',
        query: 'encodeToken'
      })
    ]).find((message) => message.toolName === 'repository.search');
    expect(sameRepository?.contentMarkdown).toBe('Searching 1 repository with 2 queries');
  });

  it('labels singular and grouped workspace history searches with their queries', () => {
    const detail = runDetail('Search workspace history.');
    const singular = commentaryMessagesForSession(detail, [
      toolEvent('history-one', 'tool.requested', 'history.search', 'history-one', {
        query: 'parser boundary'
      })
    ]).find((message) => message.toolName === 'history.search');
    expect(singular).toMatchObject({
      toolCount: 1,
      contentMarkdown: 'Searching workspace history for "parser boundary"'
    });
    expect(singular?.toolCalls?.[0]?.label).toBe('Searching workspace history for "parser boundary"');

    const grouped = commentaryMessagesForSession(detail, [
      toolEvent('history-parser', 'tool.requested', 'history.search', 'history-parser', {
        query: 'parser boundary'
      }),
      toolEvent('history-runtime', 'tool.requested', 'history.search', 'history-runtime', {
        query: 'runtime assumptions'
      })
    ]).find((message) => message.toolName === 'history.search');
    expect(grouped).toMatchObject({
      toolCount: 2,
      contentMarkdown: 'Searching workspace history with 2 queries'
    });
    expect(grouped?.toolCalls?.map((toolCall) => toolCall.label)).toEqual([
      'Searching workspace history for "parser boundary"',
      'Searching workspace history for "runtime assumptions"'
    ]);
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    const summaryTextStyles = styles.match(/\.main-commentary-tool-summary\s*>\s*span\s*\{([^}]*)\}/)?.[1] ?? '';
    expect(summaryTextStyles).toContain('min-width: 0');
    expect(summaryTextStyles).toContain('overflow: hidden');
    expect(summaryTextStyles).toContain('text-overflow: ellipsis');
    expect(summaryTextStyles).toContain('white-space: nowrap');
  });

  it('renders singular commands and file reads inline while keeping grouped calls expandable', () => {
    const detail = runDetail('Run the checks.');
    const singleHtml = renderToStaticMarkup(createElement(CommentaryView, {
      busy: false,
      detail,
      events: [toolEvent('single-shell', 'tool.requested', 'shell.run', 'single-shell', { command: 'npm test' })],
      providerModelCatalog: [],
      selectedRunId: detail.run.id,
      showBackToMain: true,
      searchHighlightQuery: '',
      onBackToMain: () => undefined,
      onSessionAction: () => undefined,
      onSteerInstruction: () => undefined
    }));
    expect(singleHtml).toContain('aria-expanded="false" class="main-commentary-tool-call-summary main-commentary-single-tool"');
    expect(singleHtml).toContain('<span class="main-commentary-single-tool-label">Running npm test</span>');
    expect(singleHtml).not.toContain('main-commentary-tool-summary');
    expect(singleHtml).toContain('lucide-chevron-right main-commentary-single-tool-chevron');
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    const singleToolStyles = styles.match(/\.main-commentary-tool-call-summary\.main-commentary-single-tool\s*\{([^}]*)\}/)?.[1] ?? '';
    const singleToolChevronStyles = styles.match(/\.main-commentary-single-tool-chevron\s*\{([^}]*)\}/)?.[1] ?? '';
    expect(singleToolStyles).toContain('padding: 0');
    expect(singleToolChevronStyles).toContain('opacity: 0');
    expect(singleToolChevronStyles).toContain('opacity 120ms ease');
    expect(styles).toContain('.main-commentary-message.kind-tool:hover .main-commentary-single-tool-chevron');

    const fileEvents = [
      toolEvent('file-one', 'tool.requested', 'file.read', 'file-one', { path: 'C:\\Users\\alice\\repo\\src\\Parser.ts' })
    ];
    expect(commentaryMessagesForSession(detail, fileEvents).find((message) => message.toolName === 'file.read'))
      .toMatchObject({ toolCount: 1, contentMarkdown: 'Reading ~/repo/src/Parser.ts' });
    const fileHtml = renderToStaticMarkup(createElement(CommentaryView, {
      busy: false,
      detail,
      events: fileEvents,
      providerModelCatalog: [],
      selectedRunId: detail.run.id,
      showBackToMain: true,
      searchHighlightQuery: '',
      onBackToMain: () => undefined,
      onSessionAction: () => undefined,
      onSteerInstruction: () => undefined
    }));
    expect(fileHtml).toContain('<span class="main-commentary-single-tool-label">Reading ~/repo/src/Parser.ts</span>');
    expect(fileHtml).toContain('lucide-chevron-right main-commentary-single-tool-chevron');
    expect(commentaryMessagesForSession(detail, fileEvents).find((message) => message.toolName === 'file.read')?.toolCalls?.[0]?.input)
      .toEqual({ path: 'C:\\Users\\alice\\repo\\src\\Parser.ts' });

    const multipleEvents = [
      toolEvent('shell-one', 'tool.requested', 'shell.run', 'shell-one', { command: 'npm test' }),
      toolEvent('shell-two', 'tool.requested', 'shell.run', 'shell-two', { command: 'npm run typecheck' })
    ];
    const multipleMessages = commentaryMessagesForSession(detail, multipleEvents);
    const shellMessage = multipleMessages.find((message) => message.toolName === 'shell.run');
    expect(shellMessage).toMatchObject({ toolCount: 2, contentMarkdown: 'Running 2 commands' });
    expect(shellMessage?.toolCalls).toHaveLength(2);
    const multipleHtml = renderToStaticMarkup(createElement(CommentaryView, {
      busy: false,
      detail,
      events: multipleEvents,
      providerModelCatalog: [],
      selectedRunId: detail.run.id,
      showBackToMain: true,
      searchHighlightQuery: '',
      onBackToMain: () => undefined,
      onSessionAction: () => undefined,
      onSteerInstruction: () => undefined
    }));
    expect(multipleHtml).toContain('class="main-commentary-tool-summary"');
    expect(multipleHtml).toContain('<span>Running 2 commands</span>');
    expect(multipleHtml).toContain('main-commentary-tool-summary-chevron');
  });

  it('unwraps the executed shell result when the requested utility is null', () => {
    const messages = commentaryMessagesForSession(runDetail('Run the local helper.'), [
      toolEvent('shell-null-request', 'tool.requested', 'shell.run', 'shell-null', {
        utility: null,
        args: []
      }),
      toolEvent('shell-null-result', 'tool.observed', 'shell.run', 'shell-null', {
        utility: null,
        args: []
      }, {
        utility: '/bin/sh',
        args: ['-lc', 'tools/rr ping']
      })
    ]);

    expect(messages.find((message) => message.toolName === 'shell.run')?.toolCalls?.[0]?.label)
      .toBe('tools/rr ping');
    expect(messages.find((message) => message.toolName === 'shell.run')?.contentMarkdown)
      .toBe('Ran tools/rr ping');
  });

  it('labels runbook tools with their selection, title, and query', () => {
    const detail = runDetail('Inspect the saved runbooks.');
    const runbookMessage = (events: TraceDisplayEvent[]): string | undefined => commentaryMessagesForSession(
      detail,
      events,
      { includeInitialPrompt: false }
    ).find((message) => message.kind === 'tool')?.contentMarkdown;

    const getEvents = (actionId: string, input: Record<string, unknown>, result: Record<string, unknown>): TraceDisplayEvent[] => [
      toolEvent(`${actionId}-request`, 'tool.requested', 'runbook.get', actionId, input),
      toolEvent(`${actionId}-result`, 'tool.observed', 'runbook.get', actionId, input, result)
    ];
    expect(runbookMessage(getEvents('get-all', { id: 'runbook_parser' }, {
      id: 'runbook_parser', title: 'Parser proof', offset: 0, totalCells: 2, cells: [{ index: 0 }, { index: 1 }]
    }))).toBe('Analyzing runbook entirety in Parser proof');
    expect(runbookMessage(getEvents('get-one', { id: 'runbook_parser', offset: 2, limit: 1 }, {
      id: 'runbook_parser', title: 'Parser proof', offset: 2, totalCells: 5, cells: [{ index: 2 }]
    }))).toBe('Analyzing runbook cell 3 in Parser proof');
    expect(runbookMessage(getEvents('get-range', { id: 'runbook_parser', offset: 1, limit: 3 }, {
      id: 'runbook_parser', title: 'Parser proof', offset: 1, totalCells: 5, cells: [{ index: 1 }, { index: 2 }, { index: 3 }]
    }))).toBe('Analyzing runbook cells 2-4 in Parser proof');

    expect(runbookMessage([
      toolEvent('append-request', 'tool.requested', 'runbook.append', 'append', { id: 'runbook_parser', expectedRevision: 2 }),
      toolEvent('append-result', 'tool.observed', 'runbook.append', 'append', { id: 'runbook_parser', expectedRevision: 2 }, {
        output: { id: 'runbook_parser', title: 'Written parser proof', revision: 3 }
      })
    ])).toBe('Revising runbook in Written parser proof');
    expect(runbookMessage([
      toolEvent('append-pending', 'tool.requested', 'runbook.append', 'append-pending', { id: 'runbook_parser', expectedRevision: 2 })
    ])).toBe('Revising runbook in runbook_parser');
    expect(runbookMessage([
      toolEvent('create-request', 'tool.requested', 'runbook.create', 'create', { title: 'Crash triage', purpose: 'Reproduce the crash' }),
      toolEvent('create-result', 'tool.observed', 'runbook.create', 'create', { title: 'Crash triage', purpose: 'Reproduce the crash' }, {
        id: 'runbook_crash', title: 'Crash triage', revision: 1
      })
    ])).toBe('Creating runbook in Crash triage');
    expect(runbookMessage([
      toolEvent('list-request', 'tool.requested', 'runbook.list', 'list', { query: 'parser boundary' }),
      toolEvent('list-result', 'tool.observed', 'runbook.list', 'list', { query: 'parser boundary' }, { runbooks: [] })
    ])).toBe('Querying runbooks for "parser boundary"');
  });

  it('labels execution, claims, subagent, and memory tools from structured metadata', () => {
    const detail = runDetail('Use the durable research state.');
    const toolMessage = (events: TraceDisplayEvent[]): string | undefined => commentaryMessagesForSession(
      detail,
      events,
      { includeInitialPrompt: false }
    ).find((message) => message.kind === 'tool')?.contentMarkdown;
    const pairedEvents = (
      toolName: string,
      actionId: string,
      input: Record<string, unknown>,
      result: unknown
    ): TraceDisplayEvent[] => [
      toolEvent(`${actionId}-request`, 'tool.requested', toolName, actionId, input),
      toolEvent(`${actionId}-result`, 'tool.observed', toolName, actionId, input, result)
    ];

    expect(toolMessage(pairedEvents('runbook.run', 'run-entirety', { id: 'runbook_parser' }, {
      output: { title: 'Parser proof', runId: 'runbook_run_one', status: 'succeeded' }
    }))).toBe('Executing runbook entirety in Parser proof');
    expect(toolMessage(pairedEvents('runbook.run', 'run-cell', {
      id: 'runbook_parser', cellId: 'cell-3'
    }, { output: { title: 'Parser proof' } }))).toBe('Executing runbook cell 3 in Parser proof');
    expect(toolMessage(pairedEvents('runbook.run', 'run-range', {
      id: 'runbook_parser', startCellId: 'cell-2', endCellId: 'cell-4'
    }, { output: { title: 'Parser proof' } }))).toBe('Executing runbook cells 2-4 in Parser proof');
    expect(toolMessage(pairedEvents('lead.list', 'lead-list', {
      statuses: ['proposed', 'refuted'], query: 'parser boundary'
    }, { leads: [] }))).toBe('Querying proposed, refuted claims for "parser boundary"');
    expect(toolMessage(pairedEvents('wait_agent', 'wait-agents', { timeout_ms: 45_000 }, {
      message: 'No activity before timeout.'
    }))).toBe('Waiting on subagents for 45s');
    expect(toolMessage(pairedEvents('list_agents', 'list-agents', {}, { agents: [] })))
      .toBe('Checking on subagents');
    expect(toolMessage(pairedEvents('memory.get', 'memory-get', { id: 'memory_parser' }, {
      id: 'memory_parser', type: 'technique', title: 'Parser boundary'
    }))).toBe('Remembering technique "Parser boundary"');
    expect(toolMessage(pairedEvents('memory.save', 'memory-save', {
      type: 'test_strategy', title: 'Parser regression suite'
    }, {
      id: 'memory_parser_tests', type: 'test_strategy', title: 'Parser regression suite'
    }))).toBe('Memorizing test strategy "Parser regression suite"');
    expect(toolMessage(pairedEvents('file.read', 'file-read', { path: 'src/parser.ts' }, { text: 'source' })))
      .toBe('Reading src/parser.ts');
    expect(toolMessage(pairedEvents('surface.get', 'surface-get', { surfaceId: 'surface_example' }, {
      id: 'surface_example', name: 'Parser entry boundary'
    }))).toBe('Inspecting surface "Parser entry boundary"');
    expect(toolMessage(pairedEvents('surface.views', 'surface-views', {
      surfaceId: 'surface_example', views: ['recon', 'threat_model', 'analysis']
    }, { surfaceId: 'surface_example', name: 'Parser entry boundary', views: {} })))
      .toBe('Inspecting recon, threat model, analysis for "Parser entry boundary"');
    expect(toolMessage(pairedEvents('surface.list', 'surface-list', { parentId: 'surface_example' }, {
      surfaces: []
    }))).toBe('Listing surfaces under "surface_example"');
    expect(toolMessage(pairedEvents('surface.list', 'resource-surfaces', { resourceId: 'resource_example' }, {
      surfaces: []
    }))).toBe('Listing surfaces for resource "resource_example"');
    expect(toolMessage(pairedEvents('surface.focus', 'surface-focus', { surfaceId: 'surface_example' }, {
      surfaceTrail: [{ id: 'surface_example', name: 'Parser entry boundary' }]
    }))).toBe('Focusing on surface "Parser entry boundary"');
    expect(toolMessage(pairedEvents('features.load', 'features-load', { features: ['code-navigation'] }, {
      loadedFeatureIds: ['code-navigation']
    }))).toBe('Loading feature "code-navigation"');
    expect(toolMessage(pairedEvents('plugins.load', 'plugins-load', { plugins: ['example-plugin', 'example-reference'] }, {
      loadedPluginIds: ['example-plugin', 'example-reference']
    }))).toBe('Loading plugins "example-plugin", "example-reference"');
    expect(toolMessage(pairedEvents('finding.list', 'finding-list', {
      query: 'memory safety'
    }, { findings: [] }))).toBe('Querying findings for "memory safety"');
    expect(toolMessage(pairedEvents('resource.catalog', 'resource-catalog', {
      operation: 'discover', name: 'Example parser'
    }, { resource: {} }))).toBe('Discovering resource "Example parser"');
    expect(toolMessage(pairedEvents('resource.catalog', 'resource-list', {
      operation: 'list'
    }, { resources: [] }))).toBe('Listing research resources');
    expect(toolMessage(pairedEvents('resource.catalog', 'resource-history', {
      operation: 'history', resourceId: 'resource_example', historyId: 'reference_example'
    }, { records: [] }))).toBe('Reading saved resource reference "reference_example"');
    expect(toolMessage(pairedEvents('finding.transition', 'finding-transition', {
      id: 'finding_parser', toStatus: 'report_ready'
    }, { status: 'report_ready' }))).toBe('Transitioning finding to report ready');
    expect(toolMessage(pairedEvents('finding.completion_check', 'finding-completion', {
      id: 'finding_parser', targetStatus: 'verified'
    }, { targetStatus: 'verified', gaps: [] }))).toBe('Checking readiness for finding verified status');

  });

  it('keeps the latest grouped tool summary collapsed by default', () => {
    const detail = runDetail('Inspect the parser.');
    const html = renderToStaticMarkup(createElement(CommentaryView, {
      busy: false,
      detail,
      events: [
        toolEvent('shell-one', 'tool.requested', 'shell.run', 'shell-one', { command: 'npm test' }),
        toolEvent('shell-two', 'tool.requested', 'shell.run', 'shell-two', { command: 'npm run typecheck' })
      ],
      providerModelCatalog: [],
      selectedRunId: detail.run.id,
      showBackToMain: true,
      searchHighlightQuery: '',
      onBackToMain: () => undefined,
      onSessionAction: () => undefined,
      onSteerInstruction: () => undefined
    }));

    expect(html).toMatch(/class="main-commentary-tool-summary" aria-expanded="false"/u);
    expect(html).not.toContain('class="main-commentary-tool-call-list"');
  });

  it('shows chat scroll fades only where more content remains', () => {
    expect(commentaryScrollFadeClasses({ scrollHeight: 100, clientHeight: 100, scrollTop: 0 })).toEqual({
      'has-top-fade': false,
      'has-bottom-fade': false
    });
    expect(commentaryScrollFadeClasses({ scrollHeight: 300, clientHeight: 100, scrollTop: 0 })).toEqual({
      'has-top-fade': false,
      'has-bottom-fade': true
    });
    expect(commentaryScrollFadeClasses({ scrollHeight: 300, clientHeight: 100, scrollTop: 100 })).toEqual({
      'has-top-fade': true,
      'has-bottom-fade': true
    });
    expect(commentaryScrollFadeClasses({ scrollHeight: 300, clientHeight: 100, scrollTop: 200 })).toEqual({
      'has-top-fade': true,
      'has-bottom-fade': false
    });
  });

  it('preserves bottom stickiness across layout-driven tool expansion and collapse', () => {
    expect(commentaryFollowLatestAfterScroll({
      wasFollowingLatest: true,
      distanceFromBottom: 180,
      userInitiated: false
    })).toBe(true);
    expect(commentaryFollowLatestAfterScroll({
      wasFollowingLatest: true,
      distanceFromBottom: 180,
      userInitiated: true
    })).toBe(false);
    expect(commentaryFollowLatestAfterScroll({
      wasFollowingLatest: false,
      distanceFromBottom: 180,
      userInitiated: false
    })).toBe(false);
    expect(commentaryFollowLatestAfterScroll({
      wasFollowingLatest: false,
      distanceFromBottom: 12,
      userInitiated: false
    })).toBe(true);
  });

  it('centers selected history within a bounded commentary render window', () => {
    expect(commentaryWindowStartForIndex(240, 0)).toBe(0);
    expect(commentaryWindowStartForIndex(240, 70)).toBe(30);
    expect(commentaryWindowStartForIndex(240, 239)).toBe(120);
  });

  it('uses measured message heights and gaps to locate history within spacers', () => {
    const offsets = commentaryMessageOffsets(
      [{ id: 'one' }, { id: 'two' }, { id: 'three' }, { id: 'four' }],
      new Map([['one', 30], ['two', 80], ['three', 40]]), 100, 14
    );
    expect(offsets).toEqual([0, 44, 138, 192, 306]);
    expect(commentarySpacerHeight(offsets, 0, 3, 14)).toBe(178);
    expect(commentarySpacerHeight(offsets, 1, 4, 14)).toBe(248);
    expect(commentarySpacerHeight(offsets, 2, 2, 14)).toBe(0);
    expect(commentaryWindowStartForOffset(offsets, 45, 3)).toBe(1);
    expect(commentaryWindowStartForOffset(offsets, 150, 3)).toBe(2);
    expect(commentaryWindowStartForOffset(offsets, 1_000, 3)).toBe(3);
  });

  it('renders only the latest bounded window for long commentary histories', () => {
    const detail = runDetail('Review the target.');
    const events = Array.from({ length: 180 }, (_, index) => displayEvent(`commentary-${index}`, {
      agentPath: '/root',
      transcriptRole: 'assistant',
      transcriptSource: 'app_server_commentary',
      messagePhase: 'commentary',
      text: `Commentary message ${index}`
    }, { sequence: index }));

    const html = renderToStaticMarkup(
      createElement(CommentaryView, {
        busy: false,
        detail,
        events,
        providerModelCatalog: [],
        selectedRunId: detail.run.id,
        showBackToMain: true,
        searchHighlightQuery: '',
        onBackToMain: () => undefined,
        onSessionAction: () => undefined,
        onSteerInstruction: () => undefined
      })
    );

    expect(html.match(/data-commentary-event-id=/g)).toHaveLength(COMMENTARY_RENDER_WINDOW_SIZE);
    expect(html).toContain('Commentary message 179');
    expect(html).not.toContain('Commentary message 0<');
    expect(html).toContain('main-commentary-spacer');
  });

  it('excludes the leading prompt from scroll-window measurements', () => {
    const detail = runDetail('Inspect the parser.');
    detail.run.status = 'active';
    detail.run.budget = { modelProvider: 'openai-codex' };
    detail.run.model = 'gpt-5.6-sol';
    detail.run.reasoningEffort = 'medium';
    const events = Array.from({ length: 180 }, (_, index) => displayEvent(`commentary-${index}`, {
        agentPath: '/root',
        transcriptRole: 'assistant',
        transcriptSource: 'app_server_commentary',
        messagePhase: 'commentary',
        text: `Checkpoint ${index}`
      }, { sequence: index }));
    const html = renderToStaticMarkup(createElement(CommentaryView, {
      busy: false,
      detail,
      events,
      providerModelCatalog: [],
      selectedRunId: detail.run.id,
      showBackToMain: false,
      searchHighlightQuery: '',
      onBackToMain: () => undefined,
      onSessionAction: () => undefined,
      onSteerInstruction: () => undefined
    }));

    expect(html.match(/data-commentary-window-item=""/g)).toHaveLength(COMMENTARY_RENDER_WINDOW_SIZE);
    expect(html.match(/<article[^>]*data-commentary-event-id="run-prompt:run_commentary"[^>]*>/)?.[0])
      .not.toContain('data-commentary-window-item');
  });

  it('keeps reasoning alongside native commentary and final messages', () => {
    const messages = commentaryMessagesForSession(runDetail('Inspect the parser.'), [
      displayEvent('prompt', {
        transcriptRole: 'user',
        transcriptSource: 'run_prompt',
        text: 'Inspect the parser.'
      }),
      displayEvent('reasoning', {
        agentPath: '/root',
        responseId: 'response_one',
        transcriptRole: 'assistant',
        transcriptSource: 'openai_reasoning_summary',
        text: 'Checking parser entrypoints.'
      }),
      displayEvent('commentary', {
        agentPath: '/root',
        responseId: 'response_one',
        messagePhase: 'commentary',
        transcriptRole: 'assistant',
        transcriptSource: 'app_server_commentary',
        text: 'I found two parser entrypoints and am checking their shared guard.'
      }),
      displayEvent('tool', { agentPath: '/root', text: 'Raw shell output.' }, { source: 'tool', type: 'tool_result' }),
      displayEvent('final', {
        agentPath: '/root',
        responseId: 'response_two',
        messagePhase: 'final_answer',
        transcriptRole: 'assistant',
        transcriptSource: 'app-server',
        text: 'The shared guard rejects the boundary safely.'
      })
    ]);

    expect(messages.map(({ kind, contentMarkdown }) => [kind, contentMarkdown])).toEqual([
      ['user', 'Inspect the parser.'],
      ['progress', 'Checking parser entrypoints.'],
      ['commentary', 'I found two parser entrypoints and am checking their shared guard.'],
      ['final_answer', 'The shared guard rejects the boundary safely.']
    ]);
  });

  it('does not remove a displayed reasoning summary when commentary arrives for its response', () => {
    const detail = runDetail('Review the example parser.');
    const reasoning = displayEvent('reasoning', {
      agentPath: '/root',
      transcriptRole: 'assistant',
      transcriptSource: 'openai_reasoning_summary',
      metadata: { responseId: 'response_example', itemId: 'thinking:0' },
      text: '**Checking parser bounds**'
    });
    const commentary = displayEvent('commentary', {
      agentPath: '/root',
      transcriptRole: 'assistant',
      transcriptSource: 'app_server_commentary',
      metadata: { responseId: 'response_example' },
      text: 'The input reaches the parser guard.'
    });
    const before = commentaryMessagesForSession(detail, [reasoning]);
    const after = commentaryMessagesForSession(detail, [reasoning, commentary]);

    expect(before.map((message) => message.id)).toEqual(['run-prompt:run_commentary', 'reasoning']);
    expect(after.map((message) => message.id)).toEqual(['run-prompt:run_commentary', 'reasoning', 'commentary']);
    expect(after[1]?.reasoningTraceLines).toEqual(['**Checking parser bounds**']);
  });

  it('projects failed final app-server results as error messages', () => {
    const messages = commentaryMessagesForSession(runDetail('Inspect the parser.'), [
      displayEvent('final-error', {
        agentPath: '/root',
        transcriptRole: 'assistant',
        transcriptSource: 'app-server',
        messagePhase: 'final_answer',
        finalResultKind: 'error',
        outputText: 'Unexpected error'
      })
    ], { includeInitialPrompt: false });

    expect(messages.map(({ kind, contentMarkdown }) => [kind, contentMarkdown])).toEqual([
      ['error', 'Unexpected error']
    ]);
  });

  it('shows the latest root checkpoint as final output when a request aborts', () => {
    const detail = runDetail('Inspect the parser.');
    detail.run.status = 'failed';
    const events = [
      displayEvent('checkpoint', {
        agentPath: '/root',
        transcriptRole: 'assistant',
        transcriptSource: 'app_server_commentary',
        messagePhase: 'commentary',
        text: 'The parser guard rejects the first malformed case; the second remains open.'
      }, { createdAt: '2026-08-03T10:00:00.000Z' }),
      displayEvent('subagent-checkpoint', {
        agentPath: '/root/reviewer',
        transcriptRole: 'assistant',
        transcriptSource: 'app_server_commentary',
        messagePhase: 'commentary',
        text: 'A separate review is in progress.'
      }, { createdAt: '2026-08-03T10:01:00.000Z' }),
      displayEvent('abort', {
        agentPath: '/root',
        transcriptRole: 'assistant',
        transcriptSource: 'app-server',
        messagePhase: 'final_answer',
        finalResultKind: 'error',
        text: 'Request was aborted'
      }, { createdAt: '2026-08-03T10:02:30.000Z' })
    ];
    const messages = commentaryMessagesForSession(detail, events, { includeInitialPrompt: false });
    expect(messages.at(-1)).toMatchObject({
      kind: 'final_answer',
      contentMarkdown: 'The parser guard rejects the first malformed case; the second remains open.',
      abortNotice: 'Request was aborted 2m 30s after the above checkpoint.'
    });

    const html = renderToStaticMarkup(createElement(CommentaryMessageRow, {
      message: messages.at(-1)!,
      searchHighlightQuery: '',
      selected: false
    }));
    expect(html).toContain('class="main-commentary-abort-notice"');
    expect(html).toContain('Request was aborted 2m 30s after the above checkpoint.');
  });

  it('uses the previous final answer for a paused session with each abort wording', () => {
    for (const abortText of ['Request was aborted', 'The operation was aborted', 'This operation was aborted']) {
      const detail = runDetail('Inspect the parser.');
      detail.run.status = 'paused';
      detail.transcriptMessages = [
        {
          id: 'checkpoint', runId: detail.run.id, attemptId: 'attempt_previous', traceEventId: null,
          role: 'assistant', phase: 'final_answer', source: 'app-server',
          contentMarkdown: 'The earlier parser check established the boundary.',
          metadata: { agentPath: '/root' }, createdAt: '2026-08-03T10:00:00.000Z'
        },
        {
          id: 'abort', runId: detail.run.id, attemptId: 'attempt_current', traceEventId: null,
          role: 'assistant', phase: 'final_answer', source: 'app-server',
          contentMarkdown: abortText,
          metadata: { agentPath: '/root' }, createdAt: '2026-08-03T10:05:00.000Z'
        }
      ];
      const messages = commentaryMessagesForSession(detail, buildTraceDisplayEventsForAgentPath(detail, null), {
        includeInitialPrompt: false
      });
      expect(messages[0]?.kind).toBe('commentary');
      expect(messages.at(-1)).toMatchObject({
        kind: 'final_answer',
        contentMarkdown: 'The earlier parser check established the boundary.',
        abortNotice: 'Request was aborted 5m 0s after the above checkpoint.'
      });
    }
  });

  it('keeps the abort output when no previous agent checkpoint exists', () => {
    const detail = runDetail('Inspect the parser.');
    detail.run.status = 'paused';
    const messages = commentaryMessagesForSession(detail, [
      displayEvent('abort', {
        agentPath: '/root',
        transcriptRole: 'assistant',
        transcriptSource: 'app-server',
        messagePhase: 'final_answer',
        text: 'The operation was aborted'
      })
    ], { includeInitialPrompt: false });
    expect(messages.at(-1)).toMatchObject({ kind: 'final_answer', contentMarkdown: 'The operation was aborted' });
  });

  it('normalizes legacy terminated app-server final text as an unexpected error', () => {
    const messages = commentaryMessagesForSession(runDetail('Inspect the parser.'), [
      displayEvent('legacy-final-error', {
        agentPath: '/root',
        transcriptRole: 'assistant',
        transcriptSource: 'app-server',
        messagePhase: 'final_answer',
        text: 'terminated'
      })
    ], { includeInitialPrompt: false });

    expect(messages.map(({ kind, contentMarkdown }) => [kind, contentMarkdown])).toEqual([
      ['error', 'Unexpected error']
    ]);
  });

  it('adds an unexpected-error final message for legacy interrupted restart recovery', () => {
    const detail = runDetail('Inspect the parser.');
    detail.run.status = 'paused';
    detail.attempts = [{ id: 'attempt_one' }] as RunDetail['attempts'];
    const messages = commentaryMessagesForSession(detail, [
      displayEvent('commentary', {
        agentPath: '/root',
        transcriptRole: 'assistant',
        transcriptSource: 'app_server_commentary',
        messagePhase: 'commentary',
        text: 'I am inspecting the parser.'
      }),
      displayEvent('recovery', { interruptedByRecovery: true }, {
        sequence: 2,
        source: 'system',
        type: 'research_event',
        summary: 'Workspace recovery paused interrupted run after app restart.'
      })
    ], { includeInitialPrompt: false });

    expect(messages.map(({ kind, contentMarkdown }) => [kind, contentMarkdown])).toEqual([
      ['commentary', 'I am inspecting the parser.'],
      ['error', 'Unexpected error']
    ]);
  });

  it('preserves legacy wrapped app-server error details when present', () => {
    const messages = commentaryMessagesForSession(runDetail('Inspect the parser.'), [
      displayEvent('legacy-wrapped-error', {
        agentPath: '/root',
        transcriptRole: 'assistant',
        transcriptSource: 'app-server',
        messagePhase: 'final_answer',
        text: 'Research agent failed: Provider request failed with status 500.'
      })
    ], { includeInitialPrompt: false });

    expect(messages.map(({ kind, contentMarkdown }) => [kind, contentMarkdown])).toEqual([
      ['error', 'Provider request failed with status 500.']
    ]);
  });

  it('retains both paired reasoning and newer fallback progress while coalescing snapshots', () => {
    const messages = commentaryMessagesForSession(runDetail('Continue the review.'), [
      displayEvent('paired-reasoning', {
        agentPath: '/root',
        responseId: 'response_native',
        transcriptRole: 'assistant',
        transcriptSource: 'openai_reasoning_summary',
        itemId: 'reasoning_native',
        text: 'Paired provider reasoning.'
      }),
      displayEvent('native', {
        agentPath: '/root',
        responseId: 'response_native',
        transcriptRole: 'assistant',
        transcriptSource: 'app_server_commentary',
        messagePhase: 'commentary',
        text: 'Native commentary for the first response.'
      }),
      displayEvent('progress-first', {
        agentPath: '/root',
        responseId: 'response_fallback',
        transcriptRole: 'assistant',
        transcriptSource: 'openai_reasoning_summary',
        itemId: 'reasoning_fallback',
        text: 'Initial fallback snapshot.'
      }),
      displayEvent('progress-completed', {
        agentPath: '/root',
        responseId: 'response_fallback',
        transcriptRole: 'assistant',
        transcriptSource: 'openai_reasoning_summary',
        itemId: 'reasoning_fallback',
        text: 'Completed fallback snapshot.'
      })
    ]);

    expect(messages.map(({ kind, contentMarkdown }) => [kind, contentMarkdown])).toEqual([
      ['user', 'Continue the review.'],
      ['progress', 'Paired provider reasoning.'],
      ['commentary', 'Native commentary for the first response.'],
      ['progress', 'Completed fallback snapshot.']
    ]);
  });

  it('keeps completed reasoning summaries when a response reuses an item ID', () => {
    const reasoning = (id: string, phase: string, text: string): TraceDisplayEvent => displayEvent(id, {
      agentPath: '/root',
      transcriptRole: 'assistant',
      transcriptSource: 'openai_reasoning_summary',
      metadata: { responseId: 'response_example', itemId: 'thinking:0', phase },
      text
    });
    const firstSnapshot = reasoning('first-snapshot', 'delta', '**Inspecting');
    const firstCompleted = reasoning('first-completed', 'completed', '**Inspecting the parser**');
    const secondSnapshot = reasoning('second-snapshot', 'delta', '**Checking');
    const secondCompleted = reasoning('second-completed', 'completed', '**Checking bounds**');
    const nextItem = displayEvent('next-item', {
      agentPath: '/root',
      transcriptRole: 'assistant',
      transcriptSource: 'app_server_commentary',
      text: 'Continuing with the example.'
    });
    const detail = runDetail('Review the parser.');
    const summaries = (events: TraceDisplayEvent[]): string[] => commentaryMessagesForSession(detail, events)
      .filter((message) => message.kind === 'progress')
      .map((message) => message.contentMarkdown);

    expect(summaries([firstSnapshot, firstCompleted])).toEqual(['**Inspecting the parser**']);
    expect(summaries([firstSnapshot, firstCompleted, secondSnapshot])).toEqual([
      '**Inspecting the parser**', '**Checking'
    ]);
    expect(summaries([firstSnapshot, firstCompleted, secondSnapshot, secondCompleted, nextItem])).toEqual([
      '**Inspecting the parser**', '**Checking bounds**'
    ]);
  });

  it('keeps the prompt and terminal result outside the collapsible work history', () => {
    const messages = commentaryMessagesForSession(runDetail('Inspect the parser.'), [
      displayEvent('commentary', {
        agentPath: '/root',
        transcriptRole: 'assistant',
        transcriptSource: 'app_server_commentary',
        messagePhase: 'commentary',
        text: 'Checking parser entrypoints.'
      }),
      displayEvent('final', {
        agentPath: '/root',
        transcriptRole: 'assistant',
        transcriptSource: 'app-server',
        messagePhase: 'final_answer',
        text: 'The parser boundary is safe.'
      })
    ]);

    const sections = commentaryMessageSections(messages, true);
    expect(sections.leading.map((message) => message.kind)).toEqual(['user']);
    expect(sections.activity.map((message) => message.kind)).toEqual(['commentary']);
    expect(sections.trailing.map((message) => message.kind)).toEqual(['final_answer']);
  });

  it('sums execution-attempt time for the run work duration', () => {
    const detail = runDetail('Inspect the parser.');
    detail.run.startedAt = '2026-08-03T10:00:00.000Z';
    detail.run.endedAt = '2026-08-03T10:04:00.000Z';
    detail.attempts = [
      { startedAt: '2026-08-03T10:00:00.000Z', endedAt: '2026-08-03T10:01:00.000Z' },
      { startedAt: '2026-08-03T10:03:00.000Z', endedAt: null }
    ] as RunDetail['attempts'];

    expect(runWorkingDurationMs(detail, Date.parse('2026-08-03T10:04:30.000Z'))).toBe(150_000);
  });

  it('treats only an active run as currently working', () => {
    expect(isRunWorkingStatus('active')).toBe(true);
    expect(isRunWorkingStatus('queued')).toBe(false);
    expect(isRunWorkingStatus('paused')).toBe(false);
    expect(isRunWorkingStatus('blocked')).toBe(false);
    expect(isRunWorkingStatus('completed')).toBe(false);
    expect(isRunWorkingStatus('failed')).toBe(false);
    expect(isRunWorkingStatus('stopped')).toBe(false);
  });

  it('caps an unfinished attempt at its last recorded activity when the run is no longer active', () => {
    const detail = runDetail('Inspect the parser.');
    detail.run.status = 'paused';
    detail.run.budget = { modelProvider: 'openai-codex' };
    detail.run.model = 'gpt-5.6-sol';
    detail.run.reasoningEffort = 'medium';
    detail.run.startedAt = '2026-08-03T09:00:00.000Z';
    detail.run.endedAt = null;
    detail.attempts = [{
      startedAt: detail.run.startedAt,
      endedAt: null
    }] as RunDetail['attempts'];
    const commentary = displayEvent('commentary', {
      agentPath: '/root',
      transcriptRole: 'assistant',
      transcriptSource: 'app_server_commentary',
      messagePhase: 'commentary',
      text: 'Checking parser entrypoints.'
    }, { createdAt: '2026-08-03T11:00:00.000Z' });
    detail.traceEvents = [commentary];

    const html = renderToStaticMarkup(
      createElement(CommentaryView, {
        busy: false,
        detail,
        events: [commentary],
        providerModelCatalog: [],
        selectedRunId: detail.run.id,
        showBackToMain: false,
        searchHighlightQuery: '',
        onBackToMain: () => undefined,
        onSessionAction: () => undefined,
        onSteerInstruction: () => undefined
      })
    );

    expect(html).toContain('Worked for 02:00:00');
    expect(html).toContain('aria-expanded="false"');
  });

  it('collapses stopped commentary and tool work behind a Worked header', () => {
    const detail = runDetail('Inspect the parser.');
    detail.run.status = 'stopped';
    detail.run.budget = { modelProvider: 'openai-codex' };
    detail.run.model = 'gpt-5.6-sol';
    detail.run.reasoningEffort = 'medium';
    detail.run.startedAt = '2026-08-03T10:00:00.000Z';
    detail.run.endedAt = '2026-08-03T10:02:30.000Z';
    detail.attempts = [{
      startedAt: detail.run.startedAt,
      endedAt: detail.run.endedAt
    }] as RunDetail['attempts'];
    const events = [
      displayEvent('commentary', {
        agentPath: '/root',
        transcriptRole: 'assistant',
        transcriptSource: 'app_server_commentary',
        messagePhase: 'commentary',
        text: 'Checking parser entrypoints.'
      }),
      displayEvent('final', {
        agentPath: '/root',
        transcriptRole: 'assistant',
        transcriptSource: 'app-server',
        messagePhase: 'final_answer',
        text: 'The parser boundary is safe.'
      })
    ];

    const html = renderToStaticMarkup(
      createElement(CommentaryView, {
        busy: false,
        detail,
        events,
        providerModelCatalog: [],
        selectedRunId: detail.run.id,
        showBackToMain: false,
        searchHighlightQuery: '',
        onBackToMain: () => undefined,
        onSessionAction: () => undefined,
        onSteerInstruction: () => undefined
      })
    );

    expect(html).toContain('Worked for 00:02:30');
    expect(html).toContain('aria-expanded="false"');
    expect(html).toContain('The parser boundary is safe.');
    expect(html).not.toContain('Checking parser entrypoints.');
  });

  it('keeps track commentary expanded while a completed epoch is deferred', () => {
    const deferred = runDetail('Inspect the example parser.');
    deferred.run.id = 'track_example';
    deferred.run.status = 'completed';
    deferred.run.budget = { modelProvider: 'openai-codex' };
    deferred.run.model = 'gpt-5.6-sol';
    deferred.run.reasoningEffort = 'medium';
    deferred.run.startedAt = '2026-08-03T10:00:00.000Z';
    deferred.run.endedAt = '2026-08-03T10:02:30.000Z';
    const latest = runningResearchTrackDetail({ ...deferred, run: { ...deferred.run } }, true);
    const detail = deferredResearchTrackDetail(latest, deferred);
    const html = renderToStaticMarkup(createElement(CommentaryView, {
      busy: false,
      detail,
      events: [],
      providerModelCatalog: [],
      selectedRunId: 'track_example',
      showBackToMain: false,
      searchHighlightQuery: '',
      onBackToMain: () => undefined,
      onSessionAction: () => undefined,
      onSteerInstruction: () => undefined
    }));

    expect(html).toContain('Working for');
    expect(html).not.toContain('Worked for');
    expect(html).not.toContain('class="harness-feedback-footer"');
  });

  it('renders xAI reasoning summaries as ordinary commentary while retaining their trace source', () => {
    const events = [
      displayEvent('grok-reasoning', {
        agentPath: '/root',
        responseId: 'response_grok',
        provider: 'xai',
        model: 'grok-4.6',
        transcriptRole: 'assistant',
        transcriptSource: 'openai_reasoning_summary',
        text: 'The redirect strips the original path, so I am checking the destination surface.'
      })
    ];
    const detail = runDetail('Inspect the redirect boundary.');
    const messages = commentaryMessagesForSession(detail, events);

    expect(messages.map(({ kind, contentMarkdown }) => [kind, contentMarkdown])).toEqual([
      ['user', 'Inspect the redirect boundary.'],
      ['commentary', 'The redirect strips the original path, so I am checking the destination surface.']
    ]);

    const html = renderToStaticMarkup(
      createElement(CommentaryView, {
        busy: true,
        detail,
        events,
        providerModelCatalog: [],
        selectedRunId: detail.run.id,
        showBackToMain: true,
        searchHighlightQuery: '',
        onBackToMain: () => undefined,
        onSessionAction: () => undefined,
        onSteerInstruction: () => undefined
      })
    );

    expect(html).toContain('The redirect strips the original path');
    expect(html).not.toContain('lucide-brain');
  });

  it('preserves each coalesced reasoning trace as its own commentary line', () => {
    const messages = commentaryMessagesForSession(runDetail('Review the parser.'), [
      displayEvent('reasoning-group', {
        agentPath: '/root',
        responseId: 'response_group',
        transcriptRole: 'assistant',
        transcriptSource: 'openai_reasoning_summary',
        text: '**Inspecting the parser**\n\n**Checking bounds**',
        reasoningSummaryTexts: ['**Inspecting the parser**', '**Checking bounds**']
      })
    ]);

    expect(messages.find((message) => message.kind === 'progress')?.reasoningTraceLines).toEqual([
      '**Inspecting the parser**',
      '**Checking bounds**'
    ]);
  });

  it('renders a brain icon for every plain-text fixture reasoning line', () => {
    const fixtureReasoning = displayEvent('fixture-reasoning', {
      fixtureOnly: true,
      text: 'Planning synthetic fixture mode\nDesigning password input via stdin'
    });
    const detail = runDetail('Exercise the fixture.');
    const messages = commentaryMessagesForSession(detail, [fixtureReasoning]);

    expect(messages.find((message) => message.kind === 'progress')?.reasoningTraceLines).toEqual([
      'Planning synthetic fixture mode',
      'Designing password input via stdin'
    ]);

    const html = renderToStaticMarkup(
      createElement(CommentaryView, {
        busy: false,
        detail,
        events: [fixtureReasoning],
        providerModelCatalog: [],
        selectedRunId: detail.run.id,
        showBackToMain: true,
        searchHighlightQuery: '',
        onBackToMain: () => undefined,
        onSessionAction: () => undefined,
        onSteerInstruction: () => undefined
      })
    );

    expect(html.match(/lucide-brain/g)).toHaveLength(2);
  });

  it('keeps reasoning and native commentary in the same turn without a response ID', () => {
    const messages = commentaryMessagesForSession(runDetail('Inspect the parser.'), [
      displayEvent('reasoning', {
        agentPath: '/root',
        turn: 3,
        provider: 'openai-codex',
        model: 'gpt-5.6-sol',
        transcriptRole: 'assistant',
        transcriptSource: 'openai_reasoning_summary',
        text: 'Private provider summary.'
      }),
      displayEvent('commentary', {
        agentPath: '/root',
        turn: 3,
        provider: 'openai-codex',
        model: 'gpt-5.6-sol',
        messagePhase: 'commentary',
        transcriptRole: 'assistant',
        transcriptSource: 'app_server_commentary',
        text: 'I found the parser boundary and am testing its guard.'
      })
    ]);

    expect(messages.map(({ kind, contentMarkdown }) => [kind, contentMarkdown])).toEqual([
      ['user', 'Inspect the parser.'],
      ['progress', 'Private provider summary.'],
      ['commentary', 'I found the parser boundary and am testing its guard.']
    ]);
  });

  it('preserves distinct reasoning summaries that become consecutive after projection', () => {
    const messages = commentaryMessagesForSession(runDetail('Review the target.'), [
      displayEvent('progress-one', {
        agentPath: '/root',
        responseId: 'response_one',
        transcriptRole: 'assistant',
        transcriptSource: 'openai_reasoning_summary',
        itemId: 'reasoning_one',
        text: 'Reading the entrypoint.'
      }),
      displayEvent('tool', { agentPath: '/root', text: 'Raw shell output.' }, { source: 'tool', type: 'tool_result' }),
      displayEvent('progress-two', {
        agentPath: '/root',
        responseId: 'response_two',
        transcriptRole: 'assistant',
        transcriptSource: 'openai_reasoning_summary',
        itemId: 'reasoning_two',
        text: 'Confirmed the input reaches the parser.'
      })
    ]);

    expect(messages.map(({ kind, contentMarkdown }) => [kind, contentMarkdown])).toEqual([
      ['user', 'Review the target.'],
      ['progress', 'Reading the entrypoint.'],
      ['progress', 'Confirmed the input reaches the parser.']
    ]);
  });

  it('shows spawned and follow-up tasks in a selected subagent feed', () => {
    const messages = commentaryMessagesForSession(runDetail('Root prompt.'), [
      displayEvent('spawn', {
        type: 'subagent.activity',
        action: 'spawned',
        agentPath: '/root/parser_review',
        message: 'Inspect the parser boundary.'
      }, { source: 'system' }),
      displayEvent('followup', {
        type: 'subagent.activity',
        action: 'followup',
        agentPath: '/root/parser_review',
        message: 'Also check the length conversion.'
      }, { source: 'system' }),
      displayEvent('message', {
        type: 'subagent.activity',
        action: 'message',
        agentPath: '/root/parser_review',
        message: 'Compare the result with the caller contract.'
      }, { source: 'system' })
    ], { includeInitialPrompt: false });

    expect(messages.map(({ kind, taskAction, taskRoute, contentMarkdown }) => [kind, taskAction, taskRoute, contentMarkdown])).toEqual([
      ['task', 'spawn', 'Parser Review', 'Inspect the parser boundary.'],
      ['task', 'followup', 'To Parser Review', 'Also check the length conversion.'],
      ['task', 'message', 'To Parser Review', 'Compare the result with the caller contract.']
    ]);
  });

  it('labels inter-agent messages with their sender and recipient', () => {
    const messages = commentaryMessagesForSession(runDetail('Coordinate the review.'), [
      displayEvent('peer-message', {
        type: 'subagent.activity',
        action: 'message',
        agentPath: '/root/builder',
        authorAgentPath: '/root/reviewer',
        message: 'Check the synthetic parser fixture.'
      }, { source: 'system' }),
      displayEvent('room-message', {
        type: 'subagent.activity',
        action: 'room_packet',
        agentPath: '/root/reviewer',
        authorAgentPath: '/root/reviewer',
        roomTitle: 'Example review room',
        message: 'The synthetic boundary is covered.'
      }, { source: 'system' })
    ], { includeInitialPrompt: false });

    expect(messages.map((message) => [message.taskAction, message.taskOrigin, message.taskDestination, message.contentMarkdown])).toEqual([
      ['message', 'Reviewer', 'Builder', 'Check the synthetic parser fixture.'],
      ['room_message', 'Reviewer', 'room "Example review room"', 'The synthetic boundary is covered.']
    ]);
    const html = renderToStaticMarkup(createElement(CommentaryMessageRow, {
      message: messages[0]!, searchHighlightQuery: '', selected: false
    }));
    expect(html).toContain('agent-communication-inbound');
    expect(html).toContain('class="main-commentary-agent-card is-communication"');
    expect(html).toMatch(/main-commentary-agent-header[\s\S]*main-commentary-agent-mail-icon[\s\S]*lucide-mail[\s\S]*Reviewer[\s\S]*lucide-arrow-right[\s\S]*Builder[\s\S]*main-commentary-agent-type[\s\S]*Collaboration/);
    expect((html.match(/lucide-mail/gu) ?? []).length).toBe(1);
    expect(html).toMatch(/main-commentary-agent-type[\s\S]*Collaboration[\s\S]*main-commentary-message-content[\s\S]*Check the synthetic parser fixture/);
    expect(html).not.toContain('main-commentary-agent-divider');
    expect(html).not.toContain('is-current-agent');

    const senderView = renderToStaticMarkup(createElement(CommentaryMessageRow, {
      message: messages[0]!, viewedAgentPath: '/root/reviewer', searchHighlightQuery: '', selected: false
    }));
    const recipientView = renderToStaticMarkup(createElement(CommentaryMessageRow, {
      message: messages[0]!, viewedAgentPath: '/root/builder', searchHighlightQuery: '', selected: false
    }));
    expect(senderView).toContain('agent-communication-outbound');
    expect(recipientView).toContain('agent-communication-inbound');
    expect(senderView).toContain('class="main-commentary-agent-name" title="Reviewer"');
    expect(senderView).toContain('class="main-commentary-agent-name" title="Builder"');
    expect(recipientView).toContain('class="main-commentary-agent-name" title="Reviewer"');
    expect(recipientView).toContain('class="main-commentary-agent-name" title="Builder"');
    expect(senderView).not.toContain('is-current-agent');
    expect(recipientView).not.toContain('is-current-agent');
  });

  it('renders a follow-up task with its sender and recipient', () => {
    const messages = commentaryMessagesForSession(runDetail('Coordinate the review.'), [
      displayEvent('followup', {
        type: 'subagent.activity',
        action: 'followup',
        agentPath: '/root/reviewer',
        authorAgentPath: '/root',
        recipientAgentPath: '/root/reviewer',
        message: 'Recheck the synthetic fixture.'
      }, { source: 'system' })
    ], { includeInitialPrompt: false });

    const html = renderToStaticMarkup(createElement(CommentaryMessageRow, {
      message: messages[0]!, searchHighlightQuery: '', selected: false
    }));
    expect(html).toContain('class="main-commentary-agent-card is-communication"');
    expect(html).toMatch(/main-commentary-agent-header[\s\S]*main-commentary-agent-mail-icon[\s\S]*lucide-mail[\s\S]*Lead[\s\S]*lucide-arrow-right[\s\S]*Reviewer[\s\S]*main-commentary-agent-type[\s\S]*Follow-Up/);
    expect(html).toContain('Recheck the synthetic fixture.');
    expect(html).toMatch(/main-commentary-agent-type[\s\S]*Follow-Up[\s\S]*main-commentary-agent-message-body[\s\S]*Recheck the synthetic fixture/);
    expect(html).not.toContain('main-commentary-agent-divider');
    expect(html).toContain('class="main-commentary-agent-name" title="Lead"');
    expect(html).not.toContain('is-current-agent');
    expect(html).toContain('agent-communication-outbound');
    const recipientView = renderToStaticMarkup(createElement(CommentaryMessageRow, {
      message: messages[0]!, viewedAgentPath: '/root/reviewer', searchHighlightQuery: '', selected: false
    }));
    expect(recipientView).toContain('agent-communication-inbound');
  });

  it('uses Lead for older messages to a subagent without sender metadata', () => {
    const messages = commentaryMessagesForSession(runDetail('Coordinate the review.'), [
      displayEvent('message', {
        type: 'subagent.activity',
        action: 'message',
        agentPath: '/root/reviewer',
        message: 'Check the synthetic fixture.'
      }, { source: 'system' })
    ], { includeInitialPrompt: false });

    const html = renderToStaticMarkup(createElement(CommentaryMessageRow, {
      message: messages[0]!, searchHighlightQuery: '', selected: false
    }));
    expect(messages[0]?.taskOrigin).toBe('Lead');
    expect(html).toContain('Reviewer');
  });

  it('keeps older replies to the lead on the incoming side', () => {
    const messages = commentaryMessagesForSession(runDetail('Coordinate the review.'), [
      displayEvent('reply', {
        type: 'subagent.activity',
        action: 'message',
        agentPath: '/root',
        message: 'The synthetic fixture is ready.'
      }, { source: 'system' })
    ], { includeInitialPrompt: false });

    const html = renderToStaticMarkup(createElement(CommentaryMessageRow, {
      message: messages[0]!, searchHighlightQuery: '', selected: false
    }));
    expect(html).toContain('agent-communication-inbound');
    expect(messages[0]?.taskOrigin).toBe('Subagent');
    expect(html).toContain('Lead');
    expect(html).not.toContain('Unknown sender');
  });

  it('shows subagent failures as terminal Commentary messages', () => {
    const messages = commentaryMessagesForSession(runDetail('Root prompt.'), [
      displayEvent('commentary', {
        agentPath: '/root/parser_review',
        transcriptRole: 'assistant',
        transcriptSource: 'app_server_commentary',
        messagePhase: 'commentary',
        text: 'I am checking the parser boundary.'
      }),
      displayEvent('error', {
        type: 'subagent.activity',
        action: 'errored',
        agentPath: '/root/parser_review',
        message: 'Provider request failed.'
      }, { source: 'system' })
    ], { includeInitialPrompt: false });

    expect(messages.map(({ kind, contentMarkdown }) => [kind, contentMarkdown])).toEqual([
      ['commentary', 'I am checking the parser boundary.'],
      ['error', 'Provider request failed.']
    ]);
  });

  it('shows an actionable error when a model safety guardrail is waiting for steering', () => {
    const detail = runDetail('Review the authorized target.');
    detail.run.model = 'gpt-6-astra';
    const messages = commentaryMessagesForSession(detail, [
      displayEvent('canonical-session-event', {
        payload: {
          type: 'model_retry',
          recoveryKind: 'safety_guardrail',
          awaitingSteering: true,
          errorMessage: 'Provider safety guardrail.'
        }
      }, { source: 'executor', type: 'research_event' }),
      displayEvent('automatic-retry', {
        type: 'model_retry',
        recoveryKind: 'safety_guardrail',
        errorMessage: 'Provider safety guardrail.'
      }, { source: 'system' }),
      displayEvent('session-steering', {
        type: 'model_retry',
        recoveryKind: 'safety_guardrail',
        awaitingSteering: true,
        errorMessage: 'Provider safety guardrail.'
      }, { source: 'system' })
    ], { includeInitialPrompt: false });

    expect(messages.map(({ id, kind, contentMarkdown }) => [id, kind, contentMarkdown])).toEqual([[
      'session-marker:automatic-retry',
      'marker',
      'Recovering from Cyber Safeguard'
    ], [
      'model-safety-pause:session-steering',
      'error',
      '`gpt-6-astra` is waiting for a steering instruction after the provider flagged the request for possible cybersecurity risk. Clarify the authorized scope or revise the request, then send the steering message to continue.'
    ]]);

    const canonicalOnly = commentaryMessagesForSession(detail, [
      displayEvent('canonical-session-event', {
        payload: {
          type: 'model_retry',
          recoveryKind: 'safety_guardrail',
          awaitingSteering: true
        }
      }, { source: 'executor', type: 'research_event' })
    ], { includeInitialPrompt: false });
    expect(canonicalOnly.map(({ id, kind }) => [id, kind])).toEqual([
      ['model-safety-pause:canonical-session-event', 'error']
    ]);
  });

  it('shows compacting and safeguard recovery as session markers without exposing provider errors', () => {
    const messages = commentaryMessagesForSession(runDetail('Review the authorized service.'), [
      displayEvent('local-compaction', { type: 'context_compacted', agentPath: '/root' }, { source: 'system' }),
      displayEvent('local-checkpoint', { type: 'research_checkpoint', reason: 'local' }, { source: 'system' }),
      displayEvent('native-checkpoint', { type: 'research_checkpoint', reason: 'native' }, { source: 'system' }),
      displayEvent('safeguard-retry', { type: 'model_retry', recoveryKind: 'safety_guardrail',
        errorMessage: 'Sensitive provider diagnostic.' }, { source: 'system' }),
      displayEvent('epoch-boundary', { type: 'track_epoch_resolving', agentPath: '/root' }, { source: 'system' }),
      displayEvent('ordinary-retry', { type: 'model_retry', recoveryKind: 'transient' }, { source: 'system' }),
      displayEvent('safeguard-pause', { type: 'model_retry', recoveryKind: 'safety_guardrail',
        awaitingSteering: true }, { source: 'system' })
    ], { includeInitialPrompt: false });

    expect(messages.map(({ kind, contentMarkdown }) => [kind, contentMarkdown])).toEqual([
      ['marker', 'Context Compacted'],
      ['marker', 'Context Compacted'],
      ['marker', 'Recovering from Cyber Safeguard'],
      ['marker', 'Resolving Epoch'],
      ['error', 'The model is waiting for a steering instruction after the provider flagged the request for possible cybersecurity risk. Clarify the authorized scope or revise the request, then send the steering message to continue.']
    ]);
    expect(JSON.stringify(messages)).not.toContain('Sensitive provider diagnostic.');
    const markerHtml = renderToStaticMarkup(createElement(CommentaryMessageRow, {
      message: messages[2]!, searchHighlightQuery: '', selected: false
    }));
    expect(markerHtml).toContain('main-commentary-session-marker is-safeguard_recovery');
    expect(markerHtml).toContain('Recovering from Cyber Safeguard');
    const epochHtml = renderToStaticMarkup(createElement(CommentaryMessageRow, {
      message: messages[3]!, searchHighlightQuery: '', selected: false
    }));
    expect(epochHtml).toContain('main-commentary-session-marker is-epoch');
    expect(epochHtml).toContain('Resolving Epoch');
  });
});

function displayEvent(
  id: string,
  payload: Record<string, unknown>,
  overrides: Partial<TraceEventRecord> = {}
): TraceDisplayEvent {
  return {
    id,
    runId: 'run_commentary',
    attemptId: 'attempt_one',
    sequence: 1,
    source: 'model',
    type: 'model_message',
    summary: 'Event.',
    payload,
    sensitivity: 'internal',
    modelVisible: true,
    createdAt: '2026-08-03T10:00:00.000Z',
    artifactId: null,
    toolCallId: null,
    approvalId: null,
    ...overrides
  };
}

function toolEvent(
  id: string,
  kind: 'tool.requested' | 'tool.observed',
  toolName: string,
  toolActionId: string,
  normalizedInputs: Record<string, unknown> = {},
  result?: unknown
): TraceDisplayEvent {
  return displayEvent(id, {
    appServerKind: kind,
    agentPath: '/root',
    toolName,
    payload: {
      toolName,
      toolActionId,
      normalizedInputs,
      ...(kind === 'tool.observed' ? { status: 'complete' } : {}),
      ...(result !== undefined ? { result } : {})
    }
  }, {
    source: 'system',
    type: 'research_event',
    summary: `app-server ${kind}: ${toolName}.`
  });
}

function runDetail(promptMarkdown: string): RunDetail {
  return {
    run: {
      id: 'run_commentary',
      promptMarkdown,
      createdAt: '2026-08-03T09:59:00.000Z'
    },
    traceEvents: [],
    transcriptMessages: []
  } as unknown as RunDetail;
}
