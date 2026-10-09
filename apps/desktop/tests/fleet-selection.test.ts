import { describe, expect, it } from 'vitest';
import type { FleetState } from '@beale/app-server-runtime/protocol';
import { defaultFleetMachineId, fleetVmRequired, isAllowedFleetMachine } from '../src/shared/fleet';

const base: FleetState = {
  role: 'primary', enabled: true, available: true, error: null,
  machines: [
    { id: 'tart:example-base', name: 'example-base', backend: 'tart', state: 'stopped', base: true, privilege: 'standard', sshConfigured: true, sshIdentityConfigured: false, sshKnownHostsConfigured: false, sshHost: null, sshUser: 'example' },
    { id: 'tart:worker-z', name: 'worker-z', backend: 'tart', state: 'stopped', base: false, privilege: 'standard', sshConfigured: true, sshIdentityConfigured: false, sshKnownHostsConfigured: false, sshHost: null, sshUser: 'example' },
    { id: 'tart:worker-a', name: 'worker-a', backend: 'tart', state: 'stopped', base: false, privilege: 'standard', sshConfigured: true, sshIdentityConfigured: false, sshKnownHostsConfigured: false, sshHost: null, sshUser: 'example' },
  ],
  primary: null, requiredWorkspaceIds: [], optionalWorkspaceIds: [], lastMachineByWorkspace: {},
};

describe('Fleet machine selection', () => {
  it('requires a worker when a base exists and falls back alphabetically', () => {
    expect(fleetVmRequired(base, 'workspace-example')).toBe(true);
    expect(defaultFleetMachineId(base, 'workspace-example')).toBe('tart:worker-a');
    expect(isAllowedFleetMachine(base, 'workspace-example', 'local')).toBe(false);
    expect(isAllowedFleetMachine(base, 'workspace-example', 'tart:example-base')).toBe(false);
  });

  it('uses the last worker and permits Local when the requirement is disabled', () => {
    const state = { ...base, lastMachineByWorkspace: { 'workspace-example': 'tart:worker-z' } };
    expect(defaultFleetMachineId(state, 'workspace-example')).toBe('tart:worker-z');
    const optional = { ...state, optionalWorkspaceIds: ['workspace-example'] };
    expect(defaultFleetMachineId(optional, 'workspace-example')).toBe('local');
    expect(isAllowedFleetMachine(optional, 'workspace-example', 'local')).toBe(true);
  });

  it('permits Local when Fleet is disabled even if a base is configured', () => {
    const state = { ...base, enabled: false };
    expect(fleetVmRequired(state, 'workspace-example')).toBe(false);
    expect(defaultFleetMachineId(state, 'workspace-example')).toBe('local');
    expect(isAllowedFleetMachine(state, 'workspace-example', 'local')).toBe(true);
  });

  it('requires a VM from a registered base before its SSH user is saved in Beale', () => {
    const state = { ...base, machines: base.machines.map((machine) =>
      machine.base ? { ...machine, sshConfigured: false } : machine) };
    expect(fleetVmRequired(state, 'workspace-example')).toBe(true);
  });
});
