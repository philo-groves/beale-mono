import assert from "node:assert/strict";
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  DEFAULT_MATHEMATICS_RESEARCH_PROFILE,
  DEFAULT_SECURITY_RESEARCH_PROFILE,
  createDeterministicAgentExecutor,
  createResearchSystemPrompt,
  defaultResearchSystemPromptTemplate,
  validateResearchSystemPromptTemplate,
  normalizeResearchProfile,
  researchProfileHash,
  resolveResearchProfile,
  resolveResearchProfileMemoryType,
  runResearchAgent,
} from "../packages/research-agent/dist/index.js";

const pluginCatalog = (...ids) => ids.map((id) => ({ id, name: id, mcpServers: [], skills: [] }));

test("prompt templates render profile sections and require the authorization boundary", () => {
  const profile = normalizeResearchProfile(DEFAULT_MATHEMATICS_RESEARCH_PROFILE);
  const options = { hasTools: true, hasMemoryTools: true, researchProfile: profile };
  const defaultPrompt = createResearchSystemPrompt(options);
  assert.equal(createResearchSystemPrompt({ ...options, promptTemplate: defaultResearchSystemPromptTemplate() }), defaultPrompt);
  const custom = createResearchSystemPrompt({ ...options, promptTemplate: "{{boundary}}\nProfile: {{profile.name}}\n{{identity}}" });
  assert.ok(custom.startsWith("Scope and authority:"));
  assert.ok(custom.includes("Profile: Mathematics"));
  assert.ok(custom.includes(profile.agent.role));
  assert.match(defaultResearchSystemPromptTemplate(), /\{\{tools\}\}\n\n\{\{plugins\}\}\n\n\{\{collaboration\}\}/);
  assert.doesNotMatch(defaultResearchSystemPromptTemplate(), /\{\{reports\}\}/);
  const withPlugins = createResearchSystemPrompt({ ...options, pluginCatalog: [{
    id: "example-plugin", name: "Example Plugin", mcpServers: [],
    skills: [{ id: "example-skill", name: "Example Skill", useWhen: "Inspect synthetic data.", path: "example.md",
      resourceCounts: { scripts: 0, references: 1, assets: 2 } }],
  }] });
  assert.match(withPlugins, /Available plugins.*\n- example-plugin \(plugin; 0 tools, 1 skill\):/);
  assert.doesNotMatch(withPlugins, /example-skill|Inspect synthetic data/);
  assert.doesNotMatch(withPlugins, /Example Plugin|Example Skill/);
  const existingTemplate = createResearchSystemPrompt({ ...options, pluginCatalog: [{
    id: "example-plugin", name: "Example Plugin", mcpServers: [], skills: [],
  }], promptTemplate: "{{boundary}}\n{{identity}}" });
  assert.match(existingTemplate, /Available plugins[^\n]*\n- example-plugin \(plugin; 0 tools, 0 skills\):/);
  assert.throws(() => validateResearchSystemPromptTemplate("{{identity}}"), /boundary/);
  assert.throws(() => validateResearchSystemPromptTemplate("{{boundary}}\n{{unknown}}"), /Unknown prompt variable/);
});

test("default prompt uses plain guidance lines and bullets only for catalogs", () => {
  const profile = normalizeResearchProfile(DEFAULT_SECURITY_RESEARCH_PROFILE);
  const prompt = createResearchSystemPrompt({
    hasTools: true,
    hasMemoryTools: true,
    hasFindingTools: true,
    hasRunbookTools: true,
    hasCollaborationTools: true,
    hasSessionDispositionTool: true,
    goalEnabled: true,
    researchProfile: profile,
    pluginCatalog: [...pluginCatalog("beale-knowledge", "beale-claims", "beale-runbooks"), ...pluginCatalog("example-plugin")],
  });
  const bulletLines = prompt.split("\n").filter((line) => line.startsWith("- "));
  assert.ok(bulletLines.some((line) => line.startsWith("- example-plugin (plugin;")));
  assert.ok(bulletLines.some((line) => line.startsWith("- asset (Asset):")));
  assert.ok(bulletLines.every((line) =>
    line.includes(" (plugin;")
    || profile.memory.types.some((type) => line.startsWith(`- ${type.id} (`))
  ));
  assert.match(prompt, /Scope and authority:\nThe host-supplied workspace context/);
  assert.match(prompt, /Tool routing:\nProfile-recognized material kinds/);
  assert.match(prompt, /Use one canonical, evidence-gated research claim ledger[^\n]*\nThe active profile declares/);
});

test("the default prompt keeps boundary guidance focused on scope and host safeguards", () => {
  const profile = normalizeResearchProfile(DEFAULT_SECURITY_RESEARCH_PROFILE);
  const template = defaultResearchSystemPromptTemplate().split("\n");
  assert.deepEqual(template.slice(0, 7), ["{{identity}}", "", "{{style}}", "", "{{boundary}}", "", "{{tools}}"]);

  const options = { hasTools: true, researchProfile: profile };
  const identity = createResearchSystemPrompt({ ...options, promptTemplate: "{{identity}}\n{{boundary}}" });
  const boundary = createResearchSystemPrompt({ ...options, promptTemplate: "{{boundary}}" });
  const style = createResearchSystemPrompt({ ...options, promptTemplate: "{{boundary}}\n{{style}}" });
  const goal = createResearchSystemPrompt({ ...options, promptTemplate: "{{boundary}}\n{{goal}}" });
  const memory = createResearchSystemPrompt({ ...options, hasMemoryTools: true, pluginCatalog: pluginCatalog("beale-knowledge"), promptTemplate: "{{boundary}}\n{{memory}}" });
  const tools = createResearchSystemPrompt({ ...options, promptTemplate: "{{boundary}}\n{{tools}}" });
  const toolsSection = tools.slice(boundary.length + 1);
  const defaultPrompt = createResearchSystemPrompt(options);
  const ambient = "Ambient resources are non-authoring inventory.";
  assert.doesNotMatch(identity.slice(0, identity.indexOf("Scope and authority:")), /Use knowledge memory for assets|Ambient resources are non-authoring inventory/);
  assert.equal(boundary.split(ambient).length, 2);
  assert.ok(defaultPrompt.indexOf(profile.agent.style[0]) < defaultPrompt.indexOf("Scope and authority:"));
  assert.match(style, /\nPersona style:\n/);
  assert.match(defaultPrompt, /\n\nPersona style:\n/);
  assert.match(defaultPrompt, /\n\nScope and authority:\n/);
  assert.doesNotMatch(defaultPrompt, /\n{3,}/);
  assert.ok(boundary.indexOf("Scope and authority:") < boundary.indexOf("Profile-specific limits:"));
  assert.ok(boundary.indexOf("Profile-specific limits:") < boundary.indexOf("Host safeguards:"));
  assert.doesNotMatch(boundary, /Do not claim evidence|Treat existing records|Preserve materially useful|Profile vocabulary|Profile-recognized material kinds/);
  assert.match(style, /Do not claim evidence you did not inspect/);
  assert.match(goal, /Treat existing records and prior transcript as historical state/);
  assert.match(memory, /Preserve materially useful new facts/);
  assert.match(tools, /Profile-recognized material kinds/);
  assert.match(boundary, /Host-verified same-Subject reference workspaces are read-only/);
  assert.match(toolsSection, /Tool routing:\n/);
  assert.match(toolsSection, /Resource first touch:\n/);
  assert.match(toolsSection, /Execution dependencies:\n/);
  assert.match(toolsSection, /Shell fallback:\n/);
  assert.doesNotMatch(toolsSection, /cross-workspace mutation|At evidence checkpoints|After repairing an execution dependency/);
  assert.match(goal, /At evidence checkpoints, rank at most three candidates/);
  assert.match(memory, /After repairing an execution dependency/);
  assert.match(boundary, /Never expose host credentials/);
});

test("goal guidance separates session work, Goal mode, and disposition", () => {
  const options = {
    hasTools: true,
    hasSessionDispositionTool: true,
    researchProfile: DEFAULT_SECURITY_RESEARCH_PROFILE,
    promptTemplate: "{{boundary}}\n{{goal}}",
  };
  const regular = createResearchSystemPrompt(options);
  const persistent = createResearchSystemPrompt({ ...options, goalEnabled: true });

  assert.match(regular, /Session progress:\nTreat existing records/);
  assert.match(regular, /At evidence checkpoints/);
  assert.match(regular, /Session disposition:\nBefore the root final response/);
  assert.doesNotMatch(regular, /Persistent Goal mode:|objective_achieved/);
  assert.match(persistent, /Persistent Goal mode:\nContinue researching/);
  assert.ok(persistent.indexOf("Persistent Goal mode:") < persistent.indexOf("Session disposition:"));
  assert.doesNotMatch(persistent, /\n{3,}/);
});

test("collaboration guidance separates delegation, topics, profile protocol, and runtime policy", () => {
  const options = {
    hasTools: true,
    hasCollaborationTools: true,
    researchProfile: DEFAULT_SECURITY_RESEARCH_PROFILE,
    promptTemplate: "{{boundary}}\n{{collaboration}}",
    collaborationGuidance: "Active collaboration settings:\nCollaboration mode is adaptive.",
  };
  const prompt = createResearchSystemPrompt(options);
  const withoutTools = createResearchSystemPrompt({ ...options, hasCollaborationTools: false, collaborationGuidance: undefined });
  const subagent = createResearchSystemPrompt({ ...options, hasCollaborationTools: false, agentPath: "example-agent", collaborationGuidance: undefined });

  assert.match(prompt, /Delegation:\nDelegate distinct, bounded work/);
  assert.match(prompt, /Research topics:\nUse topic_search and topic_list/);
  assert.match(prompt, /Profile collaboration protocol:\nKeep exploit claims/);
  assert.match(prompt, /Active collaboration settings:\nCollaboration mode is adaptive/);
  assert.ok(prompt.indexOf("Delegation:") < prompt.indexOf("Research topics:"));
  assert.ok(prompt.indexOf("Research topics:") < prompt.indexOf("Profile collaboration protocol:"));
  assert.ok(prompt.indexOf("Profile collaboration protocol:") < prompt.indexOf("Active collaboration settings:"));
  assert.doesNotMatch(prompt, /\n{3,}/);
  assert.doesNotMatch(withoutTools, /Delegation:|Research topics:|Active collaboration settings:/);
  assert.match(subagent, /Subagent assignment:\nYou are subagent example-agent/);
});

test("research profiles normalize to immutable, deterministic snapshots", () => {
  const profile = normalizeResearchProfile(DEFAULT_SECURITY_RESEARCH_PROFILE);
  const reordered = Object.fromEntries(Object.entries(structuredClone(profile)).reverse());

  assert.ok(Object.isFrozen(profile));
  assert.ok(Object.isFrozen(profile.memory.types));
  assert.equal(researchProfileHash(profile), researchProfileHash(normalizeResearchProfile(reordered)));

  const changed = structuredClone(profile);
  changed.name = "Security Renamed";
  assert.notEqual(researchProfileHash(profile), researchProfileHash(normalizeResearchProfile(changed)));
});

test("bundled profiles separate claim classifications from knowledge memory and product attention", () => {
  const security = normalizeResearchProfile(DEFAULT_SECURITY_RESEARCH_PROFILE);
  const mathematics = normalizeResearchProfile(DEFAULT_MATHEMATICS_RESEARCH_PROFILE);

  assert.equal(security.name, "Security");
  assert.equal(security.capabilities.reportsEnabled, true);
  assert.deepEqual(security.capabilities.defaultToolFamilies, ["shell", "repository-search", "file-read"]);
  assert.equal(mathematics.capabilities.reportsEnabled, true);
  assert.equal(Object.hasOwn(normalizeResearchProfile(generalResearchProfile()).capabilities, "reportsEnabled"), false);
  assert.equal(security.presentation.sessionHeatPalette, undefined);
  assert.equal(mathematics.presentation.sessionHeatPalette, undefined);
  assert.ok(security.memory.types.every((type) => type.sessionHeat === undefined));
  assert.ok(mathematics.memory.types.every((type) => type.sessionHeat === undefined));
  assert.deepEqual(
    security.memory.types.filter((type) => type.lifecycle === "active").map((type) => type.id),
    ["asset", "invariant", "mitigation", "flow-endpoint", "trajectory"],
  );
  assert.deepEqual(
    mathematics.memory.types.filter((type) => type.lifecycle === "active").map((type) => type.id),
    ["problem", "definition", "technique", "reference", "trajectory"],
  );
  assert.deepEqual(security.claims.classifications.map((classification) => classification.id),
    ["security.vulnerability", "security.primitive", "security.chain"]);
  assert.deepEqual(mathematics.claims.classifications.map((classification) => classification.id),
    ["mathematics.conjecture", "mathematics.theorem", "mathematics.counterexample"]);
});

test("memory, claims, and runbook prompt sections follow enabled plugins", () => {
  const template = "{{boundary}}\n{{memory}}\n{{claims}}\n{{runbooks}}";
  const options = { hasTools: true, hasMemoryTools: true, hasFindingTools: true, hasRunbookTools: true,
    researchProfile: DEFAULT_SECURITY_RESEARCH_PROFILE, promptTemplate: template };
  const guidance = [
    ["beale-knowledge", /Use durable memory as a concise research graph/],
    ["beale-claims", /Use one canonical, evidence-gated research claim ledger/],
    ["beale-runbooks", /Use runbooks as durable executable research artifacts/],
  ];
  for (const [enabledId] of guidance) {
    const prompt = createResearchSystemPrompt({ ...options, pluginCatalog: pluginCatalog(enabledId) });
    for (const [id, pattern] of guidance) {
      if (id === enabledId) assert.match(prompt, pattern);
      else assert.doesNotMatch(prompt, pattern);
    }
  }
  const disabled = createResearchSystemPrompt({ ...options, pluginCatalog: pluginCatalog("beale-reporting") });
  for (const [, pattern] of guidance) assert.doesNotMatch(disabled, pattern);
  assert.doesNotMatch(disabled, /Use reports as durable Markdown artifacts/);
  const legacy = createResearchSystemPrompt({ ...options, promptTemplate: "{{boundary}}\n{{reports}}", pluginCatalog: pluginCatalog("beale-reporting") });
  assert.doesNotMatch(legacy, /Use reports as durable Markdown artifacts|\{\{reports\}\}/);
});

test("bundled profiles gate collaboration recipes by domain and workflow", () => {
  const security = normalizeResearchProfile(DEFAULT_SECURITY_RESEARCH_PROFILE);
  const mathematics = normalizeResearchProfile(DEFAULT_MATHEMATICS_RESEARCH_PROFILE);
  assert.deepEqual(security.collaboration.recipes.map((recipe) => recipe.workflowIds), [["discovery", "longshot"], ["chaining"], ["reporting"]]);
  assert.deepEqual(mathematics.collaboration.recipes.map((recipe) => recipe.workflowIds), [["exploration", "longshot"], ["proof"], ["verification"], ["synthesis"]]);
  assert.ok(security.collaboration.recipes.every((recipe) => recipe.roles.length >= 2));
  assert.ok(mathematics.collaboration.recipes.every((recipe) => recipe.roles.length >= 2));
});

test("retired bundled memory types stay out of model-facing catalogs", () => {
  const securityPrompt = createResearchSystemPrompt({
    hasTools: true,
    hasMemoryTools: true,
    hasRunbookTools: true,
    pluginCatalog: pluginCatalog("beale-knowledge", "beale-runbooks"),
    researchProfile: DEFAULT_SECURITY_RESEARCH_PROFILE,
  });
  assert.match(securityPrompt, /- flow-endpoint \(Flow Endpoint\)/);
  assert.doesNotMatch(securityPrompt, /- source \(Source\)/);
  assert.doesNotMatch(securityPrompt, /- sink \(Sink\)/);
  assert.doesNotMatch(securityPrompt, /- bug \(Historical Bug\)/);
  assert.doesNotMatch(securityPrompt, /- procedure \(Procedure\)/);
  assert.match(securityPrompt, /Ambient resources are non-authoring inventory/);
  assert.match(securityPrompt, /Auto-Reviewed first-touch path/);
  assert.match(securityPrompt, /provenance and build identity; check CVEs, advisories, vendor releases/);
  assert.match(securityPrompt, /not live-target authorization/);
  assert.match(securityPrompt, /Execution dependencies:/);
  assert.match(securityPrompt, /lifecycle and ownership, route and address, listening service, host identity, then authentication/);
  assert.match(securityPrompt, /Do not substitute a host account for a guest account/);
  assert.match(securityPrompt, /prefer the known-good unprivileged mode/);
  assert.match(securityPrompt, /update its asset memory or environment runbook/);
  assert.match(securityPrompt, /matching workflow runbook for reproduction-grade evidence/);
  assert.match(securityPrompt, /Direct shell execution may support bounded exploratory tests/);
  assert.match(securityPrompt, /setup, runtime, and cleanup.*feature tags/);
  assert.match(securityPrompt, /successful runIds.*phases they prove/);
  assert.match(securityPrompt, /workspace history search to recover prior work across sessions/);
});

test("bundled profiles define domain-specific Longshot workflows", () => {
  const security = normalizeResearchProfile(DEFAULT_SECURITY_RESEARCH_PROFILE);
  const mathematics = normalizeResearchProfile(DEFAULT_MATHEMATICS_RESEARCH_PROFILE);
  const securityLongshot = security.workflows.find((workflow) => workflow.id === "longshot");
  const mathematicsLongshot = mathematics.workflows.find((workflow) => workflow.id === "longshot");

  assert.equal(security.version, "1.15.2");
  assert.equal(mathematics.version, "1.6.0");
  assert.equal(securityLongshot?.name, "Longshot");
  assert.equal(securityLongshot?.goalSuggestionCount, 4);
  assert.equal(securityLongshot?.description, "Hunt for ambitious, reportable high- or critical-severity vulnerabilities.");
  assert.match(securityLongshot?.goalSuggestionInstructions.join(" ") ?? "", /broad attack surface.*explicit systemic impact ceiling/);
  assert.match(securityLongshot?.goalSuggestionInstructions.join(" ") ?? "", /not.*binary verification task/);
  assert.match(securityLongshot?.promptInstructions.join(" ") ?? "", /severity, and reportability evidence-gated/);
  assert.equal(mathematicsLongshot?.name, "Longshot");
  assert.equal(mathematicsLongshot?.goalSuggestionCount, 4);
  assert.match(mathematicsLongshot?.description ?? "", /major mathematical breakthrough/);
  assert.match(mathematicsLongshot?.goalSuggestionInstructions.join(" ") ?? "", /specific leverage point|source of possible leverage/);
  assert.match(mathematicsLongshot?.goalSuggestionInstructions.join(" ") ?? "", /open a research program.*breakthrough-scale ceiling/);
});

test("suggestion lanes do not constrain live collaboration guidance", () => {
  const securityPrompt = createResearchSystemPrompt({ hasTools: true, hasCollaborationTools: true, researchProfile: DEFAULT_SECURITY_RESEARCH_PROFILE, workflowId: "chaining" });
  const mathematicsPrompt = createResearchSystemPrompt({ hasTools: true, hasCollaborationTools: true, researchProfile: DEFAULT_MATHEMATICS_RESEARCH_PROFILE, workflowId: "proof" });
  assert.doesNotMatch(securityPrompt, /Exploit-chain cell|reachability-analyst|Active research workflow/);
  assert.doesNotMatch(securityPrompt, /Construction Explorer/);
  assert.doesNotMatch(mathematicsPrompt, /Proof development cell|assumption-auditor|Active research workflow/);
  assert.doesNotMatch(mathematicsPrompt, /Mitigation Challenger/);
});

test("Longshot remains a suggestion lane and is absent from live prompts", () => {
  const securityPrompt = createResearchSystemPrompt({ hasTools: true, hasCollaborationTools: true, researchProfile: DEFAULT_SECURITY_RESEARCH_PROFILE, workflowId: "longshot" });
  const mathematicsPrompt = createResearchSystemPrompt({ hasTools: true, hasCollaborationTools: true, researchProfile: DEFAULT_MATHEMATICS_RESEARCH_PROFILE, workflowId: "longshot" });

  assert.doesNotMatch(securityPrompt, /Longshot|reportable high or critical impact|Security discovery cell/);
  assert.doesNotMatch(mathematicsPrompt, /Longshot|major mathematical advance|Mathematical exploration cell/);
});
test("research profile validation rejects silent schema drift", () => {
  const unknownRootField = { ...structuredClone(DEFAULT_SECURITY_RESEARCH_PROFILE), typoedWorkflows: [] };
  assert.throws(() => normalizeResearchProfile(unknownRootField), /unknown field: typoedWorkflows/);

  const unknownTypeField = structuredClone(DEFAULT_SECURITY_RESEARCH_PROFILE);
  unknownTypeField.memory.types[0].descripton = "misspelled";
  assert.throws(() => normalizeResearchProfile(unknownTypeField), /unknown field: descripton/);

  const invalidEnum = structuredClone(DEFAULT_SECURITY_RESEARCH_PROFILE);
  invalidEnum.memory.types[0].attributes = {
    score: { type: "number", description: "A bounded score.", enum: [Number.NaN] },
  };
  assert.throws(() => normalizeResearchProfile(invalidEnum), /enum does not match/);

  const invalidHeatStatus = structuredClone(DEFAULT_SECURITY_RESEARCH_PROFILE);
  invalidHeatStatus.memory.types[0].sessionHeat = { unrecorded: "high" };
  assert.throws(() => normalizeResearchProfile(invalidHeatStatus), /sessionHeat uses disallowed status unrecorded/);

  const invalidHeatColor = structuredClone(DEFAULT_SECURITY_RESEARCH_PROFILE);
  invalidHeatColor.presentation.sessionHeatPalette = {
    low: "red", medium: "#e8842c", high: "#ff4a54", critical: "#b4121c",
  };
  assert.throws(() => normalizeResearchProfile(invalidHeatColor), /must be a six-digit hex color/);

  const duplicateClaimClassification = structuredClone(DEFAULT_SECURITY_RESEARCH_PROFILE);
  duplicateClaimClassification.claims.classifications.push(structuredClone(duplicateClaimClassification.claims.classifications[0]));
  assert.throws(() => normalizeResearchProfile(duplicateClaimClassification), /Duplicate claim classification id/);

  const unknownRecipeWorkflow = structuredClone(DEFAULT_SECURITY_RESEARCH_PROFILE);
  unknownRecipeWorkflow.collaboration.recipes[0].workflowIds = ["typoed-workflow"];
  assert.throws(() => normalizeResearchProfile(unknownRecipeWorkflow), /references unknown workflow typoed-workflow/);

  const duplicateRecipeWorkflow = structuredClone(DEFAULT_SECURITY_RESEARCH_PROFILE);
  duplicateRecipeWorkflow.collaboration.recipes[1].workflowIds = ["discovery"];
  assert.throws(() => normalizeResearchProfile(duplicateRecipeWorkflow), /assigned to multiple collaboration recipes/);

  const emptyEnabledMemory = structuredClone(DEFAULT_SECURITY_RESEARCH_PROFILE);
  emptyEnabledMemory.memory.types = [];
  assert.throws(
    () => normalizeResearchProfile(emptyEnabledMemory),
    /requires at least one active, creatable memory type/,
  );

  emptyEnabledMemory.capabilities.memoryEnabled = false;
  assert.doesNotThrow(() => normalizeResearchProfile(emptyEnabledMemory));
});

test("memory type IDs stay stable across names, aliases, and retirement", () => {
  const input = generalResearchProfile();
  input.memory.types[0].aliases = ["question"];
  input.memory.types[0].lifecycle = "retired";
  input.memory.types[0].creatable = false;
  input.memory.types[0].replacedBy = "result";
  const profile = normalizeResearchProfile(input);

  assert.deepEqual(resolveResearchProfileMemoryType(profile, "question"), {
    state: "retired",
    canonicalId: "claim",
    type: profile.memory.types[0],
  });
  assert.deepEqual(resolveResearchProfileMemoryType(profile, "unrecorded"), {
    state: "unknown",
    canonicalId: "unrecorded",
  });
});

test("workspace profiles override the bundled security default", async () => {
  const workspaceRoot = join(tmpdir(), `app-server-profile-${process.pid}-${Date.now()}`);
  const profilePath = join(workspaceRoot, ".beale", "profile.json");
  try {
    await mkdir(join(workspaceRoot, ".beale"), { recursive: true });
    await writeFile(profilePath, JSON.stringify(generalResearchProfile()), "utf8");
    const resolved = await resolveResearchProfile({ workspaceRoot });
    assert.equal(resolved.source, "workspace-default");
    assert.equal(resolved.path, profilePath);
    assert.equal(resolved.profile.id, "general-research");
    assert.equal(resolved.hash, researchProfileHash(resolved.profile));
  } finally {
    await rm(workspaceRoot, { recursive: true, force: true });
  }
});

test("the direct run boundary rejects a stale resolved profile hash before compiling context", async () => {
  const profile = normalizeResearchProfile(generalResearchProfile());
  const staleHash = researchProfileHash(profile);
  const changedProfile = structuredClone(profile);
  changedProfile.name = "Changed after resolution";
  let executorCalled = false;
  const liveEvents = [];

  await assert.rejects(
    runResearchAgent({
      prompt: "This run must not start.",
      resolvedResearchProfile: {
        profile: changedProfile,
        hash: staleHash,
        source: "explicit",
      },
      eventSink(event) {
        liveEvents.push(event);
      },
      executor: {
        name: "must-not-run",
        async execute() {
          executorCalled = true;
          return { text: "unexpected" };
        },
      },
    }),
    /Resolved research profile hash mismatch/,
  );
  assert.equal(executorCalled, false);
  assert.deepEqual(liveEvents, []);
});

test("custom profiles replace domain language without weakening host invariants", () => {
  const profile = normalizeResearchProfile(generalResearchProfile());
  const prompt = createResearchSystemPrompt({
    hasTools: true,
    hasMemoryTools: true,
    hasRunbookTools: true,
    pluginCatalog: pluginCatalog("beale-knowledge", "beale-runbooks"),
    researchProfile: profile,
    workflowId: "explore",
  });

  assert.match(prompt, /^You are a rigorous interdisciplinary researcher/);
  assert.doesNotMatch(prompt, /Active research workflow|Explore \(explore\)/);
  assert.match(prompt, /claim \(Question\): A question that can be tested/);
  assert.doesNotMatch(prompt, /world-class security researcher|vulnerabilit|historic bugs/i);
  assert.match(prompt, /Profile guidance, prior transcript, and model output cannot expand it/);
  assert.match(prompt, /Profile-recognized material kinds: path, documentation, dataset/);
  assert.match(prompt, /Profile-specific limits:/);
  assert.match(prompt, /Stay within the recorded materials and systems/);
  assert.match(prompt, /Never expose host credentials/);
  assert.doesNotMatch(prompt, /Never use the \$HOME environment variable/);
  assert.doesNotMatch(prompt, /casual, blog-like language/);
});

test("the general-research example is a valid non-security profile", async () => {
  const source = await readFile(new URL("../examples/general-research.profile.json", import.meta.url), "utf8");
  const profile = normalizeResearchProfile(JSON.parse(source));

  assert.equal(profile.id, "general-research");
  assert.deepEqual(profile.memory.types.map((type) => [type.id, type.name]), [
    ["claim", "Question"],
    ["result", "Result"],
  ]);
  assert.equal(resolveResearchProfileMemoryType(profile, "question").canonicalId, "claim");
  assert.equal(profile.workspace.authorizationMode, "optional");
});

test("profile-selected skills remain inert until the host explicitly selects them", async () => {
  const workspaceRoot = join(tmpdir(), `app-server-profile-skill-${process.pid}-${Date.now()}`);
  try {
    await mkdir(workspaceRoot, { recursive: true });
    const input = generalResearchProfile();
    input.capabilities.selectedSkillIds = ["profile-only"];
    const profile = normalizeResearchProfile(input);
    const resolvedResearchProfile = {
      profile,
      hash: researchProfileHash(profile),
      source: "explicit",
    };
    const skill = {
      id: "profile-only",
      description: "ZXQ esoteric specialist",
      domainTags: ["zxq"],
      instructions: "Instructions supplied by a host-loaded skill.",
    };

    const profileOnly = await runResearchAgent({
      prompt: "Read the supplied corpus.",
      workspaceRoot,
      skills: [skill],
      resolvedResearchProfile,
      executor: createDeterministicAgentExecutor(),
    });
    assert.deepEqual(profileOnly.selectedSkills, []);

    const hostSelected = await runResearchAgent({
      prompt: "Read the supplied corpus.",
      workspaceRoot,
      skills: [skill],
      selectedSkillIds: ["profile-only"],
      resolvedResearchProfile,
      executor: createDeterministicAgentExecutor(),
    });
    assert.equal(hostSelected.selectedSkills[0]?.id, "profile-only");
    assert.ok(hostSelected.selectedSkills[0]?.selectionReasons.includes("explicitly requested"));

    const deferredPluginSkill = await runResearchAgent({
      prompt: "Read the supplied corpus.",
      workspaceRoot,
      skills: [skill],
      selectedSkillIds: ["profile-only"],
      tools: ["file.read", "memory.get", "mcp.example.tools.inspect"].map((name) => ({
        name, transportName: name.replaceAll(".", "_"), description: "Synthetic tool.",
        actionClasses: ["inspect"], sideEffects: "none", requiredPermissions: [],
      })),
      pluginCatalog: [{ id: "example-plugin", name: "Example Plugin", mcpServers: ["example.tools"], skills: [
        { id: "profile-only", name: "Profile Only", useWhen: "Read a supplied corpus.", path: "example-plugin.md" },
      ] }],
      resolvedResearchProfile,
      executor: createDeterministicAgentExecutor(),
    });
    assert.deepEqual(deferredPluginSkill.selectedSkills, []);
    assert.deepEqual(deferredPluginSkill.availableTools.map((tool) => tool.name), ["file.read"]);
  } finally {
    await rm(workspaceRoot, { recursive: true, force: true });
  }
});

function generalResearchProfile() {
  return {
    schemaVersion: 1,
    id: "general-research",
    version: "1.0.0",
    name: "General Research",
    description: "Open-ended evidence-driven research across domains.",
    agent: {
      role: "You are a rigorous interdisciplinary researcher operating inside the Pi agent harness.",
      posture: ["Explore competing explanations and design discriminating tests."],
      style: ["Write concise, evidence-calibrated prose."],
      memoryInstructions: ["Search prior questions and results before repeating work."],
      runbookInstructions: ["Record reusable methods with exact prerequisites and steps."],
    },
    memory: {
      types: [
        {
          id: "claim",
          name: "Question",
          pluralName: "Questions",
          description: "A question that can be tested against observations.",
          lifecycle: "active",
          creatable: true,
          order: 10,
          defaultStatus: "open",
          allowedStatuses: ["open", "supported", "rejected"],
          contextWeight: 4,
        },
        {
          id: "result",
          name: "Result",
          pluralName: "Results",
          description: "A durable result supported by evidence.",
          lifecycle: "active",
          creatable: true,
          order: 20,
          defaultStatus: "supported",
          allowedStatuses: ["supported", "rejected"],
        },
      ],
      statuses: [
        { id: "open", name: "Open", description: "Not yet resolved.", order: 10, polarity: "neutral" },
        { id: "supported", name: "Supported", description: "Supported by available evidence.", order: 20, polarity: "positive" },
        { id: "rejected", name: "Rejected", description: "Rejected by available evidence.", order: 30, terminal: true, polarity: "negative" },
      ],
      evidenceKinds: [{ id: "observation", name: "Observation", description: "A recorded observation.", allowsPath: true }],
      evidencePathBases: [{ id: "workspace", name: "Workspace", description: "Relative to this workspace." }],
      relations: [{ id: "supports", name: "Supports", description: "Supports another memory." }],
      defaultNodeLimit: 8,
      defaultCharacterBudget: 12_000,
    },
    workflows: [{
      id: "explore",
      name: "Explore",
      description: "Explore a bounded question without assuming an answer.",
      goalSuggestionCount: 3,
      goalSuggestionInstructions: ["Suggest distinct bounded questions."],
      promptInstructions: ["Compare at least two plausible explanations."],
      outputRequirements: ["State observations separately from inference."],
      default: true,
    }],
    capabilities: {
      defaultToolFamilies: ["repository-search", "file-read"],
      disabledToolFamilies: [],
      allowedSideEffects: ["none", "read"],
      selectedSkillIds: [],
      disabledSkillIds: [],
      allowedMcpServerIds: [],
      memoryEnabled: true,
      runbooksEnabled: true,
      collaborationEnabled: true,
    },
    workspace: {
      workspaceNoun: "Research workspace",
      subjectNoun: "Research subject",
      boundaryNoun: "Research boundary",
      authorizationMode: "optional",
      boundaryInstructions: ["Stay within the recorded materials and systems."],
      materialKinds: ["path", "documentation", "dataset"],
    },
    modelJobs: {},
    presentation: {
      newResearchLabel: "New Study",
      memoryLabel: "Memory",
      runbookLabel: "Runbooks",
      sessionLabel: "Study Session",
    },
  };
}
