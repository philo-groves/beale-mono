import type { WorkspaceRegistryEntry, WorkspaceRegistryState, ResearchSessionSummary } from '@shared/types';
import { displaySessionTitle } from '../../shared/sessionTitle';

export function researchSessionsForWorkspace(registry: WorkspaceRegistryState, workspace: WorkspaceRegistryEntry): ResearchSessionSummary[] {
  return registry.researchSessions
    .filter((session) => session.registryWorkspaceId === workspace.id)
    .sort(compareSidebarSessions);
}

function compareSidebarSessions(left: ResearchSessionSummary, right: ResearchSessionSummary): number {
  const leftMinute = timestampMinute(left.updatedAt);
  const rightMinute = timestampMinute(right.updatedAt);
  if (leftMinute !== rightMinute) return leftMinute > rightMinute ? -1 : 1;

  return promptSessionTitle(left).localeCompare(promptSessionTitle(right))
    || left.id.localeCompare(right.id);
}

function timestampMinute(iso: string): number {
  const timestamp = Date.parse(iso);
  return Number.isFinite(timestamp) ? Math.floor(timestamp / 60_000) : Number.MIN_SAFE_INTEGER;
}

export function workspaceById(registry: WorkspaceRegistryState | null, registryWorkspaceId: string | null): WorkspaceRegistryEntry | null {
  if (!registry || !registryWorkspaceId) return null;
  return registry.workspaces.find((workspace) => workspace.id === registryWorkspaceId) ?? null;
}

export function workspaceExists(registry: WorkspaceRegistryState | null, registryWorkspaceId: string | null): boolean {
  return Boolean(workspaceById(registry, registryWorkspaceId));
}

export function promptSessionTitle(session: ResearchSessionSummary): string {
  return displaySessionTitle(session.title, session.promptMarkdown);
}
