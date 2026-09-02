import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, test } from 'node:test';
import {
  inspectAppServerSessionCompletion,
  isRecoverableLongSessionFailure,
  longSessionRecoveryFallbackPrompt
} from '../dist/index.js';

const temporaryDirectories = [];

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0)) {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('classifies model-level capture errors even when the worker exits zero', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-session-recovery-'));
  temporaryDirectories.push(directory);
  const capturePath = join(directory, 'capture.json');
  writeFileSync(capturePath, JSON.stringify({
    agent: {
      status: 'error',
      outputText: 'WebSocket disconnected after the provider retry limit was reached.'
    }
  }));

  assert.deepEqual(await inspectAppServerSessionCompletion({
    code: 0,
    stderr: '',
    capturePath,
    stopRequested: false
  }), {
    succeeded: false,
    recoverable: true,
    captureAvailable: true,
    diagnostic: 'WebSocket disconnected after the provider retry limit was reached.'
  });
});

test('trusts a durable completed capture when later worker cleanup exits nonzero', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-session-recovery-complete-'));
  temporaryDirectories.push(directory);
  const capturePath = join(directory, 'capture.json');
  writeFileSync(capturePath, JSON.stringify({
    agent: { status: 'complete', outputText: 'Finished the requested review.' }
  }));

  assert.deepEqual(await inspectAppServerSessionCompletion({
    code: 1,
    stderr: 'Cleanup hook failed after capture commit.',
    capturePath,
    stopRequested: false
  }), {
    succeeded: true,
    recoverable: false,
    captureAvailable: true,
    diagnostic: null
  });
});

test('does not recover explicit stops, policy failures, credentials, or invalid configuration', async () => {
  assert.equal(isRecoverableLongSessionFailure('Provider safety guardrail repeated.'), false);
  assert.equal(isRecoverableLongSessionFailure('Authentication credentials are unavailable.'), false);
  assert.equal(isRecoverableLongSessionFailure('Invalid research profile configuration.'), false);
  assert.equal(isRecoverableLongSessionFailure('WebSocket disconnected unexpectedly.'), true);

  assert.deepEqual(await inspectAppServerSessionCompletion({
    code: 1,
    stderr: 'WebSocket disconnected unexpectedly.',
    capturePath: join(tmpdir(), 'missing-long-session-capture.json'),
    stopRequested: true
  }), {
    succeeded: false,
    recoverable: false,
    captureAvailable: false,
    diagnostic: null
  });
});

test('builds a self-contained fallback without discarding the original task', () => {
  const prompt = longSessionRecoveryFallbackPrompt(
    'Continue reviewing the kernel parser.',
    'Provider stream ended unexpectedly.'
  );
  assert.match(prompt, /Continue the same research task/u);
  assert.match(prompt, /Continue reviewing the kernel parser/u);
  assert.match(prompt, /Provider stream ended unexpectedly/u);
  assert.match(prompt, /canonical session history/u);
});
