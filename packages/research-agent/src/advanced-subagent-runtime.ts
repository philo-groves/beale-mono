import type { SimpleStreamOptions } from "@earendil-works/pi-ai";
import {
  SubagentManager,
  SUBAGENT_COLLABORATION_TOOLS,
  type CreateSubagentManagerOptions,
  type SubagentDelegationRole,
} from "./subagent-runtime.js";
import type { SubagentRuntimeFactory } from "./subagent-orchestration.js";

export const ADVANCED_SUBAGENT_ROLES = [
  {
    id: "discoverer",
    label: "Discoverer",
    instruction: "Act as a bounded discovery scout. Explore the assigned surface without duplicating known coverage, collect tool-backed observations, and return useful leads, coverage gaps, and negative results for durable campaign state without claiming independent reproduction, review, or reporting.",
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

/** Simple orchestration with a required, explicit responsibility for each delegated agent. */
export const advancedSubagentRuntimeFactory: SubagentRuntimeFactory = {
  id: "advanced",
  toolDescriptors: ADVANCED_SUBAGENT_COLLABORATION_TOOLS,
  create(options) {
    const collaboration = options.collaboration;
    const managerOptions: CreateSubagentManagerOptions = {
      mode: "advanced",
      delegationRoles: ADVANCED_SUBAGENT_ROLES,
      rootProvider: options.rootProvider,
      rootModel: options.rootModel,
      ...(options.rootReasoning ? { rootReasoning: options.rootReasoning } : {}),
      ...(options.limits?.maxThreads ? { maxThreads: options.limits.maxThreads } : {}),
      ...(options.limits?.maxDepth !== undefined ? { maxDepth: options.limits.maxDepth } : {}),
      ...(collaboration ? {
        maxThreads: collaboration.maxMembersPerRoom * collaboration.maxConcurrentRooms,
        peerChallengeRounds: collaboration.peerChallengeRounds,
        requireRoomBeforeFinal: collaboration.mode === "always",
        maxConcurrentRooms: collaboration.maxConcurrentRooms,
        maxMembersPerRoom: collaboration.maxMembersPerRoom,
        providerPreferences: collaboration.providers.map((preference) => ({
          provider: preference.provider,
          model: preference.model,
          roles: preference.roles,
          ...(preference.reasoningEffort
            ? { reasoning: preference.reasoningEffort as SimpleStreamOptions["reasoning"] }
            : {}),
          enabled: preference.enabled,
        })),
      } : {}),
      ...(options.channelContext ? { channelContext: options.channelContext } : {}),
      ...(options.signal ? { signal: options.signal } : {}),
      run: options.run,
      ...(options.onActivity ? { onActivity: options.onActivity } : {}),
      ...(options.onToolEvent ? { onToolEvent: options.onToolEvent } : {}),
    };
    return new SubagentManager(managerOptions);
  },
};
