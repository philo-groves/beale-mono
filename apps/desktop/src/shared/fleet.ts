import type { FleetState } from '@beale/app-server-runtime/protocol';

export interface FleetTargetChoice {
  id: string;
  name: string;
  ready: boolean;
  detail: string;
}

export function configuredWorkspaceTargets(state: FleetState, workspaceId: string): string[] {
  return [...new Set((state.workspaceTargets[workspaceId] ?? ['local']).map((targetId) => (
    targetId === 'browser:in-agent' ? 'local' : targetId
  )))];
}

export function availableFleetTargets(state: FleetState): FleetTargetChoice[] {
  const targets: FleetTargetChoice[] = [
    { id: 'local', name: 'Local', ready: true, detail: 'Run on this machine' },
  ];
  if (state.enabled) {
    targets.push(...state.browsers.filter((browser) => browser.enabled && browser.id !== 'in-agent').map((browser) => ({
      id: `browser:${browser.id}`, name: browser.name, ready: true,
      detail: 'Separate CDP profile',
    })));
    if (state.role === 'primary') {
      for (const machine of state.machines) {
        if (machine.canRun) targets.push({
          id: `vm:run:${machine.id}`, name: `${machine.name} · Run`,
          ready: machine.sshConfigured && machine.state !== 'unknown', detail: 'Run commands in this VM',
        });
        if (machine.canClone) targets.push({
          id: `vm:clone:${machine.id}`, name: `${machine.name} · Clone`,
          ready: machine.sshConfigured && machine.state === 'stopped', detail: 'Clone for this session',
        });
      }
      for (const sandbox of state.sandboxes) {
        if (sandbox.enabled) targets.push({
          id: `sandbox:${sandbox.provider}`, name: `${sandbox.name} sandbox`,
          ready: sandbox.credentialsConfigured, detail: 'Fresh sandbox for each session',
        });
      }
    }
  }
  return targets;
}

export function allowedFleetTargets(state: FleetState, workspaceId: string): FleetTargetChoice[] {
  const allowed = new Set(configuredWorkspaceTargets(state, workspaceId));
  return availableFleetTargets(state).filter((target) => allowed.has(target.id));
}

export function defaultFleetMachineId(state: FleetState, workspaceId: string): string | null {
  const targets = allowedFleetTargets(state, workspaceId);
  const previous = state.lastMachineByWorkspace[workspaceId];
  return targets.find((target) => target.id === previous)?.id ?? targets.find((target) => target.ready)?.id ?? targets[0]?.id ?? null;
}

export function isAllowedFleetMachine(state: FleetState, workspaceId: string, targetId: string): boolean {
  return allowedFleetTargets(state, workspaceId).some((target) => target.id === targetId);
}

export function fleetBrowserViewerTargetId(machineId: unknown): string | null {
  return typeof machineId === 'string'
    && (isFleetVmTargetId(machineId) || isStreamedHostBrowserTargetId(machineId))
    ? machineId : null;
}

export function isFleetVmTargetId(machineId: string): boolean {
  return machineId.startsWith('vm:');
}

export function isStreamedHostBrowserTargetId(machineId: string): boolean {
  return machineId.startsWith('browser:') && machineId !== 'browser:in-agent';
}
