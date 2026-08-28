import { existsSync, mkdirSync, readFileSync, unlinkSync, writeFileSync } from 'node:fs';
import { dirname } from 'node:path';
import type { ResearchModelProviderId } from '@shared/types';

const API_KEY_ENVIRONMENT_VARIABLES: Readonly<Record<ResearchModelProviderId, string>> = {
  'openai-codex': 'OPENAI_API_KEY',
  anthropic: 'ANTHROPIC_API_KEY',
  xai: 'XAI_API_KEY',
  zai: 'ZAI_API_KEY',
  openrouter: 'OPENROUTER_API_KEY'
};
const MAX_API_KEY_LENGTH = 16_384;

interface EncryptedCredentialFile {
  version: 1;
  apiKeys: Partial<Record<ResearchModelProviderId, string>>;
}

export interface CredentialEncryption {
  available(): boolean;
  encrypt(value: string): Buffer;
  decrypt(value: Buffer): string;
}

export interface ProviderCredentialStoreOptions {
  deferLoad?: boolean;
}

export class ProviderCredentialStore {
  private readonly managedApiKeys = new Map<ResearchModelProviderId, string>();
  private readonly encryptedApiKeys = new Map<ResearchModelProviderId, string>();
  private readonly initialEnvironment = new Map<ResearchModelProviderId, string | undefined>();
  private manifestLoaded = false;

  public constructor(
    private readonly path: string | null = null,
    private readonly encryption: CredentialEncryption | null = null,
    options: ProviderCredentialStoreOptions = {}
  ) {
    for (const providerId of providerIds()) {
      this.initialEnvironment.set(providerId, process.env[environmentVariable(providerId)]);
    }
    if (!options.deferLoad) this.ensureManifestLoaded();
  }

  public setApiKey(providerId: ResearchModelProviderId, apiKey: string): void {
    this.ensureManifestLoaded();
    requireProviderId(providerId);
    const normalized = apiKey.trim();
    if (!normalized) throw new Error('API key is required.');
    if (normalized.length > MAX_API_KEY_LENGTH) throw new Error('API key is too long.');
    const previousManaged = this.managedApiKeys.get(providerId);
    const previousEncrypted = this.encryptedApiKeys.get(providerId);
    const previousEnvironment = process.env[environmentVariable(providerId)];
    let encrypted: string | undefined;
    if (this.path) {
      this.requireEncryption();
      encrypted = this.encryption?.encrypt(normalized).toString('base64');
      if (!encrypted) throw new Error('Secure credential storage is unavailable on this system.');
    }
    this.managedApiKeys.set(providerId, normalized);
    if (encrypted) this.encryptedApiKeys.set(providerId, encrypted);
    process.env[environmentVariable(providerId)] = normalized;
    try {
      this.persist();
    } catch (error) {
      if (previousManaged === undefined) this.managedApiKeys.delete(providerId);
      else this.managedApiKeys.set(providerId, previousManaged);
      if (previousEncrypted === undefined) this.encryptedApiKeys.delete(providerId);
      else this.encryptedApiKeys.set(providerId, previousEncrypted);
      if (previousEnvironment === undefined) delete process.env[environmentVariable(providerId)];
      else process.env[environmentVariable(providerId)] = previousEnvironment;
      throw error;
    }
  }

  public hasManagedApiKeys(): boolean {
    this.ensureManifestLoaded();
    return this.encryptedApiKeys.size > 0;
  }

  public isApiKeyConfigured(providerId: ResearchModelProviderId): boolean {
    this.ensureManifestLoaded();
    requireProviderId(providerId);
    return Boolean(process.env[environmentVariable(providerId)]?.trim()) || this.encryptedApiKeys.has(providerId);
  }

  public providersRequiringUnlock(providerIdsToCheck: readonly ResearchModelProviderId[]): ResearchModelProviderId[] {
    this.ensureManifestLoaded();
    return uniqueProviderIds(providerIdsToCheck).filter((providerId) => (
      this.encryptedApiKeys.has(providerId)
      && !this.managedApiKeys.has(providerId)
      && !process.env[environmentVariable(providerId)]?.trim()
    ));
  }

  public unlockApiKeys(providerIdsToUnlock: readonly ResearchModelProviderId[]): boolean {
    const providerIdsRequiringUnlock = this.providersRequiringUnlock(providerIdsToUnlock);
    if (providerIdsRequiringUnlock.length === 0) return false;
    this.requireEncryption();
    const unlockedApiKeys = new Map<ResearchModelProviderId, string>();
    for (const providerId of providerIdsRequiringUnlock) {
      const encrypted = this.encryptedApiKeys.get(providerId);
      if (!encrypted) continue;
      const apiKey = this.encryption?.decrypt(Buffer.from(encrypted, 'base64')).trim();
      if (!apiKey) throw new Error(`The saved ${providerId} API key could not be read from secure storage.`);
      unlockedApiKeys.set(providerId, apiKey);
    }
    for (const [providerId, apiKey] of unlockedApiKeys) {
      this.managedApiKeys.set(providerId, apiKey);
      process.env[environmentVariable(providerId)] = apiKey;
    }
    return true;
  }

  public unlockAllApiKeys(): boolean {
    return this.unlockApiKeys(providerIds());
  }

  public removeApiKey(providerId: ResearchModelProviderId): void {
    this.ensureManifestLoaded();
    requireProviderId(providerId);
    const managed = this.managedApiKeys.get(providerId);
    const encrypted = this.encryptedApiKeys.get(providerId);
    if (!encrypted && !managed) {
      if (process.env[environmentVariable(providerId)]?.trim()) {
        throw new Error('This API key comes from the host environment and must be removed there.');
      }
      return;
    }
    const previousEnvironment = process.env[environmentVariable(providerId)];
    this.managedApiKeys.delete(providerId);
    this.encryptedApiKeys.delete(providerId);
    if (managed && process.env[environmentVariable(providerId)] === managed) {
      const initial = this.initialEnvironment.get(providerId);
      if (initial === undefined) delete process.env[environmentVariable(providerId)];
      else process.env[environmentVariable(providerId)] = initial;
    }
    try {
      this.persist();
    } catch (error) {
      if (managed) this.managedApiKeys.set(providerId, managed);
      if (encrypted) this.encryptedApiKeys.set(providerId, encrypted);
      if (previousEnvironment === undefined) delete process.env[environmentVariable(providerId)];
      else process.env[environmentVariable(providerId)] = previousEnvironment;
      throw error;
    }
  }

  private loadManifest(): void {
    if (!this.path || !existsSync(this.path)) return;
    try {
      const parsed = JSON.parse(readFileSync(this.path, 'utf8')) as EncryptedCredentialFile;
      if (parsed.version !== 1 || !parsed.apiKeys || typeof parsed.apiKeys !== 'object') return;
      for (const providerId of providerIds()) {
        const encrypted = parsed.apiKeys[providerId];
        if (typeof encrypted === 'string' && encrypted.length > 0) {
          this.encryptedApiKeys.set(providerId, encrypted);
        }
      }
    } catch {
      // A missing or unreadable credential manifest is treated as empty.
    }
  }

  private ensureManifestLoaded(): boolean {
    if (this.manifestLoaded) return false;
    this.manifestLoaded = true;
    this.loadManifest();
    return true;
  }

  private persist(): void {
    this.ensureManifestLoaded();
    if (!this.path) return;
    if (this.encryptedApiKeys.size === 0) {
      if (existsSync(this.path)) unlinkSync(this.path);
      return;
    }
    const apiKeys = Object.fromEntries(this.encryptedApiKeys) as Partial<Record<ResearchModelProviderId, string>>;
    mkdirSync(dirname(this.path), { recursive: true });
    writeFileSync(this.path, JSON.stringify({ version: 1, apiKeys } satisfies EncryptedCredentialFile), {
      encoding: 'utf8',
      mode: 0o600
    });
  }

  private requireEncryption(): void {
    if (this.path && (!this.encryption || !this.encryption.available())) {
      throw new Error('Secure credential storage is unavailable on this system.');
    }
  }
}

export function unlockProviderApiKeysForAppServerStartup(
  store: ProviderCredentialStore,
  platform: NodeJS.Platform = process.platform
): boolean {
  if (platform !== 'win32') return false;
  store.unlockAllApiKeys();
  return store.hasManagedApiKeys();
}

function providerIds(): ResearchModelProviderId[] {
  return ['openai-codex', 'anthropic', 'xai', 'zai', 'openrouter'];
}

function environmentVariable(providerId: ResearchModelProviderId): string {
  return API_KEY_ENVIRONMENT_VARIABLES[providerId];
}

function requireProviderId(providerId: ResearchModelProviderId): void {
  if (!providerIds().includes(providerId)) throw new Error('Unsupported provider credential target.');
}

function uniqueProviderIds(providerIdsToCheck: readonly ResearchModelProviderId[]): ResearchModelProviderId[] {
  const unique = new Set<ResearchModelProviderId>();
  for (const providerId of providerIdsToCheck) {
    requireProviderId(providerId);
    unique.add(providerId);
  }
  return [...unique];
}
