import { createHash } from "node:crypto";
import type {
  CampaignContradictionSummary,
  CampaignCoverageGapSummary,
  CampaignGraphEdgeSummary,
  CampaignGraphNodeSummary,
  CampaignGraphSummary,
  CampaignModelContext,
  FindingSummary,
  MemoryEdgeSummary,
  MemoryNodeSummary,
  ReportSummary,
  RunbookSummary,
  CampaignReplayMetricsSummary,
  CampaignTrackProjectionSummary,
} from "./knowledge-types.js";

const MODEL_CAMPAIGN_ACTION_LIMIT = 8;
const MODEL_CAMPAIGN_CONTRADICTION_LIMIT = 6;
const MODEL_CAMPAIGN_TRACK_LIMIT = 6;

const CONTRADICTION_RELATIONS = new Set(["contradicts", "refutes", "conflicts_with", "invalidates"]);
const SECURITY_RESEARCH_TYPES = new Set(["source", "sink", "flow-endpoint", "invariant", "trajectory"]);

export interface BuildCampaignGraphInput {
  nodes: readonly MemoryNodeSummary[];
  edges: readonly MemoryEdgeSummary[];
  findings: readonly FindingSummary[];
  runbooks: readonly RunbookSummary[];
  reports: readonly ReportSummary[];
  assetIds?: readonly string[];
  tracks?: readonly CampaignTrackProjectionSummary[];
  activeTrackId?: string | null;
  replayMetrics?: CampaignReplayMetricsSummary;
}

export function buildCampaignGraph(input: BuildCampaignGraphInput): CampaignGraphSummary {
  const graphNodes: CampaignGraphNodeSummary[] = [];
  const graphEdges: CampaignGraphEdgeSummary[] = [];
  const memoryById = new Map(input.nodes.map((node) => [node.id, node]));
  const runbookById = new Map(input.runbooks.map((runbook) => [runbook.id, runbook]));
  const reportById = new Map(input.reports.map((report) => [report.id, report]));
  const knownAssetIds = new Set([...(input.assetIds ?? []), ...input.nodes.flatMap((node) => node.assetIds)]);

  for (const assetId of [...knownAssetIds].sort()) {
    graphNodes.push({
      id: campaignNodeId("asset", assetId),
      kind: "asset",
      label: assetId,
      status: input.nodes.some((node) => node.assetIds.includes(assetId)) ? "covered" : "unexplored",
      memoryNodeId: null,
      findingId: null,
      claimId: null,
      assetId,
      evidenceCount: 0,
      updatedAt: latestTimestamp(input.nodes.filter((node) => node.assetIds.includes(assetId)).map((node) => node.updatedAt)),
    });
  }
  for (const node of input.nodes) {
    graphNodes.push({
      id: campaignNodeId("memory", node.id),
      kind: "memory",
      label: node.title,
      status: node.status,
      memoryNodeId: node.id,
      findingId: null,
      claimId: null,
      assetId: null,
      evidenceCount: node.evidenceRefs.length,
      updatedAt: node.updatedAt,
    });
    for (const assetId of node.assetIds) {
      graphEdges.push({
        fromId: campaignNodeId("asset", assetId),
        toId: campaignNodeId("memory", node.id),
        relation: "covered_by",
        contradictory: false,
      });
    }
  }
  for (const edge of input.edges) {
    graphEdges.push({
      fromId: campaignNodeId("memory", edge.fromId),
      toId: campaignNodeId("memory", edge.toId),
      relation: edge.relation,
      contradictory: contradictionRelation(edge.relation),
    });
  }
  for (const finding of input.findings) {
    const kind = finding.projection;
    graphNodes.push({
      id: campaignNodeId(kind, finding.id),
      kind,
      label: finding.title,
      status: finding.status,
      memoryNodeId: finding.memoryNodeId,
      findingId: kind === "finding" ? finding.id : null,
      claimId: finding.id,
      assetId: null,
      evidenceCount: finding.evidence.length,
      updatedAt: finding.updatedAt,
    });
    for (const componentClaimId of finding.componentClaimIds) {
      const component = input.findings.find((candidate) => candidate.id === componentClaimId);
      if (!component) continue;
      graphEdges.push({
        fromId: campaignNodeId(component.projection, component.id),
        toId: campaignNodeId(kind, finding.id),
        relation: "component_of",
        contradictory: false,
      });
    }
    if (finding.reproductionRunbookId && runbookById.has(finding.reproductionRunbookId)) {
      graphEdges.push({
        fromId: campaignNodeId(kind, finding.id),
        toId: campaignNodeId("runbook", finding.reproductionRunbookId),
        relation: "reproduced_by",
        contradictory: false,
      });
    }
    if (finding.reportId && reportById.has(finding.reportId)) {
      graphEdges.push({
        fromId: campaignNodeId(kind, finding.id),
        toId: campaignNodeId("report", finding.reportId),
        relation: "reported_by",
        contradictory: false,
      });
    }
  }
  for (const runbook of input.runbooks) {
    graphNodes.push({
      id: campaignNodeId("runbook", runbook.id),
      kind: "runbook",
      label: runbook.title,
      status: runbook.execution.latest?.status ?? "not_run",
      memoryNodeId: null,
      findingId: null,
      claimId: null,
      assetId: null,
      evidenceCount: runbook.contentRevision + runbook.execution.completedRunCount,
      updatedAt: runbook.updatedAt,
    });
  }
  for (const report of input.reports) {
    graphNodes.push({
      id: campaignNodeId("report", report.id),
      kind: "report",
      label: report.title,
      status: report.status,
      memoryNodeId: null,
      findingId: null,
      claimId: null,
      assetId: null,
      evidenceCount: report.revisions.length + (report.submissionPacket ? 1 : 0),
      updatedAt: report.updatedAt,
    });
  }

  const contradictions = campaignContradictions(input.edges, memoryById);
  const coverageGaps = campaignCoverageGaps({ ...input, assetIds: [...knownAssetIds] }, contradictions);
  const nextActions = [...coverageGaps]
    .sort((left, right) => gapRank(left.priority) - gapRank(right.priority) || left.title.localeCompare(right.title))
    .slice(0, 8);
  const momentum = campaignMomentum(input.findings, input.nodes, coverageGaps, contradictions);
  return {
    nodes: graphNodes.sort((left, right) => campaignKindRank(left.kind) - campaignKindRank(right.kind) || left.label.localeCompare(right.label)),
    edges: dedupeEdges(graphEdges),
    coverageGaps,
    contradictions,
    momentum,
    nextActions,
    counts: {
      leads: input.findings.filter((finding) => finding.projection === "lead").length,
      findings: input.findings.filter((finding) => finding.projection === "finding").length,
      verifiedFindings: input.findings.filter((finding) => ["verified", "report_ready", "disclosed"].includes(finding.status)).length,
      disclosedFindings: input.findings.filter((finding) => finding.status === "disclosed").length,
      coverageGaps: coverageGaps.length,
      contradictions: contradictions.length,
    },
    ...(input.tracks ? { tracks: [...input.tracks] } : {}),
    ...(input.activeTrackId !== undefined ? { activeTrackId: input.activeTrackId } : {}),
    ...(input.replayMetrics ? { replayMetrics: input.replayMetrics } : {}),
  };
}

export function createCampaignModelContext(campaign: CampaignGraphSummary): CampaignModelContext {
  const activeTrackSummary = campaign.activeTrackId
    ? campaign.tracks?.find((track) => track.id === campaign.activeTrackId) ?? null
    : null;
  const activeTrack = activeTrackSummary ? campaignModelTrack(activeTrackSummary) : null;
  const recentTracks = [...(campaign.tracks ?? [])]
    .filter((track) => track.id !== activeTrackSummary?.id)
    .sort((left, right) => right.updatedAt.localeCompare(left.updatedAt))
    .slice(0, MODEL_CAMPAIGN_TRACK_LIMIT)
    .map(campaignModelTrack);
  return {
    schemaVersion: 1,
    counts: { ...campaign.counts },
    momentum: {
      ...campaign.momentum,
      supportingNodeIds: campaign.momentum.supportingNodeIds.slice(0, 12),
    },
    nextActions: campaign.nextActions.slice(0, MODEL_CAMPAIGN_ACTION_LIMIT).map((action) => ({
      ...action,
      relatedNodeIds: action.relatedNodeIds.slice(0, 12),
    })),
    contradictions: campaign.contradictions.slice(0, MODEL_CAMPAIGN_CONTRADICTION_LIMIT),
    activeTrack,
    recentTracks,
    omitted: {
      nodes: campaign.nodes.length,
      edges: campaign.edges.length,
      coverageGaps: Math.max(0, campaign.coverageGaps.length - MODEL_CAMPAIGN_ACTION_LIMIT),
      contradictions: Math.max(0, campaign.contradictions.length - MODEL_CAMPAIGN_CONTRADICTION_LIMIT),
      tracks: Math.max(0, (campaign.tracks?.length ?? 0) - recentTracks.length - (activeTrack ? 1 : 0)),
    },
  };
}

function campaignModelTrack(track: CampaignTrackProjectionSummary): CampaignModelContext["recentTracks"][number] {
  return {
    id: track.id,
    title: track.title,
    objective: track.objective,
    status: track.status,
    stage: track.stage,
    source: track.source,
    updatedAt: track.updatedAt,
    revision: track.revision,
    counts: { ...track.counts },
  };
}

export function emptyCampaignGraph(): CampaignGraphSummary {
  return buildCampaignGraph({ nodes: [], edges: [], findings: [], runbooks: [], reports: [], assetIds: [] });
}

function campaignCoverageGaps(
  input: BuildCampaignGraphInput & { assetIds: readonly string[] },
  contradictions: readonly CampaignContradictionSummary[],
): CampaignCoverageGapSummary[] {
  const gaps: CampaignCoverageGapSummary[] = [];
  for (const assetId of input.assetIds) {
    if (input.nodes.some((node) => node.assetIds.includes(assetId))) continue;
    gaps.push(gap("unexplored_asset", "high", `Unexplored asset: ${assetId}`,
      "No durable research memory is associated with this authorized asset.",
      [campaignNodeId("asset", assetId)],
      `Map the attack surface of authorized asset ${assetId}. Search existing memory first, avoid repeating covered territory, and record evidence-backed boundaries, sources, sinks, or hypotheses.`));
  }
  for (const node of input.nodes) {
    if (SECURITY_RESEARCH_TYPES.has(node.type) && node.evidenceRefs.length === 0) {
      gaps.push(gap("unsupported_memory", "medium",
        `Unsupported ${node.type}: ${node.title}`,
        "The durable claim has no direct evidence reference.",
        [campaignNodeId("memory", node.id)],
        `Seek direct evidence that establishes, narrows, or genuinely contradicts ${node.title}. Treat missing support as an open obligation, and correct the durable memory rather than creating a duplicate.`));
    }
  }
  for (const finding of input.findings) {
    const related = [campaignNodeId(finding.projection, finding.id)];
    if (finding.status === "hypothesis") {
      gaps.push(gap("unobserved_hypothesis", "high", `Establish or narrow: ${finding.title}`,
        "The candidate has not crossed the direct-observation evidence gate.", related,
        `Pursue the next positive proof obligation for lead ${finding.id} and record direct observation evidence when obtained. Also capture evidence that genuinely contradicts or narrows a necessary link; an incomplete or failed attempt is not refutation. Update the same claim rather than creating a duplicate finding.`));
    } else if (finding.status === "observed") {
      gaps.push(gap("missing_reproduction", "critical", `Reproduce: ${finding.title}`,
        "The behavior was observed but has no successful reusable runbook proof.", related,
        `Create or complete a bounded runbook that reproduces claim ${finding.id} on a clean target state, execute it, and attach the successful run as evidence.`));
    } else if (finding.status === "reproduced") {
      gaps.push(gap("missing_independent_verification", "critical", `Independently verify: ${finding.title}`,
        "The reproduction has not been challenged by an independent reviewer.", related,
        `Independently verify claim ${finding.id} from its runbook and evidence. Challenge assumptions, preserve dissent, and attach independent verification evidence only if the result holds.`));
    } else if (finding.status === "verified") {
      gaps.push(gap("missing_report", "high", `Report: ${finding.title}`,
        "The verified finding has not been bound to a complete report artifact.", related,
        `Create a standalone report for verified finding ${finding.id}, cite its accepted evidence and reproduction runbook, then advance the same claim to report-ready.`));
    } else if (finding.status === "stale") {
      gaps.push(gap("stale_finding", "critical", `Revalidate stale finding: ${finding.title}`,
        finding.staleReason ?? "The source revision or execution environment changed.", related,
        `Revalidate stale claim ${finding.id} against the current source revision and environment. Start from its prior evidence and runbook; do not repeat unrelated discovery.`));
    }
  }
  for (const contradiction of contradictions) {
    gaps.push(gap("contradiction", "critical", "Resolve contradictory research claims", contradiction.summary,
      [contradiction.fromNodeId, contradiction.toNodeId],
      `Resolve campaign contradiction ${contradiction.id} with a discriminating experiment. Preserve both claims until evidence identifies which is valid or whether their conditions differ.`));
  }
  return dedupeGaps(gaps).sort((left, right) => gapRank(left.priority) - gapRank(right.priority) || left.title.localeCompare(right.title));
}

function campaignContradictions(
  edges: readonly MemoryEdgeSummary[],
  memoryById: ReadonlyMap<string, MemoryNodeSummary>,
): CampaignContradictionSummary[] {
  return edges.filter((edge) => contradictionRelation(edge.relation)).map((edge) => {
    const from = memoryById.get(edge.fromId);
    const to = memoryById.get(edge.toId);
    return {
      id: `contradiction_${hash(`${edge.fromId}\0${edge.toId}\0${edge.relation}`)}`,
      fromNodeId: campaignNodeId("memory", edge.fromId),
      toNodeId: campaignNodeId("memory", edge.toId),
      relation: edge.relation,
      summary: edge.note || `${from?.title ?? edge.fromId} ${edge.relation} ${to?.title ?? edge.toId}.`,
    };
  });
}

function campaignMomentum(
  findings: readonly FindingSummary[],
  nodes: readonly MemoryNodeSummary[],
  gaps: readonly CampaignCoverageGapSummary[],
  contradictions: readonly CampaignContradictionSummary[],
): CampaignGraphSummary["momentum"] {
  if (nodes.length === 0 && findings.length === 0) return { state: "empty", reason: "No durable campaign state has been recorded.", supportingNodeIds: [] };
  if (contradictions.length > 0 || findings.some((finding) => finding.status === "stale")) {
    const supportingNodeIds = [
      ...contradictions.flatMap((item) => [item.fromNodeId, item.toNodeId]),
      ...findings.filter((finding) => finding.status === "stale").map((finding) => campaignNodeId(finding.projection, finding.id)),
    ];
    return { state: "blocked", reason: "Contradictory or stale conclusions require resolution before advancing the campaign.", supportingNodeIds };
  }
  if (findings.length > 0 && gaps.length === 0
    && findings.every((finding) => finding.status === "disclosed" || finding.status === "rejected")) {
    return { state: "complete", reason: "Every recorded claim is published or refuted.", supportingNodeIds: findings.map((finding) => campaignNodeId(finding.projection, finding.id)) };
  }
  const active = findings.find((finding) => !["disclosed", "rejected"].includes(finding.status));
  if (active?.status === "report_ready" || active?.status === "verified") return { state: "reporting", reason: "A verified finding is moving through reporting and publication.", supportingNodeIds: [campaignNodeId(active.projection, active.id)] };
  if (active?.status === "reproduced") return { state: "verifying", reason: "A reproducible finding is awaiting independent verification.", supportingNodeIds: [campaignNodeId(active.projection, active.id)] };
  if (active?.status === "observed") return { state: "reproducing", reason: "An observed finding needs a reusable clean-state reproduction.", supportingNodeIds: [campaignNodeId(active.projection, active.id)] };
  if (active?.status === "hypothesis") return { state: "observed", reason: "A lead is awaiting positive direct observation or evidence that genuinely narrows or contradicts it.", supportingNodeIds: [campaignNodeId(active.projection, active.id)] };
  if (gaps.some((item) => item.kind === "unobserved_hypothesis")) return { state: "building", reason: "Existing hypotheses should be tested before opening overlapping exploration.", supportingNodeIds: gaps.flatMap((item) => item.relatedNodeIds).slice(0, 8) };
  return { state: "exploring", reason: "The campaign is expanding evidence-backed attack-surface coverage.", supportingNodeIds: nodes.slice(0, 8).map((node) => campaignNodeId("memory", node.id)) };
}

function gap(kind: CampaignCoverageGapSummary["kind"], priority: CampaignCoverageGapSummary["priority"], title: string, rationale: string, relatedNodeIds: string[], suggestedPrompt: string): CampaignCoverageGapSummary {
  return { id: `gap_${hash(`${kind}\0${relatedNodeIds.join("\0")}`)}`, kind, priority, title, rationale, relatedNodeIds, suggestedPrompt };
}
function dedupeEdges(edges: readonly CampaignGraphEdgeSummary[]): CampaignGraphEdgeSummary[] { const seen = new Set<string>(); return edges.filter((edge) => { const key = `${edge.fromId}\0${edge.toId}\0${edge.relation}`; if (seen.has(key)) return false; seen.add(key); return true; }); }
function dedupeGaps(gaps: readonly CampaignCoverageGapSummary[]): CampaignCoverageGapSummary[] { return [...new Map(gaps.map((item) => [item.id, item])).values()]; }
function contradictionRelation(value: string): boolean { return CONTRADICTION_RELATIONS.has(value.trim().toLowerCase().replace(/[ -]+/gu, "_")); }
function campaignNodeId(kind: CampaignGraphNodeSummary["kind"], id: string): string { return `${kind}:${id}`; }
function campaignKindRank(kind: CampaignGraphNodeSummary["kind"]): number { return { asset: 0, memory: 1, lead: 2, finding: 3, runbook: 4, report: 5 }[kind]; }
function gapRank(priority: CampaignCoverageGapSummary["priority"]): number { return { critical: 0, high: 1, medium: 2, low: 3 }[priority]; }
function latestTimestamp(values: readonly string[]): string { return [...values].sort().at(-1) ?? new Date(0).toISOString(); }
function hash(value: string): string { return createHash("sha256").update(value).digest("hex").slice(0, 20); }
