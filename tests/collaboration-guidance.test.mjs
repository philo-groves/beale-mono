import assert from "node:assert/strict";
import test from "node:test";

import { decodeResearchCollaborationConfig } from "../packages/research-agent/dist/collaboration-config.js";
import { createCollaborationSystemGuidance } from "../packages/research-agent/dist/collaboration-guidance.js";

const BASE_CONFIG = {
  mode: "adaptive",
  subagentMode: "simple",
  intensity: "balanced",
  providers: [
    { provider: "openai", model: "gpt-5.6", reasoningEffort: "high", enabled: true },
    { provider: "anthropic", model: "claude-opus-5", reasoningEffort: "high", enabled: true },
  ],
  independentFirstPass: true,
  peerChallengeRounds: 1,
  maxConcurrentRooms: 2,
  maxMembersPerRoom: 3,
};

test("collaboration config always uses the fixed team posture and balanced limits", () => {
  const config = decodeResearchCollaborationConfig({
    ...BASE_CONFIG,
    mode: "solo",
    intensity: "deep",
    maxConcurrentRooms: 4,
    maxMembersPerRoom: 4,
  });

  assert.equal(config.mode, "always");
  assert.equal(config.intensity, "balanced");
  assert.equal(config.subagentMode, "simple");
  assert.equal(config.maxConcurrentRooms, 4);
  assert.equal(config.maxMembersPerRoom, 3);
  assert.equal(config.providers.length, 2);
});

test("collaboration config preserves validated Advanced compatible roles", () => {
  const config = decodeResearchCollaborationConfig({
    ...BASE_CONFIG,
    subagentMode: "advanced",
    providers: [
      { ...BASE_CONFIG.providers[0], roles: ["discoverer", "prover"] },
      { ...BASE_CONFIG.providers[1], roles: ["reviewer"] },
    ],
  });

  assert.deepEqual(config.providers[0].roles, ["discoverer", "prover"]);
  assert.deepEqual(config.providers[1].roles, ["reviewer"]);
  assert.match(createCollaborationSystemGuidance(config, "discovery"), /Use bounded Discoverer scouts when independent attack-surface work would help/);
  assert.match(createCollaborationSystemGuidance(config, "discovery"), /first-touch foundation scout is launched by the host/);
  assert.deepEqual(decodeResearchCollaborationConfig({
    ...BASE_CONFIG,
    providers: [{ ...BASE_CONFIG.providers[0], role: "reviewer" }],
  }).providers[0].roles, ["reviewer"]);
  assert.throws(
    () => decodeResearchCollaborationConfig({
      ...BASE_CONFIG,
      providers: [{ ...BASE_CONFIG.providers[0], roles: ["observer"] }],
    }),
    /Unsupported collaboration config providers\[0\]\.roles\[0\]: observer/,
  );
  assert.throws(
    () => decodeResearchCollaborationConfig({
      ...BASE_CONFIG,
      providers: [{ ...BASE_CONFIG.providers[0], roles: [] }],
    }),
    /roles must contain at least one role/,
  );
});

test("adaptive collaboration guidance makes delegation evidence-driven", () => {
  const guidance = createCollaborationSystemGuidance(BASE_CONFIG, "discovery");

  assert.match(guidance, /^Active collaboration settings:\nCollaboration mode is adaptive/);
  assert.match(guidance, /\n\nContext and review:\nParent transcript inheritance/);
  assert.match(guidance, /\n\nSubagent workflow:\nSimple subagent mode/);
  assert.match(guidance, /\n\nCollaboration mode policy:\nAdaptive mode/);
  assert.doesNotMatch(guidance, /Channel communication is intentionally lax/);
  assert.match(guidance, /makes collaboration available, not required/);
  assert.match(guidance, /materially better evidence than continuing in the lead/);
  assert.match(guidance, /coordination cost outweighs the expected gain/);
  assert.match(guidance, /Prefer followup_task when an existing agent's context matches/);
  assert.match(guidance, /these are opportunities, not a delegation requirement/);
  assert.match(guidance, /Do not spawn merely to satisfy the mode/);
  assert.match(guidance, /Concurrency limit: 6 active subagent turns/);
  assert.match(guidance, /references\/research\//);
  assert.doesNotMatch(guidance, /actively use ordinary subagents/);
  assert.doesNotMatch(guidance, /no lifetime collaborator-invocation budget/i);
});

test("subagent guidance reserves track checkpoints for the lead", () => {
  const guidance = createCollaborationSystemGuidance(BASE_CONFIG, "discovery", { lead: false });
  assert.match(guidance, /lead owns research-track checkpoints/);
  assert.match(guidance, /do not checkpoint the track from a subagent/);
});

test("discovery-specific collaboration guidance does not leak into other workflows", () => {
  const guidance = createCollaborationSystemGuidance(BASE_CONFIG, "verification");

  assert.match(guidance, /makes collaboration available, not required/);
  assert.doesNotMatch(guidance, /Discovery may benefit/);
});

test("advanced guidance leaves optional discovery delegation to the agent", () => {
  const advanced = { ...BASE_CONFIG, subagentMode: "advanced" };
  const discovery = createCollaborationSystemGuidance(advanced, "discovery");
  const exploration = createCollaborationSystemGuidance(advanced, "exploration");
  const longshot = createCollaborationSystemGuidance(advanced, "longshot");
  const customDiscovery = createCollaborationSystemGuidance(advanced, "custom-discovery");
  const unspecified = createCollaborationSystemGuidance(advanced);

  for (const guidance of [discovery, exploration, longshot, customDiscovery, unspecified]) {
    assert.match(guidance, /Use bounded Discoverer scouts when independent attack-surface work would help/);
    assert.match(guidance, /Give each a distinct assignment/);
    assert.match(guidance, /first-touch foundation scout is launched by the host/);
    assert.doesNotMatch(guidance, /launch a fresh non-duplicative Discoverer assignment/);
    assert.doesNotMatch(guidance, /continue in the lead plus bounded Prover or Reviewer assignments/);
  }
});

test("research lanes do not change advanced collaboration guidance", () => {
  const advanced = { ...BASE_CONFIG, subagentMode: "advanced" };
  assert.equal(
    createCollaborationSystemGuidance(advanced, "research"),
    createCollaborationSystemGuidance(advanced, "chaining"),
  );
});

test("advanced collaboration guidance describes sustained role-based orchestration", () => {
  const advanced = { ...BASE_CONFIG, subagentMode: "advanced", intensity: "deep" };
  const lead = createCollaborationSystemGuidance(advanced, "discovery");
  const worker = createCollaborationSystemGuidance(advanced, "discovery", { lead: false });

  assert.match(lead, /coordinates a sustained role-based research team through direct spawning/);
  assert.match(lead, /Use Discoverer as the scout for general analysis and discovery/);
  assert.match(lead, /Use Prover to reproduce a specific finding/);
  assert.match(lead, /Use Reviewer for independent review/);
  assert.match(lead, /Use Reporter only to write a submission report for a reviewed and approved finding/);
  assert.match(lead, /Roles clarify responsibility; they do not impose a phase gate/);
  assert.match(lead, /Parent transcript inheritance is opt-in/);
  assert.match(lead, /fork_turns=all/);
  assert.match(lead, /same provider and model/);
  assert.match(lead, /fork_turns=none/);
  assert.match(worker, /delegation prompt names your Advanced role/);
  assert.doesNotMatch(lead, /delegate_batch|coordination_checkpoint|rolling, lead-owned evidence team/);
});

test("solo and always collaboration guidance retain their distinct postures", () => {
  const always = createCollaborationSystemGuidance({ ...BASE_CONFIG, mode: "always" }, "discovery");
  const solo = createCollaborationSystemGuidance({ ...BASE_CONFIG, mode: "solo" }, "discovery");

  assert.match(always, /throughout every materially separable research stage/);
  assert.doesNotMatch(always, /makes collaboration available, not required/);
  assert.match(solo, /Do not manually initiate collaboration unless the user explicitly requests it/);
  assert.match(solo, /host may still launch a first-touch foundation scout/);
});
