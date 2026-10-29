import assert from 'node:assert/strict';
import test from 'node:test';
import { createServer } from 'node:http';
import { once } from 'node:events';
import {
  createAuthenticatedModels,
  customProviderKeyEnvironmentVariable,
  discoverCustomProviderModels,
  getProviderModelCatalog,
  normalizeCustomProviderDefinition,
  readCustomProviderDefinitions,
  verifyProviderAuth,
  decodeResearchCollaborationConfig,
} from '../packages/research-agent/dist/index.js';

const definition = {
  id: 'custom-example',
  name: 'Example API',
  baseUrl: 'https://api.example.test/v1',
  contextWindow: 128_000,
  maxTokens: 16_000,
  models: [{ id: 'model-example', name: 'Example Model' }],
};

test('custom provider definitions reject embedded credentials and duplicate models', () => {
  assert.deepEqual(normalizeCustomProviderDefinition(definition), definition);
  assert.deepEqual(normalizeCustomProviderDefinition({ ...definition, apiKey: 'example-secret' }), definition);
  assert.deepEqual(normalizeCustomProviderDefinition({ ...definition, reasoning: false }), definition);
  assert.deepEqual(normalizeCustomProviderDefinition({ ...definition, reasoning: true }), { ...definition, reasoning: true });
  assert.throws(() => normalizeCustomProviderDefinition({ ...definition, baseUrl: 'https://secret@api.example.test/v1' }), /base URL/);
  assert.throws(() => normalizeCustomProviderDefinition({ ...definition, models: [definition.models[0], definition.models[0]] }), /Duplicate/);
  assert.deepEqual(readCustomProviderDefinitions(JSON.stringify([definition])), [definition]);
});

test('reasoning custom providers register DeepSeek thinking compatibility and effort levels', () => {
  const previousDefinitions = process.env.APP_SERVER_CUSTOM_PROVIDERS;
  const reasoningDefinition = { ...definition, reasoning: true,
    models: [{ id: 'deepseek-v4.1-flash', name: 'DeepSeek V4.1 Flash' }] };
  try {
    process.env.APP_SERVER_CUSTOM_PROVIDERS = JSON.stringify([reasoningDefinition]);
    const models = createAuthenticatedModels();
    const model = models.getModel(definition.id, 'deepseek-v4.1-flash');
    assert.equal(model?.reasoning, true);
    assert.equal(model?.compat?.thinkingFormat, 'deepseek');
    assert.equal(model?.compat?.maxTokensField, 'max_tokens');
    assert.equal(model?.compat?.supportsReasoningEffort, true);
    assert.equal(model?.compat?.requiresReasoningContentOnAssistantMessages, true);
    assert.deepEqual(model?.thinkingLevelMap, { minimal: null, low: 'low', medium: null, high: 'high', max: 'max' });
    const catalogModel = getProviderModelCatalog(definition.id)[0].models[0];
    assert.equal(catalogModel.reasoning, true);
    assert.deepEqual(catalogModel.effortLevels, ['off', 'low', 'high', 'max']);
  } finally {
    if (previousDefinitions === undefined) delete process.env.APP_SERVER_CUSTOM_PROVIDERS;
    else process.env.APP_SERVER_CUSTOM_PROVIDERS = previousDefinitions;
  }
});

test('non-reasoning custom providers stay flat', () => {
  const previousDefinitions = process.env.APP_SERVER_CUSTOM_PROVIDERS;
  try {
    process.env.APP_SERVER_CUSTOM_PROVIDERS = JSON.stringify([definition]);
    const models = createAuthenticatedModels();
    const model = models.getModel(definition.id, 'model-example');
    assert.equal(model?.reasoning, false);
    assert.equal(model?.compat?.supportsReasoningEffort, false);
    assert.equal(model?.compat?.thinkingFormat, undefined);
    assert.deepEqual(getProviderModelCatalog(definition.id)[0].models[0].effortLevels, ['off']);
  } finally {
    if (previousDefinitions === undefined) delete process.env.APP_SERVER_CUSTOM_PROVIDERS;
    else process.env.APP_SERVER_CUSTOM_PROVIDERS = previousDefinitions;
  }
});

test('custom collaborators may use off reasoning effort', () => {
  const config = decodeResearchCollaborationConfig({ mode: 'always', providers: [
    { provider: 'custom-example', model: 'model-example', reasoningEffort: 'off', enabled: true }
  ], maxConcurrentRooms: 2, maxMembersPerRoom: 2 });
  assert.equal(config.providers[0].reasoningEffort, 'off');
});

test('model discovery uses the configured endpoint and bearer key without redirects', async () => {
  const models = await discoverCustomProviderModels(definition.baseUrl, 'example-secret', async (url, init) => {
    assert.equal(url, 'https://api.example.test/v1/models');
    assert.equal(init.headers.Authorization, 'Bearer example-secret');
    assert.equal(init.redirect, 'error');
    return new Response(JSON.stringify({ data: [
      { id: 'model-one', name: 'Model One' },
      { id: 'model-two' },
      { id: 'model-one', name: 'Duplicate' },
    ] }), { status: 200 });
  });
  assert.deepEqual(models, [
    { id: 'model-one', name: 'Model One' },
    { id: 'model-two', name: 'model-two' },
  ]);
});

test('custom models join the catalog and authenticate through a host-only key', async () => {
  const previousDefinitions = process.env.APP_SERVER_CUSTOM_PROVIDERS;
  const keyVariable = customProviderKeyEnvironmentVariable(definition.id);
  const previousKey = process.env[keyVariable];
  try {
    process.env.APP_SERVER_CUSTOM_PROVIDERS = JSON.stringify([definition]);
    process.env[keyVariable] = 'example-secret';
    const models = createAuthenticatedModels();
    const model = models.getModel(definition.id, 'model-example');
    assert.equal(model?.api, 'openai-completions');
    assert.equal(model?.baseUrl, definition.baseUrl);
    assert.equal((await models.getAuth(definition.id))?.auth.apiKey, 'example-secret');
    assert.equal(getProviderModelCatalog(definition.id)[0].models[0].id, 'model-example');
    assert.equal((await verifyProviderAuth(definition.id, 'model-example')).configured, true);
    delete process.env[keyVariable];
    assert.equal((await verifyProviderAuth(definition.id, 'model-example')).configured, true);
  } finally {
    if (previousDefinitions === undefined) delete process.env.APP_SERVER_CUSTOM_PROVIDERS;
    else process.env.APP_SERVER_CUSTOM_PROVIDERS = previousDefinitions;
    if (previousKey === undefined) delete process.env[keyVariable];
    else process.env[keyVariable] = previousKey;
  }
});

test('custom provider sends a Chat Completions request with its host key', async () => {
  const requests = [];
  const server = createServer(async (request, response) => {
    requests.push({ path: request.url, authorization: request.headers.authorization,
      body: JSON.parse((await Array.fromAsync(request)).map((chunk) => chunk.toString()).join('')) });
    response.writeHead(200, { 'content-type': 'text/event-stream' });
    response.write('data: {"id":"chatcmpl-example","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":"Example response"},"finish_reason":null}]}\n\n');
    response.write('data: {"id":"chatcmpl-example","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}\n\n');
    response.end('data: [DONE]\n\n');
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const address = server.address();
  assert.ok(address && typeof address !== 'string');
  const priorDefinitions = process.env.APP_SERVER_CUSTOM_PROVIDERS;
  const variable = customProviderKeyEnvironmentVariable(definition.id);
  const priorKey = process.env[variable];
  try {
    process.env.APP_SERVER_CUSTOM_PROVIDERS = JSON.stringify([{ ...definition,
      baseUrl: `http://127.0.0.1:${address.port}/v1` }]);
    process.env[variable] = 'example-secret';
    const models = createAuthenticatedModels();
    const model = models.getModel(definition.id, 'model-example');
    assert.ok(model);
    const result = await models.complete(model, { messages: [{ role: 'user', content: 'Hello', timestamp: Date.now() }] });
    assert.equal(result.content[0].text, 'Example response');
    assert.equal(requests[0].path, '/v1/chat/completions');
    assert.equal(requests[0].authorization, 'Bearer example-secret');
    assert.equal(requests[0].body.model, 'model-example');
  } finally {
    if (priorDefinitions === undefined) delete process.env.APP_SERVER_CUSTOM_PROVIDERS;
    else process.env.APP_SERVER_CUSTOM_PROVIDERS = priorDefinitions;
    if (priorKey === undefined) delete process.env[variable];
    else process.env[variable] = priorKey;
    server.close();
  }
});

test('reasoning custom provider sends thinking controls and echoes reasoning content', async () => {
  const requests = [];
  const server = createServer(async (request, response) => {
    requests.push({ body: JSON.parse((await Array.fromAsync(request)).map((chunk) => chunk.toString()).join('')) });
    response.writeHead(200, { 'content-type': 'text/event-stream' });
    response.write('data: {"id":"chatcmpl-example","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"reasoning_content":"Example reasoning"},"finish_reason":null}]}\n\n');
    response.write('data: {"id":"chatcmpl-example","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":"Example response"},"finish_reason":null}]}\n\n');
    if (requests.length === 1) {
      response.write('data: {"id":"chatcmpl-example","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_example","type":"function","function":{"name":"example_tool","arguments":"{}"}}]},"finish_reason":null}]}\n\n');
    }
    response.write('data: {"id":"chatcmpl-example","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}\n\n');
    response.end('data: [DONE]\n\n');
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const address = server.address();
  assert.ok(address && typeof address !== 'string');
  const priorDefinitions = process.env.APP_SERVER_CUSTOM_PROVIDERS;
  const variable = customProviderKeyEnvironmentVariable(definition.id);
  const priorKey = process.env[variable];
  const reasoningDefinition = { ...definition, reasoning: true,
    baseUrl: `http://127.0.0.1:${address.port}/v1`,
    models: [{ id: 'deepseek-v4.1-flash', name: 'DeepSeek V4.1 Flash' }] };
  try {
    process.env.APP_SERVER_CUSTOM_PROVIDERS = JSON.stringify([reasoningDefinition]);
    process.env[variable] = 'example-secret';
    const models = createAuthenticatedModels();
    const model = models.getModel(definition.id, 'deepseek-v4.1-flash');
    assert.ok(model);
    const tools = [{ name: 'example_tool', description: 'Return a synthetic example.', parameters: { type: 'object', properties: {} } }];
    const result = await models.completeSimple(model, { tools, messages: [
      { role: 'user', content: 'Hello', timestamp: Date.now() },
      { role: 'user', content: 'Again', timestamp: Date.now() },
    ] }, { reasoning: 'high' });
    assert.notEqual(result.stopReason, 'error', result.errorMessage);
    assert.equal(result.content.find((part) => part.type === 'text')?.text, 'Example response');
    assert.equal(result.content.find((part) => part.type === 'thinking')?.thinking, 'Example reasoning');
    assert.deepEqual(requests[0].body.thinking, { type: 'enabled' });
    assert.equal(requests[0].body.reasoning_effort, 'high');
    assert.equal(requests[0].body.max_tokens, reasoningDefinition.maxTokens);
    assert.equal('reasoning' in requests[0].body, false);
    assert.equal(result.content.find((part) => part.type === 'toolCall')?.name, 'example_tool');
    for (const effort of ['off', 'low', 'max']) {
      await models.completeSimple(model, { tools, messages: [
        { role: 'user', content: 'Hello', timestamp: Date.now() }, result,
        { role: 'toolResult', toolCallId: 'call_example', toolName: 'example_tool', content: [{ type: 'text', text: 'Example output' }], isError: false, timestamp: Date.now() },
        { role: 'user', content: 'Again', timestamp: Date.now() },
      ] }, { reasoning: effort });
      const body = requests.at(-1).body;
      assert.deepEqual(body.thinking, { type: effort === 'off' ? 'disabled' : 'enabled' });
      assert.equal(body.reasoning_effort, effort === 'off' ? undefined : effort);
      assert.equal(body.messages.find((message) => message.role === 'assistant')?.reasoning_content, 'Example reasoning');
    }
  } finally {
    if (priorDefinitions === undefined) delete process.env.APP_SERVER_CUSTOM_PROVIDERS;
    else process.env.APP_SERVER_CUSTOM_PROVIDERS = priorDefinitions;
    if (priorKey === undefined) delete process.env[variable];
    else process.env[variable] = priorKey;
    server.close();
  }
});
