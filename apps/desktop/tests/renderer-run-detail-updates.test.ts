import { describe, expect, it } from 'vitest';
import type { RunDetail, RunDetailUpdate, SubagentPreviewRecord, TraceEventRecord, TranscriptMessageRecord, WorkspaceSnapshot } from '@shared/types';
import {
  mergeRunDetailUpdate,
  runDetailMetricDetail,
  runDetailUpdateCursor,
  runDetailUpdateMetricDetail,
  selectRunId,
  shortMetricId,
  snapshotMetricDetail
} from '../src/renderer/view-models/runDetailUpdates';

describe('renderer run detail update view model', () => {
  it('keeps selected run id only when the run remains present', () => {
    expect(selectRunId('run_two', snapshot(['run_one', 'run_two']))).toBe('run_two');
    expect(selectRunId('missing', snapshot(['run_one', 'run_two']))).toBeNull();
    expect(selectRunId('missing', snapshot(['run_one', 'run_two'], { run_two: 'active' }))).toBeNull();
    expect(selectRunId('run_one', null)).toBeNull();
  });

  it('builds update cursors and metric summaries', () => {
    const detail = runDetail({
      traceEvents: [traceEvent({ id: 'trace_old', sequence: 7 })],
      transcriptMessages: [transcriptMessage({ id: 'message_old' })]
    });
    const update = runDetailUpdate({
      traceEvents: [traceEvent({ id: 'trace_new', sequence: 8 })],
      transcriptMessages: [transcriptMessage({ id: 'message_new' })]
    });

    expect(runDetailUpdateCursor(detail)).toEqual({
      afterTraceSequence: 7,
      afterTranscriptCount: 1,
      afterTraceEventId: 'trace_old'
    });
    detail.traceEvents[0].payload.appServerSessionEventId = 'trace_batch_1';
    expect(runDetailUpdateCursor(detail).afterTraceEventId).toBe('trace_batch_1');
    detail.projectionCursor = { afterTraceSequence: 50, afterTranscriptCount: 75, afterTraceEventId: 'source_50' };
    expect(runDetailUpdateCursor(detail)).toEqual(detail.projectionCursor);
    expect(runDetailMetricDetail(detail)).toMatchObject({ run: 'run_test', traceEvents: 1, transcripts: 1 });
    expect(runDetailUpdateMetricDetail(update)).toMatchObject({ run: 'run_test', traceEvents: 1, transcripts: 1, versionDatabaseMs: 4.5 });
    expect(snapshotMetricDetail(snapshot(['run_one']))).toMatchObject({ active: true, runs: 1 });
    expect(shortMetricId('run_1234567890abcdef')).toBe('run_12...cdef');
  });

  it('merges incremental trace and transcript rows by id and stable order', () => {
    const current = runDetail({
      traceEvents: [
        traceEvent({ id: 'trace_2', sequence: 2, summary: 'old' }),
        traceEvent({ id: 'trace_4', sequence: 4 })
      ],
      transcriptMessages: [
        transcriptMessage({ id: 'message_b', createdAt: '2026-04-30T00:02:00.000Z', contentMarkdown: 'old' })
      ]
    });
    const update = runDetailUpdate({
      traceEvents: [
        traceEvent({ id: 'trace_3', sequence: 3 }),
        traceEvent({ id: 'trace_2', sequence: 2, summary: 'new' })
      ],
      transcriptMessages: [
        transcriptMessage({ id: 'message_a', createdAt: '2026-04-30T00:01:00.000Z' }),
        transcriptMessage({ id: 'message_b', createdAt: '2026-04-30T00:02:00.000Z', contentMarkdown: 'new' })
      ]
    });

    const merged = mergeRunDetailUpdate(current, update);

    expect(merged.traceEvents.map((event) => `${event.id}:${event.summary}`)).toEqual(['trace_2:new', 'trace_3:summary', 'trace_4:summary']);
    expect(merged.transcriptMessages.map((message) => `${message.id}:${message.contentMarkdown}`)).toEqual(['message_a:content', 'message_b:new']);
  });

  it('carries the source cursor through projected incremental merges', () => {
    const current = runDetail();
    current.projectionCursor = { afterTraceSequence: 5, afterTranscriptCount: 8, afterTraceEventId: 'trace_5' };
    const update = runDetailUpdate();
    update.projectionCursor = { afterTraceSequence: 9, afterTranscriptCount: 12, afterTraceEventId: 'trace_9' };

    expect(mergeRunDetailUpdate(current, update).projectionCursor).toEqual(update.projectionCursor);
  });

  it('replaces each subagent preview with its latest projected message', () => {
    const current = runDetail({
      subagentPreviews: [subagentPreview('/root/reviewer', 'Earlier preview.', 5)]
    });
    const update = runDetailUpdate({
      subagentPreviews: [
        subagentPreview('/root/reviewer', 'Live preview.', 8),
        subagentPreview('/root/scout', 'Scouting now.', 7)
      ]
    });

    expect(mergeRunDetailUpdate(current, update).subagentPreviews).toEqual([
      subagentPreview('/root/scout', 'Scouting now.', 7),
      subagentPreview('/root/reviewer', 'Live preview.', 8)
    ]);
  });

  it('appends already ordered detail rows without disturbing existing records', () => {
    const traceOld = traceEvent({ id: 'trace_1', sequence: 1 });
    const messageOld = transcriptMessage({ id: 'message_1', createdAt: '2026-04-30T00:01:00.000Z' });
    const traceNew = traceEvent({ id: 'trace_2', sequence: 2 });
    const messageNew = transcriptMessage({ id: 'message_2', createdAt: '2026-04-30T00:02:00.000Z' });
    const merged = mergeRunDetailUpdate(
      runDetail({ traceEvents: [traceOld], transcriptMessages: [messageOld] }),
      runDetailUpdate({ traceEvents: [traceNew], transcriptMessages: [messageNew] })
    );

    expect(merged.traceEvents).toEqual([traceOld, traceNew]);
    expect(merged.transcriptMessages).toEqual([messageOld, messageNew]);
  });

  it('does not fast-append replayed rows whose ids are already present', () => {
    const currentTrace = traceEvent({ id: 'trace_replayed', sequence: 1, summary: 'original' });
    const currentMessage = transcriptMessage({
      id: 'message_replayed',
      createdAt: '2026-04-30T00:01:00.000Z',
      contentMarkdown: 'original'
    });
    const replayedTrace = traceEvent({ id: 'trace_replayed', sequence: 2, summary: 'replayed' });
    const replayedMessage = transcriptMessage({
      id: 'message_replayed',
      createdAt: '2026-04-30T00:02:00.000Z',
      contentMarkdown: 'replayed'
    });

    const merged = mergeRunDetailUpdate(
      runDetail({ traceEvents: [currentTrace], transcriptMessages: [currentMessage] }),
      runDetailUpdate({ traceEvents: [replayedTrace], transcriptMessages: [replayedMessage] })
    );

    expect(merged.traceEvents).toEqual([replayedTrace]);
    expect(merged.transcriptMessages).toEqual([replayedMessage]);
  });

  it('replaces a provisional terminal response with its canonical transcript record', () => {
    const provisional = transcriptMessage({
      id: 'transcript_final_run_test_attempt_one',
      attemptId: 'attempt_one',
      phase: 'final_answer',
      contentMarkdown: 'Objective achieved.\n\nVerification completed.',
      source: 'app-server',
      metadata: { agentPath: '/root' },
      createdAt: '2026-04-30T00:01:00.000Z'
    });
    const canonical = transcriptMessage({
      id: 'transcript_model_output_final',
      attemptId: 'attempt_one',
      traceEventId: 'event_model_output_final',
      phase: 'final_answer',
      contentMarkdown: 'Objective achieved. Verification completed.',
      source: 'app-server',
      metadata: { agentPath: '/root', responseId: 'response_final', itemId: 'text_final' },
      createdAt: '2026-04-30T00:01:01.000Z'
    });

    const merged = mergeRunDetailUpdate(
      runDetail({ transcriptMessages: [provisional] }),
      runDetailUpdate({ transcriptMessages: [canonical] })
    );

    expect(merged.transcriptMessages).toEqual([canonical]);
  });

  it('keeps distinct canonical terminal responses with identical text', () => {
    const first = transcriptMessage({
      id: 'transcript_first',
      attemptId: 'attempt_one',
      traceEventId: 'event_first',
      phase: 'final_answer',
      contentMarkdown: 'Verification completed.',
      source: 'app-server',
      metadata: { agentPath: '/root' },
      createdAt: '2026-04-30T00:01:00.000Z'
    });
    const second = transcriptMessage({
      id: 'transcript_second',
      attemptId: 'attempt_one',
      traceEventId: 'event_second',
      phase: 'final_answer',
      contentMarkdown: 'Verification completed.',
      source: 'app-server',
      metadata: { agentPath: '/root' },
      createdAt: '2026-04-30T00:02:00.000Z'
    });

    expect(mergeRunDetailUpdate(
      runDetail({ transcriptMessages: [first] }),
      runDetailUpdate({ transcriptMessages: [second] })
    ).transcriptMessages).toEqual([first, second]);
  });

  it('retains prior aggregate records when a cursor update contains only changed records', () => {
    const current = runDetail();
    current.artifacts = [{ id: 'artifact_old' }] as RunDetail['artifacts'];
    current.policyEvents = [{ id: 'approval_old' }] as RunDetail['policyEvents'];
    const update = runDetailUpdate();
    update.artifacts = [{ id: 'artifact_new' }] as RunDetailUpdate['artifacts'];
    update.policyEvents = [];

    const merged = mergeRunDetailUpdate(current, update);

    expect(merged.artifacts.map(({ id }) => id)).toEqual(['artifact_old', 'artifact_new']);
    expect(merged.policyEvents.map(({ id }) => id)).toEqual(['approval_old']);
  });
});

function snapshot(runIds: string[], statuses: Record<string, string> = {}): WorkspaceSnapshot {
  return {
    workspace: { workspaceId: 'workspace_test' },
    runs: runIds.map((id) => ({ run: { id, status: statuses[id] ?? 'completed' } })),
    notifications: []
  } as unknown as WorkspaceSnapshot;
}

function runDetail(input: {
  traceEvents?: TraceEventRecord[];
  transcriptMessages?: TranscriptMessageRecord[];
  subagentPreviews?: SubagentPreviewRecord[];
} = {}): RunDetail {
  return {
    run: { id: 'run_test', status: 'active' },
    attempts: [],
    traceEvents: input.traceEvents ?? [],
    transcriptMessages: input.transcriptMessages ?? [],
    subagentPreviews: input.subagentPreviews ?? [],
    hypotheses: [],
    artifacts: [],
    evidence: [],
    findings: [],
    verifierContracts: [],
    verifierRuns: [],
    modelSessions: [],
    contextCompactions: [],
    policyEvents: [],
    exports: []
  } as unknown as RunDetail;
}

function runDetailUpdate(input: {
  traceEvents?: TraceEventRecord[];
  transcriptMessages?: TranscriptMessageRecord[];
  subagentPreviews?: SubagentPreviewRecord[];
} = {}): RunDetailUpdate {
  return {
    ...runDetail(input),
    version: {
      runId: 'run_test',
      version: 'version_test',
      databaseMs: 4.5
    }
  } as unknown as RunDetailUpdate;
}

function subagentPreview(agentPath: string, message: string, sequence: number): SubagentPreviewRecord {
  return { agentPath, message, sequence, createdAt: `2026-08-27T10:00:0${sequence}.000Z` };
}

function traceEvent(input: Partial<TraceEventRecord> = {}): TraceEventRecord {
  return {
    id: 'trace_test',
    runId: 'run_test',
    attemptId: null,
    sequence: 1,
    source: 'model',
    type: 'model_message',
    summary: 'summary',
    payload: {},
    sensitivity: 'internal',
    modelVisible: true,
    createdAt: '2026-04-30T00:00:00.000Z',
    artifactId: null,
    toolCallId: null,
    approvalId: null,
    ...input
  };
}

function transcriptMessage(input: Partial<TranscriptMessageRecord> = {}): TranscriptMessageRecord {
  return {
    id: 'message_test',
    runId: 'run_test',
    attemptId: null,
    traceEventId: null,
    role: 'assistant',
    contentMarkdown: 'content',
    source: 'openai',
    createdAt: '2026-04-30T00:00:00.000Z',
    metadata: {},
    ...input
  };
}
