import type { ProfilingReport, ProfilingSample, ProfilingTimingReportRow } from '@shared/types';

export interface ProfilingLagFinding {
  id: string;
  severity: 'high' | 'moderate';
  title: string;
  evidence: string;
  recommendation: string;
  score: number;
}

export function profilingLagFindings(report: ProfilingReport | null): ProfilingLagFinding[] {
  if (!report) return [];
  const findings: ProfilingLagFinding[] = [];
  const longTask = timing(report, 'renderer.longTask');
  const eventLoopLag = timing(report, 'renderer.eventLoopLag');
  const frameGap = timing(report, 'renderer.frameGap');
  const worstStallMs = Math.max(longTask?.maxMs ?? 0, eventLoopLag?.maxMs ?? 0, frameGap?.maxMs ?? 0);
  if (worstStallMs >= 34) {
    const samples = report.samples ?? [];
    const correlated = correlatedWork(samples, 'renderer.longTask')
      ?? correlatedWork(samples, 'renderer.eventLoopLag')
      ?? correlatedWork(samples, 'renderer.frameGap');
    findings.push({
      id: 'renderer-stall',
      severity: worstStallMs >= 100 ? 'high' : 'moderate',
      title: 'Renderer main-thread stalls',
      evidence: `${formatMs(worstStallMs)} worst stall${correlated ? ` near ${correlated.name} (${formatMs(correlated.durationMs ?? 0)})` : ''}.`,
      recommendation: 'Reduce or defer the correlated synchronous projection, render, or layout work.',
      score: worstStallMs
    });
  }

  const reactCommit = timing(report, 'react.commit.app');
  if (reactCommit && reactCommit.maxMs >= 16.7) {
    findings.push({
      id: 'react-commit',
      severity: reactCommit.maxMs >= 50 ? 'high' : 'moderate',
      title: 'Expensive React commits',
      evidence: `${reactCommit.count} commits averaged ${formatMs(reactCommit.avgMs)} and peaked at ${formatMs(reactCommit.maxMs)}.`,
      recommendation: 'Split frequently changing session state from stable workspace surfaces and memoize the highest-rendering subtree.',
      score: reactCommit.maxMs
    });
  }

  const snapshotApply = maxTiming(report.timings.filter((row) => (
    row.name === 'ipc.snapshot.event.apply.nextFrameLatency'
      || row.name === 'ipc.workspaceRegistry.event.apply.nextFrameLatency'
      || row.name === 'trace.runDetail.apply.nextFrameLatency'
  )));
  if (snapshotApply && snapshotApply.maxMs >= 34) {
    findings.push({
      id: 'state-apply-latency',
      severity: snapshotApply.maxMs >= 80 ? 'high' : 'moderate',
      title: 'Background updates miss frame deadlines',
      evidence: `${snapshotApply.name} peaked at ${formatMs(snapshotApply.maxMs)}.`,
      recommendation: 'Coalesce snapshot and run-detail updates, then isolate their state from stable workspace surfaces.',
      score: snapshotApply.maxMs
    });
  }

  const projectionTimings = report.timings.filter((row) => (
    row.name.startsWith('trace.buildDisplayEvents')
      || row.name.startsWith('trace.mergeRunDetailUpdate')
      || row.name.startsWith('commentary.build')
      || row.name.startsWith('trace.markup')
  ));
  const worstProjection = maxTiming(projectionTimings);
  if (worstProjection && worstProjection.maxMs >= 8) {
    findings.push({
      id: 'session-projection',
      severity: worstProjection.maxMs >= 32 ? 'high' : 'moderate',
      title: 'Session projection work is on the render path',
      evidence: `${worstProjection.name} averaged ${formatMs(worstProjection.avgMs)} and peaked at ${formatMs(worstProjection.maxMs)}.`,
      recommendation: 'Incrementally update the commentary view model instead of rebuilding it from the complete session on each poll.',
      score: worstProjection.maxMs
    });
  }

  const hottestRender = report.renders.slice().sort((left, right) => right.renders - left.renders)[0];
  if (hottestRender && hottestRender.renders >= 4) {
    findings.push({
      id: 'render-frequency',
      severity: hottestRender.renders >= 10 ? 'high' : 'moderate',
      title: 'Frequent component invalidation',
      evidence: `${hottestRender.surface} rendered ${hottestRender.renders} times in the latest profiling window.`,
      recommendation: 'Keep polling cursors and usage telemetry from invalidating stable session and sidenav props.',
      score: hottestRender.renders * 3
    });
  }

  const inputLatency = timing(report, 'input.nextFrameLatency');
  if (inputLatency && inputLatency.maxMs >= 50) {
    findings.push({
      id: 'input-latency',
      severity: inputLatency.maxMs >= 100 ? 'high' : 'moderate',
      title: 'Visible interaction delay',
      evidence: `Input-to-frame latency peaked at ${formatMs(inputLatency.maxMs)}.`,
      recommendation: 'Use the correlated sample timeline to move the blocking work out of the urgent input lane.',
      score: inputLatency.maxMs
    });
  }

  return findings.sort((left, right) => right.score - left.score).slice(0, 6);
}

function timing(report: ProfilingReport, name: string): ProfilingTimingReportRow | undefined {
  return report.timings.find((row) => row.name === name);
}

function maxTiming(rows: readonly ProfilingTimingReportRow[]): ProfilingTimingReportRow | null {
  return rows.reduce<ProfilingTimingReportRow | null>((current, row) => (
    !current || row.maxMs > current.maxMs ? row : current
  ), null);
}

function correlatedWork(samples: readonly ProfilingSample[], stallName: string): ProfilingSample | null {
  const stall = samples
    .filter((sample) => sample.name === stallName && sample.durationMs !== null)
    .sort((left, right) => (right.durationMs ?? 0) - (left.durationMs ?? 0))[0];
  if (!stall) return null;
  const stallAt = new Date(stall.at).getTime();
  return samples.reduce<ProfilingSample | null>((current, sample) => {
    if (
      sample.category !== 'timing'
        || sample.durationMs === null
        || sample.name.startsWith('renderer.')
        || isAsyncWallClockTiming(sample.name)
    ) return current;
    const distanceMs = Math.abs(new Date(sample.at).getTime() - stallAt);
    if (distanceMs > Math.max(150, stall.durationMs ?? 0)) return current;
    return !current || sample.durationMs > (current.durationMs ?? 0) ? sample : current;
  }, null);
}

function isAsyncWallClockTiming(name: string): boolean {
  return name.startsWith('ipc.') && !name.endsWith('.nextFrameLatency');
}

function formatMs(value: number): string {
  return `${Math.round(value * 10) / 10}ms`;
}
