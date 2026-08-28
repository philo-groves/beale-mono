import { describe, expect, it } from 'vitest';
import { approximateSerializedSizeBytes } from '../src/renderer/devInstrumentation';
import { profilingLagFindings } from '../src/renderer/view-models/profilingAnalysis';

describe('renderer development instrumentation', () => {
  it('bounds payload traversal and tolerates circular values', () => {
    const circular: { label: string; self?: unknown } = { label: 'payload' };
    circular.self = circular;

    expect(approximateSerializedSizeBytes(circular)).toBeGreaterThan(0);

    const largePayload = Array.from({ length: 20_000 }, () => 'payload');
    expect(approximateSerializedSizeBytes(largePayload)).toBeLessThan(JSON.stringify(largePayload).length);
  });

  it('ranks correlated long tasks and full commentary projection work as lag causes', () => {
    const generatedAt = '2026-08-27T12:00:00.000Z';
    const findings = profilingLagFindings({
      enabled: true,
      empty: false,
      reason: 'manual',
      generatedAt,
      renders: [{ surface: 'commentary.list', renders: 8, lastRender: 12, detail: { messages: 4_000 } }],
      timings: [
        { name: 'renderer.longTask', count: 1, avgMs: 92, maxMs: 92, lastMs: 92, detail: {} },
        { name: 'react.commit.app', count: 3, avgMs: 28, maxMs: 51, lastMs: 30, detail: {} },
        { name: 'ipc.snapshot.event.apply.nextFrameLatency', count: 2, avgMs: 39, maxMs: 55, lastMs: 55, detail: {} },
        { name: 'commentary.buildMessages', count: 3, avgMs: 24, maxMs: 46, lastMs: 25, detail: {} }
      ],
      events: [],
      samples: [
        { at: generatedAt, category: 'timing', name: 'commentary.buildMessages', durationMs: 46, detail: {} },
        { at: generatedAt, category: 'timing', name: 'renderer.longTask', durationMs: 92, detail: {} }
      ]
    });

    expect(findings.map((finding) => finding.id)).toEqual(expect.arrayContaining([
      'renderer-stall',
      'react-commit',
      'state-apply-latency',
      'session-projection',
      'render-frequency'
    ]));
    expect(findings.find((finding) => finding.id === 'renderer-stall')?.evidence).toContain('commentary.buildMessages');
  });
});
