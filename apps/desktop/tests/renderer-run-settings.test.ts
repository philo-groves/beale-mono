import { describe, expect, it, vi } from 'vitest';
import {
  budgetNumber,
  clientRequestId,
  defaultRunInput,
  extendBudgetLimit,
  optionalPositiveInteger,
  UNBOUNDED_ATTEMPTS,
  UNBOUNDED_MINUTES
} from '../src/renderer/view-models/runSettings';
import { resolveSessionGoal } from '../src/shared/goalObjective';

describe('renderer run settings view model', () => {
  it('keeps new research sessions unlimited by minutes but one branch by default', () => {
    expect(defaultRunInput.budget.maxMinutes).toBe(UNBOUNDED_MINUTES);
    expect(defaultRunInput.budget.maxAttempts).toBe(1);
    expect(defaultRunInput.runEngine).toBe('app-server');
    expect(defaultRunInput.goalEnabled).toBe(false);
    expect(defaultRunInput.goalObjective).toBeNull();
    expect(defaultRunInput.provider).toBeUndefined();
    expect(defaultRunInput.shellSafetyMode).toBe('auto_review');
    expect(defaultRunInput.model).toBe('');
    expect(defaultRunInput.reasoningEffort).toBe('high');
    expect(defaultRunInput.fastMode).toBe(false);
  });

  it('requires Goal mode for an assigned workflow, including continuation of an earlier run', () => {
    const input = { ...defaultRunInput, promptMarkdown: 'Review the example module.' };
    expect(resolveSessionGoal(input)).toEqual({ enabled: false, objective: null });
    const assigned = resolveSessionGoal({ ...input,
      guidanceWorkflow: { id: 'beale.repository-auditor', values: { systems: 'example module' } } });
    expect(assigned.enabled).toBe(true);
    expect(assigned.objective).toContain('Inspect all inventoried source lines');
    expect(resolveSessionGoal({ ...input, goalEnabled: true, goalObjective: 'Investigate example source.',
      guidanceWorkflow: { id: 'beale.repository-auditor', values: { systems: 'example module' } } }).objective)
      .toBe('Investigate example source.');
  });

  it('parses optional positive integers and preserves unbounded budget extension', () => {
    expect(optionalPositiveInteger('', UNBOUNDED_MINUTES)).toBe(UNBOUNDED_MINUTES);
    expect(optionalPositiveInteger('3.8', UNBOUNDED_MINUTES)).toBe(3);
    expect(budgetNumber('bad', UNBOUNDED_ATTEMPTS)).toBe(UNBOUNDED_ATTEMPTS);
    expect(extendBudgetLimit(UNBOUNDED_MINUTES, UNBOUNDED_MINUTES, 30)).toBe(UNBOUNDED_MINUTES);
    expect(extendBudgetLimit(60, UNBOUNDED_MINUTES, 30)).toBe(90);
  });

  it('builds stable-prefixed client request ids', () => {
    vi.spyOn(Date, 'now').mockReturnValue(1_000);
    vi.spyOn(Math, 'random').mockReturnValue(0.5);

    expect(clientRequestId('research_prompt')).toMatch(/^research_prompt_/);
  });
});
