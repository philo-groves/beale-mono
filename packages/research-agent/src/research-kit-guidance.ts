export const APPLE_SECURITY_BOUNTY_RESEARCH_KIT_ID = "apple-security-bounty";

export function researchKitFirstTouchGuidance(researchKitId?: string): readonly string[] {
  return researchKitId === APPLE_SECURITY_BOUNTY_RESEARCH_KIT_ID
    ? [
        "For Apple components, include relevant Apple Open Source releases and upstream project history, then compare source drops or tags against the researched build when available.",
      ]
    : [];
}
