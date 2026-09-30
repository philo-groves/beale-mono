import { createHash } from 'node:crypto';
import { describe, expect, it } from 'vitest';
import type { ApprovalRecord, RunDetail } from '@shared/types';
import { AppServerRunEngine, recoveredApprovalState } from '../src/main/appServerRunEngine';

describe('app-server approval recovery', () => {
  it('reattaches pending approvals from the current attempt', () => {
    const state = recoveredApprovalState({
      policyEvents: [
        approval('approval_shell', 'request_shell', 'shell_command'),
        approval('approval_tool', 'request_tool', 'computer_use', {
          permissionMode: 'once_per_session',
          targetBinary: 'calculator'
        })
      ]
    } as Pick<RunDetail, 'policyEvents'>, 'attempt_current');

    expect([...state.shellApprovalRecords]).toEqual([
      ['request_shell', 'approval_shell'],
      ['request_tool', 'approval_tool']
    ]);
    expect([...state.toolApprovalRequestIds]).toEqual(['request_tool']);
  });

  it('does not revive approvals from an interrupted attempt or restore old session grants', () => {
    const prior = approval('approval_prior', 'request_prior', 'shell_command');
    prior.attemptId = 'attempt_prior';
    const granted = approval('approval_granted', 'request_granted', 'computer_use', {
      permissionMode: 'once_per_session',
      targetBinary: 'calculator'
    });
    granted.decision = 'approved';
    granted.decidedAt = '2026-09-18T12:01:00.000Z';

    const state = recoveredApprovalState({ policyEvents: [prior, granted] }, 'attempt_current');

    expect(state.shellApprovalRecords.size).toBe(0);
    expect([...state.toolApprovalRequestIds]).toEqual([]);
  });

  it('requests approval for each computer action against the same application', () => {
    const approvals: Array<{ decision: string; requestedAction: Record<string, unknown> }> = [];
    const engine = Object.create(AppServerRunEngine.prototype) as {
      db: {
        getRun: () => { title: string };
        createApproval: (input: { decision: string; requestedAction: Record<string, unknown> }) => { id: string };
        appendTraceEvent: () => void;
      };
      onChange: () => void;
      recordToolAuthorizationRequested: (context: unknown, event: unknown, active: unknown) => void;
    };
    engine.db = {
      getRun: () => ({ title: 'Example session' }),
      createApproval: (input) => {
        approvals.push(input);
        return { id: `approval_${approvals.length}` };
      },
      appendTraceEvent: () => undefined
    };
    engine.onChange = () => undefined;
    const active = {
      shellApprovalRecords: new Map<string, string>(),
      resolvedShellApprovalRequestIds: new Set<string>(),
      toolApprovalRequestIds: new Set<string>()
    };
    const context = { run: { id: 'run_example', title: 'Example session' }, attempt: { id: 'attempt_current' } };
    const args = { process: 'example-app', action: 'click' };
    const argumentsHash = createHash('sha256').update(JSON.stringify(args)).digest('hex');
    for (const approvalRequestId of ['request_first', 'request_second']) {
      engine.recordToolAuthorizationRequested(context, {
        payload: { approvalRequestId, arguments: args, argumentsHash, toolName: 'click' }
      }, active);
    }

    expect(approvals.map((approval) => approval.decision)).toEqual(['pending', 'pending']);
    expect([...active.shellApprovalRecords.keys()]).toEqual(['request_first', 'request_second']);
  });
});

function approval(
  id: string,
  approvalRequestId: string,
  requestKind: ApprovalRecord['requestKind'],
  requestedAction: Record<string, unknown> = {}
): ApprovalRecord {
  return {
    id,
    runId: 'run_example',
    attemptId: 'attempt_current',
    requestKind,
    requestedAction: { approvalRequestId, ...requestedAction },
    decision: 'pending',
    reason: 'Waiting for researcher approval.',
    scopeAmendmentId: null,
    createdAt: '2026-09-18T12:00:00.000Z',
    decidedAt: null
  };
}
