import type {
  AppServerMemoryNodeSummary,
  AppServerReportSummary,
  AppServerRunbookSummary,
  ResearchProfileMemoryType,
  RunRow,
  SessionActivityInterval,
  SessionRunActivity
} from '@shared/types';

export const WORKSPACE_TIMELINE_WINDOW_MS = 4 * 60 * 60 * 1_000;
export type WorkspaceTimelineResult = 'natural_end' | 'unexpected_error' | 'safeguard_error';

export interface WorkspaceTimelineSegment {
  id: string;
  startedAt: string;
  endedAt: string | null;
  leftPercent: number;
  widthPercent: number;
}

export interface WorkspaceTimelineMemoryMarker {
  id: string;
  type: string;
  status: string;
  title: string;
  createdAt: string;
  leftPercent: number;
}

export interface WorkspaceTimelineArtifactRevisionMarker {
  id: string;
  artifactId: string;
  title: string;
  revision: number;
  createdAt: string;
  leftPercent: number;
}

export interface WorkspaceTimelineRow {
  runId: string;
  sessionRunId: string;
  title: string;
  status: SessionRunActivity['status'];
  result: WorkspaceTimelineResult | null;
  totalDurationMs: number;
  windowDurationMs: number;
  latestActivityAtMs: number;
  segments: WorkspaceTimelineSegment[];
  memoryMarkers: WorkspaceTimelineMemoryMarker[];
  runbookRevisionMarkers: WorkspaceTimelineArtifactRevisionMarker[];
  reportRevisionMarkers: WorkspaceTimelineArtifactRevisionMarker[];
}

export interface WorkspaceTimelineModel {
  rows: WorkspaceTimelineRow[];
  windowDurationMs: number;
}

export interface SessionTimelineProjection {
  totalDurationMs: number;
  segments: WorkspaceTimelineSegment[];
  memoryMarkers: WorkspaceTimelineMemoryMarker[];
  runbookRevisionMarkers: WorkspaceTimelineArtifactRevisionMarker[];
  reportRevisionMarkers: WorkspaceTimelineArtifactRevisionMarker[];
}

interface WorkspaceActivitySpan {
  startMs: number;
  endMs: number;
  startOffsetMs: number;
  endOffsetMs: number;
}

interface WorkspaceActivityClock {
  spans: WorkspaceActivitySpan[];
  totalDurationMs: number;
  windowDurationMs: number;
  windowStartOffsetMs: number;
}

interface SessionRunTimelineInput {
  row: RunRow;
  sessionRun: SessionRunActivity;
  intervals: SessionActivityInterval[];
  startedAtMs: number;
}

interface WorkspaceArtifactRevisionInput {
  artifact: AppServerRunbookSummary | AppServerReportSummary;
  revision: AppServerRunbookSummary['revisions'][number];
}

export function buildWorkspaceTimeline(
  runs: readonly RunRow[],
  memories: readonly AppServerMemoryNodeSummary[],
  runbooks: readonly AppServerRunbookSummary[],
  reports: readonly AppServerReportSummary[],
  memoryTypes: readonly ResearchProfileMemoryType[],
  nowMs: number
): WorkspaceTimelineModel {
  const sessionRunInputs: SessionRunTimelineInput[] = runs.flatMap((row) => row.sessionRuns.map((sessionRun) => {
    const intervals = normalizedIntervals(sessionRun.activityIntervals, nowMs);
    return {
      row,
      sessionRun,
      intervals,
      startedAtMs: Math.min(...intervals.map((interval) => Date.parse(interval.startedAt)))
    };
  }));
  const activityClock = buildWorkspaceActivityClock(
    sessionRunInputs.flatMap((input) => input.intervals),
    nowMs
  );
  const sessionRunsByRun = new Map<string, SessionRunTimelineInput[]>();
  for (const input of sessionRunInputs) {
    const candidates = sessionRunsByRun.get(input.row.run.id) ?? [];
    candidates.push(input);
    sessionRunsByRun.set(input.row.run.id, candidates);
  }
  for (const candidates of sessionRunsByRun.values()) {
    candidates.sort((left, right) => left.startedAtMs - right.startedAtMs || left.sessionRun.id.localeCompare(right.sessionRun.id));
  }
  const memoriesByRun = new Map<string, AppServerMemoryNodeSummary[]>();
  for (const memory of memories) {
    for (const runId of memory.sessionIds) {
      const candidates = memoriesByRun.get(runId) ?? [];
      candidates.push(memory);
      memoriesByRun.set(runId, candidates);
    }
  }
  const runbookRevisionsByRun = artifactRevisionsByRun(runbooks);
  const reportRevisionsByRun = artifactRevisionsByRun(reports);
  const memoryTypeById = new Map<string, ResearchProfileMemoryType>();
  for (const type of memoryTypes) {
    memoryTypeById.set(type.id, type);
    for (const alias of type.aliases ?? []) memoryTypeById.set(alias, type);
  }

  const rows = sessionRunInputs.flatMap(({ row, sessionRun, intervals }) => {
    const segments = intervals.flatMap((interval) => {
      const startMs = Date.parse(interval.startedAt);
      const endMs = Math.min(interval.endedAt ? Date.parse(interval.endedAt) : nowMs, nowMs);
      if (startMs > nowMs) return [];
      const startOffsetMs = activityOffsetAt(activityClock, startMs);
      const endOffsetMs = activityOffsetAt(activityClock, endMs);
      const clippedStartOffsetMs = Math.max(startOffsetMs, activityClock.windowStartOffsetMs);
      const clippedEndOffsetMs = Math.min(endOffsetMs, activityClock.totalDurationMs);
      if (clippedEndOffsetMs < clippedStartOffsetMs || endOffsetMs < activityClock.windowStartOffsetMs) return [];
      return [{
        id: interval.id,
        startedAt: interval.startedAt,
        endedAt: interval.endedAt,
        leftPercent: percentOfActivityWindow(clippedStartOffsetMs, activityClock),
        widthPercent: percentOfActivityDuration(clippedEndOffsetMs - clippedStartOffsetMs, activityClock)
      }];
    });
    const memoryMarkers = (memoriesByRun.get(row.run.id) ?? []).flatMap((memory) => {
      if (activityClock.spans.length === 0) return [];
      const createdAtMs = Date.parse(memory.createdAt);
      if (!Number.isFinite(createdAtMs) || createdAtMs > nowMs) return [];
      if (sessionRunAt(sessionRunsByRun.get(row.run.id) ?? [], createdAtMs)?.sessionRun.id !== sessionRun.id) return [];
      const activityOffsetMs = activityOffsetAt(activityClock, createdAtMs);
      if (activityOffsetMs < activityClock.windowStartOffsetMs) return [];
      const definition = memoryTypeById.get(memory.type);
      return [{
        id: memory.id,
        type: definition?.id ?? memory.type,
        status: memory.status,
        title: memory.title,
        createdAt: memory.createdAt,
        leftPercent: percentOfActivityWindow(activityOffsetMs, activityClock)
      }];
    });
    const runbookRevisionMarkers = artifactRevisionMarkers(
      runbookRevisionsByRun.get(row.run.id) ?? [],
      sessionRun.id,
      sessionRunsByRun.get(row.run.id) ?? [],
      activityClock,
      nowMs
    );
    const reportRevisionMarkers = artifactRevisionMarkers(
      reportRevisionsByRun.get(row.run.id) ?? [],
      sessionRun.id,
      sessionRunsByRun.get(row.run.id) ?? [],
      activityClock,
      nowMs
    );
    if (
      segments.length === 0
      && memoryMarkers.length === 0
      && runbookRevisionMarkers.length === 0
      && reportRevisionMarkers.length === 0
    ) return [];

    const totalDurationMs = intervals.reduce((total, interval) => {
      const startMs = Date.parse(interval.startedAt);
      const endMs = Math.min(interval.endedAt ? Date.parse(interval.endedAt) : nowMs, nowMs);
      return total + Math.max(0, endMs - startMs);
    }, 0);
    const windowDurationMs = intervals.reduce((total, interval) => {
      const startOffsetMs = Math.max(
        activityOffsetAt(activityClock, Date.parse(interval.startedAt)),
        activityClock.windowStartOffsetMs
      );
      const endOffsetMs = Math.min(
        activityOffsetAt(activityClock, Math.min(interval.endedAt ? Date.parse(interval.endedAt) : nowMs, nowMs)),
        activityClock.totalDurationMs
      );
      return total + Math.max(0, endOffsetMs - startOffsetMs);
    }, 0);
    const latestActivityAtMs = Math.max(
      ...intervals.map((interval) => interval.endedAt ? Date.parse(interval.endedAt) : nowMs),
      ...memoryMarkers.map((marker) => Date.parse(marker.createdAt)),
      ...runbookRevisionMarkers.map((marker) => Date.parse(marker.createdAt)),
      ...reportRevisionMarkers.map((marker) => Date.parse(marker.createdAt))
    );
    return [{
      runId: row.run.id,
      sessionRunId: sessionRun.id,
      title: row.run.title.trim() || 'Untitled session',
      status: sessionRun.status,
      result: workspaceTimelineResult(sessionRun),
      totalDurationMs,
      windowDurationMs,
      latestActivityAtMs,
      segments,
      memoryMarkers,
      runbookRevisionMarkers,
      reportRevisionMarkers
    }];
  }).sort((left, right) => right.latestActivityAtMs - left.latestActivityAtMs || left.sessionRunId.localeCompare(right.sessionRunId));
  return { rows, windowDurationMs: activityClock.windowDurationMs };
}

export function buildSessionTimelineProjection(
  row: RunRow,
  memories: readonly AppServerMemoryNodeSummary[],
  runbooks: readonly AppServerRunbookSummary[],
  reports: readonly AppServerReportSummary[],
  memoryTypes: readonly ResearchProfileMemoryType[],
  nowMs: number
): SessionTimelineProjection {
  const intervals = normalizedIntervals(row.sessionRuns.flatMap((sessionRun) => sessionRun.activityIntervals), nowMs);
  const activityClock = buildWorkspaceActivityClock(intervals, nowMs, Number.POSITIVE_INFINITY);
  const memoryTypeById = new Map<string, ResearchProfileMemoryType>();
  for (const type of memoryTypes) {
    memoryTypeById.set(type.id, type);
    for (const alias of type.aliases ?? []) memoryTypeById.set(alias, type);
  }
  const position = (createdAt: string): number | null => {
    if (activityClock.spans.length === 0) return null;
    const createdAtMs = Date.parse(createdAt);
    if (!Number.isFinite(createdAtMs) || createdAtMs > nowMs) return null;
    return percentOfActivityWindow(activityOffsetAt(activityClock, createdAtMs), activityClock);
  };
  const artifactMarkers = (
    artifacts: readonly (AppServerRunbookSummary | AppServerReportSummary)[]
  ): WorkspaceTimelineArtifactRevisionMarker[] => artifacts.flatMap((artifact) => (artifact.revisions ?? []).flatMap((revision) => {
    if (revision.sessionId !== row.run.id) return [];
    const leftPercent = position(revision.createdAt);
    return leftPercent === null ? [] : [{
      id: `${artifact.id}:${revision.revision}`,
      artifactId: artifact.id,
      title: artifact.title,
      revision: revision.revision,
      createdAt: revision.createdAt,
      leftPercent
    }];
  }));

  return {
    totalDurationMs: activityClock.totalDurationMs,
    segments: intervals.flatMap((interval) => {
      const startMs = Date.parse(interval.startedAt);
      const endMs = Math.min(interval.endedAt ? Date.parse(interval.endedAt) : nowMs, nowMs);
      if (startMs > nowMs) return [];
      const startOffsetMs = activityOffsetAt(activityClock, startMs);
      const endOffsetMs = activityOffsetAt(activityClock, endMs);
      return [{
        id: interval.id,
        startedAt: interval.startedAt,
        endedAt: interval.endedAt,
        leftPercent: percentOfActivityWindow(startOffsetMs, activityClock),
        widthPercent: percentOfActivityDuration(Math.max(0, endOffsetMs - startOffsetMs), activityClock)
      }];
    }),
    memoryMarkers: memories.flatMap((memory) => {
      if (!memory.sessionIds.includes(row.run.id)) return [];
      const leftPercent = position(memory.createdAt);
      if (leftPercent === null) return [];
      const definition = memoryTypeById.get(memory.type);
      return [{
        id: memory.id,
        type: definition?.id ?? memory.type,
        status: memory.status,
        title: memory.title,
        createdAt: memory.createdAt,
        leftPercent
      }];
    }),
    runbookRevisionMarkers: artifactMarkers(runbooks),
    reportRevisionMarkers: artifactMarkers(reports)
  };
}

export function workspaceTimelineResult(sessionRun: SessionRunActivity): WorkspaceTimelineResult | null {
  if (sessionRun.status === 'failed') return sessionRun.terminationCause === 'safeguard' ? 'safeguard_error' : 'unexpected_error';
  if (sessionRun.status === 'paused' && sessionRun.terminationCause === 'workspace_recovery') return 'unexpected_error';
  if (sessionRun.status === 'completed' || sessionRun.status === 'blocked' || sessionRun.status === 'stopped') return 'natural_end';
  return null;
}

function artifactRevisionMarkers(
  revisions: readonly WorkspaceArtifactRevisionInput[],
  sessionRunId: string,
  sessionRuns: readonly SessionRunTimelineInput[],
  activityClock: WorkspaceActivityClock,
  nowMs: number
): WorkspaceTimelineArtifactRevisionMarker[] {
  if (activityClock.spans.length === 0) return [];
  return revisions.flatMap(({ artifact, revision }) => {
    const createdAtMs = Date.parse(revision.createdAt);
    if (!Number.isFinite(createdAtMs) || createdAtMs > nowMs) return [];
    if (sessionRunAt(sessionRuns, createdAtMs)?.sessionRun.id !== sessionRunId) return [];
    const activityOffsetMs = activityOffsetAt(activityClock, createdAtMs);
    if (activityOffsetMs < activityClock.windowStartOffsetMs) return [];
    return [{
      id: `${artifact.id}:${revision.revision}`,
      artifactId: artifact.id,
      title: artifact.title,
      revision: revision.revision,
      createdAt: revision.createdAt,
      leftPercent: percentOfActivityWindow(activityOffsetMs, activityClock)
    }];
  });
}

function artifactRevisionsByRun(
  artifacts: readonly (AppServerRunbookSummary | AppServerReportSummary)[]
): Map<string, WorkspaceArtifactRevisionInput[]> {
  const grouped = new Map<string, WorkspaceArtifactRevisionInput[]>();
  for (const artifact of artifacts) {
    for (const revision of artifact.revisions) {
      if (!revision.sessionId) continue;
      const candidates = grouped.get(revision.sessionId) ?? [];
      candidates.push({ artifact, revision });
      grouped.set(revision.sessionId, candidates);
    }
  }
  return grouped;
}

function sessionRunAt(sessionRuns: readonly SessionRunTimelineInput[], timestampMs: number): SessionRunTimelineInput | null {
  let owner = sessionRuns[0] ?? null;
  for (const sessionRun of sessionRuns) {
    if (sessionRun.startedAtMs > timestampMs) break;
    owner = sessionRun;
  }
  return owner;
}

export function formatWorkspaceTimelineDuration(durationMs: number): string {
  const totalMinutes = Math.max(0, Math.floor(durationMs / 60_000));
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  if (hours === 0) return `${minutes}m`;
  if (minutes === 0) return `${hours}h`;
  return `${hours}h ${minutes}m`;
}

function normalizedIntervals(
  intervals: readonly SessionActivityInterval[],
  nowMs: number
): SessionActivityInterval[] {
  return intervals.filter((interval) => {
    const startMs = Date.parse(interval.startedAt);
    const endMs = interval.endedAt ? Date.parse(interval.endedAt) : nowMs;
    return Number.isFinite(startMs) && Number.isFinite(endMs) && endMs >= startMs;
  });
}

function buildWorkspaceActivityClock(
  intervals: readonly SessionActivityInterval[],
  nowMs: number,
  windowLimitMs = WORKSPACE_TIMELINE_WINDOW_MS
): WorkspaceActivityClock {
  const merged: Array<{ startMs: number; endMs: number }> = [];
  const bounds = intervals
    .map((interval) => ({
      startMs: Date.parse(interval.startedAt),
      endMs: Math.min(interval.endedAt ? Date.parse(interval.endedAt) : nowMs, nowMs)
    }))
    .filter((interval) => interval.startMs <= nowMs && interval.endMs >= interval.startMs)
    .sort((left, right) => left.startMs - right.startMs || left.endMs - right.endMs);

  for (const interval of bounds) {
    const previous = merged.at(-1);
    if (previous && interval.startMs <= previous.endMs) {
      previous.endMs = Math.max(previous.endMs, interval.endMs);
    } else {
      merged.push({ ...interval });
    }
  }

  let totalDurationMs = 0;
  const spans = merged.map((span) => {
    const startOffsetMs = totalDurationMs;
    totalDurationMs += span.endMs - span.startMs;
    return { ...span, startOffsetMs, endOffsetMs: totalDurationMs };
  });
  const windowDurationMs = Math.min(totalDurationMs, windowLimitMs);
  return {
    spans,
    totalDurationMs,
    windowDurationMs,
    windowStartOffsetMs: totalDurationMs - windowDurationMs
  };
}

function activityOffsetAt(clock: WorkspaceActivityClock, timestampMs: number): number {
  for (const span of clock.spans) {
    if (timestampMs <= span.startMs) return span.startOffsetMs;
    if (timestampMs < span.endMs) return span.startOffsetMs + timestampMs - span.startMs;
  }
  return clock.totalDurationMs;
}

function percentOfActivityWindow(activityOffsetMs: number, clock: WorkspaceActivityClock): number {
  if (clock.windowDurationMs <= 0) return 100;
  return Math.max(
    0,
    Math.min(100, ((activityOffsetMs - clock.windowStartOffsetMs) / clock.windowDurationMs) * 100)
  );
}

function percentOfActivityDuration(durationMs: number, clock: WorkspaceActivityClock): number {
  if (clock.windowDurationMs <= 0) return 0;
  return Math.max(0, (durationMs / clock.windowDurationMs) * 100);
}
