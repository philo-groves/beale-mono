import type { FleetMachine, FleetState } from '@beale/app-server-runtime/protocol';

export function fleetVmRequired(state: FleetState, workspaceId: string): boolean {
  if (state.role !== 'primary' || !state.enabled) return false;
  if (state.requiredWorkspaceIds.includes(workspaceId)) return true;
  if (state.optionalWorkspaceIds.includes(workspaceId)) return false;
  return state.machines.some((machine) => machine.base);
}

export function runnableFleetMachines(state: FleetState): FleetMachine[] {
  if (state.role !== 'primary' || !state.enabled || !state.available) return [];
  return [...state.machines, ...state.remoteMachines]
    .filter((machine) => !machine.base && !machine.owner && machine.sshConfigured && machine.state !== 'unknown')
    .sort((left, right) => left.name.localeCompare(right.name) || left.id.localeCompare(right.id));
}

export function defaultFleetMachineId(state: FleetState, workspaceId: string): string | null {
  if (!fleetVmRequired(state, workspaceId)) return 'local';
  const machines = runnableFleetMachines(state);
  const previous = state.lastMachineByWorkspace[workspaceId];
  return machines.find((machine) => machine.id === previous)?.id ?? machines[0]?.id ?? null;
}

export function isAllowedFleetMachine(state: FleetState, workspaceId: string, machineId: string): boolean {
  if (machineId === 'local') return !fleetVmRequired(state, workspaceId);
  return runnableFleetMachines(state).some((machine) => machine.id === machineId);
}
