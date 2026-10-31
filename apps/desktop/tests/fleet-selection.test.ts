import { describe, expect, it } from 'vitest';
import type { FleetState } from '@beale/app-server-runtime/protocol';
import { allowedFleetTargets, defaultFleetMachineId, fleetBrowserViewerTargetId, isAllowedFleetMachine, isFleetVmTargetId, isStreamedHostBrowserTargetId } from '../src/shared/fleet';

const state: FleetState = {
  machineId: 'machine-example', role: 'primary', enabled: true, available: true, error: null,
  appServers: [], remoteMachines: [],
  browserHomePage: 'https://duckduckgo.com/',
  sandboxes: [],
  browsers: [
    { id: 'in-agent', name: 'In-Agent Browser', kind: 'embedded', enabled: true },
    { id: 'chrome', name: 'Google Chrome', kind: 'external', enabled: true },
  ],
  machines: [{ id: 'tart:example-base', name: 'example-base', backend: 'tart', state: 'stopped', base: true,
    canRun: true, canClone: true, guestPlatform: 'darwin', privilege: 'standard', sshConfigured: true,
    sshIdentityConfigured: false, sshKnownHostsConfigured: false, sshHost: null, sshUser: 'example', owner: null }],
  primary: null, requiredWorkspaceIds: [], optionalWorkspaceIds: [], lastMachineByWorkspace: {},
  workspaceTargets: { 'workspace-example': ['browser:chrome', 'vm:run:tart:example-base'] },
};

describe('Fleet workspace targets', () => {
  it('limits the picker to checked browser and VM modes', () => {
    expect(allowedFleetTargets(state, 'workspace-example').map((target) => target.id)).toEqual([
      'browser:chrome', 'vm:run:tart:example-base',
    ]);
    expect(isAllowedFleetMachine(state, 'workspace-example', 'vm:clone:tart:example-base')).toBe(false);
    expect(isAllowedFleetMachine(state, 'workspace-example', 'local')).toBe(false);
  });

  it('uses the previous allowed target and falls back to a ready target', () => {
    expect(defaultFleetMachineId(state, 'workspace-example')).toBe('browser:chrome');
    const previous = { ...state, lastMachineByWorkspace: { 'workspace-example': 'vm:run:tart:example-base' } };
    expect(defaultFleetMachineId(previous, 'workspace-example')).toBe('vm:run:tart:example-base');
  });

  it('starts unconfigured workspaces with Local and omits the embedded browser target', () => {
    expect(allowedFleetTargets(state, 'new-workspace').map((target) => target.id)).toEqual(['local']);
    expect(isAllowedFleetMachine({ ...state, workspaceTargets: { 'new-workspace': ['local', 'browser:in-agent'] } }, 'new-workspace', 'browser:in-agent')).toBe(false);
    expect(allowedFleetTargets({ ...state, workspaceTargets: { 'new-workspace': ['browser:in-agent'] } }, 'new-workspace').map((target) => target.id)).toEqual(['local']);
  });

  it('keeps the In-Agent Browser in the embedded host view', () => {
    expect(fleetBrowserViewerTargetId('local')).toBeNull();
    expect(fleetBrowserViewerTargetId('browser:in-agent')).toBeNull();
    expect(fleetBrowserViewerTargetId('browser:chrome')).toBe('browser:chrome');
    expect(fleetBrowserViewerTargetId('vm:run:tart:example-base')).toBe('vm:run:tart:example-base');
    expect(isFleetVmTargetId('browser:chrome')).toBe(false);
    expect(isFleetVmTargetId('vm:run:tart:example-base')).toBe(true);
    expect(isStreamedHostBrowserTargetId('browser:in-agent')).toBe(false);
    expect(isStreamedHostBrowserTargetId('browser:chrome')).toBe(true);
  });

  it('offers enabled sandbox providers and marks one without an API key as not ready', () => {
    const withSandboxes: FleetState = { ...state,
      sandboxes: [
        { provider: 'e2b', name: 'E2B', enabled: true, credentialsConfigured: false },
        { provider: 'local', name: 'Docker', enabled: true, credentialsConfigured: true },
        { provider: 'daytona', name: 'Daytona', enabled: false, credentialsConfigured: true },
      ],
      workspaceTargets: { 'workspace-example': ['sandbox:e2b', 'sandbox:local', 'sandbox:daytona'] } };
    expect(allowedFleetTargets(withSandboxes, 'workspace-example').map((target) => [target.id, target.ready])).toEqual([
      ['sandbox:e2b', false], ['sandbox:local', true],
    ]);
    expect(defaultFleetMachineId(withSandboxes, 'workspace-example')).toBe('sandbox:local');
  });
});
