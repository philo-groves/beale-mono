import { homedir } from 'node:os';
import { join, resolve } from 'node:path';
import type { RepositoryCloneMode, ScopeAsset, WorkspaceScopeVersion } from '@shared/types';
import {
  inspectAppServerSources,
  materializeAppServerSource,
  materializeAppServerSourceSync
} from './appServerCliClient';

export interface SourceRepositoryCandidate {
  url: string;
  label: string;
  sourceAssetId: string;
  sourceAssetKind: ScopeAsset['kind'];
  sensitivity: string;
  clonedDirectory: string | null;
}

export interface SourceRepositorySelection {
  candidate: SourceRepositoryCandidate | null;
  candidates: SourceRepositoryCandidate[];
  reason: 'matched' | 'ambiguous' | 'not_found';
}

export interface MaterializedSourceRepository {
  repositoryUrl: string;
  localPath: string;
  cloned: boolean;
  ref: string | null;
  head: string | null;
  headRefName: string | null;
  headDescribe: string | null;
  requestedRefHead: string | null;
  requestedRefMatchesHead: boolean | null;
  cloneMode: RepositoryCloneMode;
}

export function defaultSourceRepositoryStoreDirectory(registryDirectory?: string): string {
  const explicit = process.env.APP_SERVER_REPOSITORY_STORE_DIR?.trim() || process.env.BEALE_REPOSITORY_STORE_DIR?.trim();
  return resolve(explicit || join(registryDirectory ?? join(homedir(), '.beale'), 'repositories'));
}

export function sourceRepositoryCandidates(scope: WorkspaceScopeVersion): SourceRepositoryCandidate[] {
  return (inspectAppServerSources({ scope }).candidates ?? []) as SourceRepositoryCandidate[];
}

export function selectSourceRepository(scope: WorkspaceScopeVersion, requested: string): SourceRepositorySelection {
  const selection = inspectAppServerSources({ scope, requested }).selection;
  if (!selection) throw new Error('app-server did not return a source selection.');
  return selection as SourceRepositorySelection;
}

export function extractSourceRepositoryUrls(text: string): string[] {
  return inspectAppServerSources({ text }).urls ?? [];
}

export function normalizeSourceRepositoryUrl(value: string): string | null {
  return inspectAppServerSources({ value }).normalizedUrl ?? null;
}

export const normalizeGitHubRepositoryUrl = normalizeSourceRepositoryUrl;

export function materializeGitRepository(
  candidate: SourceRepositoryCandidate,
  ref: string,
  options: { cloneMode?: RepositoryCloneMode; repositoryStoreDirectory?: string } = {}
): MaterializedSourceRepository {
  return materializeAppServerSourceSync(candidate, ref, options.repositoryStoreDirectory, options.cloneMode);
}

export async function materializeGitRepositoryAsync(
  candidate: SourceRepositoryCandidate,
  ref: string,
  options: { cloneMode?: RepositoryCloneMode; signal?: AbortSignal; repositoryStoreDirectory?: string } = {}
): Promise<MaterializedSourceRepository> {
  return materializeAppServerSource(candidate, ref, options.repositoryStoreDirectory, options.signal, options.cloneMode);
}
