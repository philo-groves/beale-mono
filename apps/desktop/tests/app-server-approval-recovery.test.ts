import { describe, expect, it } from 'vitest';
import type { ApprovalRecord, RunDetail } from '@shared/types';
import { recoveredApprovalState } from '../src/main/appServerRunEngine';

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
    expect([...state.toolApprovalSessionGrantTargets]).toEqual([['request_tool', 'calculator']]);
  });

  it('does not revive approvals from an interrupted attempt and restores current session grants', () => {
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
    expect([...state.approvedComputerUseTargetBinaries]).toEqual(['calculator']);
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
