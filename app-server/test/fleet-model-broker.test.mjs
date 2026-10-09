import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { FleetModelBrokerQueue } from '../dist/fleetModelBrokerQueue.js';

test('Fleet model broker preserves requests and results across guest app-server restarts', () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-broker-example-'));
  try {
    const guest = new FleetModelBrokerQueue(root);
    const token = guest.register('run-example');
    const request = { id: 'request-example', model: { provider: 'openai-codex', id: 'model-example' },
      context: { systemPrompt: 'Synthetic instruction', messages: [] }, options: { reasoning: 'medium' } };
    guest.submit('run-example', token, request);
    guest.submit('run-example', token, request);
    assert.throws(() => guest.submit('run-example', 'incorrect-token', request), /token/);
    assert.throws(() => guest.submit('run-example', token, { ...request, context: { systemPrompt: 'Different', messages: [] } }), /reused/);

    const restarted = new FleetModelBrokerQueue(root);
    assert.equal(restarted.token('run-example'), token);
    const pending = restarted.poll('run-example');
    assert.deepEqual(pending.request, request);
    assert.equal(restarted.poll('run-example'), null);
    const statePath = join(root, 'run-example', 'request-example.state.json');
    const state = JSON.parse(readFileSync(statePath, 'utf8'));
    writeFileSync(statePath, JSON.stringify({ ...state, leaseUntil: Date.now() - 1 }));
    const recovered = new FleetModelBrokerQueue(root).poll('run-example');
    assert.equal(recovered.request.id, request.id);
    assert.notEqual(recovered.leaseId, pending.leaseId);
    assert.throws(() => restarted.renew('run-example', request.id, pending.leaseId), /lease/);
    restarted.renew('run-example', request.id, recovered.leaseId);
    const result = Buffer.from(JSON.stringify({ state: 'done', message: { role: 'assistant', content: [], model: 'model-example', stopReason: 'stop' } }));
    restarted.beginResult('run-example', request.id, recovered.leaseId);
    restarted.appendResult('run-example', request.id, recovered.leaseId, 0, result.toString('base64'));
    restarted.sealResult('run-example', request.id, recovered.leaseId, createHash('sha256').update(result).digest('hex'));
    assert.equal(new FleetModelBrokerQueue(root).result('run-example', token, request.id).message.model, 'model-example');
    assert.equal(restarted.poll('run-example'), null);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
