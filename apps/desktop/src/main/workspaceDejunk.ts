import { resolve } from 'node:path';
import type { WorkspaceDejunkSummary } from '../shared/types';
import {
  getAppServerMaintenanceSummary,
  getAppServerMaintenanceSummaryAsync,
  runAppServerMaintenance,
  runAppServerMaintenanceAsync,
  type AppServerWorkspaceRepositoryCandidate,
  type AppServerWorkspaceRepositoryRelocation
} from './appServerCliClient';

export interface WorkspaceDejunkMaintenanceResult {
  summary: WorkspaceDejunkSummary;
  repositoryRelocations: AppServerWorkspaceRepositoryRelocation[];
}

const SUMMARY_CACHE_MS = 15_000;
const summaries = new Map<string, { cachedAt: number; summary: WorkspaceDejunkSummary }>();
const summaryRequests = new Map<string, Promise<WorkspaceDejunkSummary>>();

export function getWorkspaceDejunkSummary(workspacePath: string): WorkspaceDejunkSummary {
  const root = resolve(workspacePath);
  const cached = summaries.get(root);
  if (cached && Date.now() - cached.cachedAt < SUMMARY_CACHE_MS) return cached.summary;
  const summary = getAppServerMaintenanceSummary(root);
  summaries.set(root, { cachedAt: Date.now(), summary });
  return summary;
}

export async function getWorkspaceDejunkSummaryAsync(workspacePath: string): Promise<WorkspaceDejunkSummary> {
  const root = resolve(workspacePath);
  const cached = summaries.get(root);
  if (cached && Date.now() - cached.cachedAt < SUMMARY_CACHE_MS) return cached.summary;
  const active = summaryRequests.get(root);
  if (active) return await active;
  const request = getAppServerMaintenanceSummaryAsync(root).then((summary) => {
    summaries.set(root, { cachedAt: Date.now(), summary });
    return summary;
  });
  summaryRequests.set(root, request);
  try {
    return await request;
  } finally {
    summaryRequests.delete(root);
  }
}

export function runWorkspaceDejunk(workspacePath: string): WorkspaceDejunkSummary;
export function runWorkspaceDejunk(
  workspacePath: string,
  options: { repositoryStoreDirectory: string; repositories?: AppServerWorkspaceRepositoryCandidate[] }
): WorkspaceDejunkMaintenanceResult;
export function runWorkspaceDejunk(
  workspacePath: string,
  options?: { repositoryStoreDirectory: string; repositories?: AppServerWorkspaceRepositoryCandidate[] }
): WorkspaceDejunkSummary | WorkspaceDejunkMaintenanceResult {
  const root = resolve(workspacePath);
  if (!options) {
    const summary = runAppServerMaintenance(root);
    summaries.set(root, { cachedAt: Date.now(), summary });
    return summary;
  }
  const result = runAppServerMaintenance(root, options);
  summaries.set(root, { cachedAt: Date.now(), summary: result.summary });
  return result;
}

export async function runWorkspaceDejunkAsync(
  workspacePath: string,
  options: { repositoryStoreDirectory: string; repositories?: AppServerWorkspaceRepositoryCandidate[] }
): Promise<WorkspaceDejunkMaintenanceResult> {
  const root = resolve(workspacePath);
  const result = await runAppServerMaintenanceAsync(root, options);
  summaries.set(root, { cachedAt: Date.now(), summary: result.summary });
  return result;
}

export function invalidateWorkspaceDejunkSummary(workspacePath: string): void {
  summaries.delete(resolve(workspacePath));
}
