import { createHash } from "node:crypto";
import { access, readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { compatibleExistingPath, PRE_BEALE_DATA_DIRECTORY_NAME, preBealeHashDomain } from "./legacy-compatibility.js";

export const RESEARCH_PROFILE_SCHEMA_VERSION = 1 as const;
export const DEFAULT_RESEARCH_PROFILE_RELATIVE_PATH = ".beale/profile.json";

export type ResearchProfileAuthorizationMode =
  | "required_for_live_network"
  | "optional";

export type ResearchProfileAttributeType =
  | "string"
  | "number"
  | "boolean";

export type ResearchProfileSessionHeat = "none" | "low" | "medium" | "high" | "critical";

export interface ResearchProfileSessionHeatPalette {
  low: string;
  medium: string;
  high: string;
  critical: string;
}

export interface ResearchProfileAttributeDefinition {
  type: ResearchProfileAttributeType;
  description: string;
  pattern?: string;
  enum?: readonly (string | number | boolean)[];
}

export interface ResearchProfileMemoryRequirement {
  statuses?: readonly string[];
  requiredAttributes?: readonly string[];
  requireEvidence?: boolean;
  requireAssetLinks?: boolean;
  requiredNeighborTypes?: readonly string[];
}

export interface ResearchProfileMemoryType {
  id: string;
  name: string;
  pluralName: string;
  description: string;
  lifecycle: "active" | "retired";
  creatable: boolean;
  replacedBy?: string;
  requiresExplicitStatus?: boolean;
  aliases?: readonly string[];
  group?: string;
  icon?: string;
  color?: string;
  order: number;
  defaultStatus: string;
  allowedStatuses: readonly string[];
  sessionHeat?: Readonly<Partial<Record<string, ResearchProfileSessionHeat>>>;
  contextWeight?: number;
  attributes?: Readonly<Record<string, ResearchProfileAttributeDefinition>>;
  requirements?: readonly ResearchProfileMemoryRequirement[];
}

export interface ResearchProfileMemoryStatus {
  id: string;
  name: string;
  description: string;
  order: number;
  terminal?: boolean;
  polarity?: "positive" | "neutral" | "negative";
}

export interface ResearchProfileEvidenceKind {
  id: string;
  name: string;
  description: string;
  allowsPath?: boolean;
}

export interface ResearchProfileEvidencePathBase {
  id: string;
  name: string;
  description: string;
  pathFormat?: "relative" | "url" | "either";
}

export interface ResearchProfileMemoryRelation {
  id: string;
  name: string;
  description: string;
}

export interface ResearchProfileMemory {
  types: readonly ResearchProfileMemoryType[];
  statuses: readonly ResearchProfileMemoryStatus[];
  evidenceKinds: readonly ResearchProfileEvidenceKind[];
  evidencePathBases: readonly ResearchProfileEvidencePathBase[];
  relations?: readonly ResearchProfileMemoryRelation[];
  defaultNodeLimit?: number;
  defaultCharacterBudget?: number;
}

export interface ResearchProfileClaimClassification {
  id: string;
  name: string;
  pluralName: string;
  description: string;
  defaultProjection: "lead" | "finding";
  composite?: boolean;
  order: number;
  icon?: string;
}

export interface ResearchProfileClaims {
  classifications: readonly ResearchProfileClaimClassification[];
}

export interface ResearchProfileAgentPrompt {
  role: string;
  posture: readonly string[];
  style: readonly string[];
  memoryInstructions: readonly string[];
  runbookInstructions: readonly string[];
  reportInstructions?: readonly string[];
}

export interface ResearchProfileCollaborationRole {
  id: string;
  name: string;
  description: string;
}

export interface ResearchProfileCollaborationRecipe {
  id: string;
  name: string;
  workflowIds: readonly string[];
  roomKind: "exploration" | "validation" | "proving" | "synthesis" | "general";
  roles: readonly ResearchProfileCollaborationRole[];
  synthesisInstructions: readonly string[];
}

export interface ResearchProfileCollaboration {
  protocolInstructions: readonly string[];
  recipes: readonly ResearchProfileCollaborationRecipe[];
}

export interface ResearchProfileWorkflow {
  /** Compatibility ID for a suggestion-generation lane; never a live-agent workflow. */
  id: string;
  name: string;
  description: string;
  goalSuggestionCount: number;
  goalSuggestionInstructions: readonly string[];
  /** @deprecated Retained for profile compatibility and ignored by live sessions and prompt generation. */
  promptInstructions: readonly string[];
  /** @deprecated Retained for profile compatibility and ignored by live sessions and prompt generation. */
  outputRequirements: readonly string[];
  default?: boolean;
}

export interface ResearchProfileCapabilities {
  defaultToolFamilies: readonly string[];
  disabledToolFamilies: readonly string[];
  allowedSideEffects: readonly ("none" | "read" | "write" | "network" | "process")[];
  selectedSkillIds: readonly string[];
  disabledSkillIds: readonly string[];
  allowedMcpServerIds: readonly string[];
  memoryEnabled: boolean;
  runbooksEnabled: boolean;
  reportsEnabled?: boolean;
  collaborationEnabled: boolean;
}

export interface ResearchProfileWorkspace {
  workspaceNoun: string;
  subjectNoun: string;
  boundaryNoun: string;
  authorizationMode: ResearchProfileAuthorizationMode;
  boundaryInstructions: readonly string[];
  materialKinds: readonly string[];
}

export interface ResearchProfileModelJob {
  provider?: string;
  model?: string;
  effort?: string;
}

export interface ResearchProfileModelJobs {
  sessionTitle?: ResearchProfileModelJob;
  promptGeneration?: ResearchProfileModelJob;
  goalSuggestions?: ResearchProfileModelJob;
  memoryCuration?: ResearchProfileModelJob;
  shellReview?: ResearchProfileModelJob;
}

export interface ResearchProfilePresentation {
  newResearchLabel: string;
  memoryLabel: string;
  runbookLabel: string;
  sessionLabel: string;
  sessionHeatPalette?: ResearchProfileSessionHeatPalette;
}

export interface ResearchProfile {
  schemaVersion: typeof RESEARCH_PROFILE_SCHEMA_VERSION;
  id: string;
  version: string;
  name: string;
  description: string;
  agent: ResearchProfileAgentPrompt;
  memory: ResearchProfileMemory;
  claims: ResearchProfileClaims;
  /** Suggestion lanes stored under the historical workflows key for profile compatibility. */
  workflows: readonly ResearchProfileWorkflow[];
  collaboration: ResearchProfileCollaboration;
  capabilities: ResearchProfileCapabilities;
  workspace: ResearchProfileWorkspace;
  modelJobs: ResearchProfileModelJobs;
  presentation: ResearchProfilePresentation;
}

export interface ResolvedResearchProfile {
  profile: ResearchProfile;
  hash: string;
  source: "bundled-default" | "workspace-default" | "explicit";
  path?: string;
}

export type ResearchProfileMemoryTypeResolution =
  | { state: "active" | "retired"; canonicalId: string; type: ResearchProfileMemoryType }
  | { state: "unknown"; canonicalId: string };

const SECURITY_MEMORY_TYPE_DEFINITIONS: readonly Omit<ResearchProfileMemoryType, "lifecycle" | "creatable">[] = [
  {
    id: "asset",
    name: "Asset",
    pluralName: "Assets",
    description: "A security-relevant component, service, data object, credential, interface, or execution boundary whose compromise or protection matters. Use it to anchor affected ownership and impact; do not use it for arbitrary files with no security role.",
    group: "Assets and boundaries",
    icon: "box",
    color: "slate",
    order: 10,
    defaultStatus: "draft",
    allowedStatuses: ["draft", "suspected", "confirmed", "rejected", "stale"],
  },
  {
    id: "bug",
    name: "Historical Bug",
    pluralName: "Historical Bugs",
    description: "A confirmed historical flaw precedent that predates the current research, backed by a fixed advisory, patch, prior incident, or equivalent evidence. It must identify affected assets and set attributes.historicalPrecedent=true; a flaw established during the current research is a primitive, not a bug.",
    group: "Historical precedent",
    icon: "bug",
    color: "orange",
    order: 20,
    defaultStatus: "confirmed",
    requiresExplicitStatus: true,
    allowedStatuses: ["confirmed"],
    attributes: {
      historicalPrecedent: {
        type: "boolean",
        description: "True only for a flaw established before the current research.",
        enum: [true],
      },
    },
    requirements: [{
      requiredAttributes: ["historicalPrecedent"],
      requireEvidence: true,
      requireAssetLinks: true,
    }],
  },
  {
    id: "invariant",
    name: "Invariant",
    pluralName: "Invariants",
    description: "A security property that must remain true across relevant states or transitions. State it as a falsifiable rule whose violation would create security impact, not as a one-off observation.",
    group: "Properties and controls",
    icon: "shield-check",
    color: "blue",
    order: 30,
    defaultStatus: "draft",
    allowedStatuses: ["draft", "suspected", "confirmed", "rejected", "stale"],
  },
  {
    id: "mitigation",
    name: "Mitigation",
    pluralName: "Mitigations",
    description: "A concrete product, platform, hardware, policy, or deployment control that prevents or materially constrains exploitation. Record what it blocks and its assumptions; an ordinary validation step is not automatically a mitigation.",
    group: "Properties and controls",
    icon: "shield",
    color: "cyan",
    order: 40,
    defaultStatus: "draft",
    allowedStatuses: ["draft", "suspected", "confirmed", "rejected", "stale"],
  },
  {
    id: "source",
    name: "Source",
    pluralName: "Sources",
    description: "An attacker-controlled or lower-trust ingress from which data, control, identity, or state enters the investigated system. Name the trust boundary and reachable input, not merely a function that reads bytes.",
    group: "Assets and boundaries",
    icon: "log-in",
    color: "green",
    order: 50,
    defaultStatus: "draft",
    allowedStatuses: ["draft", "suspected", "confirmed", "rejected", "stale"],
  },
  {
    id: "sink",
    name: "Sink",
    pluralName: "Sinks",
    description: "A security-sensitive operation or state transition whose unsafe reachability can produce impact, such as memory access, code execution, authorization, disclosure, or persistence. Name the dangerous effect and required conditions.",
    group: "Assets and boundaries",
    icon: "log-out",
    color: "red",
    order: 60,
    defaultStatus: "draft",
    allowedStatuses: ["draft", "suspected", "confirmed", "rejected", "stale"],
  },
  {
    id: "flow-endpoint",
    name: "Flow Endpoint",
    pluralName: "Flow Endpoints",
    description: "A security-relevant point in a data, control, identity, or trust flow. Classify it as a source, sink, or both: sources introduce attacker-controlled or lower-trust influence, while sinks perform a security-sensitive operation or state transition.",
    group: "Assets and boundaries",
    icon: "waypoints",
    color: "green",
    order: 65,
    defaultStatus: "draft",
    allowedStatuses: ["draft", "suspected", "confirmed", "rejected", "stale"],
    attributes: {
      role: {
        type: "string",
        description: "The endpoint's direction in the relevant security flow.",
        enum: ["source", "sink", "both"],
      },
    },
    requirements: [{ requiredAttributes: ["role"] }],
  },
  {
    id: "hypothesis",
    name: "Hypothesis",
    pluralName: "Hypotheses",
    description: "A specific, testable, currently unproven security proposition. Keep it draft or suspected while active, reject it when disproven, and reclassify it as a primitive or chain when evidence proves that role; never confirm a hypothesis in place. For a flaw hypothesis, record the suspected mechanism in attributes.rootCause and a stable lowercase-hyphenated attributes.rootCauseKey.",
    group: "Investigation state",
    icon: "flask-conical",
    color: "purple",
    order: 70,
    defaultStatus: "draft",
    allowedStatuses: ["draft", "suspected", "rejected", "stale"],
    contextWeight: 8,
    attributes: {
      rootCause: { type: "string", description: "Concise suspected underlying mechanism." },
      rootCauseKey: {
        type: "string",
        description: "Stable lowercase-hyphenated identity for the suspected root cause.",
        pattern: "^[a-z0-9]+(?:-[a-z0-9]+)*$",
      },
    },
  },
  {
    id: "primitive",
    name: "Primitive",
    pluralName: "Primitives",
    description: "One independently proven security flaw or exploitation capability established during the current research, with direct code, artifact, command, or verifier evidence. Store the underlying root-cause mechanism, not each symptom, experiment, call site, or copy path, as the unit of identity; record attributes.rootCause and a stable lowercase-hyphenated attributes.rootCauseKey.",
    group: "Established results",
    icon: "git-branch",
    color: "amber",
    order: 80,
    defaultStatus: "draft",
    allowedStatuses: ["draft", "suspected", "confirmed", "rejected", "stale"],
    attributes: {
      rootCause: { type: "string", description: "Concise underlying security mechanism." },
      rootCauseKey: {
        type: "string",
        description: "Stable lowercase-hyphenated identity for the underlying root cause.",
        pattern: "^[a-z0-9]+(?:-[a-z0-9]+)*$",
      },
    },
    requirements: [{ statuses: ["confirmed"], requireEvidence: true }],
  },
  {
    id: "chain",
    name: "Chain",
    pluralName: "Chains",
    description: "An end-to-end attacker path linking one or more primitives to demonstrated security impact. Record reachability and affected context; flow-endpoint and asset relationships are ideal when supported but are not required. A confirmed chain requires proof-of-vulnerability evidence and independent review approval; do not use chain for an isolated flaw or an unlinked list of observations. Record its mechanism in attributes.rootCause and a stable lowercase-hyphenated attributes.rootCauseKey.",
    group: "Established results",
    icon: "link",
    color: "rose",
    order: 90,
    defaultStatus: "draft",
    allowedStatuses: ["draft", "suspected", "confirmed", "rejected", "stale"],
    attributes: {
      rootCause: { type: "string", description: "Concise underlying security mechanism." },
      rootCauseKey: {
        type: "string",
        description: "Stable lowercase-hyphenated identity for the underlying root cause.",
        pattern: "^[a-z0-9]+(?:-[a-z0-9]+)*$",
      },
      impact: { type: "string", description: "Security consequence if the chain succeeds." },
      reachability: { type: "string", description: "Conditions and path by which the chain can be reached." },
    },
    requirements: [
      { requiredAttributes: ["impact", "reachability"] },
      { statuses: ["confirmed"], requireEvidence: true },
    ],
  },
  {
    id: "procedure",
    name: "Procedure",
    pluralName: "Procedures",
    description: "A concise, reusable operational method for performing a bounded research task or verification. Store essential prerequisites and decision points; use a runbook for an executable multi-step command sequence or environment setup.",
    group: "Reusable methods",
    icon: "list-checks",
    color: "teal",
    order: 100,
    defaultStatus: "draft",
    allowedStatuses: ["draft", "suspected", "confirmed", "rejected", "stale"],
  },
  {
    id: "trajectory",
    name: "Trajectory",
    pluralName: "Trajectories",
    description: "A reusable sequence of significant research choices and results that explains how an investigation advanced or why a path failed. Omit routine narration and transcripts; preserve the discriminating steps and outcome.",
    group: "Reusable methods",
    icon: "route",
    color: "indigo",
    order: 110,
    defaultStatus: "draft",
    allowedStatuses: ["draft", "suspected", "confirmed", "rejected", "stale"],
  },
];

const SECURITY_MEMORY_TYPES: readonly ResearchProfileMemoryType[] = SECURITY_MEMORY_TYPE_DEFINITIONS.map((type) => ({
  ...type,
  ...(type.id === "bug" || type.id === "hypothesis" || type.id === "primitive" || type.id === "chain"
    ? { lifecycle: "retired" as const, creatable: false }
    : type.id === "source" || type.id === "sink"
      ? { lifecycle: "retired" as const, creatable: false, replacedBy: "flow-endpoint" }
      : type.id === "procedure"
        ? { lifecycle: "retired" as const, creatable: false }
        : { lifecycle: "active" as const, creatable: true }),
}));

export const DEFAULT_SECURITY_RESEARCH_PROFILE: ResearchProfile = {
  schemaVersion: RESEARCH_PROFILE_SCHEMA_VERSION,
  id: "security-research",
  version: "1.16.1",
  name: "Security",
  description: "Authorized open-ended vulnerability discovery, high-upside longshot hunting, chaining, verification, and reporting.",
  agent: {
    role: "You are a world-class security researcher with exceptional judgment, creativity, and persistence in finding novel, high-impact vulnerabilities in complex systems, operating inside the Pi agent harness.",
    posture: [
      "Assume you can perform deep source analysis, build positive proofs, design discriminating experiments, use the available tools effectively, and pursue non-obvious attack paths; do not prematurely narrow broad research to confirming or rejecting the first plausible hypothesis.",
      "For every serious candidate, pursue the positive evidence path that could establish attacker influence, a reachable dangerous sink or violated invariant, observed behavior, reproducibility, composition, and concrete impact as applicable. Pair it with evidence that would genuinely contradict or narrow a necessary link; an incomplete proof is an open obligation, not a refutation.",
    ],
    style: [
      "Write as a sharp, curious research collaborator using concise, technically precise, cohesive prose.",
      "Do not narrate routine memory updates unless they materially affect the conclusion.",
    ],
    memoryInstructions: [
      "Search claims, knowledge memories, and runbooks together with history.search early and as research crosses system boundaries. Favor security-sensitive code near sink-role flow endpoints, established findings, cited historical precedent, and relevant successful trajectories.",
      "On the first Auto-Reviewed touch of each tracked resource or canonical repository revision, complete the emitted baseline: capture exact provenance and build identity; inspect advisories, vendor bulletins, release notes, security-content pages, fixed-version records, upstream and vendor source history or drops, and referenced fixes before treating a candidate as novel. For vendor-maintained components include official source releases and upstream project history. Record dated no-match queries and deferrals as explicitly as matches.",
      "Before saving, use history.search to find an existing memory with the same underlying fact or root cause and refine it instead of creating a differently worded duplicate.",
      "When multiple leads or findings represent the same underlying claim, keep the strongest evidence-bearing record canonical and mark the weaker records as its duplicates. Preserve related primitives, variants, conditions, and chain components as distinct claims.",
      "Evidence is attached to knowledge or claim records, not stored as its own memory type. Record suspected results with lead.create; promote that same stable claim to a finding through finding.transition when direct evidence is obtained, and append same-maturity evidence by transitioning it to its current status. Classify isolated security results as security.primitive and composite paths as security.chain.",
    ],
    runbookInstructions: [
      "Search and list existing workspace runbooks before recording a repeatable proof. Reuse or create the matching workflow runbook for reproduction-grade evidence, keep setup, runtime, and cleanup in it behind feature tags, and retain it as the human-visible execution path for the proof.",
      "Direct shell execution may support bounded exploratory tests and vulnerability research. Use runbook.run for repeatable proof procedures and reproduction-grade evidence; a direct shell result alone does not supply a qualifying runbook runId.",
      "Before implementing a candidate proof, call runbook.prepare with the workflow title, purpose, and language. It finds or creates the workflow and returns a stable investigations/ candidate path and entry cell ID; supply a custom path and entry command only when needed. Keep iterative implementation in that candidate artifact. Use runbook.edit with the current revision when the entry command itself changes; rerun the same cell when only the candidate artifact changes. Set executor.timeoutSeconds on a host or Tart cell expected to exceed the 300-second default; runbook.run delegates runtime to that cell executor. For Tart VM proofing, build on the host and select a tart-vm cell executor that references the workspace-relative executable or durable artifact; select runAs root when the workspace defines root execution through passwordless sudo. runbook.run handles guest staging, invocation, evidence, and cleanup without a guest-side rewrite. Do not confuse the Guest Agent service UID with the effective UID of a root-mode proof command. Label every cell with setup, runtime, or cleanup plus any narrower feature tags, and select active tags with runbook.configure. Failed run output is the attempt history, so do not create lifecycle wrappers, duplicate runbooks, or one cell per tweak.",
      "Use successful runIds emitted by runbook.run as evidence for the phases they prove, and select the runbook that performs the canonical reproduction when setting reproductionRunbookId. Store concise facts in memory and claims; use workspace history search to recover prior work across sessions.",
    ],
    reportInstructions: [
      "List existing workspace reports before creating one.",
      "Do not create a report from leads or unverified findings. First verify the component findings, then create a security.chain composite with demonstrated reachability and impact.",
      "Create a report only for a verified security.chain finding that meets proof-of-vulnerability and independent-review requirements. Pass that claim as sourceFindingId to report.create; the tool rejects premature reports.",
      "Write for a triager who has never worked on the affected subsystem. Use normal language first, define product-specific terms before relying on them, and avoid semantic cramming, unnecessary jargon, and overusing security vocabulary.",
      "Order security reports as: Impact summary; How the affected system works; Vulnerability chain; Impact and affected systems; Reproduction using the attached submission packet; Technical root cause; Remediation and regression coverage.",
      "The impact summary must state who can trigger the issue, the triggering action, the affected deployment or components, the demonstrated security outcome, and the strongest verified limitation before implementation details.",
      "The system explanation must describe the relevant components, objects, trust or ownership boundary, normal data flow, and violated invariant as if the triager is unfamiliar with the subsystem.",
      "Present the chain as a readable narrative followed by numbered state transitions. Separate demonstrated consequences from plausible downstream consequences and preserve material limitations.",
      "Put complete scripts and bulky evidence in submission.zip. In the report, name the packet, state its hash and prerequisites, give one exact entry command, enumerate the proof actions and decisive expected output, include independent rerun and cleanup results, and point to packet files for deeper inspection.",
      "Create submission.zip inside the active workspace and pass submissionPacketPath to report.create. app-server imports the candidate packet into durable report storage; a security report is not complete without it.",
      "Reports are Markdown artifacts, not memories. Keep each one coherent and standalone, and mark it stale when superseded or no longer accurate.",
    ],
  },
  memory: {
    types: SECURITY_MEMORY_TYPES,
    statuses: [
      { id: "draft", name: "Draft", description: "Recorded but not yet assessed.", order: 10, polarity: "neutral" },
      { id: "suspected", name: "Suspected", description: "Plausible and under active investigation.", order: 20, polarity: "neutral" },
      { id: "confirmed", name: "Confirmed", description: "Supported by the evidence required for its type.", order: 30, polarity: "positive" },
      { id: "rejected", name: "Rejected", description: "Disproved or invalidated by evidence.", order: 40, terminal: true, polarity: "negative" },
      { id: "stale", name: "Stale", description: "Superseded or no longer current.", order: 50, terminal: true, polarity: "negative" },
    ],
    evidenceKinds: [
      { id: "code", name: "Code", description: "A source or binary code location.", allowsPath: true },
      { id: "artifact", name: "Artifact", description: "A durable generated or captured artifact.", allowsPath: true },
      { id: "command", name: "Command", description: "A bounded command or experiment result.", allowsPath: true },
      { id: "url", name: "URL", description: "An external web reference.", allowsPath: true },
      { id: "human_note", name: "Human Note", description: "An explicit operator-provided note.", allowsPath: false },
    ],
    evidencePathBases: [
      { id: "workspace", name: "Workspace", description: "Relative to the active workspace.", pathFormat: "relative" },
      { id: "repository", name: "Repository", description: "Relative to a recorded repository root.", pathFormat: "relative" },
      { id: "asset_root", name: "Asset Root", description: "Relative to a recorded asset root.", pathFormat: "relative" },
      { id: "external", name: "External", description: "Outside local workspace storage.", pathFormat: "either" },
    ],
    relations: [
      { id: "affects", name: "Affects", description: "The source node materially affects the target." },
      { id: "supports", name: "Supports", description: "The source node supports the target conclusion." },
      { id: "weakens", name: "Weakens", description: "The source node weakens the target conclusion." },
      { id: "reaches", name: "Reaches", description: "The source can reach the target operation or state." },
      { id: "mitigates", name: "Mitigates", description: "The source constrains or prevents the target." },
    ],
    defaultNodeLimit: 8,
    defaultCharacterBudget: 6_000,
  },
  claims: {
    classifications: [
      { id: "security.vulnerability", name: "Vulnerability Lead", pluralName: "Vulnerability Leads", description: "A testable proposition about a security weakness before direct observation.", defaultProjection: "lead", order: 10, icon: "flask-conical" },
      { id: "security.primitive", name: "Security Finding", pluralName: "Security Findings", description: "One independently identifiable security flaw or exploitation capability. Symptoms and call sites remain evidence of the same root-cause claim.", defaultProjection: "finding", order: 20, icon: "git-branch" },
      { id: "security.chain", name: "Composite Finding", pluralName: "Composite Findings", description: "An end-to-end attacker path that composes one or more component findings into demonstrated security impact.", defaultProjection: "finding", composite: true, order: 30, icon: "link" },
    ],
  },
  workflows: [
    {
      id: "discovery",
      name: "Discovery",
      description: "Find new security primitives without presuming reachability or impact.",
      goalSuggestionCount: 4,
      default: true,
      goalSuggestionInstructions: [
        "Pair a bounded subsystem, component, or attack surface with a plausible bug class or vulnerability family without assuming a flaw exists.",
        "Keep goals broad and do not make a named hypothesis, verifier, reproduction, impact determination, or source-to-sink path the goal.",
      ],
      promptInstructions: [
        "Center the prompt on open-ended vulnerability research in a bounded subsystem or attack surface.",
        "Do not frame the work as proof or disproof of a predetermined claim.",
      ],
      outputRequirements: ["Evidence-backed observations and the most useful next discriminating actions."],
    },
    {
      id: "chaining",
      name: "Chaining",
      description: "Upgrade recorded primitives toward a reportable exploit chain.",
      goalSuggestionCount: 4,
      goalSuggestionInstructions: [
        "Upgrade existing recorded primitives toward strong reportable exploit chains with triage-ready proofs of concept.",
        "Permit bounded discovery needed to fill reachability, exploitability, or impact gaps.",
      ],
      promptInstructions: ["Center confirmed existing primitives and investigate missing reachability, exploitability, or impact links."],
      outputRequirements: ["A strong evidence-supported chain with a triage-ready proof of concept."],
    },
    {
      id: "reporting",
      name: "Reporting",
      description: "Package an evidence-supported exploit chain for responsible reporting.",
      goalSuggestionCount: 4,
      goalSuggestionInstructions: ["Document evidence-supported exploit chains, their constituent bugs, and their security impact without overstating the evidence."],
      promptInstructions: ["Preserve material limitations and do not invent reachability, impact, or unsupported conclusions."],
      outputRequirements: ["A triage-ready report in the required reader-first section order, with submission.zip attached through report.create and containing the proof and necessary evidence."],
    },
    {
      id: "longshot",
      name: "Longshot",
      description: "Hunt for ambitious, reportable high- or critical-severity vulnerabilities.",
      goalSuggestionCount: 4,
      goalSuggestionInstructions: [
        "Propose open-ended research territories with a credible path to reportable high or critical impact, grounded in broad architecture, trust boundaries, prior evidence, or historical vulnerability patterns.",
        "Each goal must name a broad attack surface and an explicit systemic impact ceiling, such as platform-wide cross-tenant compromise, remote or system code execution, sandbox escape, privilege escalation, or supply-chain compromise.",
        "Favor deep trust-boundary failures, cross-component composition, and powerful attacker-controlled pivots over incremental variants. Exclude local-only or single-object outcomes unless the goal explains a credible path to systemic impact.",
        "Leave the vulnerable mechanism open. Do not turn a remembered lead into a binary verification task, center one function or route, or begin with verify, confirm, or determine whether.",
      ],
      promptInstructions: [
        "Pursue a high-upside vulnerability direction where a confirmed flaw could plausibly support reportable high or critical impact.",
        "Keep existence, reachability, exploitability, severity, and reportability evidence-gated throughout the research.",
      ],
      outputRequirements: ["A rigorously supported high-impact candidate or decisive reusable negative knowledge, with severity and reportability stated only when established."],
    },
  ],
  collaboration: {
    protocolInstructions: [
      "Keep exploit claims tied to inspected code, bounded experiments, artifacts, or verifier results; a peer assertion is not target evidence.",
      "Preserve competing root-cause and reachability explanations until a discriminating check resolves them.",
    ],
    recipes: [],
  },
  capabilities: {
    defaultToolFamilies: ["shell", "repository-search", "file-read"],
    disabledToolFamilies: [],
    allowedSideEffects: ["none", "read", "write", "process"],
    selectedSkillIds: [],
    disabledSkillIds: [],
    allowedMcpServerIds: [],
    memoryEnabled: true,
    runbooksEnabled: true,
    reportsEnabled: true,
    collaborationEnabled: true,
  },
  workspace: {
    workspaceNoun: "Research workspace",
    subjectNoun: "Scope owner or subject",
    boundaryNoun: "Authorized scope",
    authorizationMode: "required_for_live_network",
    boundaryInstructions: [
      "Only explicitly in-scope assets are authorized; exclusions and constraints override research objectives.",
      "Ambient resources are non-authoring inventory. Require Auto-Reviewed relevance before their first substantive research touch; approval permits tracking and history work, not live-target authorization.",
    ],
    materialKinds: ["repo", "path", "binary", "documentation", "service", "domain", "host", "ip_range"],
  },
  modelJobs: {},
  presentation: {
    newResearchLabel: "New Research",
    memoryLabel: "Memory",
    runbookLabel: "Runbooks",
    sessionLabel: "Research Session",
  },
};

export const BUNDLED_RESEARCH_PROFILE_IDS = ["security-research"] as const;
export type BundledResearchProfileId = typeof BUNDLED_RESEARCH_PROFILE_IDS[number];

export function bundledResearchProfile(_profileId: BundledResearchProfileId): ResearchProfile {
  return DEFAULT_SECURITY_RESEARCH_PROFILE;
}

export function getDefaultResearchProfilePath(workspaceRoot: string = process.cwd()): string {
  return compatibleExistingPath(
    resolve(workspaceRoot, DEFAULT_RESEARCH_PROFILE_RELATIVE_PATH),
    resolve(workspaceRoot, PRE_BEALE_DATA_DIRECTORY_NAME, "profile.json"),
  );
}

export async function loadResearchProfile(path: string): Promise<ResearchProfile> {
  const absolutePath = resolve(path);
  const source = await readFile(absolutePath, "utf8");
  if (Buffer.byteLength(source, "utf8") > 1_000_000) {
    throw new Error("Research profile exceeds the 1000000-byte limit.");
  }
  const parsed = JSON.parse(source) as unknown;
  return normalizeResearchProfile(parsed);
}

export async function resolveResearchProfile(options: {
  workspaceRoot?: string;
  profilePath?: string;
  profile?: unknown;
  bundledProfileId?: BundledResearchProfileId;
} = {}): Promise<ResolvedResearchProfile> {
  if (options.profile !== undefined) {
    const profile = normalizeResearchProfile(options.profile);
    return { profile, hash: researchProfileHash(profile), source: "explicit" };
  }
  if (options.profilePath) {
    const path = resolve(options.profilePath);
    const profile = await loadResearchProfile(path);
    return { profile, hash: researchProfileHash(profile), source: "explicit", path };
  }
  if (options.bundledProfileId) {
    const profile = normalizeResearchProfile(bundledResearchProfile(options.bundledProfileId));
    return { profile, hash: researchProfileHash(profile), source: "bundled-default" };
  }
  const path = getDefaultResearchProfilePath(options.workspaceRoot);
  if (await pathExists(path)) {
    const profile = await loadResearchProfile(path);
    return { profile, hash: researchProfileHash(profile), source: "workspace-default", path };
  }
  const profile = normalizeResearchProfile(DEFAULT_SECURITY_RESEARCH_PROFILE);
  return { profile, hash: researchProfileHash(profile), source: "bundled-default" };
}

export function normalizeResearchProfile(value: unknown): ResearchProfile {
  const input = record(value, "Research profile");
  assertKnownKeys(input, [
    "schemaVersion", "id", "version", "name", "description", "agent", "memory", "workflows",
    "claims", "collaboration", "capabilities", "workspace", "modelJobs", "presentation",
  ], "Research profile");
  if (input.schemaVersion !== RESEARCH_PROFILE_SCHEMA_VERSION) {
    throw new Error(`Unsupported research profile schemaVersion: ${String(input.schemaVersion)}`);
  }
  const profile: ResearchProfile = {
    schemaVersion: RESEARCH_PROFILE_SCHEMA_VERSION,
    id: identifier(input.id, "profile id"),
    version: nonEmptyString(input.version, "profile version"),
    name: nonEmptyString(input.name, "profile name"),
    description: nonEmptyString(input.description, "profile description"),
    agent: normalizeAgentPrompt(input.agent),
    memory: normalizeMemory(input.memory),
    claims: normalizeClaims(input.claims),
    workflows: normalizeWorkflows(input.workflows),
    collaboration: normalizeCollaboration(input.collaboration),
    capabilities: normalizeCapabilities(input.capabilities),
    workspace: normalizeWorkspace(input.workspace),
    modelJobs: normalizeModelJobs(input.modelJobs),
    presentation: normalizePresentation(input.presentation),
  };
  if (
    profile.capabilities.memoryEnabled
    && !profile.memory.types.some((type) => type.lifecycle === "active" && type.creatable)
  ) {
    throw new Error("A memory-enabled research profile requires at least one active, creatable memory type.");
  }
  const workflowIds = new Set(profile.workflows.map((workflow) => workflow.id));
  const recipeByWorkflow = new Map<string, string>();
  for (const recipe of profile.collaboration.recipes) {
    if (recipe.workflowIds.length === 0) throw new Error(`Collaboration recipe ${recipe.id} requires at least one workflow id.`);
    for (const workflowId of recipe.workflowIds) {
      if (!workflowIds.has(workflowId)) throw new Error(`Collaboration recipe ${recipe.id} references unknown workflow ${workflowId}.`);
      const existing = recipeByWorkflow.get(workflowId);
      if (existing) throw new Error(`Workflow ${workflowId} is assigned to multiple collaboration recipes: ${existing}, ${recipe.id}.`);
      recipeByWorkflow.set(workflowId, recipe.id);
    }
  }
  return deepFreeze(profile);
}

function normalizeClaims(value: unknown): ResearchProfileClaims {
  if (value === undefined) {
    return {
      classifications: [{
        id: "general.result",
        name: "Research Result",
        pluralName: "Research Results",
        description: "A domain-neutral research proposition or evidence-backed result.",
        defaultProjection: "lead",
        order: 10,
      }],
    };
  }
  const input = record(value, "Research profile claims");
  assertKnownKeys(input, ["classifications"], "Research profile claims");
  const classifications = array(input.classifications, "claim classifications").map((value, index) => {
    const classification = record(value, `claim classification ${index}`);
    assertKnownKeys(classification, ["id", "name", "pluralName", "description", "defaultProjection", "composite", "order", "icon"], `claim classification ${index}`);
    const defaultProjection = classification.defaultProjection;
    if (defaultProjection !== "lead" && defaultProjection !== "finding") {
      throw new Error(`Claim classification ${index} has invalid defaultProjection.`);
    }
    return {
      id: identifier(classification.id, `claim classification ${index} id`),
      name: nonEmptyString(classification.name, `claim classification ${index} name`),
      pluralName: nonEmptyString(classification.pluralName, `claim classification ${index} pluralName`),
      description: nonEmptyString(classification.description, `claim classification ${index} description`),
      defaultProjection,
      ...(classification.composite === true ? { composite: true } : {}),
      order: finiteNumber(classification.order, `claim classification ${index} order`),
      ...(optionalString(classification.icon) ? { icon: optionalString(classification.icon)! } : {}),
    } satisfies ResearchProfileClaimClassification;
  });
  if (classifications.length === 0) throw new Error("A research profile requires at least one claim classification.");
  uniqueIds(classifications, "claim classification");
  return { classifications };
}

export function researchProfileHash(profile: ResearchProfile): string {
  return createHash("sha256")
    .update(preBealeHashDomain("research-profile:v1\0"))
    .update(stableJson(profile))
    .digest("hex");
}

/** Hash used by schema-v1 snapshots written before claim classifications were added. */
export function legacyResearchProfileHash(profile: ResearchProfile): string {
  const { claims: _claims, ...legacyProfile } = profile;
  return createHash("sha256")
    .update(preBealeHashDomain("research-profile:v1\0"))
    .update(stableJson(legacyProfile))
    .digest("hex");
}

export function researchProfileMemoryType(
  profile: Pick<ResearchProfile, "memory">,
  typeOrAlias: string,
): ResearchProfileMemoryType | undefined {
  return profile.memory.types.find((type) =>
    type.id === typeOrAlias || type.aliases?.includes(typeOrAlias));
}

export function resolveResearchProfileMemoryType(
  profile: Pick<ResearchProfile, "memory">,
  typeOrAlias: string,
): ResearchProfileMemoryTypeResolution {
  const type = researchProfileMemoryType(profile, typeOrAlias);
  if (!type) return { state: "unknown", canonicalId: typeOrAlias };
  return {
    state: type.lifecycle,
    canonicalId: type.id,
    type,
  };
}

export function researchProfileWorkflow(
  profile: Pick<ResearchProfile, "workflows">,
  workflowId: string | undefined,
): ResearchProfileWorkflow {
  const selected = workflowId
    ? profile.workflows.find((workflow) => workflow.id === workflowId)
    : profile.workflows.find((workflow) => workflow.default) ?? profile.workflows[0];
  if (!selected) throw new Error(`Unknown research workflow: ${workflowId ?? "default"}`);
  return selected;
}

export function researchProfileCollaborationRecipe(
  profile: Pick<ResearchProfile, "collaboration">,
  workflowId: string,
): ResearchProfileCollaborationRecipe | undefined {
  return profile.collaboration.recipes.find((recipe) => recipe.workflowIds.includes(workflowId));
}

export function overrideResearchProfileMemoryDescriptions(
  profile: ResearchProfile,
  descriptions: Readonly<Record<string, string>>,
): ResearchProfile {
  const supported = new Set(profile.memory.types.map((type) => type.id));
  for (const [id, description] of Object.entries(descriptions)) {
    if (!supported.has(id)) throw new Error(`Unsupported memory type description: ${id}`);
    if (!description.trim()) throw new Error(`Memory type description for ${id} must be non-empty.`);
  }
  return normalizeResearchProfile({
    ...profile,
    memory: {
      ...profile.memory,
      types: profile.memory.types.map((type) => ({
        ...type,
        description: descriptions[type.id]?.trim() || type.description,
      })),
    },
  });
}

function normalizeAgentPrompt(value: unknown): ResearchProfileAgentPrompt {
  const input = record(value, "Research profile agent");
  assertKnownKeys(input, ["role", "posture", "style", "memoryInstructions", "runbookInstructions", "reportInstructions"], "Research profile agent");
  return {
    role: nonEmptyString(input.role, "agent role"),
    posture: stringArray(input.posture, "agent posture"),
    style: stringArray(input.style, "agent style"),
    memoryInstructions: stringArray(input.memoryInstructions, "memory instructions"),
    runbookInstructions: stringArray(input.runbookInstructions, "runbook instructions"),
    ...(input.reportInstructions === undefined
      ? {}
      : { reportInstructions: stringArray(input.reportInstructions, "report instructions") }),
  };
}

function normalizeMemory(value: unknown): ResearchProfileMemory {
  const input = record(value, "Research profile memory");
  assertKnownKeys(input, [
    "types", "statuses", "evidenceKinds", "evidencePathBases", "relations", "defaultNodeLimit", "defaultCharacterBudget",
  ], "Research profile memory");
  const statuses = array(input.statuses, "memory statuses").map((status, index) => {
    const item = record(status, `memory status ${index}`);
    assertKnownKeys(item, ["id", "name", "description", "order", "terminal", "polarity"], `memory status ${index}`);
    const polarity = item.polarity;
    if (polarity !== undefined && polarity !== "positive" && polarity !== "neutral" && polarity !== "negative") {
      throw new Error(`Memory status ${index} has invalid polarity.`);
    }
    return {
      id: identifier(item.id, `memory status ${index} id`),
      name: nonEmptyString(item.name, `memory status ${index} name`),
      description: nonEmptyString(item.description, `memory status ${index} description`),
      order: finiteNumber(item.order, `memory status ${index} order`),
      ...(item.terminal === true ? { terminal: true } : {}),
      ...(polarity ? { polarity } : {}),
    } satisfies ResearchProfileMemoryStatus;
  });
  uniqueIds(statuses, "memory status");
  const statusIds = new Set(statuses.map((status) => status.id));
  const types = array(input.types, "memory types").map((type, index) =>
    normalizeMemoryType(type, index, statusIds));
  uniqueIds(types, "memory type");
  const aliases = new Set<string>();
  for (const type of types) {
    for (const alias of type.aliases ?? []) {
      if (statusIds.has(alias) || types.some((candidate) => candidate.id === alias) || aliases.has(alias)) {
        throw new Error(`Duplicate or conflicting memory type alias: ${alias}`);
      }
      aliases.add(alias);
    }
  }
  const typeIds = new Set(types.map((type) => type.id));
  for (const type of types) {
    if (type.replacedBy && !typeIds.has(type.replacedBy)) {
      throw new Error(`Memory type ${type.id} is replaced by unknown memory type ${type.replacedBy}.`);
    }
    if (type.replacedBy === type.id) throw new Error(`Memory type ${type.id} cannot replace itself.`);
    for (const requirement of type.requirements ?? []) {
      for (const neighbor of requirement.requiredNeighborTypes ?? []) {
        if (!typeIds.has(neighbor)) throw new Error(`Memory type ${type.id} requires unknown neighbor type ${neighbor}.`);
      }
    }
  }
  const evidenceKinds = normalizeCatalog<ResearchProfileEvidenceKind>(input.evidenceKinds, "evidence kind", "evidence-kind");
  const evidencePathBases = normalizeCatalog<ResearchProfileEvidencePathBase>(input.evidencePathBases, "evidence path base", "path-base");
  const relations = input.relations === undefined
    ? undefined
    : normalizeCatalog<ResearchProfileMemoryRelation>(input.relations, "memory relation", "plain");
  return {
    types,
    statuses,
    evidenceKinds,
    evidencePathBases,
    ...(relations ? { relations } : {}),
    ...(input.defaultNodeLimit === undefined ? {} : { defaultNodeLimit: positiveInteger(input.defaultNodeLimit, "default memory node limit") }),
    ...(input.defaultCharacterBudget === undefined ? {} : { defaultCharacterBudget: positiveInteger(input.defaultCharacterBudget, "default memory character budget") }),
  };
}

function normalizeMemoryType(
  value: unknown,
  index: number,
  statusIds: ReadonlySet<string>,
): ResearchProfileMemoryType {
  const input = record(value, `memory type ${index}`);
  const id = identifier(input.id, `memory type ${index} id`);
  assertKnownKeys(input, [
    "id", "name", "pluralName", "description", "lifecycle", "creatable", "replacedBy", "requiresExplicitStatus", "aliases",
    "group", "icon", "color", "order", "defaultStatus", "allowedStatuses", "sessionHeat", "contextWeight", "attributes", "requirements",
  ], `memory type ${id}`);
  const allowedStatuses = stringArray(input.allowedStatuses, `memory type ${id} allowedStatuses`).map((status) => identifier(status, `memory type ${id} status`));
  if (allowedStatuses.length === 0) throw new Error(`Memory type ${id} must allow at least one status.`);
  for (const status of allowedStatuses) {
    if (!statusIds.has(status)) throw new Error(`Memory type ${id} allows unknown status ${status}.`);
  }
  const defaultStatus = identifier(input.defaultStatus, `memory type ${id} defaultStatus`);
  if (!allowedStatuses.includes(defaultStatus)) throw new Error(`Memory type ${id} defaultStatus must be allowed.`);
  const sessionHeat = input.sessionHeat === undefined
    ? undefined
    : Object.fromEntries(Object.entries(record(input.sessionHeat, `memory type ${id} sessionHeat`)).map(([status, heat]) => {
        if (!allowedStatuses.includes(status)) throw new Error(`Memory type ${id} sessionHeat uses disallowed status ${status}.`);
        return [status, sessionHeatLevel(heat, `memory type ${id} sessionHeat ${status}`)];
      }));
  const lifecycle = input.lifecycle === undefined ? "active" : input.lifecycle;
  if (lifecycle !== "active" && lifecycle !== "retired") {
    throw new Error(`Memory type ${id} has invalid lifecycle.`);
  }
  const creatable = input.creatable === undefined ? lifecycle === "active" : input.creatable;
  if (typeof creatable !== "boolean") throw new Error(`Memory type ${id} creatable must be a boolean.`);
  if (lifecycle === "retired" && creatable) throw new Error(`Retired memory type ${id} cannot be creatable.`);
  const attributes = input.attributes === undefined
    ? undefined
    : normalizeAttributes(input.attributes, id);
  const requirements = input.requirements === undefined
    ? undefined
    : array(input.requirements, `memory type ${id} requirements`).map((requirement, requirementIndex) => {
        const item = record(requirement, `memory type ${id} requirement ${requirementIndex}`);
        assertKnownKeys(item, [
          "statuses", "requiredAttributes", "requireEvidence", "requireAssetLinks", "requiredNeighborTypes",
        ], `memory type ${id} requirement ${requirementIndex}`);
        const statuses = item.statuses === undefined
          ? undefined
          : stringArray(item.statuses, `memory type ${id} requirement statuses`);
        for (const status of statuses ?? []) {
          if (!allowedStatuses.includes(status)) throw new Error(`Memory type ${id} requirement uses disallowed status ${status}.`);
        }
        const requiredAttributes = item.requiredAttributes === undefined
          ? undefined
          : stringArray(item.requiredAttributes, `memory type ${id} requiredAttributes`);
        for (const attribute of requiredAttributes ?? []) {
          if (!attributes?.[attribute]) throw new Error(`Memory type ${id} requires unknown attribute ${attribute}.`);
        }
        const requiredNeighborTypes = item.requiredNeighborTypes === undefined
          ? undefined
          : stringArray(item.requiredNeighborTypes, `memory type ${id} requiredNeighborTypes`);
        if (requiredNeighborTypes?.length && !statuses?.length) {
          throw new Error(`Memory type ${id} neighbor requirements must be limited to one or more statuses.`);
        }
        return {
          ...(statuses ? { statuses } : {}),
          ...(requiredAttributes ? { requiredAttributes } : {}),
          ...(item.requireEvidence === true ? { requireEvidence: true } : {}),
          ...(item.requireAssetLinks === true ? { requireAssetLinks: true } : {}),
          ...(requiredNeighborTypes ? { requiredNeighborTypes } : {}),
        } satisfies ResearchProfileMemoryRequirement;
      });
  return {
    id,
    name: nonEmptyString(input.name, `memory type ${id} name`),
    pluralName: nonEmptyString(input.pluralName, `memory type ${id} pluralName`),
    description: nonEmptyString(input.description, `memory type ${id} description`),
    lifecycle,
    creatable,
    ...(input.replacedBy === undefined ? {} : { replacedBy: identifier(input.replacedBy, `memory type ${id} replacedBy`) }),
    ...(input.requiresExplicitStatus === true ? { requiresExplicitStatus: true } : {}),
    ...(input.aliases === undefined ? {} : { aliases: stringArray(input.aliases, `memory type ${id} aliases`).map((alias) => identifier(alias, `memory type ${id} alias`)) }),
    ...(optionalString(input.group) ? { group: optionalString(input.group)! } : {}),
    ...(optionalString(input.icon) ? { icon: optionalString(input.icon)! } : {}),
    ...(optionalString(input.color) ? { color: optionalString(input.color)! } : {}),
    order: finiteNumber(input.order, `memory type ${id} order`),
    defaultStatus,
    allowedStatuses,
    ...(sessionHeat ? { sessionHeat } : {}),
    ...(input.contextWeight === undefined ? {} : { contextWeight: finiteNumber(input.contextWeight, `memory type ${id} contextWeight`) }),
    ...(attributes ? { attributes } : {}),
    ...(requirements ? { requirements } : {}),
  };
}

function normalizeAttributes(value: unknown, typeId: string): Record<string, ResearchProfileAttributeDefinition> {
  const input = record(value, `memory type ${typeId} attributes`);
  return Object.fromEntries(Object.entries(input).map(([name, raw]) => {
    const item = record(raw, `memory type ${typeId} attribute ${name}`);
    assertKnownKeys(item, ["type", "description", "pattern", "enum"], `memory type ${typeId} attribute ${name}`);
    if (item.type !== "string" && item.type !== "number" && item.type !== "boolean") {
      throw new Error(`Memory type ${typeId} attribute ${name} has invalid type.`);
    }
    const enumValues = item.enum === undefined ? undefined : array(item.enum, `memory type ${typeId} attribute ${name} enum`);
    if (enumValues?.some((entry) => typeof entry !== item.type || (typeof entry === "number" && !Number.isFinite(entry)))) {
      throw new Error(`Memory type ${typeId} attribute ${name} enum does not match its type.`);
    }
    if (item.pattern !== undefined) {
      if (item.type !== "string") throw new Error(`Memory type ${typeId} attribute ${name} pattern requires string type.`);
      try { new RegExp(nonEmptyString(item.pattern, `memory type ${typeId} attribute ${name} pattern`), "u"); }
      catch { throw new Error(`Memory type ${typeId} attribute ${name} has invalid pattern.`); }
    }
    return [name, {
      type: item.type,
      description: nonEmptyString(item.description, `memory type ${typeId} attribute ${name} description`),
      ...(item.pattern === undefined ? {} : { pattern: nonEmptyString(item.pattern, `memory type ${typeId} attribute ${name} pattern`) }),
      ...(enumValues ? { enum: enumValues as (string | number | boolean)[] } : {}),
    } satisfies ResearchProfileAttributeDefinition];
  }));
}

function normalizeCatalog<T extends { id: string; name: string; description: string }>(
  value: unknown,
  label: string,
  mode: "evidence-kind" | "path-base" | "plain",
): T[] {
  const catalog = array(value, `${label}s`).map((entry, index) => {
    const item = record(entry, `${label} ${index}`);
    assertKnownKeys(item, mode === "evidence-kind"
      ? ["id", "name", "description", "allowsPath"]
      : mode === "path-base"
        ? ["id", "name", "description", "pathFormat"]
        : ["id", "name", "description"], `${label} ${index}`);
    const pathFormat = item.pathFormat;
    if (mode === "path-base" && pathFormat !== undefined && pathFormat !== "relative" && pathFormat !== "url" && pathFormat !== "either") {
      throw new Error(`${label} ${index} has invalid pathFormat.`);
    }
    return {
      id: identifier(item.id, `${label} ${index} id`),
      name: nonEmptyString(item.name, `${label} ${index} name`),
      description: nonEmptyString(item.description, `${label} ${index} description`),
      ...(mode === "evidence-kind" && item.allowsPath === true ? { allowsPath: true } : {}),
      ...(mode === "path-base" && pathFormat ? { pathFormat } : {}),
    };
  });
  uniqueIds(catalog, label);
  return catalog as T[];
}

function normalizeWorkflows(value: unknown): ResearchProfileWorkflow[] {
  const workflows = array(value, "research workflows").map((workflow, index) => {
    const item = record(workflow, `research workflow ${index}`);
    const id = identifier(item.id, `research workflow ${index} id`);
    assertKnownKeys(item, [
      "id", "name", "description", "goalSuggestionCount", "goalSuggestionInstructions", "promptInstructions", "outputRequirements", "default",
    ], `research workflow ${id}`);
    return {
      id,
      name: nonEmptyString(item.name, `research workflow ${id} name`),
      description: nonEmptyString(item.description, `research workflow ${id} description`),
      goalSuggestionCount: positiveInteger(item.goalSuggestionCount, `research workflow ${id} goalSuggestionCount`),
      goalSuggestionInstructions: stringArray(item.goalSuggestionInstructions, `research workflow ${id} goalSuggestionInstructions`),
      promptInstructions: stringArray(item.promptInstructions, `research workflow ${id} promptInstructions`),
      outputRequirements: stringArray(item.outputRequirements, `research workflow ${id} outputRequirements`),
      ...(item.default === true ? { default: true } : {}),
    } satisfies ResearchProfileWorkflow;
  });
  if (workflows.length === 0) throw new Error("Research profile requires at least one workflow.");
  uniqueIds(workflows, "research workflow");
  if (workflows.filter((workflow) => workflow.default).length > 1) throw new Error("Research profile may define only one default workflow.");
  return workflows;
}

function normalizeCollaboration(value: unknown): ResearchProfileCollaboration {
  if (value === undefined) return { protocolInstructions: [], recipes: [] };
  const input = record(value, "Research profile collaboration");
  assertKnownKeys(input, ["protocolInstructions", "recipes"], "Research profile collaboration");
  const recipes = array(input.recipes, "collaboration recipes").map((rawRecipe, recipeIndex) => {
    const recipe = record(rawRecipe, `collaboration recipe ${recipeIndex}`);
    const id = identifier(recipe.id, `collaboration recipe ${recipeIndex} id`);
    assertKnownKeys(recipe, ["id", "name", "workflowIds", "roomKind", "roles", "synthesisInstructions"], `collaboration recipe ${id}`);
    if (!["exploration", "validation", "proving", "synthesis", "general"].includes(String(recipe.roomKind))) {
      throw new Error(`Collaboration recipe ${id} has invalid roomKind.`);
    }
    const roles = array(recipe.roles, `collaboration recipe ${id} roles`).map((rawRole, roleIndex) => {
      const role = record(rawRole, `collaboration recipe ${id} role ${roleIndex}`);
      assertKnownKeys(role, ["id", "name", "description"], `collaboration recipe ${id} role ${roleIndex}`);
      return {
        id: identifier(role.id, `collaboration recipe ${id} role ${roleIndex} id`),
        name: nonEmptyString(role.name, `collaboration recipe ${id} role ${roleIndex} name`),
        description: nonEmptyString(role.description, `collaboration recipe ${id} role ${roleIndex} description`),
      } satisfies ResearchProfileCollaborationRole;
    });
    if (roles.length < 2) throw new Error(`Collaboration recipe ${id} requires at least two roles.`);
    uniqueIds(roles, `collaboration recipe ${id} role`);
    return {
      id,
      name: nonEmptyString(recipe.name, `collaboration recipe ${id} name`),
      workflowIds: stringArray(recipe.workflowIds, `collaboration recipe ${id} workflowIds`).map((workflowId) => identifier(workflowId, `collaboration recipe ${id} workflow id`)),
      roomKind: recipe.roomKind as ResearchProfileCollaborationRecipe["roomKind"],
      roles,
      synthesisInstructions: stringArray(recipe.synthesisInstructions, `collaboration recipe ${id} synthesisInstructions`),
    } satisfies ResearchProfileCollaborationRecipe;
  });
  uniqueIds(recipes, "collaboration recipe");
  return {
    protocolInstructions: stringArray(input.protocolInstructions, "collaboration protocol instructions"),
    recipes,
  };
}

function normalizeCapabilities(value: unknown): ResearchProfileCapabilities {
  const input = record(value, "Research profile capabilities");
  assertKnownKeys(input, [
    "defaultToolFamilies", "disabledToolFamilies", "allowedSideEffects", "selectedSkillIds",
    "disabledSkillIds", "allowedMcpServerIds", "memoryEnabled", "runbooksEnabled", "reportsEnabled", "collaborationEnabled",
  ], "Research profile capabilities");
  const allowedSideEffects = stringArray(input.allowedSideEffects, "allowed side effects");
  if (allowedSideEffects.some((effect) => !["none", "read", "write", "network", "process"].includes(effect))) {
    throw new Error("Research profile contains an unsupported side effect.");
  }
  return {
    defaultToolFamilies: stringArray(input.defaultToolFamilies, "default tool families"),
    disabledToolFamilies: stringArray(input.disabledToolFamilies, "disabled tool families"),
    allowedSideEffects: allowedSideEffects as ResearchProfileCapabilities["allowedSideEffects"],
    selectedSkillIds: stringArray(input.selectedSkillIds, "selected skill ids"),
    disabledSkillIds: input.disabledSkillIds === undefined ? [] : stringArray(input.disabledSkillIds, "disabled skill ids"),
    allowedMcpServerIds: input.allowedMcpServerIds === undefined ? [] : stringArray(input.allowedMcpServerIds, "allowed MCP server ids"),
    memoryEnabled: optionalBoolean(input.memoryEnabled, true, "memoryEnabled"),
    runbooksEnabled: optionalBoolean(input.runbooksEnabled, true, "runbooksEnabled"),
    ...(input.reportsEnabled === undefined
      ? {}
      : { reportsEnabled: optionalBoolean(input.reportsEnabled, false, "reportsEnabled") }),
    collaborationEnabled: optionalBoolean(input.collaborationEnabled, true, "collaborationEnabled"),
  };
}

function normalizeWorkspace(value: unknown): ResearchProfileWorkspace {
  const input = record(value, "Research profile workspace");
  assertKnownKeys(input, [
    "workspaceNoun", "subjectNoun", "boundaryNoun", "authorizationMode", "boundaryInstructions", "materialKinds",
  ], "Research profile workspace");
  if (input.authorizationMode !== "required_for_live_network" && input.authorizationMode !== "optional") {
    throw new Error("Research profile workspace has invalid authorizationMode.");
  }
  return {
    workspaceNoun: nonEmptyString(input.workspaceNoun, "workspace noun"),
    subjectNoun: nonEmptyString(input.subjectNoun, "subject noun"),
    boundaryNoun: nonEmptyString(input.boundaryNoun, "boundary noun"),
    authorizationMode: input.authorizationMode,
    boundaryInstructions: stringArray(input.boundaryInstructions, "boundary instructions"),
    materialKinds: stringArray(input.materialKinds, "material kinds"),
  };
}

function normalizeModelJobs(value: unknown): ResearchProfileModelJobs {
  if (value === undefined) return {};
  const input = record(value, "Research profile modelJobs");
  assertKnownKeys(input, ["sessionTitle", "promptGeneration", "goalSuggestions", "memoryCuration", "shellReview"], "Research profile modelJobs");
  const result: ResearchProfileModelJobs = {};
  for (const key of ["sessionTitle", "promptGeneration", "goalSuggestions", "memoryCuration", "shellReview"] as const) {
    if (input[key] === undefined) continue;
    const job = record(input[key], `model job ${key}`);
    assertKnownKeys(job, ["provider", "model", "effort"], `model job ${key}`);
    result[key] = {
      ...(optionalString(job.provider) ? { provider: optionalString(job.provider)! } : {}),
      ...(optionalString(job.model) ? { model: optionalString(job.model)! } : {}),
      ...(optionalString(job.effort) ? { effort: optionalString(job.effort)! } : {}),
    };
  }
  return result;
}

function normalizePresentation(value: unknown): ResearchProfilePresentation {
  const input = record(value, "Research profile presentation");
  assertKnownKeys(input, ["newResearchLabel", "memoryLabel", "runbookLabel", "sessionLabel", "sessionHeatPalette"], "Research profile presentation");
  const palette = input.sessionHeatPalette === undefined
    ? undefined
    : record(input.sessionHeatPalette, "session heat palette");
  if (palette) assertKnownKeys(palette, ["low", "medium", "high", "critical"], "session heat palette");
  return {
    newResearchLabel: nonEmptyString(input.newResearchLabel, "new research label"),
    memoryLabel: nonEmptyString(input.memoryLabel, "memory label"),
    runbookLabel: nonEmptyString(input.runbookLabel, "runbook label"),
    sessionLabel: nonEmptyString(input.sessionLabel, "session label"),
    ...(palette ? {
      sessionHeatPalette: {
        low: hexColor(palette.low, "session heat low color"),
        medium: hexColor(palette.medium, "session heat medium color"),
        high: hexColor(palette.high, "session heat high color"),
        critical: hexColor(palette.critical, "session heat critical color"),
      },
    } : {}),
  };
}

function sessionHeatLevel(value: unknown, label: string): ResearchProfileSessionHeat {
  if (value !== "none" && value !== "low" && value !== "medium" && value !== "high" && value !== "critical") {
    throw new Error(`${label} is invalid.`);
  }
  return value;
}

function hexColor(value: unknown, label: string): string {
  const color = nonEmptyString(value, label);
  if (!/^#[a-f\d]{6}$/iu.test(color)) throw new Error(`${label} must be a six-digit hex color.`);
  return color.toLowerCase();
}

function stableJson(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(stableJson).join(",")}]`;
  if (!value || typeof value !== "object") return JSON.stringify(value);
  return `{${Object.entries(value as Record<string, unknown>)
    .filter(([, nested]) => nested !== undefined)
    .sort(([left], [right]) => left < right ? -1 : left > right ? 1 : 0)
    .map(([key, nested]) => `${JSON.stringify(key)}:${stableJson(nested)}`)
    .join(",")}}`;
}

function deepFreeze<T>(value: T): T {
  if (!value || typeof value !== "object" || Object.isFrozen(value)) return value;
  Object.freeze(value);
  for (const nested of Object.values(value as Record<string, unknown>)) deepFreeze(nested);
  return value;
}

function record(value: unknown, label: string): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(`${label} must be an object.`);
  return value as Record<string, unknown>;
}

function assertKnownKeys(
  value: Readonly<Record<string, unknown>>,
  allowedKeys: readonly string[],
  label: string,
): void {
  const allowed = new Set(allowedKeys);
  const unknown = Object.keys(value).filter((key) => !allowed.has(key));
  if (unknown.length > 0) throw new Error(`${label} contains unknown field${unknown.length === 1 ? "" : "s"}: ${unknown.join(", ")}.`);
}

function array(value: unknown, label: string): unknown[] {
  if (!Array.isArray(value)) throw new Error(`${label} must be an array.`);
  if (value.length > 512) throw new Error(`${label} exceeds the 512-item limit.`);
  return value;
}

function nonEmptyString(value: unknown, label: string): string {
  if (typeof value !== "string" || !value.trim()) throw new Error(`${label} must be a non-empty string.`);
  const normalized = value.trim();
  if (normalized.length > 64_000) throw new Error(`${label} exceeds the 64000-character limit.`);
  return normalized;
}

function optionalString(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() ? value.trim() : undefined;
}

function optionalBoolean(value: unknown, fallback: boolean, label: string): boolean {
  if (value === undefined) return fallback;
  if (typeof value !== "boolean") throw new Error(`${label} must be a boolean.`);
  return value;
}

function identifier(value: unknown, label: string): string {
  const id = nonEmptyString(value, label);
  if (!/^[a-z0-9]+(?:[._-][a-z0-9]+)*$/u.test(id)) {
    throw new Error(`${label} must use lowercase letters, numbers, dots, underscores, or hyphens.`);
  }
  return id;
}

function stringArray(value: unknown, label: string): string[] {
  const values = array(value, label).map((entry) => nonEmptyString(entry, label));
  return [...new Set(values)];
}

function finiteNumber(value: unknown, label: string): number {
  if (typeof value !== "number" || !Number.isFinite(value)) throw new Error(`${label} must be finite.`);
  return value;
}

function positiveInteger(value: unknown, label: string): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value <= 0) throw new Error(`${label} must be a positive integer.`);
  return value;
}

function uniqueIds(values: readonly { id: string }[], label: string): void {
  const ids = new Set<string>();
  for (const value of values) {
    if (ids.has(value.id)) throw new Error(`Duplicate ${label} id: ${value.id}`);
    ids.add(value.id);
  }
}

async function pathExists(path: string): Promise<boolean> {
  try { await access(path); return true; }
  catch { return false; }
}
