import { describe, expect, it } from 'vitest';
import type { RunDetail, TraceEventRecord, TranscriptMessageRecord } from '@shared/types';
import {
  projectCommentaryTraceEvent,
  projectRunDetailForRenderer
} from '../src/shared/runDetailProjection';
import {
  commentaryMessagesForSession,
  hydrateCommentaryToolCall
} from '../src/renderer/view-models/commentary';
import {
  contextMeterForDetail,
  visibleCacheHitRateLabel,
  visibleContextWindowPercentageLabel,
  visibleSessionTokenUsageLabel
} from '../src/renderer/features/momentum/contextMeter';
import { buildTraceDisplayEventsForAgentPath } from '../src/renderer/view-models/traceDisplay';
import { subagentSummaries } from '../src/renderer/view-models/subagents';

describe('run detail commentary projection', () => {
  it('removes hidden event and tool bodies while preserving render and cursor scaffolding', () => {
    const hugeValue = 'hidden'.repeat(20_000);
    const requested = toolEvent('request', 'tool.requested', {
      toolActionId: 'action_one',
      toolName: 'file.read',
      normalizedInputs: { path: 'src/parser.ts', hidden: hugeValue }
    });
    const observed = toolEvent('observation', 'tool.observed', {
      toolActionId: 'action_one',
      toolName: 'file.read',
      normalizedInputs: { path: 'src/parser.ts', hidden: hugeValue },
      result: { text: hugeValue },
      rawOutputRef: hugeValue
    });
    const hiddenExecutor = traceEvent('hidden', {
      source: 'executor',
      type: 'research_event',
      payload: { text: hugeValue, appServerSessionEventId: 'session_event_hidden', agentPath: '/root' }
    });
    const detail = runDetail({
      traceEvents: [requested, observed, hiddenExecutor],
      transcriptMessages: [transcript('Visible commentary.', 'assistant'), transcript(hugeValue, 'system')]
    });

    const projected = projectRunDetailForRenderer(detail, 'commentary');

    expect(projected).not.toBe(detail);
    expect(projected.traceEvents).toHaveLength(detail.traceEvents.length);
    expect(projected.traceEvents[0]).toMatchObject({
      id: 'request',
      payload: {
        appServerKind: 'tool.requested',
        toolName: 'file.read',
        commentaryDetailDeferred: true,
        payload: {
          toolActionId: 'action_one',
          toolName: 'file.read',
          normalizedInputs: { path: 'src/parser.ts' }
        }
      }
    });
    expect(JSON.stringify(projected.traceEvents)).not.toContain(hugeValue);
    expect(projected.traceEvents[2]?.payload).toEqual({
      agentPath: '/root',
      appServerSessionEventId: 'session_event_hidden'
    });
    expect(projected.transcriptMessages[0]?.contentMarkdown).toBe('Visible commentary.');
    expect(projected.transcriptMessages[1]?.contentMarkdown).toBe('');
    expect(JSON.stringify(projected).length).toBeLessThan(JSON.stringify(detail).length / 20);
    expect(projectRunDetailForRenderer(detail, 'full')).toBe(detail);
  });

  it('retains safeguard steering state without exposing the provider error body', () => {
    const errorMessage = 'Sensitive provider error details.';
    const storedRetry = traceEvent('stored-retry', {
      source: 'system',
      type: 'model_message',
      payload: {
        type: 'model_retry',
        agentPath: '/root',
        recoveryKind: 'safety_guardrail',
        awaitingSteering: true,
        errorMessage
      }
    });
    const canonicalRetry = traceEvent('canonical-retry', {
      source: 'executor',
      type: 'research_event',
      payload: {
        agentPath: '/root',
        payload: {
          type: 'model_retry',
          agentPath: '/root',
          recoveryKind: 'safety_guardrail',
          awaitingSteering: true,
          errorMessage
        }
      }
    });

    const projected = projectRunDetailForRenderer(runDetail({
      traceEvents: [storedRetry, canonicalRetry]
    }), 'commentary');

    expect(projected.traceEvents[0]?.payload).toMatchObject({
      type: 'model_retry',
      agentPath: '/root',
      recoveryKind: 'safety_guardrail',
      awaitingSteering: true
    });
    expect(projected.traceEvents[1]?.payload.payload).toEqual({
      type: 'model_retry',
      agentPath: '/root',
      recoveryKind: 'safety_guardrail',
      awaitingSteering: true
    });
    expect(JSON.stringify(projected)).not.toContain(errorMessage);

    const messages = commentaryMessagesForSession(
      projected,
      buildTraceDisplayEventsForAgentPath(projected, null),
      { includeInitialPrompt: false }
    );
    expect(messages.map(({ id, kind }) => [id, kind])).toEqual([
      ['model-safety-pause:stored-retry', 'error']
    ]);
  });

  it('retains only the native compaction checkpoint reason needed for a session marker', () => {
    const projected = projectRunDetailForRenderer(runDetail({ traceEvents: [
      traceEvent('native', { source: 'system', type: 'model_message', payload: {
        type: 'research_checkpoint', reason: 'native', agentPath: '/root', privateDetail: 'hidden'
      } }),
      traceEvent('local', { source: 'system', type: 'model_message', payload: {
        type: 'context_compacted', agentPath: '/root', tokensBefore: 90000, tokensAfter: 30000
      } })
    ] }), 'commentary');
    expect(projected.traceEvents[0]?.payload).toEqual({ type: 'research_checkpoint', reason: 'native', agentPath: '/root' });
    expect(projected.traceEvents[1]?.payload).toEqual({ type: 'context_compacted', agentPath: '/root' });
    const messages = commentaryMessagesForSession(projected,
      buildTraceDisplayEventsForAgentPath(projected, null), { includeInitialPrompt: false });
    expect(messages.map(({ kind, contentMarkdown }) => [kind, contentMarkdown])).toEqual([
      ['marker', 'Context Compacted'], ['marker', 'Context Compacted']
    ]);
  });

  it('retains breakout-room records while projecting commentary', () => {
    const detail = runDetail({
      breakoutRooms: [{ id: 'room_one', title: 'Parser challenge', status: 'active' }] as RunDetail['breakoutRooms'],
      breakoutRoomMembers: [{ id: 'member_one', roomId: 'room_one', agentPath: '/root/reviewer', status: 'active' }] as RunDetail['breakoutRoomMembers'],
      breakoutRoomMessages: [{ id: 'message_one', roomId: 'room_one', contentMarkdown: 'Reviewing parser state.' }] as RunDetail['breakoutRoomMessages']
    });

    const projected = projectRunDetailForRenderer(detail, 'commentary');

    expect(projected.breakoutRooms).toBe(detail.breakoutRooms);
    expect(projected.breakoutRoomMembers).toBe(detail.breakoutRoomMembers);
    expect(projected.breakoutRoomMessages).toBe(detail.breakoutRoomMessages);
  });

  it('omits unselected subagent commentary while retaining bounded lifecycle summaries and source cursors', () => {
    const oversizedMessage = 'child status '.repeat(100);
    const rootCommentary = traceEvent('root-commentary', {
      source: 'model',
      type: 'model_message',
      sequence: 1,
      payload: { agentPath: '/root', transcriptRole: 'assistant', text: 'Root update.' }
    });
    const selectedCommentary = traceEvent('selected-commentary', {
      source: 'model',
      type: 'model_message',
      sequence: 2,
      payload: { agentPath: '/root/reviewer', transcriptRole: 'assistant', text: 'Selected update.' }
    });
    const hiddenCommentary = traceEvent('hidden-commentary', {
      source: 'model',
      type: 'model_message',
      sequence: 3,
      payload: { agentPath: '/root/discoverer', transcriptRole: 'assistant', text: oversizedMessage }
    });
    const hiddenLifecycle = traceEvent('hidden-lifecycle', {
      source: 'executor',
      type: 'research_event',
      sequence: 4,
      summary: oversizedMessage,
      payload: {
        agentPath: '/root/discoverer',
        text: oversizedMessage,
        payload: {
          type: 'subagent.activity',
          action: 'completed',
          agentPath: '/root/discoverer',
          status: 'completed',
          message: oversizedMessage
        }
      }
    });
    const rootTranscript = transcript('Root transcript.', 'assistant');
    const selectedTranscript = {
      ...transcript('Selected transcript.', 'assistant'),
      id: 'transcript_selected',
      metadata: { agentPath: '/root/reviewer' }
    };
    const hiddenTranscript = {
      ...transcript(oversizedMessage, 'assistant'),
      id: 'transcript_hidden',
      metadata: { agentPath: '/root/discoverer' }
    };
    const detail = runDetail({
      traceEvents: [rootCommentary, selectedCommentary, hiddenCommentary, hiddenLifecycle],
      transcriptMessages: [rootTranscript, selectedTranscript, hiddenTranscript],
      breakoutRoomMessages: [{ id: 'hidden-room-message', contentMarkdown: oversizedMessage }] as RunDetail['breakoutRoomMessages']
    });

    const rootProjection = projectRunDetailForRenderer(detail, { mode: 'commentary', agentPath: null });
    expect(rootProjection.traceEvents.map((event) => event.id)).toEqual(['root-commentary', 'hidden-lifecycle']);
    expect(rootProjection.transcriptMessages.map((message) => message.id)).toEqual([rootTranscript.id]);
    expect(rootProjection.breakoutRoomMessages).toEqual([]);
    expect(rootProjection.subagentPreviews).toEqual([
      {
        agentPath: '/root/reviewer',
        message: 'Selected update.',
        sequence: 2,
        createdAt: selectedCommentary.createdAt
      },
      {
        agentPath: '/root/discoverer',
        message: `${oversizedMessage.slice(0, 255)}…`,
        sequence: 3,
        createdAt: hiddenCommentary.createdAt
      }
    ]);
    expect(rootProjection.projectionCursor).toEqual({
      afterTraceSequence: 4,
      afterTranscriptCount: 3,
      afterTraceEventId: 'hidden-lifecycle'
    });
    expect(JSON.stringify(rootProjection)).not.toContain(oversizedMessage);
    expect(String(rootProjection.traceEvents[1]?.payload.payload &&
      (rootProjection.traceEvents[1].payload.payload as Record<string, unknown>).message)).toHaveLength(256);
    expect(rootProjection.traceEvents[1]?.summary).toHaveLength(256);
    expect(rootProjection.traceEvents[1]?.payload.text).toBeUndefined();

    const selectedProjection = projectRunDetailForRenderer(detail, {
      mode: 'commentary',
      agentPath: '/root/reviewer'
    });
    expect(selectedProjection.traceEvents.map((event) => event.id)).toEqual([
      'root-commentary',
      'selected-commentary',
      'hidden-lifecycle'
    ]);
    expect(selectedProjection.transcriptMessages.map((message) => message.id)).toEqual([
      rootTranscript.id,
      'transcript_selected'
    ]);
    expect(selectedProjection.subagentPreviews?.map((preview) => preview.agentPath)).toEqual(['/root/discoverer']);
  });

  it('advances a projected incremental cursor across omitted source rows', () => {
    const sourceCursor = {
      afterTraceSequence: 20,
      afterTranscriptCount: 40,
      afterTraceEventId: 'trace_20'
    };
    const update = {
      ...runDetail({
        traceEvents: [traceEvent('hidden-child-event', {
          source: 'model',
          type: 'model_message',
          sequence: 21,
          payload: { agentPath: '/root/discoverer', transcriptRole: 'assistant', text: 'Not selected.' }
        })],
        transcriptMessages: [{
          ...transcript('Not selected.', 'assistant'),
          metadata: { agentPath: '/root/discoverer' }
        }]
      }),
      version: { runId: 'run_one', version: 'version_21', generatedAt: '2026-08-16T12:00:02.000Z', databaseMs: 1 }
    };

    const projected = projectRunDetailForRenderer(
      update,
      { mode: 'commentary', agentPath: null },
      sourceCursor
    );

    expect(projected.traceEvents).toEqual([]);
    expect(projected.transcriptMessages).toEqual([]);
    expect(projected.subagentPreviews).toEqual([{
      agentPath: '/root/discoverer',
      message: 'Not selected.',
      sequence: 21,
      createdAt: update.traceEvents[0]!.createdAt
    }]);
    expect(projected.projectionCursor).toEqual({
      afterTraceSequence: 21,
      afterTranscriptCount: 41,
      afterTraceEventId: 'hidden-child-event'
    });

    update.projectionCursor = {
      afterTraceSequence: 21,
      afterTranscriptCount: 41,
      afterTraceEventId: 'canonical-page-end'
    };
    expect(projectRunDetailForRenderer(update, { mode: 'commentary', agentPath: null }, sourceCursor)
      .projectionCursor?.afterTraceEventId).toBe('canonical-page-end');
    update.projectionCursor.afterTraceEventId = null;
    expect(projectRunDetailForRenderer(update, { mode: 'commentary', agentPath: null }, sourceCursor)
      .projectionCursor?.afterTraceEventId).toBeNull();
  });

  it('retains nested subagent lifecycle state for the summary and catalog', () => {
    const projected = projectCommentaryTraceEvent(traceEvent('subagent-completed', {
      source: 'executor',
      type: 'research_event',
      payload: {
        appServerKind: 'agent.event',
        agentPath: '/root/reviewer',
        payload: {
          type: 'subagent.activity',
          action: 'completed',
          agentId: 'agent_reviewer',
          agentPath: '/root/reviewer',
          provider: 'openai-codex',
          model: 'gpt-5.6-sol',
          status: 'completed',
          message: 'Review complete.',
          privateDiagnostic: 'not rendered'
        }
      }
    }));

    expect(projected.payload).toEqual({
      appServerKind: 'agent.event',
      agentPath: '/root/reviewer',
      payload: {
        type: 'subagent.activity',
        action: 'completed',
        agentId: 'agent_reviewer',
        agentPath: '/root/reviewer',
        provider: 'openai-codex',
        model: 'gpt-5.6-sol',
        status: 'completed',
        message: 'Review complete.'
      }
    });
    expect(subagentSummaries([projected])[0]?.status).toBe('completed');
  });

  it('retains routed inter-agent messages only in participant transcripts', () => {
    const message = 'Synthetic peer update. '.repeat(24);
    const activity = traceEvent('peer-message', {
      source: 'system',
      type: 'model_message',
      payload: {
        type: 'subagent.activity',
        action: 'message',
        agentPath: '/root/reviewer',
        authorAgentPath: '/root',
        recipientAgentPath: '/root/reviewer',
        message,
        privateDiagnostic: 'not rendered'
      }
    });
    const packet = traceEvent('room-packet', {
      source: 'system',
      type: 'model_message',
      payload: {
        type: 'subagent.activity',
        action: 'room_packet',
        agentPath: '/root/reviewer',
        authorAgentPath: '/root/reviewer',
        participantAgentPaths: ['/root/reviewer', '/root/builder'],
        roomName: 'example-review',
        roomTitle: 'Example review room',
        packetKind: 'observation',
        message: 'Synthetic room packet.'
      }
    });
    const followup = traceEvent('followup-task', {
      source: 'system',
      type: 'model_message',
      payload: {
        type: 'subagent.activity',
        action: 'followup',
        agentPath: '/root/reviewer',
        authorAgentPath: '/root',
        recipientAgentPath: '/root/reviewer',
        message: 'Recheck the synthetic boundary.'
      }
    });
    const projected = projectRunDetailForRenderer(
      runDetail({ traceEvents: [activity, packet, followup] }),
      { mode: 'commentary', agentPath: null }
    );

    expect(projected.traceEvents[0]?.payload).toMatchObject({
      type: 'subagent.activity',
      action: 'message',
      authorAgentPath: '/root',
      recipientAgentPath: '/root/reviewer',
      message
    });
    expect(projected.traceEvents[1]?.payload).toMatchObject({
      action: 'room_packet',
      authorAgentPath: '/root/reviewer',
      participantAgentPaths: ['/root/reviewer', '/root/builder'],
      roomTitle: 'Example review room',
      packetKind: 'observation'
    });
    expect(JSON.stringify(projected.traceEvents)).not.toContain('not rendered');
    const messages = commentaryMessagesForSession(
      projected,
      buildTraceDisplayEventsForAgentPath(projected, null),
      { includeInitialPrompt: false }
    );
    expect(messages.find((item) => item.taskAction === 'message')).toMatchObject({
      taskAction: 'message',
      taskRoute: 'Lead → Reviewer',
      contentMarkdown: message.trim()
    });
    expect(messages.find((item) => item.taskAction === 'room_message')).toBeUndefined();
    const reviewerMessages = commentaryMessagesForSession(
      projected,
      buildTraceDisplayEventsForAgentPath(projected, '/root/reviewer'),
      { includeInitialPrompt: false }
    );
    expect(reviewerMessages.find((item) => item.taskAction === 'room_message')).toMatchObject({
      taskAction: 'room_message',
      taskRoute: 'Reviewer → room "Example review room"',
      contentMarkdown: 'Synthetic room packet.'
    });
    const builderMessages = commentaryMessagesForSession(
      projected,
      buildTraceDisplayEventsForAgentPath(projected, '/root/builder'),
      { includeInitialPrompt: false }
    );
    expect(builderMessages.find((item) => item.taskAction === 'room_message')?.contentMarkdown)
      .toBe('Synthetic room packet.');
    expect(messages.find((item) => item.taskAction === 'followup')).toMatchObject({
      taskAction: 'followup',
      taskRoute: 'Lead → Reviewer',
      contentMarkdown: 'Recheck the synthetic boundary.'
    });
  });

  it('recovers a legacy message sender from an exact child tool request', () => {
    const message = 'Synthetic review update.';
    const request = childMessageRequest('send-request', '/root/reviewer', '/root', message, 1);
    const activity = traceEvent('legacy-message', {
      source: 'system', type: 'model_message', sequence: 2,
      payload: { type: 'subagent.activity', action: 'message', agentPath: '/root', message }
    });
    const nestedActivity = traceEvent('nested-legacy-message', {
      source: 'executor', type: 'research_event', sequence: 3,
      payload: { agentPath: '/root', payload: {
        type: 'subagent.activity', action: 'message', agentPath: '/root', message
      } }
    });
    const projected = projectRunDetailForRenderer(
      runDetail({ traceEvents: [request, activity, nestedActivity] }),
      { mode: 'commentary', agentPath: null }
    );

    expect(projected.traceEvents.map((event) => event.id)).toEqual(['legacy-message', 'nested-legacy-message']);
    expect(projected.traceEvents[0]?.payload.authorAgentPath).toBe('/root/reviewer');
    expect((projected.traceEvents[1]?.payload.payload as Record<string, unknown>).authorAgentPath).toBe('/root/reviewer');
    expect(activity.payload.authorAgentPath).toBeUndefined();
    const messages = commentaryMessagesForSession(projected,
      buildTraceDisplayEventsForAgentPath(projected, null), { includeInitialPrompt: false });
    expect(messages.find((item) => item.taskAction === 'message')).toMatchObject({
      taskOrigin: 'Reviewer', taskDestination: 'Lead', taskRoute: 'Reviewer → Lead'
    });
  });

  it('leaves a legacy sender unattributed when matching requests disagree', () => {
    const message = 'Synthetic shared update.';
    const requests = ['/root/reviewer', '/root/analyst'].map((agentPath, index) =>
      childMessageRequest(`send-request-${index}`, agentPath, 'root', message, index + 1));
    const activity = traceEvent('ambiguous-message', {
      source: 'system', type: 'model_message', sequence: 3,
      payload: { type: 'subagent.activity', action: 'message', agentPath: '/root', message }
    });
    const projected = projectRunDetailForRenderer(
      runDetail({ traceEvents: [...requests, activity] }),
      { mode: 'commentary', agentPath: null }
    );
    expect(projected.traceEvents[0]?.payload.authorAgentPath).toBeUndefined();
  });

  it('keeps bounded surface locations from child focus and spawn events in the root summary', () => {
    const surfaceTrail = [
      { id: 'asset-example', name: 'Example service' },
      { id: 'surface-parser', name: 'Request parser' }
    ];
    const focus = traceEvent('surface-focus', {
      source: 'executor', type: 'research_event', sequence: 3,
      payload: { appServerKind: 'tool.observed', toolName: 'surface.focus', agentPath: '/root/discoverer',
        payload: { toolName: 'surface.focus', status: 'complete', result: {
          surfaceTrail, foundation: { privateBody: 'hidden' }
        } } }
    });
    const spawn = traceEvent('surface-spawn', {
      source: 'system', type: 'model_message', sequence: 2,
      payload: { type: 'subagent.activity', action: 'spawned', agentPath: '/root/discoverer',
        surfaceTrail, privateDiagnostic: 'hidden' }
    });
    const unrelated = traceEvent('child-file-read', {
      source: 'executor', type: 'research_event', sequence: 4,
      payload: { appServerKind: 'tool.observed', toolName: 'file.read', agentPath: '/root/discoverer',
        payload: { toolName: 'file.read', status: 'complete', result: { text: 'hidden file body' } } }
    });
    const projected = projectRunDetailForRenderer(runDetail({ traceEvents: [spawn, focus, unrelated] }),
      { mode: 'commentary', agentPath: null });
    expect(projected.traceEvents.map((event) => event.id)).toEqual(['surface-spawn', 'surface-focus']);
    expect(projected.traceEvents[0]?.payload.surfaceTrail).toEqual(surfaceTrail);
    expect(projected.traceEvents[1]?.payload.payload).toMatchObject({
      toolName: 'surface.focus', status: 'complete', result: { surfaceTrail }
    });
    expect(JSON.stringify(projected.traceEvents)).not.toContain('privateBody');
    expect(JSON.stringify(projected.traceEvents)).not.toContain('privateDiagnostic');
  });

  it('defers tool input/output until the paired records are requested', () => {
    const requested = toolEvent('request', 'tool.requested', {
      toolActionId: 'action_one',
      toolName: 'file.read',
      normalizedInputs: { path: 'src/parser.ts' }
    });
    const observed = toolEvent('observation', 'tool.observed', {
      toolActionId: 'action_one',
      toolName: 'file.read',
      normalizedInputs: { path: 'src/parser.ts' },
      result: { text: 'file body' }
    });
    const detail = runDetail({ traceEvents: [requested, observed] });
    const projected = projectRunDetailForRenderer(detail, 'commentary');
    const messages = commentaryMessagesForSession(
      projected,
      buildTraceDisplayEventsForAgentPath(projected, null)
    );
    const deferred = messages.find((message) => message.kind === 'tool')?.toolCalls?.[0];

    expect(deferred).toMatchObject({
      id: 'request',
      traceEventId: 'observation',
      requestTraceEventId: 'request',
      observationTraceEventId: 'observation',
      detailsDeferred: true,
      label: 'src/parser.ts',
      input: undefined,
      output: undefined
    });

    const hydrated = hydrateCommentaryToolCall(deferred!, [requested, observed], projected);
    expect(hydrated).toMatchObject({
      detailsDeferred: false,
      label: 'src/parser.ts',
      input: { path: 'src/parser.ts' },
      output: { text: 'file body' }
    });
  });

  it('retains bounded memory-save revision metadata for creation counting', () => {
    const observed = toolEvent('memory-save', 'tool.observed', {
      toolActionId: 'memory-save-one',
      toolName: 'memory.save',
      normalizedInputs: { type: 'primitive', title: 'Parser boundary' },
      result: { id: 'memory_one', type: 'primitive', title: 'Parser boundary', status: 'draft', revision: 1, body: 'hidden' }
    });

    const projected = projectCommentaryTraceEvent(observed);

    expect((projected.payload.payload as { normalizedInputs?: unknown }).normalizedInputs).toEqual({
      type: 'primitive',
      title: 'Parser boundary'
    });
    expect((projected.payload.payload as { result?: unknown }).result).toEqual({
      id: 'memory_one',
      type: 'primitive',
      title: 'Parser boundary',
      status: 'draft',
      revision: 1
    });
  });

  it('shows a successful surface diagram update in the live Lead transcript, including subagent work', () => {
    const source = 'flowchart LR\n  request["Example request"] --> parser["Parser"]';
    const requested = toolEvent('diagram-request', 'tool.requested', {
      toolActionId: 'surface-update-example', toolName: 'surface.update',
      normalizedInputs: { surfaceId: 'surface-example', foundation: { threatModel: { diagram: source, risks: ['Example risk'] } } }
    });
    const observed = toolEvent('diagram-observed', 'tool.observed', {
      toolActionId: 'surface-update-example', toolName: 'surface.update', status: 'complete',
      normalizedInputs: { surfaceId: 'surface-example', foundation: { threatModel: { diagram: source, risks: ['Example risk'] } } },
      result: { id: 'surface-example', name: 'Example parser', diagramChange: 'updated', foundation: { hidden: 'not rendered' } }
    });
    const childEvents: TraceEventRecord[] = [requested, observed].map((event, index) => ({ ...event, sequence: index + 1,
      payload: { ...event.payload, agentPath: '/root/scout' } }));
    const pending = projectRunDetailForRenderer(runDetail({ traceEvents: [childEvents[0]!] }), { mode: 'commentary', agentPath: null });
    expect(commentaryMessagesForSession(pending, buildTraceDisplayEventsForAgentPath(pending, null))
      .some((message) => message.kind === 'diagram')).toBe(false);
    const projected = projectRunDetailForRenderer(runDetail({ traceEvents: childEvents }), { mode: 'commentary', agentPath: null });
    expect(projected.traceEvents).toHaveLength(1);
    expect(projected.traceEvents[0]?.payload.payload).toMatchObject({
      surfaceDiagram: { source, surfaceId: 'surface-example', surfaceName: 'Example parser', changeKind: 'updated' }
    });
    expect(JSON.stringify(projected.traceEvents)).not.toContain('Example risk');
    expect(JSON.stringify(projected.traceEvents)).not.toContain('not rendered');
    expect(commentaryMessagesForSession(projected, buildTraceDisplayEventsForAgentPath(projected, null))
      .find((message) => message.kind === 'diagram')?.surfaceDiagram?.source).toBe(source);
    const failed = { ...childEvents[1]!, payload: { ...childEvents[1]!.payload,
      payload: { ...childEvents[1]!.payload.payload as Record<string, unknown>, status: 'error' } } };
    const failedProjection = projectRunDetailForRenderer(runDetail({ traceEvents: [childEvents[0]!, failed] }), { mode: 'commentary', agentPath: null });
    expect(commentaryMessagesForSession(failedProjection, buildTraceDisplayEventsForAgentPath(failedProjection, null))
      .some((message) => message.kind === 'diagram')).toBe(false);
  });

  it('keeps the memory name in the compact transcript before and after save completes', () => {
    const requested = toolEvent('request', 'tool.requested', {
      toolActionId: 'memory-save-one',
      toolName: 'memory.save',
      normalizedInputs: { type: 'test_strategy', title: 'Parser regression suite', body: 'hidden memory body' }
    });
    const observed = toolEvent('observation', 'tool.observed', {
      toolActionId: 'memory-save-one',
      toolName: 'memory.save',
      normalizedInputs: { type: 'test_strategy', title: 'Parser regression suite', body: 'hidden memory body' },
      result: { id: 'memory_example', type: 'test_strategy', status: 'draft', revision: 1 }
    });
    const label = (traceEvents: TraceEventRecord[]): string | undefined => {
      const projected = projectRunDetailForRenderer(runDetail({ traceEvents }), 'commentary');
      expect(JSON.stringify(projected.traceEvents)).not.toContain('hidden memory body');
      return commentaryMessagesForSession(
        projected,
        buildTraceDisplayEventsForAgentPath(projected, null)
      ).find((message) => message.toolName === 'memory.save')?.contentMarkdown;
    };

    expect(label([requested])).toBe('Memorizing test strategy "Parser regression suite"');
    expect(label([requested, observed])).toBe('Memorizing test strategy "Parser regression suite"');
  });

  it('retains runbook execution selection and title for commentary labels', () => {
    const requested = toolEvent('request', 'tool.requested', {
      toolActionId: 'runbook-run-one',
      toolName: 'runbook.run',
      normalizedInputs: {
        id: 'runbook_parser',
        startCellId: 'cell-2',
        endCellId: 'cell-4',
        proofTarget: 'localhost',
        hidden: 'not rendered'
      }
    });
    const observed = toolEvent('observation', 'tool.observed', {
      toolActionId: 'runbook-run-one',
      toolName: 'runbook.run',
      normalizedInputs: {
        id: 'runbook_parser',
        startCellId: 'cell-2',
        endCellId: 'cell-4',
        proofTarget: 'localhost'
      },
      result: {
        runbookId: 'runbook_parser',
        title: 'Parser proof',
        runId: 'runbook_run_one',
        status: 'succeeded'
      }
    });
    const projected = projectRunDetailForRenderer(
      runDetail({ traceEvents: [requested, observed] }),
      'commentary'
    );
    const message = commentaryMessagesForSession(
      projected,
      buildTraceDisplayEventsForAgentPath(projected, null)
    ).find((candidate) => candidate.toolName === 'runbook.run');

    expect(projected.traceEvents[0]?.payload.payload).toMatchObject({
      normalizedInputs: {
        id: 'runbook_parser',
        startCellId: 'cell-2',
        endCellId: 'cell-4'
      }
    });
    expect(projected.traceEvents[1]?.payload.payload).toMatchObject({
      result: { runbookId: 'runbook_parser', title: 'Parser proof' }
    });
    expect(message?.contentMarkdown).toBe('Executing runbook cells 2-4 in Parser proof');
    expect(message?.toolCalls?.[0]?.detailsDeferred).toBe(true);
  });

  it('keeps a bounded surface name for the collapsed tool label without retaining its body', () => {
    const requested = toolEvent('request', 'tool.requested', {
      toolActionId: 'surface-one',
      toolName: 'surface.get',
      normalizedInputs: { surfaceId: 'surface_example', hidden: 'not rendered' }
    });
    const observed = toolEvent('observation', 'tool.observed', {
      toolActionId: 'surface-one',
      toolName: 'surface.get',
      normalizedInputs: { surfaceId: 'surface_example', hidden: 'not rendered' },
      result: { id: 'surface_example', name: 'Parser entry boundary', foundation: { hidden: 'not rendered' } }
    });
    const projected = projectRunDetailForRenderer(runDetail({ traceEvents: [requested, observed] }), 'commentary');
    const message = commentaryMessagesForSession(
      projected,
      buildTraceDisplayEventsForAgentPath(projected, null)
    ).find((candidate) => candidate.toolName === 'surface.get');

    expect(projected.traceEvents[1]?.payload.payload).toMatchObject({
      result: { name: 'Parser entry boundary' }
    });
    expect(JSON.stringify(projected.traceEvents)).not.toContain('not rendered');
    expect(message?.contentMarkdown).toBe('Inspecting surface "Parser entry boundary"');
  });

  it.each([
    ['finding.transition', { toStatus: 'report_ready', hidden: 'not rendered' }, { toStatus: 'report_ready' }],
    ['finding.completion_check', { targetStatus: 'verified', hidden: 'not rendered' }, { targetStatus: 'verified' }],
    ['finding.list', { query: 'memory safety', hidden: 'not rendered' }, { query: 'memory safety' }],
    ['resource.catalog', { operation: 'discover', name: 'Example parser', hidden: 'not rendered' }, { operation: 'discover', name: 'Example parser' }],
    ['surface.get', { surfaceId: 'surface_example', hidden: 'not rendered' }, { surfaceId: 'surface_example' }],
    ['surface.views', { surfaceId: 'surface_example', views: ['recon', 'analysis'], hidden: 'not rendered' },
      { surfaceId: 'surface_example', views: ['recon', 'analysis'] }],
    ['surface.list', { parentId: 'surface_example', hidden: 'not rendered' }, { parentId: 'surface_example' }],
    ['file.edit', { path: 'src/parser.ts', oldText: 'not rendered', newText: 'not rendered' }, { path: 'src/parser.ts' }],
    ['features.load', { features: ['code-navigation'], hidden: 'not rendered' }, { features: ['code-navigation'] }],
    ['plugins.load', { plugins: ['example-plugin'], hidden: 'not rendered' }, { plugins: ['example-plugin'] }]
  ])('retains %s inputs needed for collapsed commentary labels', (toolName, inputs, expectedInputs) => {
    const projected = projectCommentaryTraceEvent(toolEvent('request', 'tool.requested', {
      toolActionId: `${toolName}-one`,
      toolName,
      normalizedInputs: inputs
    }));

    expect(projected.payload.payload).toMatchObject({ normalizedInputs: expectedInputs });
    expect(JSON.stringify(projected.payload.payload)).not.toContain('not rendered');
  });

  it('preserves repository search roots and queries for commentary labels', () => {
    const traceEvents = [
      toolEvent('repository-request-one', 'tool.requested', {
        toolActionId: 'repository-one',
        toolName: 'repository.search',
        normalizedInputs: { query: 'decodeToken' }
      }),
      toolEvent('repository-observation-one', 'tool.observed', {
        toolActionId: 'repository-one',
        toolName: 'repository.search',
        normalizedInputs: { query: 'decodeToken' },
        result: { query: 'decodeToken', roots: ['/work/parser'], matches: [] }
      }),
      toolEvent('repository-request-two', 'tool.requested', {
        toolActionId: 'repository-two',
        toolName: 'repository.search',
        normalizedInputs: { query: 'token boundary' }
      }),
      toolEvent('repository-observation-two', 'tool.observed', {
        toolActionId: 'repository-two',
        toolName: 'repository.search',
        normalizedInputs: { query: 'token boundary' },
        result: { query: 'token boundary', roots: ['/work/runtime'], matches: [] }
      })
    ].map((event, index) => ({ ...event, sequence: index + 1 }));
    const detail = runDetail({ traceEvents });
    const projected = projectRunDetailForRenderer(detail, 'commentary');
    const repositoryMessage = commentaryMessagesForSession(
      projected,
      buildTraceDisplayEventsForAgentPath(projected, null)
    ).find((message) => message.toolName === 'repository.search');

    expect(repositoryMessage).toMatchObject({
      toolCount: 2,
      contentMarkdown: 'Searching 2 repositories with 2 queries'
    });
    expect(repositoryMessage?.toolCalls?.map((toolCall) => toolCall.label)).toEqual([
      'Querying parser for "decodeToken"',
      'Querying runtime for "token boundary"'
    ]);
    expect(repositoryMessage?.toolCalls?.every((toolCall) => toolCall.detailsDeferred)).toBe(true);
  });

  it('keeps content-bearing commentary events but strips unrelated payload fields', () => {
    const projected = projectCommentaryTraceEvent(traceEvent('commentary', {
      source: 'model',
      type: 'model_message',
      payload: {
        transcriptRole: 'assistant',
        transcriptSource: 'app_server_commentary',
        text: 'Rendered reasoning.',
        internalState: { large: 'not rendered' },
        metadata: { responseId: 'response_one', privateValue: 'not rendered' }
      }
    }));

    expect(projected.payload).toEqual({
      transcriptRole: 'assistant',
      transcriptSource: 'app_server_commentary',
      text: 'Rendered reasoning.',
      metadata: { responseId: 'response_one' }
    });
  });

  it('retains bounded session usage telemetry in the commentary projection', () => {
    const projected = projectRunDetailForRenderer(runDetail({
      traceEvents: [
        traceEvent('root-usage', {
          source: 'model',
          type: 'model_message',
          payload: {
            agentPath: '/root',
            usage: {
              input: 10_000,
              output: 1_000,
              cacheRead: 30_000,
              totalTokens: 41_000,
              cacheHitRate: 0.75,
              source: 'app-server reported model usage',
              privateProviderDetail: 'not rendered'
            }
          }
        }),
        traceEvent('auxiliary-usage', {
          source: 'model',
          type: 'model_message',
          payload: {
            payload: { agentPath: '/auxiliary-model', contextUsageEligible: false, privateValue: 'not rendered' },
            usage: { input: 500_000, output: 10_000, totalTokens: 510_000 }
          }
        })
      ]
    }), 'commentary');

    expect(projected.traceEvents[0]?.payload.usage).toEqual({
      input: 10_000,
      output: 1_000,
      cacheRead: 30_000,
      totalTokens: 41_000,
      cacheHitRate: 0.75,
      source: 'app-server reported model usage'
    });
    expect(projected.traceEvents[1]?.payload.payload).toEqual({
      agentPath: '/auxiliary-model',
      contextUsageEligible: false
    });
    const meter = contextMeterForDetail(projected);
    expect(visibleSessionTokenUsageLabel(meter)).toBe('41k');
    expect(visibleCacheHitRateLabel(meter)).toBe('75%');
    expect(visibleContextWindowPercentageLabel(meter)).toBe('15%');
  });

  it('retains collaborator usage telemetry in the root commentary projection', () => {
    const projected = projectRunDetailForRenderer(runDetail({
      traceEvents: [
        traceEvent('root-usage', {
          source: 'model',
          type: 'model_message',
          payload: { agentPath: '/root', usage: { input: 100, output: 10, cacheRead: 900, totalTokens: 1_010 } }
        }),
        traceEvent('collaborator-usage', {
          source: 'model',
          type: 'model_message',
          payload: { agentPath: '/root/scout', usage: { input: 200, output: 20, cacheRead: 800, totalTokens: 1_020 } }
        }),
        traceEvent('unrelated-agent-usage', {
          source: 'model',
          type: 'model_message',
          payload: { agentPath: '/auxiliary-model', usage: { input: 300, output: 30, cacheRead: 700, totalTokens: 1_030 } }
        })
      ]
    }), { mode: 'commentary', agentPath: null });

    expect(projected.traceEvents.map((event) => event.id)).toEqual(['root-usage', 'collaborator-usage']);
    expect(projected.traceEvents[1]?.payload).toMatchObject({
      agentPath: '/root/scout',
      usage: { input: 200, output: 20, cacheRead: 800, totalTokens: 1_020 }
    });
    const meter = contextMeterForDetail(projected);
    expect(meter.totalSessionTokens).toBe(2_030);
    expect(meter.cacheReadTokens).toBe(1_700);
    expect(meter.cachePromptTokens).toBe(2_000);
    expect(visibleContextWindowPercentageLabel(meter)).toBe('0%');
  });

  it('retains a bounded shell command for its pre-expansion label', () => {
    const command = `printf 'visible command' ${'x'.repeat(600)}`;
    const projected = projectCommentaryTraceEvent(toolEvent('shell', 'tool.requested', {
      toolActionId: 'shell_action',
      toolName: 'shell.run',
      normalizedInputs: { command, secret: 'not rendered' }
    }));

    expect(projected.payload.payload).toEqual({
      toolActionId: 'shell_action',
      toolName: 'shell.run',
      normalizedInputs: { command: `${command.slice(0, 255)}…` }
    });
  });

  it('retains only the executed shell identity needed to label null-utility requests', () => {
    const projected = projectCommentaryTraceEvent(toolEvent('shell-result', 'tool.observed', {
      toolActionId: 'shell_action',
      toolName: 'shell.run',
      normalizedInputs: { utility: null, args: [], secret: 'not rendered' },
      result: {
        utility: '/bin/sh',
        args: ['-lc', 'tools/rr ping'],
        stdout: 'large output is deferred',
        stderr: ''
      }
    }));

    expect(projected.payload.payload).toEqual({
      toolActionId: 'shell_action',
      toolName: 'shell.run',
      normalizedInputs: { args: [] },
      result: { utility: '/bin/sh', args: ['-lc', 'tools/rr ping'] }
    });
  });
});

function toolEvent(id: string, kind: 'tool.requested' | 'tool.observed', payload: Record<string, unknown>): TraceEventRecord {
  const toolName = typeof payload.toolName === 'string' ? payload.toolName : 'file.read';
  return traceEvent(id, {
    source: 'executor',
    type: 'research_event',
    summary: `app-server ${kind}: ${toolName}.`,
    payload: {
      appServerKind: kind,
      toolName,
      agentPath: '/root',
      appServerSessionEventId: id,
      payload
    }
  });
}

function childMessageRequest(
  id: string,
  agentPath: string,
  target: string,
  message: string,
  sequence: number
): TraceEventRecord {
  const event = toolEvent(id, 'tool.requested', {
    toolName: 'send_message', normalizedInputs: { target, message }
  });
  return { ...event, sequence, payload: { ...event.payload, agentPath } };
}

function traceEvent(
  id: string,
  input: Partial<TraceEventRecord> & Pick<TraceEventRecord, 'source' | 'type' | 'payload'>
): TraceEventRecord {
  return {
    id,
    runId: 'run_one',
    attemptId: 'attempt_one',
    sequence: id === 'request' ? 1 : id === 'observation' ? 2 : 3,
    summary: input.summary ?? 'Event.',
    sensitivity: 'internal',
    modelVisible: true,
    createdAt: '2026-08-16T12:00:00.000Z',
    artifactId: null,
    toolCallId: null,
    approvalId: null,
    ...input
  };
}

function transcript(contentMarkdown: string, role: TranscriptMessageRecord['role']): TranscriptMessageRecord {
  return {
    id: `transcript_${role}`,
    runId: 'run_one',
    attemptId: 'attempt_one',
    traceEventId: null,
    role,
    contentMarkdown,
    source: role === 'assistant' ? 'app_server_commentary' : 'system',
    metadata: { agentPath: '/root', privateValue: 'not rendered' },
    createdAt: '2026-08-16T12:00:01.000Z'
  };
}

function runDetail(input: {
  traceEvents?: TraceEventRecord[];
  transcriptMessages?: TranscriptMessageRecord[];
  breakoutRooms?: RunDetail['breakoutRooms'];
  breakoutRoomMembers?: RunDetail['breakoutRoomMembers'];
  breakoutRoomMessages?: RunDetail['breakoutRoomMessages'];
}): RunDetail {
  return {
    run: {
      id: 'run_one',
      promptMarkdown: '',
      createdAt: '2026-08-16T12:00:00.000Z',
      status: 'active'
    },
    attempts: [],
    traceEvents: input.traceEvents ?? [],
    transcriptMessages: input.transcriptMessages ?? [],
    breakoutRooms: input.breakoutRooms,
    breakoutRoomMembers: input.breakoutRoomMembers,
    breakoutRoomMessages: input.breakoutRoomMessages,
    artifacts: [],
    verifierContracts: [],
    verifierRuns: [],
    modelSessions: [],
    contextCompactions: [],
    policyEvents: [],
    exports: []
  } as unknown as RunDetail;
}
