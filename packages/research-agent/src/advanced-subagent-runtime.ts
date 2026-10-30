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
  ADVANCED_SUBAGENT_ROLES[2],
  {
    id: "investigator",
    label: "Investigator",
    instruction: "Resolve the assigned existing finding using independent, tool-backed evidence. Inspect its history and current claim, reproduce or seek contrary evidence, and move it to verified or refuted when justified. Record a concrete progress blocker when a necessary dependency prevents a decision. Preserve negative results; do not leave an eligible finding untouched or fabricate a conclusion.",
  },
  {
    id: "chain_analyst",
    label: "Chain analyst",
    instruction: "Analyze the assigned Chain finding and its linked Primitive or Chain components. Test the missing composition edge, attacker control, reachability, and resulting security impact. Continue to verification only when evidence supports the chain. Record a precise blocker or refute an invalid theory; avoid speculative loops and unrelated exploration.",
  },
  {
    id: "triager",
    label: "Triage specialist",
    instruction: "Assess the assigned verified finding against program scope, rules, and impact threshold. Record reportable or not_reportable with policy references and a reason. If nonreportability proves the entire linked surface cannot affect an eligible target, record a supported not_applicable foundation.scopeAssessment on that surface; do not exclude a component merely because one finding fails an impact threshold. For reportable findings, build and verify a portable proof package on a distinct environment, include exact reproduction steps and allowed evidence, and call triage.package. Record a concrete blocker if a complete package cannot be built. Never report unverified or nonreportable findings.",
  },
  {
    id: "reporter",
    label: "Reporter",
    instruction: "Write a clean submission report for the assigned triaged finding. Use its accepted portable proof package, current verified evidence, and the existing report writing guidance. Preserve limitations and use report.create only when the finding remains reportable and the package is current.",
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
