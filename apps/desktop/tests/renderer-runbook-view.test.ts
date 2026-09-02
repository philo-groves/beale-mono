import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import type { AppServerRunbookDocument, AppServerRunbookSummary } from '../src/shared/types';
import { RunbookView, runbookViewUpdateKey } from '../src/renderer/features/research/RunbookView';

const summary: AppServerRunbookSummary = {
  id: 'runbook-1',
  workspaceId: 'workspace-1',
  workspaceName: 'Demo',
  subjectId: null,
  subjectName: null,
  sessionId: 'session-1',
  title: 'Validate parser boundary',
  purpose: 'Capture a repeatable validation\n  procedure with one display line.',
  artifactId: 'artifact-1',
  revision: 4,
  contentRevision: 2,
  execution: {
    runCount: 3,
    completedRunCount: 3,
    executedCellCount: 5,
    latest: { runId: 'runbook_run_latest', status: 'succeeded', startedAt: '2026-07-23T10:59:58.750Z' },
    latestSuccessfulRunId: 'runbook_run_latest'
  },
  revisions: [],
  authors: [
    { provider: 'openai', model: 'gpt-5.6' },
    { provider: 'anthropic', model: 'claude-opus-4-6' }
  ],
  createdAt: '2026-07-23T10:00:00.000Z',
  updatedAt: '2026-07-23T11:00:00.000Z'
};

const document: AppServerRunbookDocument = {
  runbookId: summary.id,
  nbformat: 4,
  nbformatMinor: 5,
  language: 'typescript',
  revision: 4,
  latestRun: null,
  cells: [
    {
      id: 'markdown',
      type: 'markdown',
      source: '# Procedure\n\n- Build fixture\n- Run verifier',
      language: null,
      executionCount: null,
      latestRun: null,
      outputs: []
    },
    {
      id: 'code',
      type: 'code',
      source: 'const verified: boolean = true;',
      language: 'typescript',
      executionCount: 3,
      latestRun: {
        runId: 'runbook-run-3',
        status: 'succeeded',
        startedAt: '2026-07-23T10:59:58.750Z',
        completedAt: '2026-07-23T11:00:00.000Z',
        durationMs: 1250,
        exitCode: 0,
        error: null,
        proofTarget: 'device',
        deviceOs: 'iOS 27.0'
      },
      outputs: [
        { kind: 'stream', text: 'verified\n', streamName: 'stdout', mimeType: 'text/plain' },
        { kind: 'display', text: '**Result:** pass', streamName: null, mimeType: 'text/markdown' }
      ]
    },
    {
      id: 'raw',
      type: 'raw',
      source: 'Keep this note visible.',
      language: null,
      executionCount: null,
      latestRun: null,
      outputs: []
    }
  ]
};

describe('RunbookView', () => {
  it('renders all cells, formatted Markdown, highlighted code, outputs, and Back navigation', () => {
    const html = renderToStaticMarkup(createElement(RunbookView, {
      runbook: summary,
      document,
      loading: false,
      error: null,
      onBackToMain: () => undefined
    }));

    expect(html).toContain('Back to Main');
    expect(html).toContain('Authored by');
    expect(html).toContain('gpt-5.6');
    expect(html).toContain('claude-opus-4-6');
    expect(html).toContain('<p>Capture a repeatable validation procedure with one display line.</p>');
    expect(html).toContain('<h1>Procedure</h1>');
    expect(html).toContain('<ul>');
    expect(html).toContain('class="hljs language-typescript"');
    expect(html).toContain('verified');
    expect(html).toContain('<strong>Result:</strong> pass');
    expect(html).toContain('Keep this note visible.');
    expect(html).toContain('Succeeded · 1.3s');
    expect(html).toContain('Device · iOS 27.0');
    expect(html).toContain('Content revision 2');
    expect(html).toContain('Latest run Succeeded');
    expect(html).not.toContain('>Active<');
    expect(html).toContain('3 completed runs');
    expect(html).toContain('5 cells executed');
    expect(html).toContain('aria-label="Run cell 2"');
    expect((html.match(/class="runbook-cell /g) ?? []).length).toBe(3);
  });

  it('enables whole-run and cell controls for a healthy runbook in its live session', () => {
    const executableDocument: AppServerRunbookDocument = {
      ...document,
      language: 'sh',
      latestRun: null,
      cells: [
        ...document.cells.map((cell) => cell.id === 'code'
          ? { ...cell, language: 'sh', latestRun: null }
          : cell),
        {
          ...document.cells[1],
          id: 'code-2',
          source: 'printf "done\\n"',
          language: 'sh',
          latestRun: null
        }
      ]
    };
    const html = renderToStaticMarkup(createElement(RunbookView, {
      runbook: summary,
      document: executableDocument,
      loading: false,
      error: null,
      executionAvailable: true,
      onRun: async () => undefined,
      onBackToMain: () => undefined
    }));

    expect(html).toContain('Healthy runbook: run cells are bounded and repeatable');
    expect(html).toContain('class="runbook-run-button"');
    expect(html).toContain('aria-label="Proof target"');
    expect(html).toContain('aria-label="Runbook range start"');
    expect(html).toContain('aria-label="Runbook range end"');
    expect(html).toContain('<option value="localhost" selected="">Localhost</option>');
    expect(html).toContain('aria-label="Run cell 2"');
    expect(html).not.toContain('class="runbook-run-button" disabled=""');
  });

  it('supports embedded rendering and versions appended content for follow-to-bottom updates', () => {
    const appendedDocument: AppServerRunbookDocument = {
      ...document,
      cells: document.cells.map((cell, index) => index === 1
        ? { ...cell, outputs: [...cell.outputs, { kind: 'stream', text: 'new output\n', streamName: 'stdout', mimeType: 'text/plain' }] }
        : cell)
    };
    const html = renderToStaticMarkup(createElement(RunbookView, {
      runbook: summary,
      document: appendedDocument,
      loading: false,
      error: null,
      followLatest: true,
      showBackButton: false,
      onBackToMain: () => undefined
    }));

    expect(html).not.toContain('Back to Main');
    expect(html).toContain('new output');
    expect(runbookViewUpdateKey(summary, appendedDocument, false, null)).not.toBe(
      runbookViewUpdateKey(summary, document, false, null)
    );
  });

  it('renders duplicate runbooks as the bottom detail section', () => {
    const canonical = {
      ...summary,
      duplicateRunbooks: [{
        id: 'runbook-duplicate',
        title: 'Repeated parser procedure',
        purpose: 'The same procedure.',
        revision: 3,
        markedAt: '2026-09-02T12:00:00.000Z'
      }]
    };
    const candidate = { ...summary, id: 'runbook-other', title: 'Other parser procedure' };
    const html = renderToStaticMarkup(createElement(RunbookView, {
      runbook: canonical,
      runbookCandidates: [canonical, candidate],
      document,
      loading: false,
      error: null,
      onBackToMain: () => undefined,
      onMarkDuplicate: () => undefined,
      onUndoDuplicate: () => undefined
    }));

    expect(html).toContain('aria-label="Duplicate runbook management"');
    expect(html).toContain('Repeated parser procedure');
    expect(html).toContain('Other parser procedure');
    expect(html.lastIndexOf('<section')).toBe(html.indexOf('<section class="memory-catalog-subsection campaign-claim-duplicates runbook-duplicates"'));
  });

  it('bounds initial work for large runbooks and collapses oversized cell content and output', () => {
    const largeDocument: AppServerRunbookDocument = {
      ...document,
      cells: Array.from({ length: 40 }, (_, index) => ({
        ...document.cells[1]!,
        id: `code-${index}`,
        source: index === 0 ? 'x'.repeat(24_005) : `printf "cell ${index}\\n"`,
        outputs: index === 0
          ? [{ kind: 'stream', text: 'y'.repeat(12_008), streamName: 'stdout', mimeType: 'text/plain' }]
          : []
      }))
    };
    const html = renderToStaticMarkup(createElement(RunbookView, {
      runbook: summary,
      document: largeDocument,
      loading: false,
      error: null,
      onBackToMain: () => undefined
    }));

    expect((html.match(/class="runbook-cell /g) ?? []).length).toBe(32);
    expect(html).toContain('Rendering 8 remaining cells.');
    expect(html).toContain('Show full cell (5 more characters)');
    expect(html).toContain('Show full output (8 more characters)');
    expect(html).not.toContain('x'.repeat(24_005));
    expect(html).not.toContain('y'.repeat(12_008));
  });
});
