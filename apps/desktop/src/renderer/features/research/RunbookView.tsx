import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { JSX } from 'react';
import { ArrowLeft, BookOpen, CircleAlert, CircleCheck, Clock3, LoaderCircle, Play } from 'lucide-react';
import type {
  AppServerRunbookCell,
  AppServerRunbookDocument,
  AppServerRunbookOutput,
  AppServerRunbookSummary,
  RunbookExecutionSelection,
  RunbookProofTarget,
  RunbookProofTargetSelection
} from '@shared/types';
import { traceLabel } from '../../lib/formatting';
import { ModelAuthors } from '../../app/ModelAuthors';
import { runbookDescriptionText, runbookExecutionStatus } from '../../view-models/runbooks';
import { renderHighlightedCodeBlock, renderTraceProseText } from '../traces/traceMarkup';

const RUNBOOK_INITIAL_CELL_RENDER_COUNT = 32;
const RUNBOOK_CELL_RENDER_CHUNK = 24;
const RUNBOOK_CELL_SOURCE_PREVIEW_CHARACTERS = 24_000;
const RUNBOOK_OUTPUT_PREVIEW_CHARACTERS = 12_000;

export const RunbookView = memo(function RunbookView({
  runbook,
  document,
  loading,
  error,
  onBackToMain,
  onRun,
  executionAvailable = false,
  connectedDeviceOs = null,
  showBackButton = true,
  followLatest = false
}: {
  runbook: AppServerRunbookSummary;
  document: AppServerRunbookDocument | null;
  loading: boolean;
  error: string | null;
  onBackToMain: () => void;
  onRun?: (selection: RunbookExecutionSelection, target: RunbookProofTargetSelection) => Promise<void>;
  executionAvailable?: boolean;
  connectedDeviceOs?: string | null;
  showBackButton?: boolean;
  followLatest?: boolean;
}): JSX.Element {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const followLatestRef = useRef(true);
  const runbookIdRef = useRef(runbook.id);
  const [requestedCellId, setRequestedCellId] = useState<string | null | undefined>(undefined);
  const [proofTarget, setProofTarget] = useState<RunbookProofTarget>('localhost');
  const [deviceOs, setDeviceOs] = useState('');
  const [rangeStartCellId, setRangeStartCellId] = useState('');
  const [rangeEndCellId, setRangeEndCellId] = useState('');
  const executionRunning = document?.latestRun?.status === 'running' || document?.latestRun?.status === 'queued';
  const executableCellOptions = useMemo(() => document?.cells
    .map((cell, index) => ({ cell, index }))
    .filter(({ cell }) => cell.type === 'code') ?? [], [document?.cells]);
  const executableCells = useMemo(() => executableCellOptions.map(({ cell }) => cell), [executableCellOptions]);
  const unhealthyCells = useMemo(
    () => executableCells.filter((cell) => !isSupportedRunbookLanguage(cell.language)),
    [executableCells]
  );
  const rangeStartIndex = rangeStartCellId
    ? executableCells.findIndex((cell) => cell.id === rangeStartCellId)
    : 0;
  const rangeEndIndex = rangeEndCellId
    ? executableCells.findIndex((cell) => cell.id === rangeEndCellId)
    : executableCells.length - 1;
  const rangeValid = executableCells.length > 0 && rangeStartIndex >= 0
    && rangeEndIndex >= 0 && rangeStartIndex <= rangeEndIndex;
  const selectedRangeCells = rangeValid ? executableCells.slice(rangeStartIndex, rangeEndIndex + 1) : [];
  const selectedRangeHealthy = selectedRangeCells.every((cell) => isSupportedRunbookLanguage(cell.language));
  const targetValid = proofTarget !== 'device' || deviceOs.trim().length > 0;
  const canRun = executionAvailable && Boolean(onRun) && rangeValid && selectedRangeHealthy
    && targetValid && !executionRunning && requestedCellId === undefined;
  const updateKey = useMemo(
    () => runbookViewUpdateKey(runbook, document, loading, error),
    [document, error, loading, runbook]
  );
  const hasLiveRunStatus = document?.latestRun?.status === 'running'
    || document?.cells.some((cell) => cell.latestRun?.status === 'running') === true;
  const [now, setNow] = useState(() => Date.now());
  const documentIdentity = document?.runbookId ?? runbook.id;
  const [renderWindow, setRenderWindow] = useState(() => ({
    runbookId: documentIdentity,
    count: Math.min(document?.cells.length ?? 0, RUNBOOK_INITIAL_CELL_RENDER_COUNT)
  }));
  const renderedCellCount = renderWindow.runbookId === documentIdentity
    ? renderWindow.count
    : Math.min(document?.cells.length ?? 0, RUNBOOK_INITIAL_CELL_RENDER_COUNT);

  useEffect(() => {
    if (!hasLiveRunStatus) return undefined;
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [hasLiveRunStatus]);

  useEffect(() => {
    const cellCount = document?.cells.length ?? 0;
    if (renderWindow.runbookId !== documentIdentity) {
      setRenderWindow({ runbookId: documentIdentity, count: Math.min(cellCount, RUNBOOK_INITIAL_CELL_RENDER_COUNT) });
      return undefined;
    }
    if (renderWindow.count >= cellCount) return undefined;
    const frame = window.requestAnimationFrame(() => {
      setRenderWindow((current) => current.runbookId === documentIdentity
        ? { ...current, count: Math.min(cellCount, current.count + RUNBOOK_CELL_RENDER_CHUNK) }
        : current);
    });
    return () => window.cancelAnimationFrame(frame);
  }, [document?.cells.length, documentIdentity, renderWindow]);

  useEffect(() => {
    if (executionRunning) setRequestedCellId(undefined);
  }, [document?.latestRun?.runId, executionRunning]);

  useEffect(() => {
    if (proofTarget === 'device' && !deviceOs.trim() && connectedDeviceOs) setDeviceOs(connectedDeviceOs);
  }, [connectedDeviceOs, deviceOs, proofTarget]);

  useEffect(() => {
    setRangeStartCellId('');
    setRangeEndCellId('');
  }, [runbook.id]);

  const requestExecution = useCallback(async (selection: RunbookExecutionSelection = {}): Promise<void> => {
    if (!onRun) return;
    setRequestedCellId(selection.cellId ?? null);
    try {
      await onRun(selection, {
        proofTarget,
        ...(proofTarget === 'device' ? { deviceOs: deviceOs.trim() } : {})
      });
    } catch {
      setRequestedCellId(undefined);
      return;
    }
    setRequestedCellId(undefined);
  }, [deviceOs, onRun, proofTarget]);
  const requestCellExecution = useCallback(
    (cellId: string) => requestExecution({ cellId }),
    [requestExecution]
  );

  const scrollToLatest = useCallback((): void => {
    const scroll = scrollRef.current;
    if (!scroll) return;
    scroll.scrollTop = Math.max(0, scroll.scrollHeight - scroll.clientHeight);
  }, []);

  const syncScroll = useCallback((): void => {
    if (followLatest && followLatestRef.current) scrollToLatest();
  }, [followLatest, scrollToLatest]);

  useLayoutEffect(() => {
    if (runbookIdRef.current !== runbook.id) {
      runbookIdRef.current = runbook.id;
      followLatestRef.current = true;
    }
    const frame = window.requestAnimationFrame(syncScroll);
    return () => window.cancelAnimationFrame(frame);
  }, [runbook.id, syncScroll, updateKey]);

  const handleScroll = useCallback((): void => {
    const scroll = scrollRef.current;
    if (!followLatest || !scroll) return;
    const distanceFromBottom = scroll.scrollHeight - scroll.clientHeight - scroll.scrollTop;
    followLatestRef.current = distanceFromBottom <= 24;
  }, [followLatest]);

  return (
    <section className="main-trace-view runbook-view" aria-label={`Runbook: ${runbook.title}`}>
      {showBackButton ? (
        <button type="button" className="back-to-main-button trace-back-to-main-button" onClick={onBackToMain}>
          <ArrowLeft size={15} aria-hidden="true" />
          Back to Main
        </button>
      ) : null}
      <div className="runbook-view-scroll" ref={scrollRef} onScroll={handleScroll}>
        <header className="runbook-view-header">
          <div className="runbook-view-heading-row">
            <span className="runbook-view-eyebrow"><BookOpen size={15} aria-hidden="true" /> Runbook</span>
            <div className="runbook-run-controls">
              <label className="runbook-target-field">
                <span>Proof target</span>
                <select
                  aria-label="Proof target"
                  disabled={executionRunning || requestedCellId !== undefined}
                  value={proofTarget}
                  onChange={(event) => setProofTarget(event.currentTarget.value as RunbookProofTarget)}
                >
                  <option value="localhost">Localhost</option>
                  <option value="device">Device</option>
                  <option value="vm">VM</option>
                  <option value="web">Web</option>
                  <option value="other">Other</option>
                </select>
              </label>
              {proofTarget === 'device' ? (
                <label className="runbook-target-field runbook-device-os-field">
                  <span>Device OS</span>
                  <input
                    aria-label="Target device OS"
                    disabled={executionRunning || requestedCellId !== undefined}
                    maxLength={120}
                    placeholder="iOS 27.0"
                    value={deviceOs}
                    onChange={(event) => setDeviceOs(event.currentTarget.value)}
                  />
                </label>
              ) : null}
              {executableCellOptions.length > 1 ? (
                <>
                  <label className="runbook-target-field runbook-range-field">
                    <span>Start</span>
                    <select
                      aria-label="Runbook range start"
                      disabled={executionRunning || requestedCellId !== undefined}
                      value={rangeStartCellId}
                      onChange={(event) => setRangeStartCellId(event.currentTarget.value)}
                    >
                      <option value="">First code cell</option>
                      {executableCellOptions.map(({ cell, index }) => (
                        <option key={`start-${cell.id}`} value={cell.id}>Cell {index + 1}</option>
                      ))}
                    </select>
                  </label>
                  <label className="runbook-target-field runbook-range-field">
                    <span>End</span>
                    <select
                      aria-label="Runbook range end"
                      disabled={executionRunning || requestedCellId !== undefined}
                      value={rangeEndCellId}
                      onChange={(event) => setRangeEndCellId(event.currentTarget.value)}
                    >
                      <option value="">Last code cell</option>
                      {executableCellOptions.map(({ cell, index }) => (
                        <option key={`end-${cell.id}`} value={cell.id}>Cell {index + 1}</option>
                      ))}
                    </select>
                  </label>
                </>
              ) : null}
              <button
                type="button"
                className="runbook-run-button"
                disabled={!canRun}
                title={!executionAvailable ? 'Runbooks execute only in their active app-server session.' : !rangeValid ? 'Choose a start cell that precedes the end cell.' : !selectedRangeHealthy ? 'Every selected code cell needs a supported language.' : !targetValid ? 'Enter the target device OS.' : undefined}
                onClick={() => void requestExecution({
                  ...(rangeStartCellId ? { startCellId: rangeStartCellId } : {}),
                  ...(rangeEndCellId ? { endCellId: rangeEndCellId } : {})
                })}
              >
                {executionRunning || requestedCellId === null ? <LoaderCircle className="runbook-view-spinner" size={13} aria-hidden="true" /> : <Play size={13} aria-hidden="true" />}
                {executionRunning || requestedCellId === null ? 'Running' : rangeStartCellId || rangeEndCellId ? 'Run range' : 'Run'}
              </button>
            </div>
          </div>
          <h2>{runbook.title}</h2>
          <ModelAuthors authors={runbook.authors} />
          {runbook.purpose ? <p>{runbookDescriptionText(runbook.purpose)}</p> : null}
          <div className="runbook-view-meta">
            <span>Content revision {runbook.contentRevision}</span>
            <span>{runbook.execution.completedRunCount} completed {runbook.execution.completedRunCount === 1 ? 'run' : 'runs'}</span>
            <span>{runbook.execution.executedCellCount} {runbook.execution.executedCellCount === 1 ? 'cell' : 'cells'} executed</span>
            <span>Latest run {runbookExecutionStatus(runbook).label}</span>
            {document?.language ? <span>{document.language}</span> : null}
            {document?.latestRun ? <RunStatus now={now} state={document.latestRun} /> : null}
          </div>
          <div className={`runbook-guidance${unhealthyCells.length > 0 || executableCells.length === 0 ? ' needs-attention' : ''}`}>
            {unhealthyCells.length > 0 || executableCells.length === 0 ? <CircleAlert size={14} aria-hidden="true" /> : <CircleCheck size={14} aria-hidden="true" />}
            <span>{runbookGuidance(executableCells.length, unhealthyCells.length, executionAvailable)}</span>
          </div>
        </header>

        {loading ? (
          <div className="runbook-view-state"><LoaderCircle className="runbook-view-spinner" size={18} aria-hidden="true" /> Loading runbook.</div>
        ) : error ? (
          <div className="runbook-view-state is-error"><CircleAlert size={18} aria-hidden="true" /> {error}</div>
        ) : document && document.cells.length > 0 ? (
          <div className="runbook-cell-list">
            {document.cells.slice(0, renderedCellCount).map((cell, index) => (
              <RunbookCellView
                cell={cell}
                executionAvailable={executionAvailable}
                executionRunning={executionRunning || requestedCellId !== undefined || !targetValid}
                index={index}
                key={`${cell.id}:${index}`}
                now={cell.latestRun?.status === 'running' ? now : 0}
                requested={requestedCellId === cell.id}
                onRun={onRun ? requestCellExecution : undefined}
              />
            ))}
            {renderedCellCount < document.cells.length ? (
              <div className="runbook-cell-render-progress" aria-live="polite">
                Rendering {document.cells.length - renderedCellCount} remaining cells.
              </div>
            ) : null}
          </div>
        ) : (
          <div className="runbook-view-state">This runbook has no cells.</div>
        )}
      </div>
    </section>
  );
});

export function runbookViewUpdateKey(
  runbook: AppServerRunbookSummary,
  document: AppServerRunbookDocument | null,
  loading: boolean,
  error: string | null
): string {
  const cells = document?.cells.map((cell) => [
    cell.id,
    cell.executionCount ?? '',
    cell.latestRun?.runId ?? '',
    cell.latestRun?.status ?? '',
    cell.latestRun?.durationMs ?? '',
    cell.outputs.length,
    cell.outputs.at(-1)?.text.length ?? 0
  ].join(':')).join('|') ?? '';
  return `${runbook.id}:${document?.revision ?? runbook.revision}:${document?.latestRun?.status ?? ''}:${document?.latestRun?.proofTarget ?? ''}:${document?.latestRun?.deviceOs ?? ''}:${loading ? 'loading' : 'ready'}:${error ?? ''}:${cells}`;
}

interface RunbookCellViewProps {
  cell: AppServerRunbookCell;
  index: number;
  executionAvailable: boolean;
  executionRunning: boolean;
  now: number;
  requested: boolean;
  onRun?: (cellId: string) => Promise<void>;
}

const RunbookCellView = memo(function RunbookCellView({ cell, index, executionAvailable, executionRunning, now, requested, onRun }: RunbookCellViewProps): JSX.Element {
  const supported = isSupportedRunbookLanguage(cell.language);
  const running = cell.latestRun?.status === 'running' || requested;
  return (
    <article className={`runbook-cell runbook-cell-${cell.type}`}>
      <header className="runbook-cell-header">
        <span>{cell.type === 'code' ? cell.language ?? 'Code' : traceLabel(cell.type)}</span>
        <span className="runbook-cell-header-actions">
          {cell.latestRun ? <RunStatus now={now} state={cell.latestRun} compact /> : null}
          <span>Cell {index + 1}{cell.executionCount === null ? '' : ` · [${cell.executionCount}]`}</span>
          {cell.type === 'code' ? (
            <button
              type="button"
              className="runbook-cell-run-button"
              disabled={!executionAvailable || executionRunning || !supported || !onRun}
              title={!supported ? 'Add a supported language before running this cell.' : 'Run this cell'}
              aria-label={`Run cell ${index + 1}`}
              onClick={() => void onRun?.(cell.id)}
            >
              {running ? <LoaderCircle className="runbook-view-spinner" size={12} aria-hidden="true" /> : <Play size={12} aria-hidden="true" />}
              {running ? 'Running' : 'Run'}
            </button>
          ) : null}
        </span>
      </header>
      <RunbookCellSource cell={cell} />
      {cell.outputs.length > 0 ? (
        <div className="runbook-output-list" aria-label={`Outputs for cell ${index + 1}`}>
          {cell.outputs.map((output, outputIndex) => (
            <RunbookOutputView key={`${cell.id}-output-${outputIndex}`} output={output} />
          ))}
        </div>
      ) : null}
    </article>
  );
}, runbookCellViewPropsEqual);

const RunbookCellSource = memo(function RunbookCellSource({ cell }: { cell: AppServerRunbookCell }): JSX.Element {
  const truncated = cell.source.length > RUNBOOK_CELL_SOURCE_PREVIEW_CHARACTERS;
  const [expanded, setExpanded] = useState(false);
  const source = truncated && !expanded
    ? cell.source.slice(0, RUNBOOK_CELL_SOURCE_PREVIEW_CHARACTERS)
    : cell.source;
  const content = useMemo(() => cell.type === 'markdown'
    ? renderTraceProseText(source, 'agent_output')
    : cell.type === 'code'
      ? renderHighlightedCodeBlock(source, cell.language)
      : <pre>{source}</pre>, [cell.language, cell.type, source]);
  return (
    <div className="runbook-cell-content">
      {content}
      {truncated ? (
        <button
          type="button"
          className="runbook-expand-button"
          aria-expanded={expanded}
          onClick={() => setExpanded((current) => !current)}
        >
          {expanded ? 'Show less' : `Show full cell (${cell.source.length - RUNBOOK_CELL_SOURCE_PREVIEW_CHARACTERS} more characters)`}
        </button>
      ) : null}
    </div>
  );
}, (previous, next) => previous.cell.id === next.cell.id
  && previous.cell.type === next.cell.type
  && previous.cell.language === next.cell.language
  && previous.cell.source === next.cell.source);

function runbookCellViewPropsEqual(previous: RunbookCellViewProps, next: RunbookCellViewProps): boolean {
  if (previous.index !== next.index
    || previous.executionAvailable !== next.executionAvailable
    || previous.executionRunning !== next.executionRunning
    || previous.requested !== next.requested
    || previous.onRun !== next.onRun) return false;
  if ((previous.cell.latestRun?.status === 'running' || next.cell.latestRun?.status === 'running') && previous.now !== next.now) return false;
  return appServerRunbookCellsEqual(previous.cell, next.cell);
}

function appServerRunbookCellsEqual(previous: AppServerRunbookCell, next: AppServerRunbookCell): boolean {
  if (previous.id !== next.id
    || previous.type !== next.type
    || previous.source !== next.source
    || previous.language !== next.language
    || previous.executionCount !== next.executionCount
    || runbookRunStatusKey(previous.latestRun) !== runbookRunStatusKey(next.latestRun)
    || previous.outputs.length !== next.outputs.length) return false;
  return previous.outputs.every((output, index) => {
    const candidate = next.outputs[index];
    return candidate !== undefined
      && output.kind === candidate.kind
      && output.text === candidate.text
      && output.streamName === candidate.streamName
      && output.mimeType === candidate.mimeType;
  });
}

function runbookRunStatusKey(state: AppServerRunbookCell['latestRun']): string {
  return state
    ? `${state.runId}:${state.status}:${state.startedAt}:${state.completedAt ?? ''}:${state.durationMs ?? ''}:${state.exitCode ?? ''}:${state.error ?? ''}:${state.proofTarget}:${state.deviceOs ?? ''}`
    : '';
}

function RunStatus({ state, now, compact = false }: {
  state: NonNullable<AppServerRunbookDocument['latestRun']>;
  now: number;
  compact?: boolean;
}): JSX.Element {
  const startedAt = Date.parse(state.startedAt);
  const liveDuration = state.status === 'running' && Number.isFinite(startedAt) ? Math.max(0, now - startedAt) : null;
  const durationMs = state.durationMs ?? liveDuration;
  const duration = durationMs === null ? null : formatDuration(durationMs);
  const target = state.proofTarget === 'device' && state.deviceOs
    ? `Device · ${state.deviceOs}`
    : proofTargetLabel(state.proofTarget);
  return (
    <span className={`runbook-run-status status-${state.status}`} title={state.error ?? undefined}>
      {state.status === 'running' || state.status === 'queued'
        ? <LoaderCircle className="runbook-view-spinner" size={12} aria-hidden="true" />
        : <Clock3 size={12} aria-hidden="true" />}
      {compact ? traceLabel(state.status) : `Last run ${traceLabel(state.status)}`}{duration ? ` · ${duration}` : ''}{` · ${target}`}
    </span>
  );
}

function proofTargetLabel(target: RunbookProofTarget): string {
  return target === 'localhost' ? 'Localhost' : target === 'device' ? 'Device' : target === 'vm' ? 'VM' : target === 'web' ? 'Web' : 'Other';
}

export function isSupportedRunbookLanguage(language: string | null): boolean {
  if (!language) return false;
  return ['shell', 'sh', 'posix-shell', 'bash', 'zsh', 'python', 'python3', 'py', 'javascript', 'js', 'node', 'ruby', 'perl', 'powershell', 'pwsh']
    .includes(language.trim().toLowerCase());
}

function formatDuration(durationMs: number): string {
  if (durationMs < 1_000) return `${Math.round(durationMs)}ms`;
  if (durationMs < 60_000) return `${(durationMs / 1_000).toFixed(durationMs < 10_000 ? 1 : 0)}s`;
  const minutes = Math.floor(durationMs / 60_000);
  const seconds = Math.round((durationMs % 60_000) / 1_000);
  return `${minutes}m ${seconds}s`;
}

function runbookGuidance(codeCells: number, unhealthyCells: number, executionAvailable: boolean): string {
  if (codeCells === 0) return 'Add code cells for each bounded proof step; keep prerequisites, expected evidence, interpretation, and cleanup in markdown.';
  if (unhealthyCells > 0) return `${unhealthyCells} code ${unhealthyCells === 1 ? 'cell needs' : 'cells need'} an explicit supported language before this runbook is healthy.`;
  if (!executionAvailable) return 'Healthy runbook: bounded, repeatable cells with explicit languages. Reopen its active session to execute it.';
  return 'Healthy runbook: run cells are bounded and repeatable; keep prerequisites, expected evidence, interpretation, and cleanup explicit.';
}

const RunbookOutputView = memo(function RunbookOutputView({ output }: { output: AppServerRunbookOutput }): JSX.Element {
  const truncated = output.text.length > RUNBOOK_OUTPUT_PREVIEW_CHARACTERS;
  const [expanded, setExpanded] = useState(false);
  const text = truncated && !expanded
    ? output.text.slice(0, RUNBOOK_OUTPUT_PREVIEW_CHARACTERS)
    : output.text;
  const label = output.kind === 'stream'
    ? output.streamName ?? 'output'
    : output.kind === 'error'
      ? 'error'
      : output.mimeType ?? 'output';
  return (
    <section className={`runbook-output runbook-output-${output.kind} ${output.streamName === 'stderr' ? 'is-stderr' : ''}`}>
      <span className="runbook-output-label">{label}</span>
      {output.mimeType === 'text/markdown'
        ? renderTraceProseText(text, 'agent_output')
        : <pre>{text}</pre>}
      {truncated ? (
        <button
          type="button"
          className="runbook-expand-button"
          aria-expanded={expanded}
          onClick={() => setExpanded((current) => !current)}
        >
          {expanded ? 'Show less' : `Show full output (${output.text.length - RUNBOOK_OUTPUT_PREVIEW_CHARACTERS} more characters)`}
        </button>
      ) : null}
    </section>
  );
}, (previous, next) => previous.output.kind === next.output.kind
  && previous.output.text === next.output.text
  && previous.output.streamName === next.output.streamName
  && previous.output.mimeType === next.output.mimeType);
