import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { createServer } from 'node:http';
import { once } from 'node:events';
import { join } from 'node:path';
import { describe, expect, it, vi } from 'vitest';
import { ProviderCredentialStore } from '../src/main/providerCredentialStore';
import { WorkspaceRegistry } from '../src/main/workspaceRegistry';
import { WorkspaceService } from '../src/main/workspaceService';
import { appServerProcessEnvironment } from '../src/main/appServerRunEngine';

describe('provider settings', () => {
  it('discovers and persists a custom provider while encrypting its API key', async () => {
    const root = mkdtempSync(join(tmpdir(), 'beale-custom-provider-settings-'));
    const path = join(root, 'credentials.json');
    const stateFile = join(root, 'app-server.json');
    const priorEnvironment = {
      stateFile: process.env.BEALE_APP_SERVER_STATE_FILE,
      parentPid: process.env.BEALE_APP_SERVER_PARENT_PID,
      port: process.env.BEALE_APP_SERVER_PORT
    };
    process.env.BEALE_APP_SERVER_STATE_FILE = stateFile;
    process.env.BEALE_APP_SERVER_PARENT_PID = String(process.pid);
    process.env.BEALE_APP_SERVER_PORT = '0';
    let variable: string | undefined;
    const server = createServer((request, response) => {
      expect(request.url).toBe('/v1/models');
      expect(request.headers.authorization).toBe('Bearer example-secret');
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify({ data: [{ id: 'model-example', name: 'Example Model' }] }));
    });
    server.listen(0, '127.0.0.1');
    await once(server, 'listening');
    const address = server.address();
    if (!address || typeof address === 'string') throw new Error('Expected a TCP listener.');
    const baseUrl = `http://127.0.0.1:${address.port}/v1`;
    const credentials = new ProviderCredentialStore(path, {
      available: () => true,
      encrypt: (value) => Buffer.from(`encrypted:${value}`, 'utf8'),
      decrypt: (value) => value.toString('utf8').replace(/^encrypted:/u, '')
    });
    let environmentChanges = 0;
    const providerEnvironmentChanged = (): void => { environmentChanges += 1; };
    const service = new WorkspaceService(() => undefined, {
      workspaceRegistryDirectory: join(root, 'registry'),
      providerCredentialStore: credentials,
      providerEnvironmentChanged
    });
    try {
      const settings = await service.saveCustomProvider({ name: 'Example API', baseUrl, apiKey: 'example-secret' });
      const provider = settings.customProviders?.[0];
      expect(provider?.id).toMatch(/^custom-/u);
      variable = `BEALE_CUSTOM_PROVIDER_KEY_${provider!.id.slice(7).replaceAll('-', '_').toUpperCase()}`;
      expect(provider?.models).toEqual([{ id: 'model-example', name: 'Example Model' }]);
      expect(settings.modelDefaults[provider!.id]).toEqual({ largeModel: 'model-example', smallModel: 'model-example', reasoningEffort: 'off' });
      expect(readFileSync(path, 'utf8')).not.toContain('example-secret');
      expect(environmentChanges).toBe(1);
      expect((await service.getResearchProviderModelCatalog()).find((catalog) => catalog.providerId === provider!.id)?.models[0]?.id).toBe('model-example');
      await service.saveCustomProvider({ id: provider!.id, name: 'Example API', baseUrl, reasoning: true });
      service.setProviderModelDefaults(provider!.id, { largeModel: 'model-example', smallModel: 'model-example', reasoningEffort: 'high' });
      const refreshed = await service.saveCustomProvider({ id: provider!.id, name: 'Example API', baseUrl });
      expect(refreshed.customProviders?.[0]?.reasoning).toBe(true);
      expect(refreshed.modelDefaults[provider!.id]?.reasoningEffort).toBe('high');
      const catalog = (await service.getResearchProviderModelCatalog()).find((entry) => entry.providerId === provider!.id);
      expect(catalog?.models[0]?.effortLevels).toContain('high');
      const disabled = await service.saveCustomProvider({ id: provider!.id, name: 'Example API', baseUrl, reasoning: false });
      expect(disabled.modelDefaults[provider!.id]?.reasoningEffort).toBe('off');
    } finally {
      server.close();
      service.close();
      if (variable) delete process.env[variable];
      if (existsSync(stateFile)) {
        const record = JSON.parse(readFileSync(stateFile, 'utf8')) as { pid?: unknown };
        if (typeof record.pid === 'number' && record.pid > 0 && record.pid !== process.pid) {
          try { process.kill(record.pid); } catch { /* already stopped */ }
        }
      }
      for (const [key, value] of Object.entries({
        BEALE_APP_SERVER_STATE_FILE: priorEnvironment.stateFile,
        BEALE_APP_SERVER_PARENT_PID: priorEnvironment.parentPid,
        BEALE_APP_SERVER_PORT: priorEnvironment.port
      })) {
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
      }
      rmSync(root, { recursive: true, force: true });
    }
  });
  it('passes custom provider metadata to direct app-server completions without embedding keys', () => {
    const definition = { id: 'custom-example' as const, name: 'Example API', baseUrl: 'https://api.example.test/v1',
      contextWindow: 128_000, maxTokens: 16_000, models: [{ id: 'model-example', name: 'Example Model' }] };
    const env = appServerProcessEnvironment(null, {}, [definition]);
    expect(JSON.parse(env.APP_SERVER_CUSTOM_PROVIDERS ?? 'null')).toEqual([definition]);
    expect(env.APP_SERVER_CUSTOM_PROVIDERS).not.toContain('example-secret');
  });
  it('unlocks only selected session providers after an explicit access request', async () => {
    const root = mkdtempSync(join(tmpdir(), 'beale-provider-access-'));
    const path = join(root, 'credentials.json');
    const originalXaiApiKey = process.env.XAI_API_KEY;
    const decrypt = vi.fn((value: Buffer) => value.toString('utf8'));
    const encryption = {
      available: () => true,
      encrypt: (value: string) => Buffer.from(value, 'utf8'),
      decrypt
    };
    const saved = new ProviderCredentialStore(path, encryption);
    saved.setApiKey('xai', 'saved-xai-key');
    delete process.env.XAI_API_KEY;
    const credentialStore = new ProviderCredentialStore(path, encryption, { deferLoad: true });
    const providerEnvironmentChanged = vi.fn();
    const service = new WorkspaceService(() => undefined, {
      workspaceRegistryDirectory: join(root, 'registry'),
      providerCredentialStore: credentialStore,
      providerEnvironmentChanged
    });

    try {
      expect(service.getProviderCredentialAccessRequest(['anthropic'])).toEqual({ providerIds: [] });
      expect(service.getProviderCredentialAccessRequest(['xai', 'anthropic'])).toEqual({ providerIds: ['xai'] });
      expect(decrypt).not.toHaveBeenCalled();

      await service.unlockProviderApiKeys(['xai']);

      expect(decrypt).toHaveBeenCalledOnce();
      expect(process.env.XAI_API_KEY).toBe('saved-xai-key');
      expect(providerEnvironmentChanged).toHaveBeenCalledOnce();
      expect(service.getProviderCredentialAccessRequest(['xai'])).toEqual({ providerIds: [] });
    } finally {
      service.close();
      if (originalXaiApiKey === undefined) delete process.env.XAI_API_KEY;
      else process.env.XAI_API_KEY = originalXaiApiKey;
      rmSync(root, { recursive: true, force: true });
    }
  });

  it('returns and persists OpenRouter API-key authentication and refreshes the run host', async () => {
    const root = mkdtempSync(join(tmpdir(), 'beale-provider-settings-'));
    const setApiKey = vi.fn();
    const providerEnvironmentChanged = vi.fn();
    const credentialStore = { setApiKey } as unknown as ProviderCredentialStore;
    const service = new WorkspaceService(() => undefined, {
      workspaceRegistryDirectory: join(root, 'registry'),
      providerCredentialStore: credentialStore,
      providerEnvironmentChanged
    });

    try {
      const settings = await service.configureProviderApiKey('openrouter', 'test-openrouter-key');

      expect(setApiKey).toHaveBeenCalledWith('openrouter', 'test-openrouter-key');
      expect(providerEnvironmentChanged).toHaveBeenCalledOnce();
      expect(settings.preferredAuthenticationMethods?.openrouter).toBe('api_key');
      service.close();

      const registry = new WorkspaceRegistry(join(root, 'registry'));
      try {
        expect(registry.getProviderSettings().preferredAuthenticationMethods?.openrouter).toBe('api_key');
      } finally {
        registry.close();
      }
    } finally {
      service.close();
      rmSync(root, { recursive: true, force: true });
    }
  });
});
