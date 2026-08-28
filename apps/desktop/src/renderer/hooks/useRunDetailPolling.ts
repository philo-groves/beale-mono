import { startTransition, useCallback, useEffect, useRef, useState } from 'react';
import type { RunDetail, RunDetailProjection, RunRecord, RunStatus } from '@shared/types';
import { runDetailProjectionMetricLabel } from '../../shared/runDetailProjection';
import { devInstrumentation, recordNextFrameTiming } from '../devInstrumentation';
import { errorMessage } from '../lib/errors';
import {
  mergeRunDetailUpdate,
  runDetailMetricDetail,
  runDetailUpdateCursor,
  runDetailUpdateMetricDetail,
  shortMetricId
} from '../view-models/runDetailUpdates';

const ACTIVE_RUN_DETAIL_POLL_MS = 750;
const LARGE_RUN_DETAIL_POLL_MS = 1_250;
const VERY_LARGE_RUN_DETAIL_POLL_MS = 2_000;

export function useRunDetailPolling({
  selectedRunId,
  selectedRunState,
  projection,
  refreshKey,
  onError
}: {
  selectedRunId: string | null;
  selectedRunState: RunStatus | null;
  projection: RunDetailProjection;
  refreshKey: string | null;
  onError: (message: string) => void;
}): {
  runDetail: RunDetail | null;
  sessionSetupPending: boolean;
  clearRunDetail: () => void;
  primeRunDetail: (run: RunRecord) => void;
} {
  const [runDetail, setRunDetail] = useState<RunDetail | null>(null);
  const [setupPendingRunId, setSetupPendingRunId] = useState<string | null>(null);
  const requestSeqRef = useRef(0);
  const versionRef = useRef<string | null>(null);
  const projectionRef = useRef<RunDetailProjection | null>(null);
  const detailRef = useRef<RunDetail | null>(null);

  useEffect(() => {
    detailRef.current = runDetail;
  }, [runDetail]);

  const clearRunDetail = useCallback(() => {
    versionRef.current = null;
    projectionRef.current = null;
    detailRef.current = null;
    setRunDetail(null);
    setSetupPendingRunId(null);
  }, []);

  const primeRunDetail = useCallback((run: RunRecord): void => {
    requestSeqRef.current += 1;
    window.beale.cancelRunDetailRequests(run.id);
    const detail = optimisticRunDetail(run);
    versionRef.current = null;
    projectionRef.current = projection;
    detailRef.current = detail;
    setRunDetail(detail);
    setSetupPendingRunId(run.id);
  }, [projection]);

  useEffect(() => {
    const requestSeq = ++requestSeqRef.current;
    if (!selectedRunId || selectedRunState === null) {
      clearRunDetail();
      return undefined;
    }

    let projectionRefreshPending = false;
    if (detailRef.current?.run.id !== selectedRunId) {
      versionRef.current = null;
      projectionRef.current = projection;
      detailRef.current = null;
      setRunDetail(null);
      setSetupPendingRunId(null);
    } else if (projectionRef.current !== projection) {
      // A projection switch changes only which agent commentary accompanies the
      // same session. Retain the current detail until its replacement arrives so
      // the session workspace and right-sidenav navigation never unmount.
      versionRef.current = null;
      projectionRef.current = projection;
      projectionRefreshPending = true;
    }
    let disposed = false;
    let inFlight = false;
    let consecutiveFailures = 0;
    let pollTimer: number | null = null;
    const scheduleNextPoll = (): void => {
      const needsEnrichment = runDetailNeedsEnrichment(detailRef.current, projection);
      if (disposed || (selectedRunState !== 'active' && !needsEnrichment)) return;
      if (pollTimer !== null) window.clearTimeout(pollTimer);
      pollTimer = window.setTimeout(
        refreshRunDetail,
        needsEnrichment && consecutiveFailures === 0
          ? 0
          : activeRunDetailPollMs(detailRef.current, consecutiveFailures)
      );
    };
    const refreshRunDetail = (): void => {
      if (inFlight) return;
      inFlight = true;
      const currentDetail = detailRef.current;
      const projectionMetric = runDetailProjectionMetricLabel(projection);
      const request = projectionRefreshPending || currentDetail?.run.id !== selectedRunId
        ? devInstrumentation
            .timeAsync('ipc.getRunDetail', () => window.beale.getRunDetail(selectedRunId, projection), { run: shortMetricId(selectedRunId), projection: projectionMetric })
            .then((detail) => ({ detail, version: `initial:${requestSeq}`, update: null }))
        : devInstrumentation
            .timeAsync(
              'ipc.getRunDetailUpdate',
              () => window.beale.getRunDetailUpdate(selectedRunId, runDetailUpdateCursor(currentDetail), projection),
              { run: shortMetricId(selectedRunId), projection: projectionMetric }
            )
            .then(async (update) => {
              if (!disposed && requestSeq === requestSeqRef.current && update.version.version === versionRef.current) {
                return null;
              }
              await yieldToRenderer();
              if (disposed || requestSeq !== requestSeqRef.current) return null;
              const updateMetricDetail = runDetailUpdateMetricDetail(update);
              const detail = devInstrumentation.time('trace.mergeRunDetailUpdate', () => mergeRunDetailUpdate(currentDetail, update), {
                ...updateMetricDetail,
                currentTraceEvents: currentDetail.traceEvents.length,
                currentTranscripts: currentDetail.transcriptMessages.length
              });
              return { detail, version: update.version.version, update };
            });
      request
        .then((result) => {
          consecutiveFailures = 0;
          if (!result) return;
          const { detail, version, update } = result;
          if (!disposed && requestSeq === requestSeqRef.current && sessionSetupComplete(detail, version)) {
            setSetupPendingRunId((current) => current === selectedRunId ? null : current);
          }
          if (update) {
            devInstrumentation.recordPayload('ipc.getRunDetailUpdate.payload', update, runDetailUpdateMetricDetail(update));
          } else {
            devInstrumentation.recordPayload('ipc.getRunDetail.payload', detail, runDetailMetricDetail(detail));
          }
          if (!disposed && requestSeq === requestSeqRef.current) {
            if (version !== versionRef.current) {
              projectionRefreshPending = false;
              const applyStartedAt = performance.now();
              const applyDetail = runDetailApplyMetricDetail(detail, update);
              versionRef.current = version;
              detailRef.current = detail;
              // Run-detail materialization can invalidate the commentary, session
              // summary, and right navigation together. Keep these background
              // polling commits interruptible so pointer and typing work stays in
              // the urgent lane. The canonical ref advances immediately, so a
              // continuous stream cannot make the next poll request stale data.
              startTransition(() => setRunDetail(detail));
              devInstrumentation.recordEvent(update ? 'trace.runDetail.incrementalApply' : 'trace.runDetail.fullApply', applyDetail);
              recordNextFrameTiming('trace.runDetail.apply.nextFrameLatency', applyStartedAt, applyDetail);
            } else {
              devInstrumentation.recordEvent('ipc.getRunDetail.versionRaceSkipped', {
                run: shortMetricId(detail.run.id)
              });
            }
          }
        })
        .catch((caught: unknown) => {
          if (!disposed && requestSeq === requestSeqRef.current) {
            const message = errorMessage(caught);
            if (!shouldReportRunDetailError(detailRef.current, selectedRunId)) {
              consecutiveFailures += 1;
              devInstrumentation.recordEvent('ipc.getRunDetailUpdate.retry', {
                run: shortMetricId(selectedRunId),
                consecutiveFailures,
                message
              });
            } else {
              onError(message);
            }
          }
        })
        .finally(() => {
          inFlight = false;
          scheduleNextPoll();
        });
    };

    refreshRunDetail();
    return () => {
      disposed = true;
      if (pollTimer !== null) window.clearTimeout(pollTimer);
      window.beale.cancelRunDetailRequests(selectedRunId);
    };
  }, [clearRunDetail, onError, projection, refreshKey, selectedRunId, selectedRunState]);

  return {
    runDetail,
    sessionSetupPending: setupPendingRunId === runDetail?.run.id,
    clearRunDetail,
    primeRunDetail
  };
}

function yieldToRenderer(): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, 0));
}

export function optimisticRunDetail(run: RunRecord): RunDetail {
  return {
    run,
    researchProfile: null,
    nextStepSuggestions: null,
    attempts: [],
    traceEvents: [],
    transcriptMessages: [],
    breakoutRooms: [],
    breakoutRoomMembers: [],
    breakoutRoomMessages: [],
    artifacts: [],
    verifierContracts: [],
    verifierRuns: [],
    modelSessions: [],
    contextCompactions: [],
    policyEvents: [],
    exports: []
  };
}

export function sessionSetupComplete(detail: RunDetail, version: string): boolean {
  return version.startsWith('honeycrisp:')
    || detail.run.status !== 'active'
    || detail.attempts.length > 0
    || detail.traceEvents.length > 0
    || detail.transcriptMessages.length > 0;
}

export function activeRunDetailPollMs(detail: RunDetail | null, consecutiveFailures = 0): number {
  const recordCount = (detail?.traceEvents.length ?? 0) + (detail?.transcriptMessages.length ?? 0);
  const baseDelay = recordCount >= 20_000
    ? VERY_LARGE_RUN_DETAIL_POLL_MS
    : recordCount >= 5_000
      ? LARGE_RUN_DETAIL_POLL_MS
      : ACTIVE_RUN_DETAIL_POLL_MS;
  return Math.min(10_000, baseDelay * (2 ** Math.min(4, Math.max(0, consecutiveFailures))));
}

export function runDetailNeedsEnrichment(
  detail: RunDetail | null,
  projection: RunDetailProjection
): boolean {
  return projection !== 'full' && Boolean(detail) && detail?.honeycrispMemory === undefined;
}

export function shouldReportRunDetailError(detail: RunDetail | null, selectedRunId: string): boolean {
  return detail?.run.id !== selectedRunId;
}

function runDetailApplyMetricDetail(detail: RunDetail, update: { traceEvents: unknown[]; transcriptMessages: unknown[] } | null): Record<string, string | number | boolean> {
  return {
    run: shortMetricId(detail.run.id),
    status: detail.run.status,
    incremental: Boolean(update),
    addedTraceEvents: update?.traceEvents.length ?? detail.traceEvents.length,
    addedTranscripts: update?.transcriptMessages.length ?? detail.transcriptMessages.length,
    totalTraceEvents: detail.traceEvents.length,
    totalTranscripts: detail.transcriptMessages.length
  };
}
