import assert from 'node:assert/strict';
import test from 'node:test';
import { brokeredModels, ProviderAuthenticationRouter, resolveResearchModelConfig, verifyBrokerModelAccess } from '../packages/research-agent/dist/index.js';

test('brokered models retry a lost connection with the same request ID and omit guest credentials', async () => {
  const requests = [];
  let droppedSubmit = false;
  let droppedResult = false;
  const responseMessage = { role: 'assistant', content: [{ type: 'text', text: 'Synthetic completion' }],
    api: 'openai-codex-responses', provider: 'openai-codex', model: 'model-example',
    stopReason: 'stop', usage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 2,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } }, timestamp: Date.now() };
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (_url, init) => {
    if (init?.method === 'POST') {
      requests.push(JSON.parse(init.body));
      if (!droppedSubmit) { droppedSubmit = true; throw new TypeError('Synthetic connection reset'); }
      return Response.json({});
    }
    if (!droppedResult) { droppedResult = true; throw new TypeError('Synthetic connection reset'); }
    return Response.json({ state: 'done', message: responseMessage });
  };
  try {
    const url = 'http://127.0.0.1:12345/broker';
    const model = { provider: 'openai-codex', id: 'model-example', api: 'openai-codex-responses' };
    const models = brokeredModels({ getModel: () => model }, { url, token: 'synthetic-session-token' });
    const result = await models.completeSimple(model, { systemPrompt: 'Synthetic prompt', messages: [] },
      { apiKey: 'synthetic-guest-secret', headers: { authorization: 'synthetic-guest-header' } });
    assert.equal(result.content[0].text, 'Synthetic completion');
    assert.equal(requests.length, 2);
    assert.equal(requests[0].id, requests[1].id);
    assert.equal(requests[0].options.apiKey, undefined);
    assert.equal(requests[0].options.headers, undefined);
  } finally { globalThis.fetch = originalFetch; }
});

test('brokered runtime model resolution does not require guest provider credentials', async () => {
  const previous = process.env.APP_SERVER_MODEL_BROKER_URL;
  process.env.APP_SERVER_MODEL_BROKER_URL = 'http://127.0.0.1:1/broker';
  try {
    const resolved = await resolveResearchModelConfig({ provider: 'openai-codex', model: 'gpt-6-sol',
      verifyProviderAuth: async () => { throw new Error('Guest auth should not be read.'); } });
    assert.equal(resolved.provider, 'openai-codex');
    assert.equal(resolved.model, 'gpt-6-sol');
  } finally {
    if (previous === undefined) delete process.env.APP_SERVER_MODEL_BROKER_URL;
    else process.env.APP_SERVER_MODEL_BROKER_URL = previous;
  }
});

test('broker preflight rejects subscription SDK paths before a VM is cloned', async () => {
  await assert.rejects(verifyBrokerModelAccess('anthropic', 'model-example', {}), /cannot yet run/);
  await assert.rejects(verifyBrokerModelAccess('zai', 'model-example', { zai: 'subscription' }), /cannot yet run/);
});

test('a brokered guest keeps the primary API-key route without a local key', () => {
  const previousUrl = process.env.APP_SERVER_MODEL_BROKER_URL;
  const previousKey = process.env.ZAI_API_KEY;
  process.env.APP_SERVER_MODEL_BROKER_URL = 'http://127.0.0.1:1/broker';
  delete process.env.ZAI_API_KEY;
  try {
    assert.equal(new ProviderAuthenticationRouter({ zai: 'api_key' }).method('zai'), 'api_key');
  } finally {
    if (previousUrl === undefined) delete process.env.APP_SERVER_MODEL_BROKER_URL;
    else process.env.APP_SERVER_MODEL_BROKER_URL = previousUrl;
    if (previousKey === undefined) delete process.env.ZAI_API_KEY;
    else process.env.ZAI_API_KEY = previousKey;
  }
});
