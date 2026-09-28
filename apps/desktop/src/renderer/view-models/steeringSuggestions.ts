import type { RunStatus } from '@shared/types';

export type SteeringInputTabAction = 'accept_suggestion' | 'show_suggestion' | 'none';

export function steeringSuggestionAutoVisible(status: RunStatus | null): boolean {
  return status === 'blocked' || status === 'completed' || status === 'failed' || status === 'stopped';
}

export function steeringInputTabAction(input: {
  instruction: string;
  suggestion: string | null;
  suggestionShowing: boolean;
}): SteeringInputTabAction {
  if (input.instruction.trim() || !input.suggestion) return 'none';
  return input.suggestionShowing ? 'accept_suggestion' : 'show_suggestion';
}
