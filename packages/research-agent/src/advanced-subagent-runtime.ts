import type { ModelThinkingLevel } from "@earendil-works/pi-ai";
import {
  SubagentManager,
  SUBAGENT_COLLABORATION_TOOLS,
  type CreateSubagentManagerOptions,
  type SubagentDelegationRole,
} from "./subagent-runtime.js";
import type { CreateSubagentRuntimeOptions, SubagentRuntimeFactory } from "./subagent-orchestration.js";

export const ADVANCED_SUBAGENT_ROLES = [
  {
    id: "discoverer",
    label: "Discoverer",
    instruction: "Act as a bounded discovery scout. Map attacker-controlled entry paths, trust boundaries, security-relevant sinks, and plausible unauthorized effects on the assigned surface without duplicating known coverage. Return tool-backed security leads, open reachability or chain questions, coverage gaps, and meaningful negative results for durable campaign state. Do not treat ordinary correctness defects as security findings without a supported security consequence or claim independent reproduction, review, or reporting.",
  },
  {
    id: "prover",
    label: "Prover",
    instruction: "Reproduce a specific finding. Record the exact environment, prerequisites, steps, results, evidence, and failed attempts needed to establish whether the finding is reproducible; do not approve the finding.",
  },
  {
    id: "reviewer",
    label: "Reviewer",
    instruction: "Independently review the finding and its reproduction. Seek contrary evidence, verify the evidence, impact, and scope, and return approve, reject, or needs-work with a clear basis; do not write the submission report.",
  },
  {
    id: "reporter",
    label: "Reporter",
    instruction: "Write a submission report only for a reviewed and approved finding. Use the canonical evidence and reproduction, preserve their limits, and do not invent claims or promote an unreviewed finding.",
  },
] as const satisfies readonly SubagentDelegationRole[];

export type AdvancedSubagentRole = typeof ADVANCED_SUBAGENT_ROLES[number]["id"];

export const ADVANCED_SUBAGENT_COLLABORATION_TOOLS = SUBAGENT_COLLABORATION_TOOLS;

export const TRACK_SUBAGENT_ROLES = [
  ADVANCED_SUBAGENT_ROLES[0],
  ADVANCED_SUBAGENT_ROLES[1],
  {
    ...ADVANCED_SUBAGENT_ROLES[2],
    instruction: "Independently review the assigned Finding and its reproduction. Seek contrary evidence and check the current rating, impact, scope, and program threshold before approving. If the claim has been downrated to low or informational without a credible path to program-level impact, preserve the proof as context, return needs-work, and do not advance it to verified. Otherwise verify the evidence and return approve, reject, or needs-work with a clear basis; do not write the submission report.",
  },
  {
    id: "investigator",
    label: "Investigator",
    instruction: "Investigate the assigned existing Lead or Finding using independent, tool-backed evidence. Inspect its history and current claim, including whether the theory has a plausible in-scope attacker path and security impact. Reassess the qualitative rating before expensive proof work. If bounded evidence leaves only an informational or low-impact behavior without a credible path to program-level impact, preserve the useful context and stop short of reproduction or verification; do not complete the queue by verifying a nonreportable primitive. For a Lead, test the missing observation and promote the same claim to observed only after direct evidence; refute a contradicted theory when justified. For an observed or reproduced Finding, seek reproduction and contrary evidence, then move a supported claim to verified or refuted when justified. If an assigned verified or stale Finding lacks current reproduction or independent review, transition it to stale when needed. Refresh only the missing provenance: move to reproduced with a qualifying current runbook execution if proof is stale, and obtain a fresh independent review of stable claim content before restoring verified. Do not combine content or provenance changes with independent review in one transition. Mere input reachability or expected API behavior is not a verified Primitive; preserve that evidence without advancing an unsupported security claim. Record a concrete progress blocker when a necessary dependency prevents a decision. Preserve negative results and avoid overlapping the current discovery assignment.",
  },
  {
    id: "chain_analyst",
    label: "Chain analyst",
    instruction: "Analyze the assigned Chain Lead or Finding and its linked Primitive or Chain components. Test the missing composition edge, attacker control, reachability, and resulting security impact. Promote a Lead to observed only with direct evidence, and continue to verification only when the chain is supported. Record a precise blocker or refute an invalid theory; avoid speculative loops and unrelated exploration.",
  },
  {
    id: "triager",
    label: "Triage specialist",
    instruction: "Assess the assigned verified finding against program scope, rules, and impact threshold. Record reportable or not_reportable with policy references and a reason. If nonreportability proves the entire linked surface cannot affect an eligible target, record a supported not_applicable foundation.scopeAssessment on that surface; do not exclude a component merely because one finding fails an impact threshold. For reportable findings, build a portable proof package, verify that its reproduction run is current and came from a recorded authorized execution, include exact reproduction steps and allowed evidence, and call triage.package. The verified finding state already supplies the independent review. Rebuild the package when its claim binding is stale after re-verification. Record a concrete blocker if a complete package cannot be built. Never report unverified or nonreportable findings.",
  },
  {
    id: "reporter",
    label: "Reporter",
    instruction: "Write a clean submission report for the assigned triaged finding. Use its accepted portable proof package, current verified evidence, and the existing report writing guidance. Preserve limitations and use report.create only when the finding remains reportable and the package is current. Check report_ready prerequisites before drafting. If current runbook reproduction or independent verification is missing, do not retry report.create or attach changed provenance while the claim remains verified. Transition the claim to stale with a precise reason so an Investigator can bind current reproduction and obtain a fresh independent review; end this Reporter assignment. A later Reporter can write the report after re-verification and renewed triage.",
  },
] as const satisfies readonly SubagentDelegationRole[];

function createRoleManager(options: CreateSubagentRuntimeOptions,
  roles: readonly SubagentDelegationRole[], trackMode = false): SubagentManager {
  const collaboration = options.collaboration;
  const managerOptions: CreateSubagentManagerOptions = {
    mode: "advanced",
    delegationRoles: roles,
    rootProvider: options.rootProvider,
    rootModel: options.rootModel,
    ...(options.rootReasoning ? { rootReasoning: options.rootReasoning } : {}),
    ...(options.limits?.maxThreads ? { maxThreads: options.limits.maxThreads } : {}),
    ...(options.limits?.maxDepth !== undefined ? { maxDepth: options.limits.maxDepth } : {}),
    ...(collaboration ? {
      maxThreads: collaboration.maxMembersPerRoom * collaboration.maxConcurrentRooms,
      peerChallengeRounds: trackMode ? 0 : collaboration.peerChallengeRounds,
      requireRoomBeforeFinal: !trackMode && collaboration.mode === "always",
      maxConcurrentRooms: collaboration.maxConcurrentRooms,
      maxMembersPerRoom: collaboration.maxMembersPerRoom,
      providerPreferences: collaboration.providers.map((preference) => ({
        provider: preference.provider,
        model: preference.model,
        roles: preference.roles,
        ...(preference.reasoningEffort
          ? { reasoning: preference.reasoningEffort as ModelThinkingLevel }
          : {}),
        enabled: preference.enabled,
      })),
    } : {}),
    ...(options.signal ? { signal: options.signal } : {}),
    run: options.run,
    ...(options.onActivity ? { onActivity: options.onActivity } : {}),
    ...(options.onToolEvent ? { onToolEvent: options.onToolEvent } : {}),
    ...(options.surfaceStore ? { surfaceStore: options.surfaceStore } : {}),
    ...(options.allowManualDelegation !== undefined ? { allowManualDelegation: options.allowManualDelegation } : {}),
  };
  // Keep room for both automatic workers and their children under a legacy solo cap.
  if (trackMode) {
    managerOptions.maxDepth = 2;
    managerOptions.maxChildrenPerNonRootAgent = 1;
    managerOptions.maxThreads = Math.max(4, managerOptions.maxThreads ?? 4);
  }
  return new SubagentManager(managerOptions);
}

/** Sustained role-based orchestration with an explicit responsibility for each delegated agent. */
export const advancedSubagentRuntimeFactory: SubagentRuntimeFactory = {
  id: "advanced",
  toolDescriptors: ADVANCED_SUBAGENT_COLLABORATION_TOOLS,
  create(options) {
    return createRoleManager(options, ADVANCED_SUBAGENT_ROLES);
  },
};

/** Track discovery, independent verification, and durable-funnel worker roles. */
export const trackSubagentRuntimeFactory: SubagentRuntimeFactory = {
  id: "advanced",
  toolDescriptors: SUBAGENT_COLLABORATION_TOOLS,
  create(options) { return createRoleManager(options, TRACK_SUBAGENT_ROLES, true); },
};
