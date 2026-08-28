import type { FindingSummary, MemoryNodeSummary } from "./knowledge-types.js";

export type ResearchAttentionHeat = "none" | "low" | "medium" | "high" | "critical";

/**
 * Product-level attention policy. Research profiles describe domain semantics;
 * they do not decide notification urgency or shell color.
 */
export function attentionHeatForClaim(claim: Pick<FindingSummary,
  "projection" | "maturity" | "freshness" | "workflow">): ResearchAttentionHeat {
  if (claim.workflow === "closed" || claim.workflow === "published") return "none";
  if (claim.freshness === "stale") return claim.projection === "finding" ? "high" : "low";
  if (claim.projection === "lead") return "low";
  if (claim.maturity === "observed") return "medium";
  if (claim.maturity === "reproduced") return "high";
  if (claim.maturity === "verified") return claim.workflow === "reporting" ? "high" : "critical";
  return "none";
}

export function attentionHeatForMemoryNode(node: Pick<MemoryNodeSummary, "type" | "status">): ResearchAttentionHeat {
  const type = node.type.trim().toLowerCase();
  const status = node.status.trim().toLowerCase();
  if ((type === "sink" || type === "flow-endpoint") && (status === "confirmed" || status === "verified")) return "low";
  return "none";
}

export function maximumAttentionHeat(values: readonly ResearchAttentionHeat[]): ResearchAttentionHeat {
  const levels: readonly ResearchAttentionHeat[] = ["none", "low", "medium", "high", "critical"];
  return values.reduce((maximum, value) => levels.indexOf(value) > levels.indexOf(maximum) ? value : maximum, "none");
}
