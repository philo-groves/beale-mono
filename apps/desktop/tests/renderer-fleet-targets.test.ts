import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import type { FleetState } from '@beale/app-server-runtime/protocol';
import type { WorkspaceRegistryEntry } from '@shared/types';
import { FleetTargetsWorkspace, nextWorkspaceTargetIds } from '../src/renderer/features/fleet/FleetTargetsWorkspace';

function workspace(workspaceId: string, workspaceName: string): WorkspaceRegistryEntry {
  return {
    id: `registry-${workspaceId}`, workspacePath: `/workspaces/${workspaceId}`, workspaceId, workspaceName,
    researchProfileId: 'security-research', researchKitId: 'general', scopeOwner: 'ExampleCo',
    descriptionMarkdown: '', rulesMarkdown: '', expiresAt: null,
    createdAt: '2000-01-01T00:00:00.000Z', updatedAt: '2000-01-01T00:00:00.000Z',
    lastOpenedAt: null, runCount: 0, lastRunAt: null
  };
}

const state: FleetState = {
  machineId: 'machine-example', role: 'primary', enabled: true, available: true, error: null,
  appServers: [{ id: 'server-example', name: 'Example Server', url: 'https://server.example.test' }],
  browsers: [{ id: 'browser-example', name: 'Example Browser', kind: 'external', enabled: true }],
  browserHomePage: 'https://example.test/',
  machines: [{ id: 'tart:example-vm', name: 'Example VM', backend: 'tart', state: 'stopped', base: false,
    canRun: true, canClone: true, guestPlatform: 'linux', privilege: 'standard', sshConfigured: false,
    sshIdentityConfigured: false, sshKnownHostsConfigured: false, sshHost: null, sshUser: null, owner: null },
  { id: 'tart:unconfigured-example', name: 'Unconfigured VM', backend: 'tart', state: 'stopped', base: false,
    canRun: false, canClone: false, guestPlatform: 'linux', privilege: 'standard', sshConfigured: false,
    sshIdentityConfigured: false, sshKnownHostsConfigured: false, sshHost: null, sshUser: null, owner: null }],
  remoteMachines: [], primary: null, requiredWorkspaceIds: [], optionalWorkspaceIds: [], lastMachineByWorkspace: {},
  workspaceTargets: {
    'workspace-one': ['local', 'browser:browser-example', 'browser:missing'],
    'workspace-two': ['browser:browser-example', 'vm:run:tart:example-vm']
  }
};

describe('Fleet workspace targets', () => {
  it('shows separate target lists and per-workspace selection', () => {
    const props = {
      state, workspaces: [workspace('workspace-one', 'Example One'), workspace('workspace-two', 'Example Two')],
      busy: false, onScopeChange: () => undefined, onStateChange: () => undefined,
      onRefresh: async () => undefined
    };
    const all = renderToStaticMarkup(createElement(FleetTargetsWorkspace, { ...props, selectedWorkspaceId: null }));
    expect(all).toContain('aria-label="Fleet workspace scope"');
    expect(all).toContain('>All Fleet</span>');
    expect(all).toContain('<h1>All Fleet</h1>');
    expect(all).toContain('>Example One</span>');
    expect(all).toContain('>Example Two</span>');
    expect(all).toContain('aria-label="App Servers"');
    expect(all).toContain('aria-label="Browsers"');
    expect(all).toContain('aria-label="Virtual Machines"');
    expect(all).toMatch(/class="fleet-target-heading-label"><svg[\s\S]*?<\/svg><h2>App Servers<\/h2>/);
    expect(all).toMatch(/class="fleet-target-heading-label"><svg[\s\S]*?<\/svg><h2>Browsers<\/h2>/);
    expect(all).toMatch(/class="fleet-target-heading-label"><svg[\s\S]*?<\/svg><h2>Virtual Machines<\/h2>/);
    expect(all).not.toContain('Run and clone permissions are assigned separately');
    expect(all).toContain('aria-label="Local for all workspaces" aria-checked="mixed"');
    expect(all).toContain('1 of 2 workspaces');
    expect(all).toContain('Remote host');
    expect(all).toContain('Unconfigured VM');
    expect(all).toContain('Configure run or clone in the sidebar');
    const one = renderToStaticMarkup(createElement(FleetTargetsWorkspace, { ...props, selectedWorkspaceId: 'workspace-one' }));
    expect(one).toContain('Example One Fleet</h1>');
    expect(one).toContain('aria-label="Local for Example One" aria-checked="true" checked=""');
    expect(one).toContain('aria-label="Example Browser for Example One" aria-checked="true" checked=""');
    expect(one).toContain('Configure in Fleet before use');
  });

  it('keeps supported targets and drops stale target IDs when changing a workspace', () => {
    expect(nextWorkspaceTargetIds(state, 'workspace-one', 'vm:clone:tart:example-vm', true))
      .toEqual(['local', 'browser:browser-example', 'vm:clone:tart:example-vm']);
    expect(nextWorkspaceTargetIds(state, 'workspace-one', 'local', false))
      .toEqual(['browser:browser-example']);
    expect(nextWorkspaceTargetIds(state, 'workspace-two', 'vm:run:tart:example-vm', true))
      .toEqual(['browser:browser-example', 'vm:run:tart:example-vm']);
  });
});
