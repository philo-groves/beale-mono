import type { FleetMachine, FleetState } from '@beale/app-server-runtime/protocol';

export function fleetVmRequired(state: FleetState, workspaceId: string): boolean {
  if (state.role !== 'primary' || !state.enabled) return false;
  if (state.requiredWorkspaceIds.includes(workspaceId)) return true;
  if (state.optionalWorkspaceIds.includes(workspaceId)) return false;
  return [...state.machines, ...state.remoteMachines].some((machine) => machine.base);
}

export function runnableFleetMachines(state: FleetState): FleetMachine[] {
  if (state.role !== 'primary' || !state.enabled) return [];
  return [...(state.available ? state.machines : []), ...state.remoteMachines]
    .filter((machine) => machine.base && machine.state === 'stopped')
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

export function selectedFleetMachineIds(
  state: FleetState,
  workspaceId: string,
  requestedIds: readonly string[] | undefined,
  requestedId?: string,
): string[] {
  const requested = requestedIds?.length ? requestedIds : requestedId ? [requestedId] : [];
  const valid = [...new Set(requested)].filter((id) => isAllowedFleetMachine(state, workspaceId, id));
  if (valid.includes('local')) return ['local'];
  if (valid.length) return valid;
  const fallback = defaultFleetMachineId(state, workspaceId);
  return fallback ? [fallback] : [];
}

export function isAllowedFleetMachineSelection(state: FleetState, workspaceId: string, ids: readonly string[]): boolean {
  return ids.length > 0 && new Set(ids).size === ids.length
    && (ids.length === 1 || !ids.includes('local'))
    && ids.every((id) => isAllowedFleetMachine(state, workspaceId, id));
}
