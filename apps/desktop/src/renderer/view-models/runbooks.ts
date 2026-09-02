import type { AppServerRunbookSummary } from '@shared/types';

export function runbookBelongsToSession(runbook: AppServerRunbookSummary, sessionId: string): boolean {
  return runbook.sessionId === sessionId
    || runbook.revisions.some((revision) => revision.sessionId === sessionId);
}

export function runbookDescriptionText(value: string): string {
  return value
    .replace(/[ \t]*(?:\r\n|\r|\n)[ \t]*/g, ' ')
    .replace(/ {2,}/g, ' ')
    .trim();
}

export interface RunbookExecutionStatusPresentation {
  id: 'not-run' | NonNullable<AppServerRunbookSummary['execution']['latest']>['status'];
  label: 'Not Run' | 'Running' | 'Succeeded' | 'Failed' | 'Blocked';
}

export function runbookExecutionStatus(
  runbook: Pick<AppServerRunbookSummary, 'execution'>
): RunbookExecutionStatusPresentation {
  const status = runbook.execution.latest?.status;
  if (status === 'running') return { id: status, label: 'Running' };
  if (status === 'succeeded') return { id: status, label: 'Succeeded' };
  if (status === 'failed') return { id: status, label: 'Failed' };
  if (status === 'blocked') return { id: status, label: 'Blocked' };
  return { id: 'not-run', label: 'Not Run' };
}
