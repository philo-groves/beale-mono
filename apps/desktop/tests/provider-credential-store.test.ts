import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  ProviderCredentialStore,
  unlockProviderApiKeysForAppServerStartup
} from '../src/main/providerCredentialStore';

const originalEnvironment = {
  OPENAI_API_KEY: process.env.OPENAI_API_KEY,
  ANTHROPIC_API_KEY: process.env.ANTHROPIC_API_KEY,
  XAI_API_KEY: process.env.XAI_API_KEY,
  ZAI_API_KEY: process.env.ZAI_API_KEY,
  OPENROUTER_API_KEY: process.env.OPENROUTER_API_KEY
};
const directories: string[] = [];

afterEach(() => {
  restoreEnvironment('OPENAI_API_KEY');
  restoreEnvironment('ANTHROPIC_API_KEY');
  restoreEnvironment('XAI_API_KEY');
  restoreEnvironment('ZAI_API_KEY');
  restoreEnvironment('OPENROUTER_API_KEY');
  for (const directory of directories.splice(0)) rmSync(directory, { recursive: true, force: true });
});

describe('provider credential store', () => {
  it('persists encrypted API keys and restores them without writing plaintext', () => {
    delete process.env.XAI_API_KEY;
    const directory = mkdtempSync(join(tmpdir(), 'beale-provider-credentials-'));
    directories.push(directory);
    const path = join(directory, 'credentials.json');
    const encryption = {
      available: () => true,
      encrypt: (value: string) => Buffer.from(`encrypted:${[...value].reverse().join('')}`, 'utf8'),
      decrypt: (value: Buffer) => [...value.toString('utf8').replace(/^encrypted:/u, '')].reverse().join('')
    };
    const key = 'xai-test-secret-value';

    new ProviderCredentialStore(path, encryption).setApiKey('xai', key);

    expect(process.env.XAI_API_KEY).toBe(key);
    expect(readFileSync(path, 'utf8')).not.toContain(key);
    delete process.env.XAI_API_KEY;
    const restored = new ProviderCredentialStore(path, encryption);
    expect(process.env.XAI_API_KEY).toBeUndefined();
    restored.unlockApiKeys(['xai']);
    expect(process.env.XAI_API_KEY).toBe(key);
  });

  it('maps Z.ai API-key credentials to the dedicated host environment variable', () => {
    delete process.env.ZAI_API_KEY;
    const store = new ProviderCredentialStore();
    store.setApiKey('zai', 'zai-test-secret');
    expect(process.env.ZAI_API_KEY).toBe('zai-test-secret');
    store.removeApiKey('zai');
    expect(process.env.ZAI_API_KEY).toBeUndefined();
  });

  it('maps OpenRouter credentials to OPENROUTER_API_KEY', () => {
    delete process.env.OPENROUTER_API_KEY;
    const store = new ProviderCredentialStore();
    store.setApiKey('openrouter', 'openrouter-test-secret');
    expect(process.env.OPENROUTER_API_KEY).toBe('openrouter-test-secret');
    store.removeApiKey('openrouter');
    expect(process.env.OPENROUTER_API_KEY).toBeUndefined();
  });

  it('reads credential metadata without decryption and unlocks only the requested provider', () => {
    delete process.env.XAI_API_KEY;
    delete process.env.ANTHROPIC_API_KEY;
    const directory = mkdtempSync(join(tmpdir(), 'beale-provider-credentials-'));
    directories.push(directory);
    const path = join(directory, 'credentials.json');
    let decryptions = 0;
    const encryption = {
      available: () => true,
      encrypt: (value: string) => Buffer.from(value, 'utf8'),
      decrypt: (value: Buffer) => {
        decryptions += 1;
        return value.toString('utf8');
      }
    };
    const saved = new ProviderCredentialStore(path, encryption);
    saved.setApiKey('xai', 'deferred-key');
    saved.setApiKey('anthropic', 'unused-key');
    delete process.env.XAI_API_KEY;
    delete process.env.ANTHROPIC_API_KEY;

    const store = new ProviderCredentialStore(path, encryption, { deferLoad: true });
    expect(decryptions).toBe(0);
    expect(process.env.XAI_API_KEY).toBeUndefined();

    expect(store.hasManagedApiKeys()).toBe(true);
    expect(store.isApiKeyConfigured('xai')).toBe(true);
    expect(store.providersRequiringUnlock(['xai'])).toEqual(['xai']);
    expect(decryptions).toBe(0);
    expect(process.env.XAI_API_KEY).toBeUndefined();
    expect(process.env.ANTHROPIC_API_KEY).toBeUndefined();

    expect(store.unlockApiKeys(['xai'])).toBe(true);
    expect(decryptions).toBe(1);
    expect(process.env.XAI_API_KEY).toBe('deferred-key');
    expect(process.env.ANTHROPIC_API_KEY).toBeUndefined();
    expect(store.providersRequiringUnlock(['xai', 'anthropic'])).toEqual(['anthropic']);
  });

  it('unlocks saved keys before app-server startup on Windows only', () => {
    delete process.env.XAI_API_KEY;
    const directory = mkdtempSync(join(tmpdir(), 'beale-provider-credentials-'));
    directories.push(directory);
    const path = join(directory, 'credentials.json');
    const decrypt = vi.fn((value: Buffer) => value.toString('utf8'));
    const encryption = {
      available: () => true,
      encrypt: (value: string) => Buffer.from(value, 'utf8'),
      decrypt
    };
    new ProviderCredentialStore(path, encryption).setApiKey('xai', 'windows-startup-key');
    delete process.env.XAI_API_KEY;

    const macosStore = new ProviderCredentialStore(path, encryption, { deferLoad: true });
    expect(unlockProviderApiKeysForAppServerStartup(macosStore, 'darwin')).toBe(false);
    expect(decrypt).not.toHaveBeenCalled();

    const windowsStore = new ProviderCredentialStore(path, encryption, { deferLoad: true });
    expect(unlockProviderApiKeysForAppServerStartup(windowsStore, 'win32')).toBe(true);
    expect(decrypt).toHaveBeenCalledOnce();
    expect(process.env.XAI_API_KEY).toBe('windows-startup-key');
  });

  it('does not initialize secure storage when no managed credential file exists', () => {
    const directory = mkdtempSync(join(tmpdir(), 'beale-provider-credentials-'));
    directories.push(directory);
    let availabilityChecks = 0;
    const store = new ProviderCredentialStore(join(directory, 'missing.json'), {
      available: () => {
        availabilityChecks += 1;
        return true;
      },
      encrypt: (value: string) => Buffer.from(value, 'utf8'),
      decrypt: (value: Buffer) => value.toString('utf8')
    }, { deferLoad: true });

    expect(store.hasManagedApiKeys()).toBe(false);

    expect(availabilityChecks).toBe(0);
  });

  it('does not initialize secure storage for an empty managed credential file', () => {
    const directory = mkdtempSync(join(tmpdir(), 'beale-provider-credentials-'));
    directories.push(directory);
    const path = join(directory, 'credentials.json');
    writeFileSync(path, JSON.stringify({ version: 1, apiKeys: {} }));
    let availabilityChecks = 0;
    const store = new ProviderCredentialStore(path, {
      available: () => {
        availabilityChecks += 1;
        return true;
      },
      encrypt: (value: string) => Buffer.from(value, 'utf8'),
      decrypt: (value: Buffer) => value.toString('utf8')
    }, { deferLoad: true });

    expect(store.hasManagedApiKeys()).toBe(false);

    expect(availabilityChecks).toBe(0);
  });

  it('removes the credential file after the final managed API key is removed', () => {
    delete process.env.XAI_API_KEY;
    const directory = mkdtempSync(join(tmpdir(), 'beale-provider-credentials-'));
    directories.push(directory);
    const path = join(directory, 'credentials.json');
    const encryption = {
      available: () => true,
      encrypt: (value: string) => Buffer.from(value, 'utf8'),
      decrypt: (value: Buffer) => value.toString('utf8')
    };
    const store = new ProviderCredentialStore(path, encryption, { deferLoad: true });
    store.setApiKey('xai', 'managed-key');
    expect(existsSync(path)).toBe(true);

    store.removeApiKey('xai');

    expect(existsSync(path)).toBe(false);
  });

  it('removes only Beale-managed keys and preserves host-environment ownership', () => {
    delete process.env.OPENAI_API_KEY;
    const store = new ProviderCredentialStore();
    store.setApiKey('openai-codex', 'managed-key');
    store.removeApiKey('openai-codex');
    expect(process.env.OPENAI_API_KEY).toBeUndefined();

    process.env.ANTHROPIC_API_KEY = 'environment-key';
    const environmentStore = new ProviderCredentialStore();
    expect(() => environmentStore.removeApiKey('anthropic')).toThrow('comes from the host environment');
    expect(process.env.ANTHROPIC_API_KEY).toBe('environment-key');
  });
});

function restoreEnvironment(name: keyof typeof originalEnvironment): void {
  const value = originalEnvironment[name];
  if (value === undefined) delete process.env[name];
  else process.env[name] = value;
}
