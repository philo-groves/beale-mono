import { describe, expect, it } from 'vitest';
import { AppServerRunEngine } from '../src/main/appServerRunEngine';

interface EngineHarness {
  activeRuns: Map<string, { rootTurnOffset: number; liveAppServerEventIds: Set<string> }>;
  db: {
    appendTraceEvent: (event: Record<string, unknown>) => { id: string };
    createTranscriptMessage: (message: Record<string, unknown>) => void;
  };
  onChange: (options?: unknown) => void;
  recordLiveEvent: (
    context: { run: { id: string }; attempt: { id: string } },
    event: { schemaVersion: number; kind: string; timestamp: string; payload: Record<string, unknown> }
  ) => void;
}

describe('app-server live event deduplication', () => {
  it('records one transcript when the transport replays a stable event id', () => {
    const traces: Record<string, unknown>[] = [];
    const transcripts: Record<string, unknown>[] = [];
    const engine = Object.create(AppServerRunEngine.prototype) as EngineHarness;
    engine.activeRuns = new Map([['run_replay', {
      rootTurnOffset: 0,
      liveAppServerEventIds: new Set<string>()
    }]]);
    engine.db = {
      appendTraceEvent: (event) => {
        traces.push(event);
        return { id: `trace_${traces.length}` };
      },
      createTranscriptMessage: (message) => { transcripts.push(message); }
    };
    engine.onChange = () => undefined;
    const context = { run: { id: 'run_replay' }, attempt: { id: 'attempt_follow_up' } };
    const event = {
      schemaVersion: 1,
      kind: 'model.output',
      timestamp: '2026-09-03T00:00:00.000Z',
      payload: {
        eventId: 'event_stable_commentary',
        phase: 'completed',
        messagePhase: 'commentary',
        text: 'Inspecting the parser boundary.',
        responseId: 'response_one',
        itemId: 'text:0',
        agentPath: '/root'
      }
    };

    engine.recordLiveEvent(context, event);
    engine.recordLiveEvent(context, event);

    expect(traces).toHaveLength(1);
    expect(transcripts).toHaveLength(1);
    expect(transcripts[0]?.metadata).toMatchObject({ appServerEventId: 'event_stable_commentary' });
  });
});
