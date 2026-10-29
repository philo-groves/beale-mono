import assert from 'node:assert/strict';
import test from 'node:test';
import { reviewResearchTrackContext } from '../dist/researchTrackContextReview.js';

const startedAt = '2026-01-01T01:00:00.000Z';
const rootEvent = (type, payload, timestamp = '2026-01-01T01:01:00.000Z') => ({
  kind: 'agent.event', timestamp, payload: { type, agentPath: '/root', ...payload },
});

test('track context review retains healthy prior model context', () => {
  assert.deepEqual(reviewResearchTrackContext({ captureAvailable: true, attemptStartedAt: startedAt,
    events: [rootEvent('context_composed', { estimatedTokens: 38_000 }),
      rootEvent('turn_completed', { contextWindow: 100_000 })] }),
  { mode: 'resume', reason: 'healthy', estimatedTokens: 38_000, contextWindow: 100_000 });
});

test('track context review resets after sustained context pressure', () => {
  const events = [rootEvent('context_composed', { estimatedTokens: 84_000 }),
    rootEvent('turn_completed', { contextWindow: 100_000 })];
  assert.equal(reviewResearchTrackContext({ captureAvailable: true, attemptStartedAt: startedAt, events }).reason,
    'context_pressure');
  assert.equal(reviewResearchTrackContext({ captureAvailable: false, attemptStartedAt: startedAt, events }).reason,
    'capture_unavailable');
});

test('track context review uses the compacted context and ignores other agents or attempts', () => {
  const events = [rootEvent('context_composed', { estimatedTokens: 99_000 }, '2026-01-01T00:59:00.000Z'),
    { ...rootEvent('context_composed', { estimatedTokens: 99_000 }), agentPath: '/root/scout',
      payload: { type: 'context_composed', agentPath: '/root/scout', estimatedTokens: 99_000 } },
    rootEvent('context_composed', { estimatedTokens: 83_000 }),
    rootEvent('context_compacted', { tokensAfter: 30_000, contextWindow: 100_000 })];
  assert.deepEqual(reviewResearchTrackContext({ captureAvailable: true, attemptStartedAt: startedAt, events }),
    { mode: 'resume', reason: 'healthy', estimatedTokens: 30_000, contextWindow: 100_000 });
});

test('track context review resets when repeated compaction still leaves a crowded context', () => {
  const events = [rootEvent('context_compacted', { tokensAfter: 67_000, contextWindow: 100_000 }),
    rootEvent('context_compacted', { tokensAfter: 69_000, contextWindow: 100_000 })];
  assert.equal(reviewResearchTrackContext({ captureAvailable: true, attemptStartedAt: startedAt, events }).reason,
    'repeated_compaction');
});

test('track context review accounts for system and tool tokens after message compaction', () => {
  const events = [rootEvent('context_composed', { estimatedTokens: 90_000,
    systemPromptCharacters: 20_000, toolDefinitionCharacters: 20_000 }),
  rootEvent('context_compacted', { tokensAfter: 73_000, contextWindow: 100_000 })];
  assert.equal(reviewResearchTrackContext({ captureAvailable: true, attemptStartedAt: startedAt, events }).reason,
    'context_pressure');
});

test('track context review keeps native-compacted context after a high-usage turn', () => {
  const events = [rootEvent('context_composed', { estimatedTokens: 91_000 }),
    rootEvent('turn_completed', { contextWindow: 100_000 }),
    rootEvent('research_checkpoint', { reason: 'native' })];
  assert.equal(reviewResearchTrackContext({ captureAvailable: true, attemptStartedAt: startedAt, events }).mode,
    'resume');
});

test('track context review retains a capture compacted after the last model request', () => {
  const events = [rootEvent('context_composed', { estimatedTokens: 91_000 }),
    rootEvent('turn_completed', { contextWindow: 100_000 }),
    rootEvent('research_checkpoint', { reason: 'local', phase: 'resumable_capture' })];
  assert.equal(reviewResearchTrackContext({ captureAvailable: true, attemptStartedAt: startedAt, events }).mode,
    'resume');
});
