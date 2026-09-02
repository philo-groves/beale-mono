import type { ResearchProfile } from "./research-profile.js";

export interface ModelAuthorSummary {
  provider: string;
  model: string;
}

export interface ResearchProfileSnapshot {
  id: string;
  workspaceId: string;
  profileId: string;
  profileVersion: string;
  profileHash: string;
  source: "bundled-default" | "workspace-default" | "explicit";
  sourcePath: string | null;
  profile: ResearchProfile;
  active: boolean;
  createdAt: string;
}

export interface MemoryDirectorySummary {
  name: "artifacts";
  path: string;
  purpose: string;
  exists: boolean;
  entryCount: number;
}

export interface MemoryEvidenceRefSummary {
  id: string;
  kind: string;
  pathBase: string | null;
  path: string | null;
  locator: Record<string, unknown>;
  summary: string;
  createdAt: string;
}

export type MemoryNodeValidationKind = "full" | "scoped" | "inherited";

export interface MemoryNodeCatalogValidationSummary {
  nodeRevision: number;
  catalogHash: string;
  contentHash: string;
  kind: MemoryNodeValidationKind;
  validatedAt: string;
  researchProfile?: { hash: string; id: string; version: string };
}

export type MemoryNodeProvenanceSummary =
  | { state: "legacy_unrecorded"; catalogHash: null; activeCatalog: false; validation: null }
  | { state: "catalog_unvalidated"; catalogHash: string; activeCatalog: boolean; validation: null }
  | { state: "active_validated"; catalogHash: string; activeCatalog: true; validation: MemoryNodeCatalogValidationSummary }
  | { state: "foreign_validated"; catalogHash: string; activeCatalog: false; validation: MemoryNodeCatalogValidationSummary };

export interface MemoryNodeSummary {
  id: string;
  sessionIds: string[];
  workspaces: Array<{ id: string; name: string }>;
  subjectId: string;
  subjectName: string;
  type: string;
  title: string;
  summary: string;
  body: string;
  status: string;
  confidence: number;
  assetIds: string[];
  tags: string[];
  attributes: Record<string, unknown>;
  evidenceRefs: MemoryEvidenceRefSummary[];
  createdAt: string;
  updatedAt: string;
  revision: number;
  authors: ModelAuthorSummary[];
  provenance?: MemoryNodeProvenanceSummary;
}

export interface MemoryEdgeSummary {
  fromId: string;
  toId: string;
  relation: string;
  note: string;
  createdAt: string;
  updatedAt: string;
}

export type FindingStatus =
  | "hypothesis"
  | "observed"
  | "reproduced"
  | "verified"
  | "report_ready"
  | "disclosed"
  | "stale"
  | "rejected";

export type ResearchClaimProjection = "lead" | "finding";
export type ResearchClaimMaturity = "proposed" | "observed" | "reproduced" | "verified" | "refuted";
export type ResearchClaimFreshness = "current" | "stale";
export type ResearchClaimWorkflow = "open" | "active" | "reporting" | "published" | "closed";
export type ResearchClaimRating = "informational" | "low" | "medium" | "high" | "critical";

export type FindingEvidenceKind =
  | "code"
  | "artifact"
  | "command"
  | "url"
  | "calculation"
  | "proof"
  | "publication"
  | "human_review"
  | "runbook_execution"
  | "independent_verification"
  | "report"
  | "disclosure";

export interface FindingEvidenceSummary {
  id: string;
  kind: FindingEvidenceKind;
  referenceId: string | null;
  contentHash: string | null;
  summary: string;
  sessionId: string | null;
  actorId: string | null;
  independent: boolean;
  metadata: Record<string, unknown>;
  createdAt: string;
}

export interface FindingTransitionSummary {
  id: string;
  revision: number;
  fromStatus: FindingStatus | null;
  toStatus: FindingStatus;
  reason: string;
  sessionId: string | null;
  actorId: string | null;
  evidenceIds: string[];
  createdAt: string;
}

export type FindingReachabilityState = "not_assessed" | "unreachable" | "conditional" | "reachable";
export type FindingRiskTreatment = "unreviewed" | "remediate" | "mitigated" | "accepted" | "transferred";
export type FindingCvssVersion = "4.0" | "3.1";
export type FindingCvssNomenclature = "CVSS-B" | "CVSS-BT" | "CVSS-BE" | "CVSS-BTE" | "CVSS:3.1";

export interface FindingReachabilityAssessment {
  state: FindingReachabilityState;
  conditions: string;
  evidenceIds: string[];
  assessorId: string | null;
  assessedAt: string | null;
  sourceRevision: string | null;
  environmentFingerprint: string | null;
}

export interface FindingRiskDecision {
  treatment: FindingRiskTreatment;
  actorId: string;
  rationale: string;
  decidedAt: string;
  expiresAt: string | null;
}

export interface FindingCvssAssessment {
  version: FindingCvssVersion;
  vector: string;
  score: number;
  nomenclature: FindingCvssNomenclature;
  assessorId: string;
  assessedAt: string;
  environmentFingerprint: string | null;
}

export interface FindingAffectedVersion {
  assetId: string | null;
  range: string;
  fixedVersion: string | null;
}

export interface FindingExternalReference {
  kind: string;
  identifier: string;
  url: string | null;
}

export interface FindingSecurityTracking {
  reachability: FindingReachabilityAssessment;
  riskTreatment: FindingRiskTreatment;
  riskDecisions: FindingRiskDecision[];
  cvssAssessments: FindingCvssAssessment[];
  affectedAssetIds: string[];
  affectedVersions: FindingAffectedVersion[];
  externalReferences: FindingExternalReference[];
}

export interface FindingSummary {
  id: string;
  workspaceId: string;
  subjectId: string;
  /** Legacy memory identity retained only for migration/audit. New claims do not require a memory node. */
  memoryNodeId: string | null;
  originSessionId: string | null;
  projection: ResearchClaimProjection;
  maturity: ResearchClaimMaturity;
  freshness: ResearchClaimFreshness;
  workflow: ResearchClaimWorkflow;
  /** Untrusted qualitative estimate supplied by a research agent; not an evidence-backed risk assessment. */
  rating: ResearchClaimRating;
  classification: string;
  componentClaimIds: string[];
  title: string;
  summary: string;
  impact: string;
  securityTracking: FindingSecurityTracking | null;
  status: FindingStatus;
  staleFromStatus: FindingStatus | null;
  confidence: number;
  sourceRevision: string | null;
  environmentFingerprint: string | null;
  reproductionRunbookId: string | null;
  reportId: string | null;
  disclosureReference: string | null;
  staleReason: string | null;
  evidence: FindingEvidenceSummary[];
  transitions: FindingTransitionSummary[];
  authors: ModelAuthorSummary[];
  createdAt: string;
  updatedAt: string;
  revision: number;
}

export type ResearchClaimSummary = FindingSummary;
export type LeadSummary = FindingSummary & { projection: "lead" };

export type CampaignNodeKind = "memory" | "lead" | "finding" | "runbook" | "report" | "asset";

export interface CampaignGraphNodeSummary {
  id: string;
  kind: CampaignNodeKind;
  label: string;
  status: string;
  memoryNodeId: string | null;
  findingId: string | null;
  claimId: string | null;
  assetId: string | null;
  evidenceCount: number;
  updatedAt: string;
}

export interface CampaignGraphEdgeSummary {
  fromId: string;
  toId: string;
  relation: string;
  contradictory: boolean;
}

export type CampaignGapKind =
  | "unexplored_asset"
  | "unsupported_memory"
  | "unobserved_hypothesis"
  | "missing_reproduction"
  | "missing_independent_verification"
  | "missing_report"
  | "stale_finding"
  | "contradiction";

export interface CampaignCoverageGapSummary {
  id: string;
  kind: CampaignGapKind;
  priority: "critical" | "high" | "medium" | "low";
  title: string;
  rationale: string;
  relatedNodeIds: string[];
  suggestedPrompt: string;
}

export interface CampaignContradictionSummary {
  id: string;
  fromNodeId: string;
  toNodeId: string;
  relation: string;
  summary: string;
}

export interface CampaignTrackProjectionSummary {
  id: string;
  title: string;
  objective: string;
  status: "active" | "blocked" | "complete" | "archived";
  stage: "orienting" | "exploring" | "testing" | "reproducing" | "verifying" | "reporting" | "complete" | "blocked";
  source: "runtime" | "shadow" | "replay" | "manual";
  sessionIds: string[];
  updatedAt: string;
  revision: number;
  questions: CampaignQuestionProjectionSummary[];
  experiments: CampaignExperimentProjectionSummary[];
  observations: CampaignObservationProjectionSummary[];
  counts: {
    questions: number;
    openQuestions: number;
    experiments: number;
    observations: number;
    openNextActions: number;
    memoryNodes: number;
    evidenceRefs: number;
    findings: number;
    runbooks: number;
    reports: number;
  };
}

export interface CampaignQuestionProjectionSummary {
  id: string;
  investigationId: string;
  text: string;
  status: "open" | "answered" | "blocked" | "superseded";
  priority: "critical" | "high" | "medium" | "low";
  answer: string;
  updatedAt: string;
  revision: number;
}

export interface CampaignExperimentProjectionSummary {
  id: string;
  investigationId: string;
  questionId: string | null;
  runbookId: string | null;
  title: string;
  status: "planned" | "running" | "succeeded" | "failed" | "inconclusive" | "blocked";
  resultSummary: string;
  startedAt: string | null;
  completedAt: string | null;
  updatedAt: string;
  revision: number;
}

export interface CampaignObservationProjectionSummary {
  id: string;
  investigationId: string;
  experimentId: string | null;
  kind: "source" | "runtime" | "artifact" | "verifier" | "human" | "historical";
  outcome: "supports" | "refutes" | "narrows" | "neutral";
  summary: string;
  createdAt: string;
}

export interface CampaignReplayMetricsSummary {
  schemaVersion: 1;
  mode: "historical" | "shadow" | "active";
  workspaceId: string;
  sessionCount: number;
  generatedTrackCount: number;
  linkedMemoryNodeCount: number;
  repeatedMemoryCandidateCount: number;
  rejectedHypothesisResurrectionCount: number;
  environmentTaggedNodeRate: number;
  crossSessionReuseRate: number;
  medianMinutesToFirstEvidence: number | null;
}

export type CampaignMomentumState =
  | "empty"
  | "exploring"
  | "building"
  | "observed"
  | "reproducing"
  | "verifying"
  | "reporting"
  | "complete"
  | "blocked";

export interface CampaignGraphSummary {
  nodes: CampaignGraphNodeSummary[];
  edges: CampaignGraphEdgeSummary[];
  coverageGaps: CampaignCoverageGapSummary[];
  contradictions: CampaignContradictionSummary[];
  momentum: {
    state: CampaignMomentumState;
    reason: string;
    supportingNodeIds: string[];
  };
  nextActions: CampaignCoverageGapSummary[];
  counts: {
    leads: number;
    findings: number;
    verifiedFindings: number;
    disclosedFindings: number;
    coverageGaps: number;
    contradictions: number;
  };
  tracks?: CampaignTrackProjectionSummary[];
  activeTrackId?: string | null;
  replayMetrics?: CampaignReplayMetricsSummary;
}

/**
 * Bounded campaign state supplied at session startup. The complete campaign
 * graph remains durable host/UI state and is available through focused tools;
 * it is intentionally not copied into every model context.
 */
export interface CampaignModelContext {
  schemaVersion: 1;
  counts: CampaignGraphSummary["counts"];
  momentum: CampaignGraphSummary["momentum"];
  nextActions: CampaignCoverageGapSummary[];
  contradictions: CampaignContradictionSummary[];
  activeTrack: CampaignModelTrackContext | null;
  recentTracks: CampaignModelTrackContext[];
  omitted: {
    nodes: number;
    edges: number;
    coverageGaps: number;
    contradictions: number;
    tracks: number;
  };
}

export type CampaignModelTrackContext = Pick<
  CampaignTrackProjectionSummary,
  "id" | "title" | "objective" | "status" | "stage" | "source" | "updatedAt" | "revision" | "counts"
>;

export interface ArtifactRevisionSummary {
  revision: number;
  sessionId: string | null;
  createdAt: string;
}

export interface RunbookSummary {
  id: string;
  workspaceId: string;
  workspaceName: string;
  subjectId: string | null;
  subjectName: string | null;
  sessionId: string | null;
  title: string;
  purpose: string;
  artifactId: string;
  revision: number;
  contentRevision: number;
  execution: {
    runCount: number;
    completedRunCount: number;
    executedCellCount: number;
    latest: {
      runId: string;
      status: "running" | "succeeded" | "failed" | "blocked";
      startedAt: string;
    } | null;
    latestSuccessfulRunId: string | null;
  };
  revisions: ArtifactRevisionSummary[];
  authors: ModelAuthorSummary[];
  createdAt: string;
  updatedAt: string;
}

export interface ReportSummary {
  id: string;
  workspaceId: string;
  workspaceName: string;
  subjectId: string | null;
  subjectName: string | null;
  sessionId: string | null;
  title: string;
  summary: string;
  status: "complete" | "stale";
  triageStatus: "editing" | "submitted" | "reviewing" | "rejected" | "accepted";
  artifactId: string;
  submissionPacket: ReportSubmissionPacketSummary | null;
  recording: ReportRecordingSummary | null;
  revision: number;
  revisions: ArtifactRevisionSummary[];
  authors: ModelAuthorSummary[];
  createdAt: string;
  updatedAt: string;
}

export interface ReportSubmissionPacketSummary {
  artifactId: string;
  filename: string;
  sizeBytes: number;
  contentHash: string;
}

export interface ReportRecordingSummary {
  artifactId: string;
  filename: string;
  sizeBytes: number;
  contentHash: string;
}

export type MemoryDreamingAction = "prune" | "merge_duplicates" | "revise" | "reclassify";

export interface MemoryDreamingChangeSummary {
  id: string;
  runId: string;
  action: MemoryDreamingAction;
  title: string;
  nodeType: string;
  hiddenNodeIds: string[];
  survivorNodeId: string | null;
  reason: string;
  createdAt: string;
  restoredAt: string | null;
  canRestore: boolean;
}

export interface MemoryDreamingRunSummary {
  id: string;
  status: "completed" | "restored" | "failed";
  model: string;
  reasoningEffort: string;
  inputNodeCount: number;
  inputSessionCount: number;
  prunedNodeCount: number;
  duplicateHiddenCount: number;
  duplicateGroupCount: number;
  reclassifiedNodeCount: number;
  editedNodeCount: number;
  createdAt: string;
  completedAt: string;
  restoredAt: string | null;
  errorMessage: string | null;
}

export interface MemoryDreamingSummary {
  available: boolean;
  scope: "workspace";
  hiddenNodeCount: number;
  restorableChangeCount: number;
  lastRun: MemoryDreamingRunSummary | null;
  changes: MemoryDreamingChangeSummary[];
}

export interface MemorySummary {
  status: "missing" | "empty" | "ready" | "error";
  source: "none" | "app_server_sqlite";
  contextWorkspaceId: string;
  contextSubjectId: string;
  activeCatalogHash?: string | null;
  databasePath: string;
  storageRoot: string;
  artifactDirectoryPath: string;
  databaseSizeBytes: number;
  nodeCount: number;
  edgeCount: number;
  evidenceRefCount: number;
  storageArtifactCount: number;
  runbookCount: number;
  reportCount: number;
  latestNodeUpdatedAt: string | null;
  nodeTypeCounts: Record<string, number>;
  nodeStatusCounts: Record<string, number>;
  nodeProvenanceCounts?: Partial<Record<MemoryNodeProvenanceSummary["state"], number>>;
  nodes: MemoryNodeSummary[];
  edges: MemoryEdgeSummary[];
  runbooks: RunbookSummary[];
  reports: ReportSummary[];
  leads: LeadSummary[];
  findings: FindingSummary[];
  campaign: CampaignGraphSummary;
  dreaming: MemoryDreamingSummary;
  directories: MemoryDirectorySummary[];
  lastError: string | null;
}

export interface RunbookOutput {
  kind: "stream" | "display" | "error";
  text: string;
  streamName: "stdout" | "stderr" | null;
  mimeType: string | null;
}

export interface RunbookExecutionSummary {
  runId: string;
  status: "queued" | "running" | "succeeded" | "failed" | "blocked" | "skipped";
  startedAt: string;
  completedAt: string | null;
  durationMs: number | null;
  exitCode: number | null;
  error: string | null;
  proofTarget: "localhost" | "device" | "vm" | "web" | "other";
  deviceOs: string | null;
}

export interface RunbookCell {
  id: string;
  type: "markdown" | "code" | "raw";
  source: string;
  language: string | null;
  executionCount: number | null;
  outputs: RunbookOutput[];
  latestRun: RunbookExecutionSummary | null;
}

export interface RunbookDocument {
  runbookId: string;
  nbformat: 4;
  nbformatMinor: number;
  language: string | null;
  revision: number | null;
  latestRun: RunbookExecutionSummary | null;
  cells: RunbookCell[];
}

export interface ReportDocument {
  reportId: string;
  content: string;
}
