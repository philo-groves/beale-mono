import assert from 'node:assert/strict';
import test from 'node:test';
import { PairingWindowController } from '../dist/pairingWindowController.js';

test('reveals a newly created pairing window before asynchronous QR loading', async () => {
  const events = [];
  let finishLoading;
  const loading = new Promise((resolve) => {
    finishLoading = resolve;
  });
  const window = fakeWindow(events);
  const controller = new PairingWindowController();

  const showing = controller.show(
    () => {
      events.push('create');
      return window;
    },
    async () => {
      events.push('load');
      await loading;
    },
  );

  assert.deepEqual(events, ['create', 'show', 'focus', 'load']);
  finishLoading();
  await showing;
  assert.deepEqual(events, ['create', 'show', 'focus', 'load', 'show', 'focus']);
});

test('a second click synchronously reveals the existing pairing window while it loads', async () => {
  const events = [];
  let finishLoading;
  const loading = new Promise((resolve) => {
    finishLoading = resolve;
  });
  const window = fakeWindow(events);
  const controller = new PairingWindowController();
  const first = controller.show(() => window, () => loading);

  await controller.show(() => {
    throw new Error('must not create a second window');
  }, async () => {
    throw new Error('must not start a second load');
  });

  assert.deepEqual(events, ['show', 'focus', 'show', 'focus']);
  finishLoading();
  await first;
});

function fakeWindow(events) {
  let destroyed = false;
  const listeners = new Map();
  return {
    isDestroyed: () => destroyed,
    show: () => events.push('show'),
    focus: () => events.push('focus'),
    destroy: () => {
      destroyed = true;
      listeners.get('closed')?.();
    },
    on: (event, listener) => listeners.set(event, listener),
  };
}
