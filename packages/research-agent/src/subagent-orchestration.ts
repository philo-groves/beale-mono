import type { AgentMessage, AgentTool } from "@earendil-works/pi-agent-core";
import type { SimpleStreamOptions } from "@earendil-works/pi-ai";
import {
  SubagentManager,
  SUBAGENT_COLLABORATION_TOOLS,
  type CreateSubagentManagerOptions,
  type SubagentActivity,
  type SubagentChannelContext,
  type SubagentRunRequest,
  type SubagentRunResult,
} from "./subagent-runtime.js";
import type {
  ResearchCollaborationConfig,
  ResearchCollaborationToolDescriptor,
  ResearchEvent,
} from "./types.js";
import { advancedSubagentRuntimeFactory } from "./advanced-subagent-runtime.js";

export {
  SUBAGENT_COLLABORATION_TOOLS,
  type SubagentActivity,
  type SubagentChannelContext,
  type SubagentRunRequest,
  type SubagentRunResult,
} from "./subagent-runtime.js";

/**
 * Provider executors depend on this surface instead of the default subagent
 * implementation. A different orchestration pattern can implement the same
 * lifecycle without being coupled to a provider's agent loop.
 */
export interface SubagentRuntime {
  readonly mode: ResearchCollaborationConfig["subagentMode"];
  capturesContext(toolName: string): boolean;
  captureContext(agentId: string, toolCallId: string, messages: readonly AgentMessage[]): void;
  releaseContext(toolCallId: string): void;
  releaseContextsForAgent(agentId: string): void;
  createTools(agentId: string): AgentTool[];
  collaborationFollowUp(agentId: string): AgentMessage[];
  takeMailbox(agentId: string): AgentMessage[];
  broadcastHostSteering(messages: readonly AgentMessage[]): void;
  allToolEvents(): ResearchEvent[];
  snapshot(): Record<string, unknown>;
  settle(): Promise<void>;
  interruptAll(): void;
}

export interface CreateSubagentRuntimeOptions {
  rootProvider: string;
  rootModel: string;
  rootReasoning?: SimpleStreamOptions["reasoning"];
  limits?: {
    maxThreads?: number;
    maxDepth?: number;
  };
  collaboration?: ResearchCollaborationConfig;
  channelContext?: SubagentChannelContext;
  signal?: AbortSignal;
  run(request: SubagentRunRequest): Promise<SubagentRunResult>;
  onActivity?: (activity: SubagentActivity) => void | Promise<void>;
  onToolEvent?: (event: ResearchEvent) => void | Promise<void>;
}

export interface SubagentRuntimeFactory {
  readonly id: ResearchCollaborationConfig["subagentMode"];
  readonly toolDescriptors: readonly ResearchCollaborationToolDescriptor[];
  create(options: CreateSubagentRuntimeOptions): SubagentRuntime;
}

/** The current, conservative orchestration pattern. */
export const simpleSubagentRuntimeFactory: SubagentRuntimeFactory = {
  id: "simple",
  toolDescriptors: SUBAGENT_COLLABORATION_TOOLS,
  create(options) {
    const collaboration = options.collaboration;
    const managerOptions: CreateSubagentManagerOptions = {
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

export const defaultSubagentRuntimeFactory = simpleSubagentRuntimeFactory;

export function subagentRuntimeFactoryForMode(
  mode: ResearchCollaborationConfig["subagentMode"],
): SubagentRuntimeFactory {
  return mode === "advanced" ? advancedSubagentRuntimeFactory : simpleSubagentRuntimeFactory;
}

export function createSubagentRuntime(
  options: CreateSubagentRuntimeOptions,
  factory: SubagentRuntimeFactory = defaultSubagentRuntimeFactory,
): SubagentRuntime {
  return factory.create(options);
}
