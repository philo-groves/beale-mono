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
    ...subagentModeGuidance(config.subagentMode, lead),
    ...(lead ? modeGuidance(config.mode) : []),
  ].join(" ");
}

function runtimeGuidance(config: ResearchCollaborationConfig): readonly string[] {
  return [
    "For an explicit collaborator route, pass provider and model as separate fields with fork_turns set to none or a bounded number. With fork_turns=all, omit provider, model, and reasoning_effort so the child inherits the parent route.",
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
      "Maintain continuous discovery coverage with bounded Discoverer scouts whenever material, independently explorable attack surface remains. Run discovery alongside proof, review, and reporting work when those tasks are separable. When a Discoverer completes, preserve its leads, observations, coverage, and negative results, then launch a fresh non-duplicative Discoverer assignment if meaningful unexplored surface remains. Stop refreshing scouts when coverage is exhausted or the remaining work is sequential; do not preserve an idle agent merely to keep one alive.",
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
