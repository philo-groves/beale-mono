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
      contentMarkdown: 'Recovering from Cyber Safeguard'
    }]);
    expect(JSON.stringify(messages)).not.toContain('Sensitive provider diagnostic.');
  });

  it('keeps a canonical track epoch boundary visible in the session transcript', () => {
    const [trace] = traceFromSessionEvent('session_example', {
      id: 'track_epoch_resolving_attempt-example', kind: 'agent.event', timestamp: '2026-01-01T00:01:00.000Z',
      summary: 'Research track epoch resolved.',
      payload: { type: 'track_epoch_resolving', attemptId: 'attempt-example', agentPath: '/root',
        internalDetail: 'Hidden example detail.' }, agentPath: '/root'
    }, 2);
    const detail = {
      run: { id: 'session_example', status: 'active', model: 'example-model',
        promptMarkdown: 'Review the authorized example service.', createdAt: '2026-01-01T00:00:00.000Z' },
      attempts: [], traceEvents: [trace], transcriptMessages: [], breakoutRoomMessages: [],
      modelSessions: [], contextCompactions: []
    } as unknown as RunDetail;
    const projected = projectRunDetailForRenderer(detail, { mode: 'commentary', agentPath: null });
    expect(projected.traceEvents[0]?.payload.payload).toEqual({ type: 'track_epoch_resolving', agentPath: '/root' });
    const messages = commentaryMessagesForSession(
      projected, buildTraceDisplayEventsForAgentPath(projected, null), { includeInitialPrompt: false }
    );
    expect(messages.map(({ kind, markerType, contentMarkdown }) => ({ kind, markerType, contentMarkdown }))).toEqual([{
      kind: 'marker', markerType: 'epoch', contentMarkdown: 'Resolving Epoch'
    }]);
    expect(JSON.stringify(messages)).not.toContain('Hidden example detail.');
  });

  it('shows a failed next epoch as paused without projecting private diagnostics', () => {
    const [trace] = traceFromSessionEvent('session_example', {
      id: 'track_epoch_paused_example', kind: 'agent.event', timestamp: '2026-01-01T00:02:00.000Z',
      summary: 'Research track paused while preparing the next epoch.',
      payload: { type: 'track_epoch_paused', cause: 'ssh_host_key', agentPath: '/root',
        internalDetail: 'Private example diagnostic.' }, agentPath: '/root'
    }, 3);
    const detail = {
      run: { id: 'session_example', status: 'completed', model: 'example-model',
        promptMarkdown: 'Review the authorized example service.', createdAt: '2026-01-01T00:00:00.000Z' },
      attempts: [], traceEvents: [trace], transcriptMessages: [], breakoutRoomMessages: [],
      modelSessions: [], contextCompactions: []
    } as unknown as RunDetail;
    const projected = projectRunDetailForRenderer(detail, { mode: 'commentary', agentPath: null });
    expect(projected.traceEvents[0]?.payload.payload).toEqual({ type: 'track_epoch_paused',
      cause: 'ssh_host_key', agentPath: '/root' });
    const messages = commentaryMessagesForSession(
      projected, buildTraceDisplayEventsForAgentPath(projected, null), { includeInitialPrompt: false }
    );
    expect(messages.map(({ kind, markerType, contentMarkdown }) => ({ kind, markerType, contentMarkdown }))).toEqual([{
      kind: 'marker', markerType: 'epoch', contentMarkdown: 'Research Track Paused · SSH host key verification failed'
    }]);
    expect(JSON.stringify(messages)).not.toContain('Private example diagnostic.');
  });

  it('renders a child agent diagram update from a canonical tool observation in the root session', () => {
    const source = 'flowchart LR\n  input["Example input"] --> parser["Example parser"]';
    const [trace] = traceFromSessionEvent('session_example', {
      id: 'event_diagram', kind: 'research.event', timestamp: '2026-01-01T00:00:00.000Z',
      summary: 'Surface updated.',
      payload: { agentPath: '/root/scout', event: {
        id: 'research_event_diagram', kind: 'tool.observed', timestamp: '2026-01-01T00:00:00.000Z',
        payload: { toolActionId: 'action_diagram', toolName: 'surface.update', status: 'complete',
          normalizedInputs: { surfaceId: 'surface-example' },
          surfaceDiagram: { source, surfaceId: 'surface-example', surfaceName: 'Example parser', changeKind: 'created' } }
      } }
    }, 1);
    const detail = {
      run: { id: 'session_example', status: 'active', model: 'example-model',
        promptMarkdown: 'Review the authorized example service.', createdAt: '2026-01-01T00:00:00.000Z' },
      attempts: [], traceEvents: [trace], transcriptMessages: [], breakoutRoomMessages: [],
      modelSessions: [], contextCompactions: []
    } as unknown as RunDetail;
    const projected = projectRunDetailForRenderer(detail, { mode: 'commentary', agentPath: null });
    const messages = commentaryMessagesForSession(
      projected, buildTraceDisplayEventsForAgentPath(projected, null), { includeInitialPrompt: false }
    );
    expect(messages.find((message) => message.kind === 'diagram')?.surfaceDiagram)
      .toEqual({ source, surfaceId: 'surface-example', surfaceName: 'Example parser', changeKind: 'created' });
    expect(messages.find((message) => message.kind === 'diagram')?.contentMarkdown).toBe('Example parser');
  });
});
