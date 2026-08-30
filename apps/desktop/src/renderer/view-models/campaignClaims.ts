import type { HoneycrispFindingSummary, ResearchClaimRating } from '@shared/types';
import { traceLabel } from '../lib/formatting';

export type CampaignClaimRatingValue = ResearchClaimRating | 'none';

export function campaignClaimIsActive(claim: HoneycrispFindingSummary): boolean {
  return claim.status !== 'rejected';
}

export function cvssQualitativeSeverityLabel(score: number): 'None' | 'Low' | 'Medium' | 'High' | 'Critical' {
  if (score <= 0) return 'None';
  if (score < 4) return 'Low';
  if (score < 7) return 'Medium';
  if (score < 9) return 'High';
  return 'Critical';
}

export function campaignClaimRatingPresentation(claim: HoneycrispFindingSummary): {
  label: string;
  title: string;
  value: CampaignClaimRatingValue;
} {
  const latestCvss = claim.securityTracking?.cvssAssessments.at(-1) ?? null;
  if (!latestCvss) {
    const label = traceLabel(claim.rating);
    return { label, title: `Untrusted rating: ${label}`, value: claim.rating };
  }
  const score = latestCvss.score.toFixed(1);
  const severity = cvssQualitativeSeverityLabel(latestCvss.score);
  const label = `${severity} (CVSS ${score})`;
  return { label, title: `CVSS rating: ${label}`, value: severity.toLocaleLowerCase() as CampaignClaimRatingValue };
}
