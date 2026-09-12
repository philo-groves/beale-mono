import type { ResearchCollaborationConfig } from "./types.js";

const ALL_SUBAGENT_ROLES = ["discoverer", "prover", "reviewer", "reporter"] as const;

export function createCollaborationSystemGuidance(
  config: ResearchCollaborationConfig,
  _workflowId?: string,
  options: { lead?: boolean } = {},
): string {
  const enabled = config.providers.filter((provider) => provider.enabled);
  const lead = options.lead ?? true;
  return [
    `Collaboration mode is ${config.mode}; subagent mode is ${config.subagentMode}; intensity is ${config.intensity}. Enabled collaborator routes: ${enabled.map((provider) => `${provider.provider}/${provider.model} (${(provider.roles?.length ? provider.roles : ALL_SUBAGENT_ROLES).join(", ")})`).join("; ") || "none"}.`,
    ...runtimeGuidance(config),
    "Startup context contains only a bounded campaign-state projection. Query the specific memory, finding, runbook, report, or investigation catalog needed for the assignment instead of assuming the full campaign was injected.",
    "Channel communication is intentionally lax: post useful work as it becomes available, preserve dissent, and do not wait for a quorum or protocol phase.",
    ...(lead ? ["Before recording session disposition or sending the final response, resolve every active delegated subagent by waiting for its result or explicitly interrupting it when its result is no longer needed. After a reviewer or other subagent can mutate durable state, re-read the canonical record and base the final response on that current revision rather than the pre-delegation snapshot."] : []),
    ...subagentModeGuidance(config.subagentMode, lead),
    ...(lead ? modeGuidance(config.mode) : []),
  ].join(" ");
}

function runtimeGuidance(config: ResearchCollaborationConfig): readonly string[] {
  return [
    "Parent transcript inheritance is opt-in. Omit fork_turns for a fresh child, or set it to a bounded number only when recent parent turns are necessary. With fork_turns=all, omit provider, model, and reasoning_effort so the child inherits the complete parent route and history.",
    "For independent verification, spawn a distinct Reviewer with fork_turns=none and without channel_name. The Reviewer may use the same provider and model, then must inspect canonical claims and executions through durable tools rather than inherited conversation history.",
    `Concurrency limit: ${config.maxConcurrentRooms * config.maxMembersPerRoom} active subagent turns. Channels themselves persist and do not consume active-turn capacity.`,
  ];
}

function subagentModeGuidance(
  mode: ResearchCollaborationConfig["subagentMode"],
  lead: boolean,
): readonly string[] {
  if (mode === "advanced") {
    if (!lead) {
      return [
        "Your delegation prompt names your Advanced role and its responsibility. Stay within that role, ground conclusions in evidence, and return the result to the delegating agent.",
      ];
    }
    return [
      "Advanced subagent mode uses the same direct spawning, messaging, follow-up, interruption, waiting, and channel collaboration behavior as Simple mode, with a required role for every delegated subagent.",
      "Use at most one Discoverer at a time, only for a named independently explorable gap that does not duplicate active or completed coverage. A completed Discoverer is not a vacancy to refill: preserve its durable result, then continue in the lead or reuse that agent with followup_task when its context fits the next bounded question.",
      "For chain closure, proof, verification, reporting, synthesis, or other sequential work, continue in the lead plus bounded Prover or Reviewer assignments. Use a Discoverer only when a specific missing link has genuinely independent search space.",
      "Use Discoverer as the scout for general analysis and discovery. Use Prover to reproduce a specific finding and record exact prerequisites, steps, results, and evidence. Use Reviewer for independent review of the finding and reproduction, including contrary evidence and an approve, reject, or needs-work decision. Use Reporter only to write a submission report for a reviewed and approved finding.",
      "Choose the role that matches the bounded assignment. Roles clarify responsibility; they do not impose a phase gate or require all four roles for every task.",
    ];
  }
  return [
    "Simple subagent mode exposes direct spawning, messaging, follow-up, interruption, waiting, and channel collaboration with the established behavior.",
  ];
}

function modeGuidance(
  mode: ResearchCollaborationConfig["mode"],
): readonly string[] {
  if (mode === "adaptive") {
    return [
      "Adaptive mode makes collaboration available, not required. Delegate only when clean separation or independent review is likely to produce materially better evidence than continuing in the lead.",
      "At major evidence or subsystem transitions, continue solo when work is sequential or coordination cost outweighs the expected gain.",
      "Prefer followup_task when an existing agent's context matches new work, and avoid duplicate assignments.",
      "Parallel source-to-sink tracing, adjacent attack-surface exploration, variant analysis, or independent challenge may be useful when they are cleanly separable; these are opportunities, not a delegation requirement.",
      "Use a durable channel for related research that later sessions should inherit. Do not spawn merely to satisfy the mode.",
    ];
  }
  if (mode === "always") {
    return [
      "Use collaboration throughout every materially separable research stage that benefits from independent coverage or review.",
      "Use a relevant existing channel where possible, and attach subagents whose work should become reusable workspace research.",
    ];
  }
  return [
    "Do not initiate collaboration unless the user explicitly requests it. Continue the research in the lead session.",
  ];
}
