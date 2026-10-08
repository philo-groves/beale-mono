import { useEffect, useState } from 'react';
import type { JSX } from 'react';
import type { SessionWorkflowAssignment } from '@shared/types';

export function SessionWorkflowProgress({ sessionId }: { sessionId: string }): JSX.Element | null {
  const [assignment, setAssignment] = useState<SessionWorkflowAssignment | null>(null);
  useEffect(() => {
    let disposed = false;
    const refresh = (): void => {
      void window.beale.getSessionWorkflow(sessionId).then((current) => {
        if (!disposed) setAssignment(current);
      }).catch(() => { if (!disposed) setAssignment(null); });
    };
    refresh();
    const timer = window.setInterval(refresh, 10_000);
    return () => { disposed = true; window.clearInterval(timer); };
  }, [sessionId]);
  if (!assignment) return null;
  const step = assignment.definition.steps[assignment.stepIndex];
  return <div className="session-workflow-progress" role="status">
    <strong>{assignment.definition.title}</strong>
    <span>{assignment.completedAt ? 'Complete' : `Step ${assignment.stepIndex + 1} of ${assignment.definition.steps.length}: ${step?.title ?? 'Complete'}`}</span>
    {assignment.definition.kind === 'repository_auditor' ? <>
      <span>{new Set(assignment.auditTargets.map((target) => target.system)).size} systems · {assignment.scopePaths.length} inventoried paths · {assignment.coveredFileCount} of {assignment.fileCount} text files fully covered</span>
      {assignment.auditTargets.length ? <details><summary>Declared audit scope</summary><ul>
        {assignment.auditTargets.map((target) => <li key={`${target.system}:${target.path}`}>{target.system}: <code>{target.path}</code></li>)}
      </ul></details> : null}
    </> : null}
    {assignment.completedSteps.length ? <details><summary>Completed step notes</summary><ol>
      {assignment.completedSteps.map((completed, index) => <li key={`${completed.stepId}:${completed.completedAt}:${index}`}>
        <strong>{assignment.definition.steps.find((step) => step.id === completed.stepId)?.title ?? completed.stepId}:</strong> {completed.note}
      </li>)}
    </ol></details> : null}
  </div>;
}
