import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { WorkspaceRegistry } from '../../../app-server/src/workspaceRegistryStore';
import type { AppServerSessionSummary } from '../src/main/appServerCliClient';
import type { WorkspaceSnapshot } from '../src/shared/types';

const temporaryDirectories: string[] = [];

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0)) {
    rmSync(directory, { recursive: true, force: true });
  }
});

describe('workspace registry synchronization', () => {
  it('does not duplicate the AGENTS.md-backed description in registry metadata', () => {
    const registryDirectory = mkdtempSync(join(tmpdir(), 'beale-registry-description-'));
    temporaryDirectories.push(registryDirectory);
    const registry = new WorkspaceRegistry(registryDirectory);
    const snapshot = registrySnapshot();
    snapshot.activeScope.descriptionMarkdown = '# Workspace instructions';
    snapshot.runs = [];

    try {
      registry.syncWorkspace(snapshot);
      expect(registry.getState().workspaces[0]?.descriptionMarkdown).toBe('');
      expect(registry.getState().workspaces[0]?.researchKitId).toBe('general');
    } finally {
      registry.close();
    }
  });

  it('commits workspace metadata and session rows atomically', () => {
    const registryDirectory = mkdtempSync(join(tmpdir(), 'beale-registry-sync-'));
    temporaryDirectories.push(registryDirectory);
    const registry = new WorkspaceRegistry(registryDirectory);
    const snapshot = registrySnapshot();
    snapshot.runs.push({
      ...snapshot.runs[0]!,
      run: { ...snapshot.runs[0]!.run, id: null as unknown as string }
    });

    try {
      expect(() => registry.syncWorkspace(snapshot)).toThrow();
      expect(registry.getState()).toMatchObject({ workspaces: [], researchSessions: [] });
    } finally {
      registry.close();
    }
  });

  it('persists a workspace memory backend without removing stored workspace metadata', () => {
    const registryDirectory = mkdtempSync(join(tmpdir(), 'beale-registry-memory-backend-'));
    temporaryDirectories.push(registryDirectory);
    const registry = new WorkspaceRegistry(registryDirectory);

    try {
      registry.syncWorkspace(registrySnapshot());
      const workspace = registry.getState().workspaces[0]!;
      expect(workspace.memoryBackend).toBe('app-server');
      expect(registry.setWorkspaceMemoryBackend(workspace.id, 'disabled')).toMatchObject({
        id: workspace.id,
        memoryBackend: 'disabled',
        workspaceName: workspace.workspaceName
      });
      registry.syncWorkspace(registrySnapshot());
      expect(registry.getState().workspaces[0]?.memoryBackend).toBe('disabled');
    } finally {
      registry.close();
    }
  });

  it('updates workspace recency without replaying session synchronization', () => {
    const registryDirectory = mkdtempSync(join(tmpdir(), 'beale-registry-recency-'));
    temporaryDirectories.push(registryDirectory);
    const registry = new WorkspaceRegistry(registryDirectory);

    try {
      registry.syncWorkspace(registrySnapshot(), { rememberLast: false });
      const workspace = registry.getState().workspaces[0]!;
      const sessionBefore = registry.getState().researchSessions[0]!;
      registry.rememberWorkspaceOpened(workspace.id, '2026-08-27T14:00:00.000Z');

      expect(registry.getLastKnownWorkspace()).toMatchObject({
        id: workspace.id,
        lastOpenedAt: '2026-08-27T14:00:00.000Z'
      });
      expect(registry.getState().researchSessions[0]).toEqual(sessionBefore);
    } finally {
      registry.close();
    }
  });

  it('updates one active session without replaying the full workspace snapshot', () => {
    const registryDirectory = mkdtempSync(join(tmpdir(), 'beale-registry-session-sync-'));
    temporaryDirectories.push(registryDirectory);
    const registry = new WorkspaceRegistry(registryDirectory);
    const snapshot = registrySnapshot();

    try {
      registry.syncWorkspace(snapshot);
      const updatedRow = structuredClone(snapshot.runs[0]!);
      updatedRow.run.title = 'Updated active title';
      updatedRow.run.status = 'active';
      updatedRow.run.endedAt = null;
      updatedRow.lastMessageAt = '2026-08-27T15:30:00.000Z';

      expect(registry.syncResearchSession(
        'security-research',
        snapshot.workspace.workspacePath,
        snapshot.workspace.workspaceId,
        updatedRow
      )).toBe(true);
      expect(registry.getState().researchSessions[0]).toMatchObject({
        runId: updatedRow.run.id,
        title: 'Updated active title',
        status: 'active',
        updatedAt: '2026-08-27T15:30:00.000Z'
      });
    } finally {
      registry.close();
    }
  });

  it('imports canonical sessions created by another app-server client', () => {
    const registryDirectory = mkdtempSync(join(tmpdir(), 'beale-registry-cross-client-session-'));
    temporaryDirectories.push(registryDirectory);
    const registry = new WorkspaceRegistry(registryDirectory);
    const snapshot = registrySnapshot();
    snapshot.runs = [];

    try {
      registry.syncWorkspace(snapshot);
      registry.reconcileAppServerSessions(
        'security-research',
        snapshot.workspace.workspaceId,
        [canonicalSessionSummary()]
      );

      expect(registry.getState().researchSessions).toMatchObject([{
        runId: 'session_ios',
        registryWorkspaceId: registry.getState().workspaces[0]?.id,
        workspaceId: snapshot.workspace.workspaceId,
        title: 'iOS research session',
        status: 'active',
        runEngine: 'app-server',
        mode: 'open_discovery',
        promptMarkdown: 'Inspect the authorized target from iOS.',
        model: 'gpt-5.6-sol',
        reasoningEffort: 'high',
        sandboxProfile: 'host',
        updatedAt: '2026-08-28T12:01:00.000Z'
      }]);
    } finally {
      registry.close();
    }
  });

  it('touches one active session timestamp without rebuilding its registry row', () => {
    const registryDirectory = mkdtempSync(join(tmpdir(), 'beale-registry-session-activity-'));
    temporaryDirectories.push(registryDirectory);
    const registry = new WorkspaceRegistry(registryDirectory);
    const snapshot = registrySnapshot();

    try {
      registry.syncWorkspace(snapshot);
      const before = registry.getState().researchSessions[0]!;
      expect(registry.touchResearchSessionActivity(
        'security-research',
        snapshot.workspace.workspaceId,
        before.runId,
        '2026-08-27T16:00:00.000Z'
      )).toBe(true);
      expect(registry.getState().researchSessions[0]).toEqual({
        ...before,
        updatedAt: '2026-08-27T16:00:00.000Z'
      });
      expect(registry.touchResearchSessionActivity(
        'security-research',
        snapshot.workspace.workspaceId,
        'run_missing',
        '2026-08-27T16:01:00.000Z'
      )).toBe(false);
    } finally {
      registry.close();
    }
  });

  it('archives and restores session summaries without deleting their registry rows', () => {
    const registryDirectory = mkdtempSync(join(tmpdir(), 'beale-registry-archive-'));
    temporaryDirectories.push(registryDirectory);
    const registry = new WorkspaceRegistry(registryDirectory);

    try {
      registry.syncWorkspace(registrySnapshot());
      const session = registry.getState().researchSessions[0]!;
      registry.archiveResearchSession(session.id, '2026-08-24T12:00:00.000Z');
      expect(registry.getState().researchSessions).toEqual([]);
      expect(registry.getState().archivedResearchSessions).toMatchObject([{
        id: session.id,
        runId: session.runId,
        archivedAt: '2026-08-24T12:00:00.000Z'
      }]);

      registry.syncWorkspace(registrySnapshot());
      expect(registry.getState().researchSessions).toEqual([]);
      registry.restoreResearchSession(session.id);
      expect(registry.getState().researchSessions[0]?.id).toBe(session.id);
      expect(registry.getState().archivedResearchSessions).toEqual([]);
    } finally {
      registry.close();
    }
  });

  it('keeps internal quick-chat workspaces out of public state while retaining their archive history', () => {
    const registryDirectory = mkdtempSync(join(tmpdir(), 'beale-registry-internal-'));
    temporaryDirectories.push(registryDirectory);
    const registry = new WorkspaceRegistry(registryDirectory);
    const snapshot = registrySnapshot();
    snapshot.workspace.workspacePath = join(registry.internalWorkspaceDirectory, 'quick-chats');
    snapshot.workspace.workspaceDirectories = [snapshot.workspace.workspacePath];
    snapshot.activeScope.workspaceName = 'Quick Chats';
    snapshot.runs[0]!.run.mode = 'quick-chat';

    try {
      registry.syncWorkspace(snapshot, { rememberLast: false });
      expect(registry.getWorkspaceByPath(snapshot.workspace.workspacePath)?.workspaceName).toBe('Quick Chats');
      expect(registry.getState()).toMatchObject({ workspaces: [], researchSessions: [] });
      expect(registry.listArchivedQuickChats()).toMatchObject([{
        runId: snapshot.runs[0]!.run.id,
        mode: 'quick-chat'
      }]);
      expect(registry.getLastKnownWorkspace()).toBeNull();
    } finally {
      registry.close();
    }
  });

  it('orders sessions and timestamps them by their latest message', () => {
    const registryDirectory = mkdtempSync(join(tmpdir(), 'beale-registry-last-message-'));
    temporaryDirectories.push(registryDirectory);
    const registry = new WorkspaceRegistry(registryDirectory);
    const snapshot = registrySnapshot();
    snapshot.runs[0]!.lastMessageAt = '2026-08-26T02:30:00.000Z';
    snapshot.runs[0]!.run.status = 'active';
    snapshot.runs[0]!.run.endedAt = null;
    snapshot.runs[0]!.sessionRuns = [{
      id: 'session_run_valid',
      runId: snapshot.runs[0]!.run.id,
      attemptId: 'attempt_resumed',
      status: 'active',
      terminationCause: null,
      activityIntervals: [{
        id: 'activity_resumed',
        runId: snapshot.runs[0]!.run.id,
        attemptId: 'attempt_resumed',
        startedAt: '2026-08-27T02:30:00.000Z',
        endedAt: null
      }]
    }];
    const newerSessionWithOlderMessage = structuredClone(snapshot.runs[0]!);
    newerSessionWithOlderMessage.run.id = 'run_newer_creation';
    newerSessionWithOlderMessage.run.title = 'Newer creation, older message';
    newerSessionWithOlderMessage.run.createdAt = '2026-08-25T12:00:00.000Z';
    newerSessionWithOlderMessage.run.startedAt = '2026-08-25T12:00:00.000Z';
    newerSessionWithOlderMessage.lastMessageAt = '2026-08-25T12:01:00.000Z';
    newerSessionWithOlderMessage.sessionRuns = [];
    snapshot.runs.push(newerSessionWithOlderMessage);

    try {
      registry.syncWorkspace(snapshot);
      expect(registry.getState().researchSessions.map((session: { runId: string }) => session.runId)).toEqual([
        'run_valid',
        'run_newer_creation'
      ]);
      expect(registry.getState().researchSessions[0]).toMatchObject({
        status: 'active',
        updatedAt: '2026-08-26T02:30:00.000Z'
      });
    } finally {
      registry.close();
    }
  });
});

function registrySnapshot(): WorkspaceSnapshot {
  const createdAt = '2026-08-18T12:00:00.000Z';
  return {
    workspace: {
      workspaceId: 'workspace_atomic',
      workspacePath: 'C:\\research\\atomic',
      workspaceDirectories: ['C:\\research\\atomic']
    },
    activeScope: {
      workspaceName: 'Atomic Workspace',
      scopeOwner: 'Researcher',
      descriptionMarkdown: '',
      rulesMarkdown: '',
      expiresAt: null
    },
    researchProfile: { profileId: 'security-research' },
    runs: [{
      engine: 'app-server',
      run: {
        id: 'run_valid',
        status: 'completed',
        mode: 'dynamic',
        title: 'Valid session',
        promptMarkdown: '',
        summary: '',
        finalDisposition: null,
        model: 'gpt-5.6-sol',
        reasoningEffort: 'high',
        sandboxProfile: 'host',
        budget: {},
        createdAt,
        startedAt: createdAt,
        endedAt: createdAt
      }
    }]
  } as unknown as WorkspaceSnapshot;
}

function canonicalSessionSummary(): AppServerSessionSummary {
  return {
    schemaVersion: 1,
    id: 'session_ios',
    workspaceId: 'workspace_atomic',
    status: 'active',
    title: 'iOS research session',
    prompt: 'Inspect the authorized target from iOS.',
    summary: 'Research is active.',
    provider: 'openai-codex',
    model: 'gpt-5.6-sol',
    reasoningEffort: 'high',
    workflowId: null,
    profile: { id: 'security-research' },
    metadata: { source: 'beale-app-server' },
    finalDisposition: null,
    attempts: [{
      id: 'attempt_ios',
      parentAttemptId: null,
      status: 'active',
      summary: 'Starting the app-server research session.',
      startedAt: '2026-08-28T12:00:00.000Z',
      endedAt: null,
      metadata: {}
    }],
    lastMessageAt: '2026-08-28T12:01:00.000Z',
    createdAt: '2026-08-28T12:00:00.000Z',
    startedAt: '2026-08-28T12:00:00.000Z',
    endedAt: null,
    updatedAt: '2026-08-28T12:01:00.000Z',
    revision: 2
  };
}
