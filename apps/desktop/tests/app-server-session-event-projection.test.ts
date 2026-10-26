import { describe, expect, it } from 'vitest';
import type { RunDetail } from '@shared/types';
import { sessionRun, traceFromSessionEvent } from '../src/main/appServerSessionBoundary';
import { projectRunDetailForRenderer } from '../src/shared/runDetailProjection';
import { contextMeterForDetail } from '../src/renderer/features/momentum/contextMeter';
import { buildTraceDisplayEventsForAgentPath } from '../src/renderer/view-models/traceDisplay';
import { commentaryMessagesForSession } from '../src/renderer/view-models/commentary';

describe('canonical session model usage projection', () => {
  it('recovers collaboration choices from existing track restart metadata', () => {
    const collaboration = {
      mode: 'adaptive', subagentMode: 'advanced', intensity: 'normal',
      providers: [{ provider: 'openai-codex', model: 'example-model', enabled: true }]
    };
    const run = sessionRun({
      id: 'session_example', status: 'active', title: 'Example research', prompt: 'Review example scope.',
      model: 'example-model', reasoningEffort: 'high', summary: '',
      createdAt: '2026-01-01T00:00:00.000Z', startedAt: '2026-01-01T00:00:00.000Z', endedAt: null,
      metadata: { bealeRun: { budget: {} }, appServerRestartLaunch: { launch: { collaboration } } }
    } as unknown as Parameters<typeof sessionRun>[0]);

    expect(run.budget.collaboration).toEqual(collaboration);
  });

  it('retains root turn usage and context size in the session renderer', () => {
    const [trace] = traceFromSessionEvent('session_example', {
      id: 'event_example', kind: 'agent.event', timestamp: '2026-01-01T00:00:00.000Z',
      summary: 'Model turn completed.',
      payload: {
        type: 'turn_completed', agentPath: '/root', contextWindow: 400_000,
        usage: { input: 20_000, cacheRead: 40_000, output: 5_000, totalTokens: 65_000 }
      }
    }, 1);
    const detail = {
      traceEvents: [trace], transcriptMessages: [], breakoutRoomMessages: [],
      modelSessions: [], contextCompactions: [], tokenUsage: { totalTokens: 65_000 }
    } as unknown as RunDetail;
    const projected = projectRunDetailForRenderer(detail, { mode: 'commentary', agentPath: null });
    const meter = contextMeterForDetail(projected);

    expect(projected.traceEvents[0]).toMatchObject({
      type: 'model_message', payload: {
        agentPath: '/root', contextWindow: 400_000,
        usage: { input: 20_000, cacheRead: 40_000, totalTokens: 65_000 }
      }
    });
    expect(meter.inputTokens).toBe(60_000);
    expect(meter.tokenLimit).toBe(400_000);
    expect(meter.fraction).toBe(0.15);
    expect(meter.totalSessionTokens).toBe(65_000);
  });

  it('shows a canonical provider safeguard retry as a session marker', () => {
    const [trace] = traceFromSessionEvent('session_example', {
      id: 'event_safeguard', kind: 'agent.event', timestamp: '2026-01-01T00:00:00.000Z',
      summary: 'Model retry.',
      payload: {
        type: 'model_retry', agentPath: '/root', recoveryKind: 'safety_guardrail',
        errorMessage: 'Sensitive provider diagnostic.'
      }
    }, 1);
    const detail = {
      run: {
        id: 'session_example', status: 'active', model: 'example-model',
        promptMarkdown: 'Review the authorized example service.', createdAt: '2026-01-01T00:00:00.000Z'
      },
      attempts: [], traceEvents: [trace], transcriptMessages: [], breakoutRoomMessages: [],
      modelSessions: [], contextCompactions: []
    } as unknown as RunDetail;
    const projected = projectRunDetailForRenderer(detail, { mode: 'commentary', agentPath: null });
    const messages = commentaryMessagesForSession(
      projected, buildTraceDisplayEventsForAgentPath(projected, null), { includeInitialPrompt: false }
    );

    expect(messages.map(({ kind, markerType, contentMarkdown }) => ({ kind, markerType, contentMarkdown }))).toEqual([{
      kind: 'marker', markerType: 'safeguard_recovery',
      contentMarkdown: 'Cybersecurity safeguard encountered · Beale recovering'
    }]);
    expect(JSON.stringify(messages)).not.toContain('Sensitive provider diagnostic.');
  });
});
