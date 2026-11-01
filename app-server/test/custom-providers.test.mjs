import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { WorkspaceRegistry } from '../dist/workspaceRegistryStore.js';
import { AppServerHostRegistry } from '../dist/hostRegistry.js';
import { AppServerHostService } from '../dist/hostService.js';
import { appServerSessionEnvironment } from '../dist/sessionLaunch.js';

test('custom provider metadata persists in the registry and reaches the host catalog', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-custom-provider-'));
  const previous = process.env.APP_SERVER_CUSTOM_PROVIDERS;
  const definition = { id: 'custom-example', name: 'Example API', baseUrl: 'https://api.example.test/v1', contextWindow: 128_000, maxTokens: 16_000,
    models: [{ id: 'model-example', name: 'Example Model' }] };
  try {
    const registry = new WorkspaceRegistry(root);
    registry.saveCustomProvider(definition);
    registry.setProviderModelDefaults(definition.id, { largeModel: 'model-example', smallModel: 'model-example', reasoningEffort: 'off' });
    registry.setDefaultProviderId(definition.id);
    assert.deepEqual(registry.getProviderSettings().customProviders, [definition]);
    registry.close();

    const host = new AppServerHostRegistry({ registryDirectory: root });
    assert.deepEqual(host.providerSettings().customProviders, [definition]);
    assert.equal(host.providerSettings().defaultProviderId, definition.id);
    assert.deepEqual(JSON.parse(process.env.APP_SERVER_CUSTOM_PROVIDERS), [definition]);
    const catalog = await new AppServerHostService({ registry: host }).providerCatalog();
    assert.equal(catalog.defaultProviderId, definition.id);
    assert.deepEqual(catalog.providers.find((provider) => provider.providerId === definition.id)?.models.map((model) => model.id), ['model-example']);
    const env = appServerSessionEnvironment({
      provider: { contextSize: 'default', authenticationPreferences: {}, riskAcknowledgements: [] },
      storage: { databasePath: join(root, 'memory.sqlite'), artifactDirectoryPath: join(root, 'artifacts') },
    }, { APP_SERVER_CUSTOM_PROVIDERS: JSON.stringify([definition]), BEALE_CUSTOM_PROVIDER_KEY_EXAMPLE: 'example-secret' });
    assert.deepEqual(JSON.parse(env.APP_SERVER_CUSTOM_PROVIDERS), [definition]);
    assert.equal(env.BEALE_CUSTOM_PROVIDER_KEY_EXAMPLE, 'example-secret');

    const reopened = new WorkspaceRegistry(root);
    assert.equal(reopened.getProviderSettings().modelDefaults[definition.id].largeModel, 'model-example');
    reopened.removeCustomProvider(definition.id);
    assert.equal(reopened.getProviderSettings().defaultProviderId, null);
    assert.deepEqual(reopened.getProviderSettings().customProviders, []);
    reopened.close();
  } finally {
    if (previous === undefined) delete process.env.APP_SERVER_CUSTOM_PROVIDERS;
    else process.env.APP_SERVER_CUSTOM_PROVIDERS = previous;
    rmSync(root, { recursive: true, force: true });
  }
});

test('reasoning custom providers persist and accept reasoning defaults', () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-custom-provider-reasoning-'));
  const reasoningDefinition = { id: 'custom-reasoning', name: 'Reasoning API', baseUrl: 'https://api.example.test/v1',
    contextWindow: 128_000, maxTokens: 16_000, reasoning: true,
    models: [{ id: 'deepseek-v4.1-flash', name: 'DeepSeek V4.1 Flash' }] };
  const flatDefinition = { id: 'custom-flat', name: 'Flat API', baseUrl: 'https://api.example.test/v1',
    contextWindow: 128_000, maxTokens: 16_000,
    models: [{ id: 'model-example', name: 'Example Model' }] };
  try {
    const registry = new WorkspaceRegistry(root);
    registry.saveCustomProvider(reasoningDefinition);
    registry.saveCustomProvider(flatDefinition);
    assert.deepEqual(registry.getProviderSettings().customProviders, [reasoningDefinition, flatDefinition]);
    registry.setProviderModelDefaults(reasoningDefinition.id,
      { largeModel: 'deepseek-v4.1-flash', smallModel: 'deepseek-v4.1-flash', reasoningEffort: 'high' });
    assert.equal(registry.getProviderSettings().modelDefaults[reasoningDefinition.id].reasoningEffort, 'high');
    assert.throws(() => registry.setProviderModelDefaults(reasoningDefinition.id,
      { largeModel: 'deepseek-v4.1-flash', smallModel: 'deepseek-v4.1-flash', reasoningEffort: 'medium' }), /supported reasoning effort/);
    assert.throws(() => registry.setProviderModelDefaults(flatDefinition.id,
      { largeModel: 'model-example', smallModel: 'model-example', reasoningEffort: 'high' }), /without reasoning effort/);
    registry.close();
    const reopened = new WorkspaceRegistry(root);
    assert.equal(reopened.getProviderSettings().customProviders?.[0].reasoning, true);
    assert.equal(reopened.getProviderSettings().modelDefaults[reasoningDefinition.id].reasoningEffort, 'high');
    reopened.close();
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('custom provider model allowlists persist and gate model defaults', () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-custom-provider-allowlist-'));
  const restricted = { id: 'custom-restricted', name: 'Restricted API', baseUrl: 'https://api.example.test/v1',
    contextWindow: 128_000, maxTokens: 16_000,
    models: [{ id: 'model-one', name: 'Model One' }, { id: 'model-two', name: 'Model Two' }],
    modelAllowlist: ['model-two'] };
  try {
    const registry = new WorkspaceRegistry(root);
    registry.saveCustomProvider(restricted);
    const saved = registry.getProviderSettings().customProviders?.[0];
    assert.deepEqual(saved?.models, [{ id: 'model-two', name: 'Model Two' }]);
    assert.deepEqual(saved?.modelAllowlist, ['model-two']);
    registry.setProviderModelDefaults(restricted.id,
      { largeModel: 'model-two', smallModel: 'model-two', reasoningEffort: 'off' });
    assert.throws(() => registry.setProviderModelDefaults(restricted.id,
      { largeModel: 'model-one', smallModel: 'model-one', reasoningEffort: 'off' }), /available models/);
    registry.close();
    const reopened = new WorkspaceRegistry(root);
    assert.deepEqual(reopened.getProviderSettings().customProviders?.[0].models, [{ id: 'model-two', name: 'Model Two' }]);
    reopened.close();
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
