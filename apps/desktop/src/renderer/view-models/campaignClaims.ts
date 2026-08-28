import type { HoneycrispFindingSummary } from '@shared/types';

export function campaignClaimIsActive(claim: HoneycrispFindingSummary): boolean {
  return claim.status !== 'rejected';
}
