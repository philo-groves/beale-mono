import assert from 'node:assert/strict';
import { chmodSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

import {
  APP_SERVER_LAUNCH_ENVIRONMENT_FLAG,
  consumeAppServerLaunchEnvironment,
} from '../dist/launchEnvironment.js';

test('consumes and removes a private one-shot launch environment', () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-launch-environment-'));
  const path = join(directory, 'environment.json');
  const argv = ['electron', 'app-server', APP_SERVER_LAUNCH_ENVIRONMENT_FLAG, path, '--check'];
  const environment = { KEEP: 'before' };
  try {
    writeFileSync(path, JSON.stringify({ KEEP: 'after', PROVIDER_TOKEN: 'host-only' }), { mode: 0o600 });
    assert.equal(consumeAppServerLaunchEnvironment(argv, environment), true);
    assert.deepEqual(environment, { KEEP: 'after', PROVIDER_TOKEN: 'host-only' });
    assert.deepEqual(argv, ['electron', 'app-server', '--check']);
    assert.equal(existsSync(path), false);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('rejects a launch environment readable by other users and still removes it', { skip: process.platform === 'win32' }, () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-launch-environment-mode-'));
  const path = join(directory, 'environment.json');
  try {
    writeFileSync(path, JSON.stringify({ PROVIDER_TOKEN: 'host-only' }), { mode: 0o600 });
    chmodSync(path, 0o644);
    assert.throws(
      () => consumeAppServerLaunchEnvironment(['electron', APP_SERVER_LAUNCH_ENVIRONMENT_FLAG, path], {}),
      /must not be accessible to other users/,
    );
    assert.equal(existsSync(path), false);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('rejects malformed launch environment content without retaining it', () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-launch-environment-invalid-'));
  const path = join(directory, 'environment.json');
  try {
    writeFileSync(path, JSON.stringify({ VALID: 'yes', INVALID: 1 }), { mode: 0o600 });
    assert.throws(
      () => consumeAppServerLaunchEnvironment(['electron', APP_SERVER_LAUNCH_ENVIRONMENT_FLAG, path], {}),
      /JSON object of strings/,
    );
    assert.equal(existsSync(path), false);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
