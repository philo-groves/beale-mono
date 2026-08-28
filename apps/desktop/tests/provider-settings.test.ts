import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it, vi } from 'vitest';
import { ProviderCredentialStore } from '../src/main/providerCredentialStore';
import { WorkspaceRegistry } from '../src/main/workspaceRegistry';
import { WorkspaceService } from '../src/main/workspaceService';

describe('provider settings', () => {
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
