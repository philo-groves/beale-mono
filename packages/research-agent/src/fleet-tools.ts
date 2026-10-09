import { nowIso } from './ids.js';
import type { ResearchExecutableTool, ResearchToolExecutionResult } from './tool-registry.js';
import type { ResearchToolAction } from './types.js';

export interface FleetToolMachine {
  id: string;
  name: string;
  backend: 'tart' | 'hyper-v';
  state: 'running' | 'stopped' | 'unknown';
  base: boolean;
  privilege: 'standard' | 'elevated';
  sshConfigured: boolean;
}
export interface FleetToolState { enabled: boolean; available: boolean; machines: FleetToolMachine[] }
export type FleetToolOperation = 'fleet.state' | 'fleet.clone' | 'fleet.start' | 'fleet.stop';
export type FleetToolInvoker = (operation: FleetToolOperation, input?: Record<string, unknown>) => Promise<FleetToolState>;

const TOOLS = [
  { name: 'fleet.list', transportName: 'fleet_list', description: 'List configured Fleet VMs, including clone-only bases and running workers.', operation: 'fleet.state', actionClass: 'inspect', sideEffects: 'read', parameters: { type: 'object', properties: {}, additionalProperties: false } },
  { name: 'fleet.clone', transportName: 'fleet_clone', description: 'Clone an operator-designated stopped base VM into a new worker VM.', operation: 'fleet.clone', actionClass: 'experiment', sideEffects: 'process', parameters: { type: 'object', required: ['baseId', 'name'], properties: { baseId: { type: 'string' }, name: { type: 'string' } }, additionalProperties: false } },
  { name: 'fleet.start', transportName: 'fleet_start', description: 'Start a Fleet worker VM. Base VMs cannot be started.', operation: 'fleet.start', actionClass: 'experiment', sideEffects: 'process', parameters: { type: 'object', required: ['machineId'], properties: { machineId: { type: 'string' } }, additionalProperties: false } },
  { name: 'fleet.stop', transportName: 'fleet_stop', description: 'Gracefully stop a Fleet worker VM.', operation: 'fleet.stop', actionClass: 'experiment', sideEffects: 'process', parameters: { type: 'object', required: ['machineId'], properties: { machineId: { type: 'string' } }, additionalProperties: false } },
] as const;

export function createFleetTools(invoke: FleetToolInvoker): ResearchExecutableTool[] {
  return TOOLS.map((spec): ResearchExecutableTool => ({
    descriptor: {
      name: spec.name, transportName: spec.transportName, description: spec.description,
      actionClasses: [spec.actionClass], sideEffects: spec.sideEffects,
      requiredPermissions: [spec.sideEffects === 'read' ? 'fleet:read' : 'fleet:manage'],
      inputSchema: spec.parameters,
      metadata: { provider: 'appServer.built_in', safetyProfile: 'fleet-operator' },
    },
    parameters: spec.parameters,
    async execute(action): Promise<ResearchToolExecutionResult> {
      const startedAt = nowIso();
      try {
        const input = spec.operation === 'fleet.clone'
          ? { baseId: required(action, 'baseId'), name: required(action, 'name') }
          : spec.operation === 'fleet.start' || spec.operation === 'fleet.stop'
            ? { machineId: required(action, 'machineId') } : {};
        const state = await invoke(spec.operation, input);
        const output = { enabled: state.enabled, available: state.available,
          machines: state.machines.map(({ id, name, backend, state, base, privilege, sshConfigured }) =>
            ({ id, name, backend, state, base, privilege, sshConfigured })) };
        return { action, status: 'complete', startedAt, completedAt: nowIso(),
          summary: `${spec.name} completed.`, output, followUpActions: [] };
      } catch (error) {
        return { action, status: 'error', startedAt, completedAt: nowIso(),
          summary: `${spec.name} failed.`, error: { message: error instanceof Error ? error.message : String(error) }, followUpActions: [] };
      }
    },
  }));
}

function required(action: ResearchToolAction, key: string): string {
  const value = action.input[key];
  if (typeof value !== 'string' || !value.trim()) throw new Error(`${key} is required.`);
  return value.trim();
}
