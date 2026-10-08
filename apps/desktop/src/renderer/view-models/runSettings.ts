import type { ProviderSettings, ResearchModelProviderId, SessionWorkflowDefinition, ShellSafetyMode, StartRunInput } from '@shared/types';
import { DEFAULT_RESEARCH_REASONING_EFFORT } from '../../shared/modelDefaults';
import { DEFAULT_SHELL_SAFETY_MODE } from '../../shared/shellSafety';
import { DEFAULT_RESEARCH_COLLABORATION } from '../../shared/collaboration';
import { assignedWorkflowGoalObjective } from '../../shared/goalObjective';

export const UNBOUNDED_MINUTES = 999_999;
export const UNBOUNDED_ATTEMPTS = 999_999;

export const defaultRunInput: StartRunInput = {
  runEngine: 'app-server',
  shellSafetyMode: DEFAULT_SHELL_SAFETY_MODE,
  goalEnabled: false,
  goalObjective: null,
  promptMarkdown: '',
  mode: 'dynamic',
  attemptStrategy: 'iterative_research',
  model: '',
  reasoningEffort: DEFAULT_RESEARCH_REASONING_EFFORT,
  fastMode: false,
  collaboration: { ...DEFAULT_RESEARCH_COLLABORATION, providers: [] },
  sandboxProfile: 'host',
  budget: {
    maxMinutes: UNBOUNDED_MINUTES,
    maxAttempts: 1,
    maxCostUsd: 0,
    repeatSchedule: { type: 'none' }
  }
};

export function workflowRunInput(workflow: Pick<SessionWorkflowDefinition, 'id' | 'title' | 'description' | 'fields'>, shellSafetyMode: ShellSafetyMode, values: Record<string, string>, providerSettings: ProviderSettings): StartRunInput & { provider: ResearchModelProviderId } {
  const provider = providerSettings.defaultProviderId;
  const defaults = provider ? providerSettings.modelDefaults[provider] : null;
  if (!provider || !defaults?.largeModel) throw new Error('Choose a default Lead provider and large model in Provider settings before running a workflow.');
  if (defaults.reasoningEffort === 'off') throw new Error('The default large model needs a supported reasoning effort for Advanced collaboration.');
  const modelSelection = { provider, model: defaults.largeModel, reasoningEffort: defaults.reasoningEffort };
  const configuration = workflow.fields.map((field) => `- ${field.label}: ${values[field.id]?.trim() || '(empty)'}`).join('\n');
  return {
    ...defaultRunInput,
    goalEnabled: true,
    goalObjective: assignedWorkflowGoalObjective(workflow.id, workflow.title),
    provider: modelSelection.provider,
    model: modelSelection.model,
    reasoningEffort: modelSelection.reasoningEffort,
    collaboration: { ...DEFAULT_RESEARCH_COLLABORATION, subagentMode: 'advanced',
      providers: [{ ...modelSelection, enabled: true }] },
    shellSafetyMode,
    promptMarkdown: `Run the assigned ${workflow.title} workflow.\n\n${workflow.description}\n\nSession configuration:\n${configuration || '(No fields)'}\n\nFollow the assigned runbook in order and record progress after each cell.`,
    guidanceWorkflow: { id: workflow.id, values: { ...values } },
    budget: { ...defaultRunInput.budget }
  };
}

export function budgetNumber(value: unknown, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback;
}

export function optionalPositiveInteger(rawValue: string, fallback: number): number {
  const trimmed = rawValue.trim();
  if (!trimmed) return fallback;
  const value = Math.floor(Number(trimmed));
  return Number.isFinite(value) ? Math.max(1, value) : fallback;
}

export function extendBudgetLimit(value: unknown, unboundedValue: number, step: number): number {
  const current = budgetNumber(value, unboundedValue);
  return current >= unboundedValue ? unboundedValue : current + step;
}

export function clientRequestId(prefix: string): string {
  return `${prefix}_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 10)}`;
}
