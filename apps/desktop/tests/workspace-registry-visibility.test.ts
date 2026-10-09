import { describe, expect, it } from 'vitest';
import { visibleSecurityWorkspaceRegistryState } from '../src/main/workspaceRegistry';
import type { WorkspaceRegistryState } from '../src/shared/types';

describe('workspace registry visibility', () => {
  it('hides historical non-security workspaces and their sessions without mutating the saved state', () => {
    const state = {
      registryPath: '/tmp/example-registry.sqlite',
      workspaces: [
        { id: 'registry-security-example', researchProfileId: 'security-research' },
        { id: 'registry-legacy-example', researchProfileId: 'mathematics' },
      ],
      researchSessions: [
        { registryWorkspaceId: 'registry-security-example', id: 'session-security-example' },
        { registryWorkspaceId: 'registry-legacy-example', id: 'session-legacy-example' },
      ],
      archivedResearchSessions: [
        { registryWorkspaceId: 'registry-legacy-example', id: 'session-archived-example' },
      ],
    } as WorkspaceRegistryState;

    const visible = visibleSecurityWorkspaceRegistryState(state);
    expect(visible.workspaces.map((workspace) => workspace.id)).toEqual(['registry-security-example']);
    expect(visible.researchSessions.map((session) => session.id)).toEqual(['session-security-example']);
    expect(visible.archivedResearchSessions).toEqual([]);
    expect(state.workspaces).toHaveLength(2);
    expect(state.researchSessions).toHaveLength(2);
  });
});
