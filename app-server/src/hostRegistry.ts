import { existsSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import type {
  BealeAppServerWorkspaceSummary,
  HoneycrispProviderAuthenticationMethod,
  HoneycrispProviderRiskAcknowledgement
} from 'honeycrisp/protocol';

export type AppServerMemoryBackendId = 'honeycrisp' | 'disabled';

interface SqlRow {
  [key: string]: unknown;
}

export interface AppServerHostWorkspace extends BealeAppServerWorkspaceSummary {
  workspacePath: string;
  workspaceDirectories: string[];
  memoryBackend: AppServerMemoryBackendId;
}

export interface AppServerHostProviderSettings {
  defaultProviderId: string | null;
  modelDefaults: Readonly<Record<string, { leadModel?: string; smallModel?: string; reasoningEffort?: string }>>;
  authenticationPreferences: Readonly<Record<string, HoneycrispProviderAuthenticationMethod>>;
  riskAcknowledgements: readonly HoneycrispProviderRiskAcknowledgement[];
}

export interface AppServerHostStorage {
  databasePath: string;
  artifactDirectoryPath: string;
}

export interface AppServerHostRegistryOptions {
  registryDirectory?: string;
  honeycrispDatabasePath?: string;
  honeycrispArtifactDirectory?: string;
}

const PROFILE_ID_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/u;

/**
 * App-server-owned view of Beale host metadata. This opens only Beale's
 * workspace registry; canonical Honeycrisp storage remains behind the app-server
 * protocol boundary.
 */
export class AppServerHostRegistry {
  public readonly registryDirectory: string;
  public readonly registryPath: string;
  public readonly shellOptionsPath: string;

  public constructor(private readonly options: AppServerHostRegistryOptions = {}) {
    this.registryDirectory = resolve(
      options.registryDirectory
        ?? process.env.BEALE_WORKSPACE_REGISTRY_DIR?.trim()
        ?? join(homedir(), '.beale')
    );
    this.registryPath = join(this.registryDirectory, 'workspace-registry.sqlite');
    this.shellOptionsPath = join(this.registryDirectory, 'shell-options.json');
  }

  public listWorkspaces(): BealeAppServerWorkspaceSummary[] {
    return this.withDatabase((database) => {
      if (!tableExists(database, 'workspaces')) return [];
      const rows = database.prepare(`${workspaceProjection(database)}
        ORDER BY w.updated_at DESC, w.id DESC
      `).all() as SqlRow[];
      return rows
        .filter((row) => !this.isInternalWorkspacePath(requiredText(row, 'workspace_path')))
        .map(projectWorkspaceSummary);
    }, []);
  }

  private isInternalWorkspacePath(workspacePath: string): boolean {
    const internalRoot = join(this.registryDirectory, 'internal-workspaces');
    const candidate = relative(internalRoot, resolve(workspacePath));
    return candidate === '' || (!candidate.startsWith('..') && !isAbsolute(candidate));
  }

  public resolveWorkspace(identifier: string): AppServerHostWorkspace | null {
    const normalized = identifier.trim();
    if (!normalized) return null;
    return this.withDatabase((database) => {
      if (!tableExists(database, 'workspaces')) return null;
      const row = database.prepare(`${workspaceProjection(database)}
        WHERE w.id = ? OR w.workspace_id = ?
        ORDER BY CASE WHEN w.id = ? THEN 0 ELSE 1 END
        LIMIT 1
      `).get(normalized, normalized, normalized) as SqlRow | undefined;
      return row ? projectHostWorkspace(row) : null;
    }, null);
  }

  public providerSettings(): AppServerHostProviderSettings {
    const meta = this.readMeta();
    const modelDefaults = recordOfRecords(meta.get('provider_model_defaults_json'));
    const authenticationPreferences = authenticationPreferenceRecord(
      meta.get('provider_preferred_authentication_methods_json')
    );
    const riskAcknowledgements: HoneycrispProviderRiskAcknowledgement[] = [];
    for (const [key, acknowledgement] of [
      ['openai_trusted_access_cyber_risk_acknowledged', 'openai-codex'],
      ['anthropic_cvp_risk_acknowledged', 'anthropic'],
      ['xai_policy_use_risk_acknowledged', 'xai'],
      ['zai_policy_use_risk_acknowledged', 'zai'],
      ['openrouter_policy_use_risk_acknowledged', 'openrouter']
    ] as const) {
      if (meta.get(key) === '1') riskAcknowledgements.push(acknowledgement);
    }
    return {
      defaultProviderId: nonEmpty(meta.get('default_provider_id')),
      modelDefaults,
      authenticationPreferences,
      riskAcknowledgements
    };
  }

  public memoryTypeDescriptions(): Readonly<Record<string, string>> | null {
    const value = parseJson(this.readMeta().get('memory_type_descriptions_json'));
    if (!isRecord(value)) return null;
    const entries = Object.entries(value).filter(
      (entry): entry is [string, string] => Boolean(entry[0].trim()) && typeof entry[1] === 'string'
    );
    return entries.length > 0 ? Object.fromEntries(entries) : null;
  }

  public storageForProfile(profileId: string): AppServerHostStorage {
    const normalizedProfileId = profileId.trim() || 'security-research';
    if (!PROFILE_ID_PATTERN.test(normalizedProfileId)) {
      throw new Error(`Unsupported research profile id: ${profileId}`);
    }
    const configuredDatabase = this.options.honeycrispDatabasePath
      ?? process.env.HONEYCRISP_DATABASE_PATH?.trim();
    const configuredArtifacts = this.options.honeycrispArtifactDirectory
      ?? process.env.HONEYCRISP_ARTIFACT_DIRECTORY?.trim();
    const configuredRegistryDirectory = this.options.registryDirectory
      ?? process.env.BEALE_WORKSPACE_REGISTRY_DIR?.trim();
    const databasePath = configuredDatabase
      ? normalizedProfileId === 'security-research'
        ? resolve(configuredDatabase)
        : join(dirname(resolve(configuredDatabase)), 'profiles', normalizedProfileId, 'memory.sqlite')
      : configuredRegistryDirectory
        ? resolve(configuredRegistryDirectory, 'honeycrisp', 'profiles', normalizedProfileId, 'memory.sqlite')
        : join(homedir(), '.honeycrisp', 'profiles', normalizedProfileId, 'memory.sqlite');
    const artifactDirectoryPath = configuredArtifacts
      ? normalizedProfileId === 'security-research'
        ? resolve(configuredArtifacts)
        : join(dirname(resolve(configuredArtifacts)), normalizedProfileId, 'artifacts')
      : join(dirname(databasePath), 'artifacts');
    return { databasePath, artifactDirectoryPath };
  }

  private readMeta(): Map<string, string> {
    return this.withDatabase((database) => {
      if (!tableExists(database, 'registry_meta')) return new Map<string, string>();
      const rows = database.prepare('SELECT key, value FROM registry_meta').all() as SqlRow[];
      return new Map(rows.flatMap((row) => {
        const key = nonEmpty(row.key);
        return key && typeof row.value === 'string' ? [[key, row.value] as const] : [];
      }));
    }, new Map<string, string>());
  }

  private withDatabase<T>(read: (database: DatabaseSync) => T, fallback: T): T {
    if (!existsSync(this.registryPath)) return fallback;
    let database: DatabaseSync | null = null;
    try {
      database = new DatabaseSync(this.registryPath, { readOnly: true });
      return read(database);
    } catch {
      return fallback;
    } finally {
      database?.close();
    }
  }
}

function projectHostWorkspace(row: SqlRow): AppServerHostWorkspace {
  const summary = projectWorkspaceSummary(row);
  const workspacePath = resolve(requiredText(row, 'workspace_path'));
  return {
    ...summary,
    workspacePath,
    workspaceDirectories: workspaceDirectories(row.workspace_directories_json, workspacePath),
    memoryBackend: appServerMemoryBackendId(row.memory_backend)
  };
}

function appServerMemoryBackendId(value: unknown): AppServerMemoryBackendId {
  return value === 'disabled' ? 'disabled' : 'honeycrisp';
}

function projectWorkspaceSummary(row: SqlRow): BealeAppServerWorkspaceSummary {
  return {
    id: requiredText(row, 'id'),
    workspaceId: requiredText(row, 'workspace_id'),
    name: requiredText(row, 'workspace_name'),
    researchProfileId: nonEmpty(row.research_profile_id) ?? 'security-research',
    researchKitId: nonEmpty(row.research_kit_id) ?? 'general',
    runCount: finiteNumber(row.run_count),
    lastRunAt: nonEmpty(row.last_run_at),
    updatedAt: requiredText(row, 'updated_at')
  };
}

function workspaceDirectories(value: unknown, workspacePath: string): string[] {
  const parsed = parseJson(value);
  const paths = Array.isArray(parsed)
    ? parsed.flatMap((entry) => typeof entry === 'string' && entry.trim() ? [resolve(entry)] : [])
    : [];
  return [...new Set([workspacePath, ...paths])];
}

function authenticationPreferenceRecord(value: unknown): Record<string, HoneycrispProviderAuthenticationMethod> {
  const parsed = parseJson(value);
  if (!isRecord(parsed)) return {};
  return Object.fromEntries(Object.entries(parsed).flatMap(([key, entry]) => (
    (entry === 'subscription' || entry === 'api_key') && key.trim()
      ? [[key, entry] as const]
      : []
  )));
}

function recordOfRecords(value: unknown): Record<string, { leadModel?: string; smallModel?: string; reasoningEffort?: string }> {
  const parsed = parseJson(value);
  if (!isRecord(parsed)) return {};
  return Object.fromEntries(Object.entries(parsed).flatMap(([provider, entry]) => {
    if (!provider.trim() || !isRecord(entry)) return [];
    const projected = {
      ...(nonEmpty(entry.largeModel) ? { leadModel: nonEmpty(entry.largeModel)! } : {}),
      ...(nonEmpty(entry.smallModel) ? { smallModel: nonEmpty(entry.smallModel)! } : {}),
      ...(nonEmpty(entry.reasoningEffort) ? { reasoningEffort: nonEmpty(entry.reasoningEffort)! } : {})
    };
    return [[provider, projected] as const];
  }));
}

function tableExists(database: DatabaseSync, name: string): boolean {
  return Boolean(database.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?").get(name));
}

function workspaceProjection(database: DatabaseSync): string {
  const sessionProjection = tableExists(database, 'research_sessions')
    ? `(SELECT COUNT(*) FROM research_sessions s WHERE s.registry_workspace_id = w.id) AS run_count,
       (SELECT MAX(s.updated_at) FROM research_sessions s WHERE s.registry_workspace_id = w.id) AS last_run_at`
    : '0 AS run_count, NULL AS last_run_at';
  return `SELECT w.*, ${sessionProjection} FROM workspaces w`;
}

function requiredText(row: SqlRow, key: string): string {
  const value = nonEmpty(row[key]);
  if (!value) throw new Error(`Workspace registry row is missing ${key}.`);
  return value;
}

function nonEmpty(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value.trim() : null;
}

function finiteNumber(value: unknown): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : 0;
}

function parseJson(value: unknown): unknown {
  if (typeof value !== 'string' || !value.trim()) return null;
  try {
    return JSON.parse(value) as unknown;
  } catch {
    return null;
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}
