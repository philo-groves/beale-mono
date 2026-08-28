import { useEffect, useRef } from 'react';
import type {
  ProfilingEventReportRow,
  ProfilingMetricDetail,
  ProfilingMetricValue,
  ProfilingReport,
  ProfilingRenderReportRow,
  ProfilingSample,
  ProfilingTimingReportRow
} from '@shared/types';

export type DevMetricValue = ProfilingMetricValue;
export type DevMetricDetail = ProfilingMetricDetail;

const DEV_INSTRUMENTATION_STORAGE_KEY = 'beale.devInstrumentation';
const DEV_INSTRUMENTATION_QUERY_KEY = 'bealePerf';
const DEV_INSTRUMENTATION_FLUSH_MS = 3_000;
const POINTER_MOVE_LATENCY_SAMPLE_MS = 180;
const PAYLOAD_SIZE_ESTIMATE_VALUE_LIMIT = 4_000;
const MAX_PROFILE_SAMPLES = 240;
const MAX_PROFILE_SESSION_SAMPLES = 4_096;
const FRAME_GAP_THRESHOLD_MS = 34;
const EVENT_LOOP_SAMPLE_MS = 100;
const EVENT_LOOP_LAG_THRESHOLD_MS = 24;

interface RenderStat {
  count: number;
  lastRender: number;
  detail: DevMetricDetail;
}

interface TimingStat {
  count: number;
  totalMs: number;
  maxMs: number;
  lastMs: number;
  detail: DevMetricDetail;
}

interface EventStat {
  count: number;
  detail: DevMetricDetail;
}

interface PendingProfilingSample {
  atMs: number;
  category: ProfilingSample['category'];
  name: string;
  durationMs: number | null;
  detail: DevMetricDetail;
}

export type DevPerformanceReport = ProfilingReport;

interface BealeDevPerformanceControls {
  enable(): void;
  disable(): void;
  report(): DevPerformanceReport;
  status(): { available: boolean; enabled: boolean; optIn: boolean };
}

type BealeDevPerformanceWindow = Window & {
  bealeDevPerformance?: BealeDevPerformanceControls;
};

class RendererDevInstrumentation {
  private enabled = false;
  private devConsoleEnabled = false;
  private profilingEnabled = false;
  private announced = false;
  private flushTimer: number | null = null;
  private reportSink: ((report: ProfilingReport) => void) | null = null;
  private readonly renderStats = new Map<string, RenderStat>();
  private readonly timingStats = new Map<string, TimingStat>();
  private readonly eventStats = new Map<string, EventStat>();
  private readonly samples: PendingProfilingSample[] = [];
  private readonly sessionRenderStats = new Map<string, RenderStat>();
  private readonly sessionTimingStats = new Map<string, TimingStat>();
  private readonly sessionEventStats = new Map<string, EventStat>();
  private readonly sessionSamples: PendingProfilingSample[] = [];
  private longTaskObserver: PerformanceObserver | null = null;
  private frameRequestId: number | null = null;
  private eventLoopTimer: number | null = null;
  private lastReport: ProfilingReport | null = null;

  public constructor() {
    if (typeof window === 'undefined') return;
    this.devConsoleEnabled = this.computeDevConsoleEnabled();
    this.syncEnabled();
    this.installControls();
    if (this.enabled) {
      this.announce();
      this.ensureFlushTimer();
    }
  }

  public isEnabled(): boolean {
    return this.enabled;
  }

  public configureProfiling({
    enabled,
    onReport
  }: {
    enabled: boolean;
    onReport: ((report: ProfilingReport) => void) | null;
  }): void {
    const wasProfilingEnabled = this.profilingEnabled;
    this.profilingEnabled = enabled;
    this.reportSink = enabled ? onReport : null;
    if (enabled && !wasProfilingEnabled) {
      this.clearSessionStats();
    }
    this.syncEnabled();
  }

  public recordRender(surface: string, renderCount: number, detail: DevMetricDetail = {}): void {
    if (!this.enabled) return;
    const normalizedDetail = normalizeDetail(detail);
    updateRenderStat(this.renderStats, surface, renderCount, normalizedDetail);
    if (this.profilingEnabled) {
      updateRenderStat(this.sessionRenderStats, surface, renderCount, normalizedDetail);
    }
    this.ensureFlushTimer();
  }

  public recordTiming(name: string, durationMs: number, detail: DevMetricDetail = {}): void {
    if (!this.enabled) return;
    this.recordSample('timing', name, durationMs, detail);
    const normalizedDetail = normalizeDetail(detail);
    updateTimingStat(this.timingStats, name, durationMs, normalizedDetail);
    if (this.profilingEnabled) {
      updateTimingStat(this.sessionTimingStats, name, durationMs, normalizedDetail);
    }
    this.ensureFlushTimer();
  }

  public recordEvent(name: string, detail: DevMetricDetail = {}): void {
    if (!this.enabled) return;
    this.recordSample('event', name, null, detail);
    const normalizedDetail = normalizeDetail(detail);
    updateEventStat(this.eventStats, name, normalizedDetail);
    if (this.profilingEnabled) {
      updateEventStat(this.sessionEventStats, name, normalizedDetail);
    }
    this.ensureFlushTimer();
  }

  public recordPayload(name: string, payload: unknown, detail: DevMetricDetail = {}): void {
    if (!this.enabled) return;
    scheduleIdleWork(() => {
      if (!this.enabled) return;
      const startedAt = performance.now();
      const bytes = approximateSerializedSizeBytes(payload);
      this.recordTiming(`${name}.serializeEstimate`, performance.now() - startedAt, detail);
      this.recordEvent(name, { ...detail, kb: Math.round(bytes / 1024) });
    });
  }

  public time<T>(name: string, operation: () => T, detail: DevMetricDetail = {}): T {
    if (!this.enabled) return operation();
    const startedAt = performance.now();
    try {
      return operation();
    } finally {
      this.recordTiming(name, performance.now() - startedAt, detail);
    }
  }

  public async timeAsync<T>(name: string, operation: () => Promise<T>, detail: DevMetricDetail = {}): Promise<T> {
    if (!this.enabled) return operation();
    const startedAt = performance.now();
    try {
      return await operation();
    } finally {
      this.recordTiming(name, performance.now() - startedAt, detail);
    }
  }

  public recordReactCommit(
    surface: string,
    phase: 'mount' | 'update' | 'nested-update',
    actualDurationMs: number,
    baseDurationMs: number,
    startTime: number,
    commitTime: number
  ): void {
    this.recordTiming(`react.commit.${surface}`, actualDurationMs, {
      phase,
      baseMs: roundMs(baseDurationMs),
      startTime: roundMs(startTime),
      commitTime: roundMs(commitTime)
    });
  }

  public report(): DevPerformanceReport {
    if (!this.enabled) return emptyPerformanceReport('disabled');
    if (this.hasStats() || (this.profilingEnabled && this.hasSessionStats())) return this.flush('manual');
    return this.lastReport ?? emptyPerformanceReport('manual');
  }

  private computeDevConsoleEnabled(): boolean {
    if (!isDevRendererRuntime()) return false;
    const queryValue = queryOptInValue();
    if (queryValue === '1' || queryValue === 'true') {
      writeOptIn(true);
      return true;
    }
    if (queryValue === '0' || queryValue === 'false') {
      writeOptIn(false);
      return false;
    }
    return readOptIn();
  }

  private installControls(): void {
    if (!isDevRendererRuntime()) return;
    const target = window as BealeDevPerformanceWindow;
    target.bealeDevPerformance = {
      enable: () => {
        writeOptIn(true);
        this.devConsoleEnabled = true;
        this.syncEnabled();
        this.announce();
        this.ensureFlushTimer();
        console.info('[Beale perf] Enabled. Reload to capture mount-time render probes.');
      },
      disable: () => {
        writeOptIn(false);
        this.devConsoleEnabled = false;
        this.syncEnabled();
        console.info('[Beale perf] Disabled.');
      },
      report: () => this.report(),
      status: () => ({ available: isDevRendererRuntime(), enabled: this.enabled, optIn: readOptIn() })
    };
  }

  private syncEnabled(): void {
    const nextEnabled = this.devConsoleEnabled || this.profilingEnabled;
    if (nextEnabled === this.enabled) {
      if (nextEnabled) {
        this.ensureFlushTimer();
        this.startRuntimeObservers();
      }
      return;
    }

    this.enabled = nextEnabled;
    if (this.enabled) {
      this.ensureFlushTimer();
      this.startRuntimeObservers();
      return;
    }

    this.clearFlushTimer();
    this.renderStats.clear();
    this.timingStats.clear();
    this.eventStats.clear();
    this.samples.length = 0;
    this.clearSessionStats();
    this.lastReport = null;
    this.stopRuntimeObservers();
  }

  private announce(): void {
    if (this.announced) return;
    this.announced = true;
    console.info('[Beale perf] Developer instrumentation enabled. Use window.bealeDevPerformance.report() for a returned report object.');
  }

  private ensureFlushTimer(): void {
    if (!this.enabled || this.flushTimer !== null || typeof window === 'undefined') return;
    this.flushTimer = window.setInterval(() => this.flush('interval'), DEV_INSTRUMENTATION_FLUSH_MS);
  }

  private clearFlushTimer(): void {
    if (this.flushTimer === null || typeof window === 'undefined') return;
    window.clearInterval(this.flushTimer);
    this.flushTimer = null;
  }

  private hasStats(): boolean {
    return this.renderStats.size > 0 || this.timingStats.size > 0 || this.eventStats.size > 0;
  }

  private hasSessionStats(): boolean {
    return this.sessionRenderStats.size > 0 || this.sessionTimingStats.size > 0 || this.sessionEventStats.size > 0;
  }

  private flush(reason: 'interval' | 'manual'): DevPerformanceReport {
    if (!this.enabled) return emptyPerformanceReport('disabled');
    if (!this.hasStats() && !(reason === 'manual' && this.profilingEnabled && this.hasSessionStats())) {
      return this.lastReport ?? emptyPerformanceReport(reason);
    }

    const report = this.buildReport(reason);
    if (this.devConsoleEnabled) {
      console.groupCollapsed(`[Beale perf] ${reason} ${new Date().toLocaleTimeString()}`);
      if (report.renders.length > 0) {
        console.table(
          report.renders.map((row) => ({
            ...row,
            detail: formatDetail(row.detail)
          }))
        );
      }
      if (report.timings.length > 0) {
        console.table(
          report.timings.map((row) => ({
            ...row,
            detail: formatDetail(row.detail)
          }))
        );
      }
      if (report.events.length > 0) {
        console.table(
          report.events.map((row) => ({
            ...row,
            detail: formatDetail(row.detail)
          }))
        );
      }
      console.groupEnd();
    }

    this.renderStats.clear();
    this.timingStats.clear();
    this.eventStats.clear();
    this.samples.length = 0;
    this.lastReport = report;
    this.reportSink?.(report);
    return report;
  }

  private buildReport(reason: 'interval' | 'manual'): ProfilingReport {
    const useSessionStats = reason === 'manual' && this.profilingEnabled;
    const renderStats = useSessionStats ? this.sessionRenderStats : this.renderStats;
    const timingStats = useSessionStats ? this.sessionTimingStats : this.timingStats;
    const eventStats = useSessionStats ? this.sessionEventStats : this.eventStats;
    const pendingSamples = useSessionStats ? this.sessionSamples : this.samples;
    const renders: ProfilingRenderReportRow[] = Array.from(renderStats.entries()).map(([surface, stat]) => ({
      surface,
      renders: stat.count,
      lastRender: stat.lastRender,
      detail: stat.detail
    }));
    const timings: ProfilingTimingReportRow[] = Array.from(timingStats.entries()).map(([name, stat]) => ({
      name,
      count: stat.count,
      avgMs: roundMs(stat.totalMs / stat.count),
      maxMs: roundMs(stat.maxMs),
      lastMs: roundMs(stat.lastMs),
      detail: stat.detail
    }));
    const events: ProfilingEventReportRow[] = Array.from(eventStats.entries()).map(([name, stat]) => ({
      name,
      count: stat.count,
      detail: stat.detail
    }));
    const samples: ProfilingSample[] = pendingSamples.map((sample) => ({
      at: new Date(performance.timeOrigin + sample.atMs).toISOString(),
      category: sample.category,
      name: sample.name,
      durationMs: sample.durationMs === null ? null : roundMs(sample.durationMs),
      detail: sample.detail
    }));
    return {
      enabled: true,
      empty: renders.length === 0 && timings.length === 0 && events.length === 0,
      reason,
      generatedAt: new Date().toISOString(),
      renders,
      timings,
      events,
      samples
    };
  }

  private recordSample(
    category: ProfilingSample['category'],
    name: string,
    durationMs: number | null,
    detail: DevMetricDetail
  ): void {
    const sample = {
      atMs: performance.now(),
      category,
      name,
      durationMs,
      detail: normalizeDetail(detail)
    } satisfies PendingProfilingSample;
    appendBoundedSample(this.samples, sample, MAX_PROFILE_SAMPLES);
    if (this.profilingEnabled) {
      appendBoundedSample(this.sessionSamples, sample, MAX_PROFILE_SESSION_SAMPLES);
    }
  }

  private clearSessionStats(): void {
    this.sessionRenderStats.clear();
    this.sessionTimingStats.clear();
    this.sessionEventStats.clear();
    this.sessionSamples.length = 0;
  }

  private startRuntimeObservers(): void {
    if (typeof window === 'undefined') return;
    if (!this.longTaskObserver && typeof PerformanceObserver !== 'undefined') {
      try {
        this.longTaskObserver = new PerformanceObserver((list) => {
          for (const entry of list.getEntries()) {
            this.recordTiming('renderer.longTask', entry.duration, {
              entry: entry.name || 'self',
              startTime: roundMs(entry.startTime)
            });
          }
        });
        this.longTaskObserver.observe({ type: 'longtask', buffered: false });
      } catch {
        this.longTaskObserver = null;
      }
    }

    if (this.frameRequestId === null) {
      let previousFrameAt = performance.now();
      const measureFrame = (now: number): void => {
        if (!this.enabled) {
          this.frameRequestId = null;
          return;
        }
        const gapMs = now - previousFrameAt;
        previousFrameAt = now;
        if (gapMs >= FRAME_GAP_THRESHOLD_MS) {
          this.recordTiming('renderer.frameGap', gapMs, {
            missedFrames: Math.max(1, Math.round(gapMs / 16.67) - 1),
            visibility: document.visibilityState
          });
        }
        this.frameRequestId = window.requestAnimationFrame(measureFrame);
      };
      this.frameRequestId = window.requestAnimationFrame(measureFrame);
    }

    if (this.eventLoopTimer === null) {
      let expectedAt = performance.now() + EVENT_LOOP_SAMPLE_MS;
      this.eventLoopTimer = window.setInterval(() => {
        const now = performance.now();
        const lagMs = Math.max(0, now - expectedAt);
        expectedAt = now + EVENT_LOOP_SAMPLE_MS;
        if (lagMs >= EVENT_LOOP_LAG_THRESHOLD_MS) {
          this.recordTiming('renderer.eventLoopLag', lagMs, {
            visibility: document.visibilityState
          });
        }
      }, EVENT_LOOP_SAMPLE_MS);
    }
  }

  private stopRuntimeObservers(): void {
    this.longTaskObserver?.disconnect();
    this.longTaskObserver = null;
    if (typeof window !== 'undefined' && this.frameRequestId !== null) {
      window.cancelAnimationFrame(this.frameRequestId);
    }
    this.frameRequestId = null;
    if (typeof window !== 'undefined' && this.eventLoopTimer !== null) {
      window.clearInterval(this.eventLoopTimer);
    }
    this.eventLoopTimer = null;
  }
}

export const devInstrumentation = new RendererDevInstrumentation();

export function useDevRenderProbe(surface: string, detail?: DevMetricDetail | (() => DevMetricDetail)): void {
  const renderCountRef = useRef(0);
  renderCountRef.current += 1;

  useEffect(() => {
    if (!devInstrumentation.isEnabled()) return;
    devInstrumentation.recordRender(surface, renderCountRef.current, typeof detail === 'function' ? detail() : detail);
  });
}

export function recordNextFrameTiming(name: string, startedAt: number, detail: DevMetricDetail = {}): void {
  if (!devInstrumentation.isEnabled() || typeof window === 'undefined') return;
  window.requestAnimationFrame(() => {
    devInstrumentation.recordTiming(name, performance.now() - startedAt, detail);
  });
}

export function useDevInputLatencyProbe(): void {
  useEffect(() => {
    if (typeof window === 'undefined') return undefined;
    let pending = false;
    let pointerMovePending = false;
    let lastPointerMoveSampleAt = 0;

    const handleInputSignal = (event: Event): void => {
      if (!devInstrumentation.isEnabled()) return;
      if (pending) return;
      pending = true;
      const startedAt = performance.now();
      window.requestAnimationFrame(() => {
        pending = false;
        devInstrumentation.recordTiming('input.nextFrameLatency', performance.now() - startedAt, {
          event: event.type,
          target: eventTargetLabel(event.target)
        });
      });
    };

    const handlePointerMoveSignal = (event: PointerEvent): void => {
      if (!devInstrumentation.isEnabled()) return;
      const now = performance.now();
      if (pointerMovePending || now - lastPointerMoveSampleAt < POINTER_MOVE_LATENCY_SAMPLE_MS) return;
      pointerMovePending = true;
      lastPointerMoveSampleAt = now;
      window.requestAnimationFrame(() => {
        pointerMovePending = false;
        devInstrumentation.recordTiming('input.pointerMove.nextFrameLatency', performance.now() - now, {
          target: eventTargetLabel(event.target)
        });
      });
    };

    window.addEventListener('beforeinput', handleInputSignal, true);
    window.addEventListener('keydown', handleInputSignal, true);
    window.addEventListener('pointerdown', handleInputSignal, true);
    window.addEventListener('pointermove', handlePointerMoveSignal, true);
    return () => {
      window.removeEventListener('beforeinput', handleInputSignal, true);
      window.removeEventListener('keydown', handleInputSignal, true);
      window.removeEventListener('pointerdown', handleInputSignal, true);
      window.removeEventListener('pointermove', handlePointerMoveSignal, true);
    };
  }, []);
}

export function approximateSerializedSizeBytes(value: unknown): number {
  const pending: unknown[] = [value];
  const seen = new WeakSet<object>();
  let bytes = 0;
  let visited = 0;

  while (pending.length > 0 && visited < PAYLOAD_SIZE_ESTIMATE_VALUE_LIMIT) {
    const current = pending.pop();
    visited += 1;
    if (current === null) {
      bytes += 4;
    } else if (typeof current === 'string') {
      bytes += current.length + 2;
    } else if (typeof current === 'number' || typeof current === 'boolean') {
      bytes += String(current).length;
    } else if (Array.isArray(current)) {
      if (seen.has(current)) continue;
      seen.add(current);
      bytes += current.length > 0 ? current.length + 1 : 2;
      for (let index = current.length - 1; index >= 0 && pending.length < PAYLOAD_SIZE_ESTIMATE_VALUE_LIMIT; index -= 1) {
        pending.push(current[index]);
      }
    } else if (typeof current === 'object') {
      if (seen.has(current)) continue;
      seen.add(current);
      let propertyCount = 0;
      for (const key in current) {
        if (!Object.prototype.hasOwnProperty.call(current, key)) continue;
        bytes += key.length + 2;
        pending.push((current as Record<string, unknown>)[key]);
        propertyCount += 1;
        if (pending.length >= PAYLOAD_SIZE_ESTIMATE_VALUE_LIMIT) break;
      }
      bytes += propertyCount > 0 ? propertyCount + 1 : 2;
    }
  }

  return bytes;
}

function scheduleIdleWork(operation: () => void): void {
  if (typeof window === 'undefined') return;
  if (typeof window.requestIdleCallback === 'function') {
    window.requestIdleCallback(operation, { timeout: 1_000 });
    return;
  }
  window.setTimeout(operation, 0);
}

function isDevRendererRuntime(): boolean {
  if (typeof window === 'undefined') return false;
  const { hostname, protocol } = window.location;
  return protocol === 'http:' && (hostname === 'localhost' || hostname === '127.0.0.1' || hostname === '[::1]');
}

function queryOptInValue(): string | null {
  try {
    return new URLSearchParams(window.location.search).get(DEV_INSTRUMENTATION_QUERY_KEY);
  } catch {
    return null;
  }
}

function readOptIn(): boolean {
  try {
    return window.localStorage.getItem(DEV_INSTRUMENTATION_STORAGE_KEY) === '1';
  } catch {
    return false;
  }
}

function writeOptIn(enabled: boolean): void {
  try {
    if (enabled) {
      window.localStorage.setItem(DEV_INSTRUMENTATION_STORAGE_KEY, '1');
    } else {
      window.localStorage.removeItem(DEV_INSTRUMENTATION_STORAGE_KEY);
    }
  } catch {
    // Ignore storage failures in dev-only instrumentation.
  }
}

function emptyPerformanceReport(reason: 'manual' | 'interval' | 'disabled'): DevPerformanceReport {
  return {
    enabled: reason !== 'disabled',
    empty: true,
    reason,
    generatedAt: new Date().toISOString(),
    renders: [],
    timings: [],
    events: [],
    samples: []
  };
}

function normalizeDetail(detail: DevMetricDetail | undefined): DevMetricDetail {
  if (!detail) return {};
  return Object.fromEntries(Object.entries(detail).filter(([, value]) => value !== undefined));
}

function updateRenderStat(
  stats: Map<string, RenderStat>,
  surface: string,
  renderCount: number,
  detail: DevMetricDetail
): void {
  const existing = stats.get(surface);
  if (existing) {
    existing.count += 1;
    existing.lastRender = renderCount;
    existing.detail = detail;
    return;
  }
  stats.set(surface, { count: 1, lastRender: renderCount, detail });
}

function updateTimingStat(
  stats: Map<string, TimingStat>,
  name: string,
  durationMs: number,
  detail: DevMetricDetail
): void {
  const existing = stats.get(name);
  if (existing) {
    existing.count += 1;
    existing.totalMs += durationMs;
    existing.maxMs = Math.max(existing.maxMs, durationMs);
    existing.lastMs = durationMs;
    existing.detail = detail;
    return;
  }
  stats.set(name, { count: 1, totalMs: durationMs, maxMs: durationMs, lastMs: durationMs, detail });
}

function updateEventStat(stats: Map<string, EventStat>, name: string, detail: DevMetricDetail): void {
  const existing = stats.get(name);
  if (existing) {
    existing.count += 1;
    existing.detail = detail;
    return;
  }
  stats.set(name, { count: 1, detail });
}

function appendBoundedSample(
  samples: PendingProfilingSample[],
  sample: PendingProfilingSample,
  limit: number
): void {
  samples.push(sample);
  if (samples.length > limit) {
    samples.splice(0, samples.length - limit);
  }
}

function formatDetail(detail: DevMetricDetail): string {
  const entries = Object.entries(detail).filter(([, value]) => value !== undefined);
  if (entries.length === 0) return '';
  return entries.map(([key, value]) => `${key}=${String(value)}`).join(' ');
}

function roundMs(value: number): number {
  return Math.round(value * 10) / 10;
}

function eventTargetLabel(target: EventTarget | null): string {
  if (!(target instanceof Element)) return 'unknown';
  const role = target.getAttribute('role');
  const type = target instanceof HTMLInputElement || target instanceof HTMLButtonElement ? target.type : null;
  return [target.tagName.toLowerCase(), type, role].filter(Boolean).join(':');
}
